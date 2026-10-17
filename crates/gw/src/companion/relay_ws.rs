//! `/v1/relay/ws?target=<ws(s)://host[:port]/path>[&insecure=1]` (WebSocket) — a WebSocket
//! proxy: LG webOS SSAP (ws://tv:3000, wss://tv:3001), the Home Assistant WebSocket API, Neo4j
//! over WebSocket, the Samsung Tizen remote (wss 8002), MQTT over WebSocket.
//!
//! * A LAN target (see [`super::lan`]) must be on `lan_hosts` — the same allow-list, and the
//!   same first-use approval, as relay/lan. `insecure=1` accepts a self-signed certificate.
//! * A public target must be `wss://` and its host on `ws_allow` (added by the user; never by
//!   a prompt). `insecure` is refused for it.
//! * The connection goes to the resolved, checked address (no second lookup to rebind).
//! * Text and binary frames pass through unchanged; pings are answered at each end; a close is
//!   passed on. Subprotocols the page offers besides the companion's (`gw-companion.*`,
//!   `bearer.*`) are offered upstream, and the one the upstream picks is picked for the page.
//! * The token comes as `bearer.<token>` in `Sec-WebSocket-Protocol`; with `gw-companion.codes`
//!   offered, refusals are close codes (4400, 4401, 4403, 4502) as for relay/tcp.

use super::{check_token, err, lan, pairing::TokenRecord, relay_lan, settings::parse_host_entry, tagged, wants_close_codes, ws_close_code, ws_refuse, Gateway, Origin, WS_CODES};
use axum::{
    extract::{
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::{header, HeaderMap, StatusCode},
    response::Response,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::tungstenite::{self as tg, client::IntoClientRequest};

#[derive(Deserialize)]
pub struct WsQuery {
    #[serde(default)]
    target: String,
    #[serde(default)]
    insecure: String,
}

pub trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

/// Is `host`:`port` on the public `wss://` allow-list?
pub fn ws_listed(host: &str, port: u16, list: &[String]) -> bool {
    list.iter().filter_map(|e| parse_host_entry(e)).any(|(h, p)| h == host && p.is_none_or(|p| p == port))
}

/// rustls configs: webpki roots, or (LAN `insecure=1`) any certificate.
pub fn tls_config(insecure: bool) -> Arc<rustls::ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let b = rustls::ClientConfig::builder_with_provider(provider.clone()).with_safe_default_protocol_versions().expect("tls versions");
    let mut c = if insecure {
        b.dangerous().with_custom_certificate_verifier(Arc::new(NoVerify(provider))).with_no_client_auth()
    } else {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        b.with_root_certificates(roots).with_no_client_auth()
    };
    c.alpn_protocols = vec![b"http/1.1".to_vec()];
    Arc::new(c)
}

/// Accepts any server certificate (signatures are still checked) — LAN self-signed devices.
#[derive(Debug)]
struct NoVerify(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(&self, m: &[u8], c: &rustls::pki_types::CertificateDer<'_>, d: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(m, c, d, &self.0.signature_verification_algorithms)
    }
    fn verify_tls13_signature(&self, m: &[u8], c: &rustls::pki_types::CertificateDer<'_>, d: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(m, c, d, &self.0.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

pub async fn handle(
    State(gw): State<Arc<Gateway>>,
    axum::Extension(Origin(origin)): axum::Extension<Origin>,
    h: HeaderMap,
    Query(q): Query<WsQuery>,
    ws: Result<WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>,
) -> Response {
    let Ok(ws) = ws else {
        return err(StatusCode::BAD_REQUEST, "this endpoint is a WebSocket");
    };
    let codes = wants_close_codes(&h);
    let refuse = |ws: WebSocketUpgrade, status: StatusCode, msg: String, rec: Option<&TokenRecord>| {
        let r = if codes { ws_refuse(ws, ws_close_code(status), &msg) } else { err(status, msg) };
        match rec {
            Some(rec) => tagged(r, rec),
            None => r,
        }
    };
    if !gw.settings().relays.ws {
        return refuse(ws, StatusCode::SERVICE_UNAVAILABLE, "the WebSocket relay is turned off in the companion settings".into(), None);
    }
    let (rec, presented) = match check_token(&gw, &h, &origin, "relay.ws") {
        Ok(x) => x,
        Err((status, msg)) => return refuse(ws, status, msg, None),
    };
    let insecure = matches!(q.insecure.as_str(), "1" | "true");
    let (url, host, port, tls) = match parse(&q.target) {
        Ok(x) => x,
        Err(m) => return refuse(ws, StatusCode::BAD_REQUEST, m, Some(&rec)),
    };
    // Where to connect: a public wss:// host the user listed, or a LAN host (allow-list / approval).
    let s = gw.settings();
    let public = tls && ws_listed(&host, port, &s.ws_allow) && lan::literal(&host).is_none_or(|ip| !lan::ip_lan(ip));
    let addrs: Vec<IpAddr> = if public {
        if insecure {
            return refuse(ws, StatusCode::FORBIDDEN, "insecure=1 is only for LAN devices".into(), Some(&rec));
        }
        match tokio::net::lookup_host((host.as_str(), port)).await {
            Ok(it) => {
                let a: Vec<IpAddr> = it.map(|s| s.ip()).collect();
                if a.iter().any(|ip| ip.is_loopback() || ip.is_unspecified()) {
                    return refuse(ws, StatusCode::FORBIDDEN, format!("{host} resolves to this machine"), Some(&rec));
                }
                a
            }
            Err(e) => return refuse(ws, StatusCode::BAD_GATEWAY, format!("cannot resolve {host}: {e}"), Some(&rec)),
        }
    } else {
        let detail = format!("WebSocket {}", q.target.chars().take(160).collect::<String>());
        match relay_lan::admit(&gw, &host, port, &origin, &rec.tool, &detail).await {
            Ok(a) => a,
            Err((code, mut msg)) => {
                if !tls && msg.contains("public address") {
                    msg.push_str("; public WebSocket targets must be wss:// and on the companion's ws_allow list");
                } else if msg.contains("public address") {
                    msg.push_str("; to reach this public wss:// host, add it to ws_allow in the Companion settings");
                }
                return refuse(ws, code, msg, Some(&rec));
            }
        }
    };
    // The page's own subprotocols (not ours) are offered upstream.
    let theirs: Vec<String> = h
        .get_all(header::SEC_WEBSOCKET_PROTOCOL)
        .iter()
        .flat_map(|v| v.to_str().unwrap_or("").split(',').map(|p| p.trim().to_string()).collect::<Vec<_>>())
        .filter(|p| !p.is_empty() && !p.starts_with("gw-companion.") && !p.starts_with("bearer."))
        .collect();
    let upstream = match connect(&url, &host, port, tls, insecure, &addrs, &theirs).await {
        Ok(x) => x,
        Err(m) => return refuse(ws, StatusCode::BAD_GATEWAY, m, Some(&rec)),
    };
    let (up, picked) = upstream;
    let mut protos: Vec<String> = vec![];
    if let Some(p) = picked {
        protos.push(p);
    } else {
        protos.push("gw-companion.v1".into());
        protos.extend(presented.subprotocol);
    }
    debug_assert!(!protos.iter().any(|p| p == WS_CODES));
    let r = ws.protocols(protos).max_message_size(16 << 20).on_upgrade(move |sock| pump(sock, up));
    tagged(r, &rec)
}

/// `ws(s)://host[:port]/path` → (url, host, port, tls).
pub fn parse(target: &str) -> Result<(String, String, u16, bool), String> {
    let t = target.trim();
    if t.is_empty() {
        return Err("missing ?target=ws://host:port/path".into());
    }
    let u = reqwest::Url::parse(t).map_err(|e| format!("bad target {t:?}: {e}"))?;
    let tls = match u.scheme() {
        "ws" => false,
        "wss" => true,
        s => return Err(format!("target scheme {s:?}: ws:// or wss:// only")),
    };
    if !u.username().is_empty() || u.password().is_some() || u.fragment().is_some() {
        return Err("target may not have user:password@ or a #fragment".into());
    }
    let host = u.host_str().ok_or("target has no host")?.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    let port = u.port_or_known_default().unwrap_or(if tls { 443 } else { 80 });
    Ok((u.to_string(), host, port, tls))
}

type Upstream = tokio_tungstenite::WebSocketStream<Box<dyn Io>>;

async fn connect(url: &str, host: &str, port: u16, tls: bool, insecure: bool, addrs: &[IpAddr], protos: &[String]) -> Result<(Upstream, Option<String>), String> {
    let mut last = format!("cannot connect to {host}:{port}");
    let mut tcp = None;
    for ip in addrs {
        match tokio::time::timeout(Duration::from_secs(10), tokio::net::TcpStream::connect(SocketAddr::new(*ip, port))).await {
            Ok(Ok(s)) => {
                tcp = Some(s);
                break;
            }
            Ok(Err(e)) => last = format!("cannot connect to {host}:{port}: {e}"),
            Err(_) => last = format!("timed out connecting to {host}:{port}"),
        }
    }
    let tcp = tcp.ok_or(last)?;
    let _ = tcp.set_nodelay(true);
    let io: Box<dyn Io> = if tls {
        let name = match lan::literal(host) {
            Some(ip) => rustls::pki_types::ServerName::IpAddress(ip.into()),
            None => rustls::pki_types::ServerName::try_from(host.to_string()).map_err(|e| format!("bad TLS name {host}: {e}"))?,
        };
        let c = tokio_rustls::TlsConnector::from(tls_config(insecure));
        match tokio::time::timeout(Duration::from_secs(10), c.connect(name, tcp)).await {
            Ok(Ok(s)) => Box::new(s),
            Ok(Err(e)) => return Err(format!("TLS with {host}:{port} failed: {e}{}", if insecure { "" } else { " (a self-signed LAN device needs &insecure=1)" })),
            Err(_) => return Err(format!("TLS with {host}:{port} timed out")),
        }
    } else {
        Box::new(tcp)
    };
    let mut req = url.into_client_request().map_err(|e| format!("bad target: {e}"))?;
    if !protos.is_empty() {
        req.headers_mut().insert("sec-websocket-protocol", protos.join(", ").parse().map_err(|_| "bad subprotocol".to_string())?);
    }
    let cfg = tg::protocol::WebSocketConfig::default().max_message_size(Some(16 << 20));
    match tokio::time::timeout(Duration::from_secs(15), tokio_tungstenite::client_async_with_config(req, io, Some(cfg))).await {
        Ok(Ok((s, resp))) => {
            let picked = resp.headers().get("sec-websocket-protocol").and_then(|v| v.to_str().ok()).map(str::to_string);
            Ok((s, picked))
        }
        Ok(Err(e)) => Err(format!("WebSocket handshake with {host}:{port} failed: {e}")),
        Err(_) => Err(format!("WebSocket handshake with {host}:{port} timed out")),
    }
}

async fn pump(sock: WebSocket, up: Upstream) {
    let (mut tx, mut rx) = sock.split();
    let (mut utx, mut urx) = up.split();
    let to_up = async {
        while let Some(Ok(m)) = rx.next().await {
            let out = match m {
                Message::Text(t) => tg::Message::Text(t.as_str().into()),
                Message::Binary(b) => tg::Message::Binary(b.to_vec().into()),
                Message::Close(c) => {
                    let f = c.map(|c| tg::protocol::CloseFrame { code: c.code.into(), reason: c.reason.as_str().to_string().into() });
                    let _ = utx.send(tg::Message::Close(f)).await;
                    break;
                }
                _ => continue,
            };
            if utx.send(out).await.is_err() {
                break;
            }
        }
        let _ = utx.close().await;
    };
    let to_page = async {
        while let Some(Ok(m)) = urx.next().await {
            let out = match m {
                tg::Message::Text(t) => Message::Text(t.as_str().to_string().into()),
                tg::Message::Binary(b) => Message::Binary(b.to_vec().into()),
                tg::Message::Close(c) => {
                    let f = c.map(|c| CloseFrame { code: c.code.into(), reason: c.reason.as_str().to_string().into() });
                    let _ = tx.send(Message::Close(f)).await;
                    return;
                }
                _ => continue,
            };
            if tx.send(out).await.is_err() {
                return;
            }
        }
        let _ = tx.send(Message::Close(None)).await;
    };
    tokio::select! {
        _ = to_up => {},
        _ = to_page => {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn targets() {
        assert_eq!(parse("ws://TV.local:3000/").unwrap().1, "tv.local");
        assert_eq!(parse("wss://tv.local/x").unwrap().2, 443);
        assert!(parse("http://x/").is_err() && parse("ws://u:p@x/").is_err() && parse("").is_err());
        assert!(ws_listed("mqtt.example.org", 443, &["mqtt.example.org".into()]));
        assert!(!ws_listed("mqtt.example.org", 8443, &["mqtt.example.org:443".into()]));
    }
}
