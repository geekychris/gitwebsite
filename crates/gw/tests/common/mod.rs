//! Fakes shared by the companion v1.2 tests: an echo HTTP server, a self-signed TLS server
//! (HTTPS and WSS), a WebSocket echo server, and a fake S3 that checks SigV4 signatures with
//! its own small implementation (validated against AWS's published test-suite vector).
#![allow(dead_code)]

use axum::{extract::Request, routing::any, Router};
use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const SITE: &str = "https://geekychris.github.io";

pub async fn serve(r: Router) -> u16 {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let p = l.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(l, r).await.unwrap() });
    p
}

/// Echoes method, path, query and selected headers; `/redir` answers 302 to elsewhere.
pub fn echo() -> Router {
    Router::new().fallback(any(|req: Request| async move {
        let (parts, body) = req.into_parts();
        let body = axum::body::to_bytes(body, 1 << 20).await.unwrap();
        if parts.uri.path() == "/redir" {
            let mut r = axum::response::Response::new(axum::body::Body::empty());
            *r.status_mut() = axum::http::StatusCode::FOUND;
            r.headers_mut().insert("location", "http://8.8.8.8/elsewhere".parse().unwrap());
            return r;
        }
        let h = |k: &str| parts.headers.get(k).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        axum::response::IntoResponse::into_response(axum::Json(json!({
            "method": parts.method.as_str(), "path": parts.uri.path(), "query": parts.uri.query().unwrap_or(""),
            "authorization": h("authorization"), "cookie": h("cookie"), "origin": h("origin"), "hue": h("hue-application-key"),
            "x_auth_token": h("x-auth-token"), "x_upstream": h("x-upstream-authorization"), "content_type": h("content-type"),
            "body": String::from_utf8_lossy(&body),
        })))
    }))
}

pub fn tls_acceptor() -> tokio_rustls::TlsAcceptor {
    let ck = rcgen::generate_simple_self_signed(vec!["localhost".into(), "127.0.0.1".into()]).unwrap();
    let cert = ck.cert.der().clone();
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(ck.key_pair.serialize_der()));
    let cfg = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .unwrap();
    tokio_rustls::TlsAcceptor::from(Arc::new(cfg))
}

/// A self-signed HTTPS server answering every request with `{"tls":true,"path":…}`.
pub async fn https_fake() -> u16 {
    let acc = tls_acceptor();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let p = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((s, _)) = l.accept().await else { break };
            let acc = acc.clone();
            tokio::spawn(async move {
                let Ok(mut t) = acc.accept(s).await else { return };
                let mut buf = vec![0u8; 8192];
                let mut got = vec![];
                while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                    match t.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => got.extend_from_slice(&buf[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&got).to_string();
                let path = head.split_whitespace().nth(1).unwrap_or("").to_string();
                let body = json!({"tls": true, "path": path}).to_string();
                let resp = format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
                let _ = t.write_all(resp.as_bytes()).await;
                let _ = t.shutdown().await;
            });
        }
    });
    p
}

/// A WebSocket echo server (plain, or self-signed wss). It picks the subprotocol `mqtt` when
/// offered, and checks that no companion subprotocol or token reaches it.
pub async fn ws_echo(tls: bool) -> u16 {
    use tokio_tungstenite::tungstenite::handshake::server::{Request as WsReq, Response as WsResp};
    let acc = tls.then(tls_acceptor);
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let p = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((s, _)) = l.accept().await else { break };
            let acc = acc.clone();
            tokio::spawn(async move {
                let cb = |req: &WsReq, mut resp: WsResp| {
                    let offered = req.headers().get("sec-websocket-protocol").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                    assert!(!offered.contains("bearer.") && !offered.contains("gw-companion"), "{offered}");
                    assert!(req.headers().get("origin").is_none());
                    if offered.split(',').any(|p| p.trim() == "mqtt") {
                        resp.headers_mut().insert("sec-websocket-protocol", "mqtt".parse().unwrap());
                    }
                    Ok(resp)
                };
                async fn run<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(ws: tokio_tungstenite::WebSocketStream<S>) {
                    let (mut tx, mut rx) = ws.split();
                    while let Some(Ok(m)) = rx.next().await {
                        if m.is_text() || m.is_binary() {
                            if tx.send(m).await.is_err() {
                                break;
                            }
                        } else if m.is_close() {
                            break;
                        }
                    }
                }
                match acc {
                    Some(a) => {
                        let Ok(t) = a.accept(s).await else { return };
                        if let Ok(ws) = tokio_tungstenite::accept_hdr_async(t, cb).await {
                            run(ws).await
                        }
                    }
                    None => {
                        if let Ok(ws) = tokio_tungstenite::accept_hdr_async(s, cb).await {
                            run(ws).await
                        }
                    }
                }
            });
        }
    });
    p
}

// ------------------------------------------------------------------ fake S3

fn hmac(k: &[u8], d: &str) -> Vec<u8> {
    let mut m = Hmac::<Sha256>::new_from_slice(k).unwrap();
    m.update(d.as_bytes());
    m.finalize().into_bytes().to_vec()
}

fn enc(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

fn dec(s: &str) -> String {
    urlencoding::decode(s).map(|c| c.into_owned()).unwrap_or_default()
}

/// An independent SigV4 check (the server side): rebuild the canonical request from what
/// arrived and compare signatures. `query` is the raw query string, `path` the raw path.
/// Returns Err(reason) when it doesn't match.
pub fn verify(method: &str, path: &str, query: &str, headers: &HashMap<String, String>, secret: &str, service: &str, presign_payload: &str) -> Result<String, String> {
    let mut q: Vec<(String, String)> = query.split('&').filter(|p| !p.is_empty()).map(|p| {
        let (k, v) = p.split_once('=').unwrap_or((p, ""));
        (dec(k), dec(v))
    }).collect();
    let presigned = q.iter().any(|(k, _)| k == "X-Amz-Signature");
    let (cred, signed, sig, date, payload) = if presigned {
        let get = |n: &str| q.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone()).unwrap_or_default();
        let r = (get("X-Amz-Credential"), get("X-Amz-SignedHeaders"), get("X-Amz-Signature"), get("X-Amz-Date"), presign_payload.to_string());
        q.retain(|(k, _)| k != "X-Amz-Signature");
        r
    } else {
        let a = headers.get("authorization").ok_or("no authorization")?;
        let a = a.strip_prefix("AWS4-HMAC-SHA256 ").ok_or("not sigv4")?;
        let mut m = BTreeMap::new();
        for part in a.split(", ") {
            let (k, v) = part.split_once('=').ok_or("bad authorization")?;
            m.insert(k.to_string(), v.to_string());
        }
        let payload = headers.get("x-amz-content-sha256").cloned().unwrap_or_else(|| hex::encode(Sha256::digest(b"")));
        (m["Credential"].clone(), m["SignedHeaders"].clone(), m["Signature"].clone(), headers.get("x-amz-date").cloned().ok_or("no x-amz-date")?, payload)
    };
    let mut cq: Vec<(String, String)> = q.iter().map(|(k, v)| (enc(k), enc(v))).collect();
    cq.sort();
    let cq = cq.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&");
    let mut ch = String::new();
    for n in signed.split(';') {
        let v = headers.get(n).ok_or(format!("signed header {n} missing"))?;
        ch.push_str(&format!("{n}:{}\n", v.split_whitespace().collect::<Vec<_>>().join(" ")));
    }
    let creq = format!("{method}\n{path}\n{cq}\n{ch}\n{signed}\n{payload}");
    let (ak, scope) = cred.split_once('/').ok_or("bad credential")?;
    let parts: Vec<&str> = scope.split('/').collect();
    if parts.len() != 4 || parts[2] != service {
        return Err(format!("bad scope {scope}"));
    }
    let sts = format!("AWS4-HMAC-SHA256\n{date}\n{scope}\n{}", hex::encode(Sha256::digest(creq.as_bytes())));
    let k = hmac(format!("AWS4{secret}").as_bytes(), parts[0]);
    let k = hmac(&k, parts[1]);
    let k = hmac(&k, parts[2]);
    let k = hmac(&k, "aws4_request");
    let want = hex::encode(hmac(&k, &sts));
    if want == sig {
        Ok(ak.to_string())
    } else {
        Err(format!("SignatureDoesNotMatch (canonical request:\n{creq})"))
    }
}

#[derive(Default)]
pub struct S3State {
    pub objects: Mutex<BTreeMap<String, (String, Vec<u8>, Vec<(String, String)>)>>,
    pub requests: Mutex<Vec<String>>,
}

/// A path-style fake S3 (one access key/secret). Every request must carry a valid SigV4
/// signature (header or presigned query), else 403 SignatureDoesNotMatch.
pub fn s3_fake(access: &'static str, secret: &'static str, st: Arc<S3State>) -> Router {
    Router::new().fallback(any(move |req: Request| {
        let st = st.clone();
        async move {
            use axum::http::StatusCode;
            use axum::response::IntoResponse;
            let (parts, body) = req.into_parts();
            let body = axum::body::to_bytes(body, 200 << 20).await.unwrap();
            let headers: HashMap<String, String> = parts.headers.iter().map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string())).collect();
            let path = parts.uri.path().to_string();
            let query = parts.uri.query().unwrap_or("").to_string();
            match verify(parts.method.as_str(), &path, &query, &headers, secret, "s3", "UNSIGNED-PAYLOAD") {
                Ok(ak) if ak == access => {}
                Ok(ak) => return (StatusCode::FORBIDDEN, format!("<Error><Code>InvalidAccessKeyId</Code><Message>{ak}</Message></Error>")).into_response(),
                Err(e) => return (StatusCode::FORBIDDEN, format!("<Error><Code>SignatureDoesNotMatch</Code><Message>{e}</Message></Error>")).into_response(),
            }
            st.requests.lock().unwrap().push(format!("{} {path}?{query}", parts.method));
            let segs: Vec<String> = path.trim_start_matches('/').splitn(2, '/').map(dec).collect();
            let (bucket, key) = (segs.first().cloned().unwrap_or_default(), segs.get(1).cloned().unwrap_or_default());
            let id = format!("{bucket}/{key}");
            let m = parts.method.as_str();
            if bucket.is_empty() {
                return ([("content-type", "application/xml")], "<ListAllMyBucketsResult><Buckets><Bucket><Name>photos</Name></Bucket></Buckets></ListAllMyBucketsResult>".to_string()).into_response();
            }
            if key.is_empty() && m == "GET" {
                assert!(query.contains("list-type=2"), "{query}");
                let prefix = query.split('&').find_map(|p| p.strip_prefix("prefix=")).map(dec).unwrap_or_default();
                let objs = st.objects.lock().unwrap();
                let items: String = objs.iter().filter(|(k, _)| k.starts_with(&format!("{bucket}/{prefix}"))).map(|(k, (_, b, _))| {
                    format!("<Contents><Key>{}</Key><Size>{}</Size></Contents>", &k[bucket.len() + 1..], b.len())
                }).collect();
                return ([("content-type", "application/xml")], format!("<ListBucketResult><Name>{bucket}</Name>{items}</ListBucketResult>")).into_response();
            }
            match m {
                "PUT" => {
                    let meta: Vec<(String, String)> = headers.iter().filter(|(k, _)| k.starts_with("x-amz-meta-")).map(|(k, v)| (k.clone(), v.clone())).collect();
                    st.objects.lock().unwrap().insert(id, (headers.get("content-type").cloned().unwrap_or_default(), body.to_vec(), meta));
                    ([("etag", "\"abc\"")], "").into_response()
                }
                "GET" | "HEAD" => {
                    let Some((ct, b, meta)) = st.objects.lock().unwrap().get(&id).cloned() else {
                        return (StatusCode::NOT_FOUND, "<Error><Code>NoSuchKey</Code></Error>").into_response();
                    };
                    let mut r = if let Some(range) = headers.get("range").and_then(|r| r.strip_prefix("bytes=")) {
                        let (a, z) = range.split_once('-').unwrap();
                        let (a, z): (usize, usize) = (a.parse().unwrap(), z.parse::<usize>().unwrap().min(b.len() - 1));
                        let mut r = axum::response::Response::new(axum::body::Body::from(b[a..=z].to_vec()));
                        *r.status_mut() = StatusCode::PARTIAL_CONTENT;
                        r.headers_mut().insert("content-range", format!("bytes {a}-{z}/{}", b.len()).parse().unwrap());
                        r
                    } else {
                        let mut r = axum::response::Response::new(if m == "HEAD" { axum::body::Body::empty() } else { axum::body::Body::from(b.clone()) });
                        r.headers_mut().insert("content-length", b.len().to_string().parse().unwrap());
                        r
                    };
                    r.headers_mut().insert("content-type", ct.parse().unwrap());
                    r.headers_mut().insert("etag", "\"abc\"".parse().unwrap());
                    for (k, v) in meta {
                        r.headers_mut().insert(axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(), v.parse().unwrap());
                    }
                    r
                }
                "DELETE" => {
                    st.objects.lock().unwrap().remove(&id);
                    StatusCode::NO_CONTENT.into_response()
                }
                _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
            }
        }
    }))
}

/// The fake's verifier accepts AWS's own test-suite request (get-vanilla) — so it is a fair judge.
pub fn verifier_accepts_aws_vector() {
    let mut h = HashMap::new();
    h.insert("host".to_string(), "example.amazonaws.com".to_string());
    h.insert("x-amz-date".to_string(), "20150830T123600Z".to_string());
    h.insert("authorization".to_string(), "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, SignedHeaders=host;x-amz-date, Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31".to_string());
    assert_eq!(verify("GET", "/", "", &h, "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY", "service", "").unwrap(), "AKIDEXAMPLE");
    h.insert("authorization".to_string(), h["authorization"].replace("5fa0", "5fa1"));
    assert!(verify("GET", "/", "", &h, "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY", "service", "").is_err());
    let q = "X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=AKIDEXAMPLE%2F20150830%2Fus-east-1%2Fservice%2Faws4_request&X-Amz-Date=20150830T123600Z&X-Amz-SignedHeaders=host&X-Amz-Expires=3600&X-Amz-Signature=e93c787ed7f371d5c6b165c1b38ede9550f4dce4144713e844b25b7192d3865d";
    let mut h2 = HashMap::new();
    h2.insert("host".to_string(), "example.amazonaws.com".to_string());
    // The suite's query vector signs the empty-payload hash (S3 presigning uses UNSIGNED-PAYLOAD).
    let empty = hex::encode(Sha256::digest(b""));
    assert_eq!(verify("GET", "/", q, &h2, "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY", "service", &empty).unwrap(), "AKIDEXAMPLE");
    assert!(verify("GET", "/", q, &h2, "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY", "service", "UNSIGNED-PAYLOAD").is_err());
}
