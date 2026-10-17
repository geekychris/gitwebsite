//! Companion v1.2 end to end: relay/lan (allow-list, first-use approval, public-IP refusal,
//! DNS-rebinding protection, self-signed TLS, header passing), relay/ws (plain and self-signed
//! wss, approval, subprotocols, close codes), relay/s3 (a fake S3 that verifies every SigV4
//! signature), TCP first-use approval and the new fixed upstreams.

mod common;

use common::*;
use futures_util::{SinkExt, StreamExt};
use gw::companion::{secrets::Store, settings::{S3Profile, Settings}, Gateway};
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};

async fn gateway(s: Settings, secrets: Store) -> (Arc<Gateway>, String) {
    s.validate().unwrap();
    let gw = Arc::new(Gateway::new(s, None, None, secrets));
    let p = serve(gw::companion::router(gw.clone())).await;
    gw.port.store(p, Ordering::Relaxed);
    gw.ui_port.store(1, Ordering::Relaxed);
    (gw, format!("http://127.0.0.1:{p}"))
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().timeout(Duration::from_secs(30)).build().unwrap()
}

/// Pair from the site origin with `scopes` (approved directly, as the app would).
async fn pair(gw: &Gateway, base: &str, scopes: &[&str]) -> String {
    let v: Value = client().post(format!("{base}/v1/pair")).header("origin", SITE).json(&json!({"tool": "t12", "scopes": scopes})).send().await.unwrap().json().await.unwrap();
    gw.pairing.decide(v["code"].as_str().unwrap(), true, &gw.tokens).unwrap();
    let v: Value = client().get(format!("{base}{}", v["poll"].as_str().unwrap())).header("origin", SITE).send().await.unwrap().json().await.unwrap();
    v["token"].as_str().unwrap().to_string()
}

/// Wait until an approval of `kind` is pending and return it.
async fn pending(gw: &Gateway, kind: &str) -> gw::companion::approvals::ApprovalRequest {
    for _ in 0..200 {
        if let Some(a) = gw.approvals.list().into_iter().find(|a| a.kind == kind) {
            return a;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("no {kind} approval appeared");
}

#[tokio::test]
async fn hello_is_v12_and_new_scopes_are_opt_in() {
    let (gw, base) = gateway(Settings::default(), Store::memory(&[])).await;
    let v: Value = client().get(format!("{base}/v1/hello")).header("origin", SITE).send().await.unwrap().json().await.unwrap();
    assert_eq!(v["version"], "1.2");
    for c in ["relay.lan", "relay.ws", "relay.s3", "relay.http", "relay.tcp", "view"] {
        assert!(v["capabilities"].as_array().unwrap().contains(&json!(c)), "{c}");
    }
    // An empty scope list is the v1 relays only; the new ones must be asked for.
    let t = pair(&gw, &base, &[]).await;
    let v: Value = client().get(format!("{base}/v1/hello")).header("origin", SITE).bearer_auth(&t).send().await.unwrap().json().await.unwrap();
    assert!(!v["scopes"].as_array().unwrap().contains(&json!("relay.lan")));
    let r = client().get(format!("{base}/v1/relay/lan/127.0.0.1:1/x")).header("origin", SITE).bearer_auth(&t).send().await.unwrap();
    assert_eq!(r.status(), 403);
    // Preflight allows the new headers and methods.
    let r = client().request(reqwest::Method::OPTIONS, format!("{base}/v1/relay/lan/hue.local/api")).header("origin", SITE)
        .header("access-control-request-method", "PATCH").send().await.unwrap();
    assert!(r.headers()["access-control-allow-methods"].to_str().unwrap().contains("PATCH"));
    // The new fixed upstreams are listed.
    let st = gw.status();
    for u in ["noaa-coops", "ndbc", "pskreporter", "open-meteo", "open-elevation"] {
        assert!(st["upstreams"].as_array().unwrap().contains(&json!(u)), "{u}");
    }
}

#[tokio::test]
async fn lan_relay_approval_headers_and_refusals() {
    let up = serve(echo()).await;
    let (gw, base) = gateway(Settings::default(), Store::memory(&[])).await;
    let t = pair(&gw, &base, &["relay.lan"]).await;
    let c = client();
    // First request to a new LAN host waits for approval; approving adds the host.
    let req = {
        let (u, t) = (format!("{base}/v1/relay/lan/127.0.0.1:{up}/api/states?x=1&tls=0"), t.clone());
        tokio::spawn(async move {
            client().post(u).header("origin", SITE).bearer_auth(&t).header("x-upstream-authorization", "Bearer HA-TOKEN")
                .header("hue-application-key", "HUE").header("x-auth-token", "XT").header("cookie", "a=b")
                .header("content-type", "application/json").body("{\"on\":true}").send().await.unwrap()
        })
    };
    let a = pending(&gw, "lan").await;
    assert_eq!((a.target.as_str(), a.origin.as_str(), a.tool.as_str()), ("127.0.0.1", SITE, "t12"));
    assert!(a.detail.starts_with("POST http://127.0.0.1:"), "{}", a.detail);
    gw.decide_approval(&a.code, true, "test").unwrap();
    let r = req.await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["x-relay-upstream"], "lan");
    let v: Value = r.json().await.unwrap();
    assert_eq!((v["method"].as_str(), v["path"].as_str(), v["query"].as_str()), (Some("POST"), Some("/api/states"), Some("x=1")));
    assert_eq!((v["authorization"].as_str(), v["hue"].as_str(), v["x_auth_token"].as_str()), (Some("Bearer HA-TOKEN"), Some("HUE"), Some("XT")));
    assert_eq!((v["cookie"].as_str(), v["origin"].as_str(), v["x_upstream"].as_str()), (Some(""), Some(""), Some("")));
    assert_eq!(v["body"], "{\"on\":true}");
    assert_eq!(gw.settings().lan_hosts, vec!["127.0.0.1"]);
    assert!(gw.audit_tail(5).iter().any(|e| e["event"] == "approval" && e["decision"] == "allowed" && e["target"] == "127.0.0.1"));
    // Now allowed without asking; localhost is the same host. The companion token is never forwarded.
    let v: Value = c.get(format!("{base}/v1/relay/lan/localhost:{up}/x")).header("origin", SITE).bearer_auth(&t).send().await.unwrap().json().await.unwrap();
    assert_eq!((v["path"].as_str(), v["authorization"].as_str()), (Some("/x"), Some("")));
    // A redirect is not followed by the browser: Location comes back as X-Relay-Location.
    let r = c.get(format!("{base}/v1/relay/lan/127.0.0.1:{up}/redir")).header("origin", SITE).bearer_auth(&t).send().await.unwrap();
    assert_eq!(r.status(), 302);
    assert!(r.headers().get("location").is_none());
    assert_eq!(r.headers()["x-relay-location"], "http://8.8.8.8/elsewhere");

    let status = |p: String| {
        let (c, t) = (client(), t.clone());
        async move { c.get(p).header("origin", SITE).bearer_auth(&t).send().await.unwrap().status().as_u16() }
    };
    // Public addresses are refused at once (no prompt).
    assert_eq!(status(format!("{base}/v1/relay/lan/8.8.8.8/x")).await, 403);
    assert_eq!(status(format!("{base}/v1/relay/lan/[2001:4860::8888]/x")).await, 403);
    // DNS rebinding: names that resolve (even partly) to public addresses, or a name that
    // resolves to loopback, are refused — even when listed.
    gw.dns.write().unwrap().insert("evil.local".into(), vec!["8.8.8.8".parse().unwrap()]);
    gw.dns.write().unwrap().insert("mixed.test".into(), vec!["192.168.77.1".parse().unwrap(), "1.1.1.1".parse().unwrap()]);
    gw.dns.write().unwrap().insert("rebind.test".into(), vec!["127.0.0.1".parse().unwrap()]);
    gw.settings.write().unwrap().lan_hosts.extend(["evil.local".to_string(), "mixed.test".into(), "rebind.test".into()]);
    for h in ["evil.local", "mixed.test", format!("rebind.test:{up}").as_str()] {
        assert_eq!(status(format!("{base}/v1/relay/lan/{h}/x")).await, 403, "{h}");
    }
    assert!(gw.approvals.list().is_empty(), "public targets never prompt");
    // The connect-time check (reqwest's resolver) refuses a name that flips to a public address.
    gw.dns.write().unwrap().insert("flip.test".into(), vec!["8.8.4.4".parse().unwrap()]);
    let e = gw.lan_http.get("http://flip.test/").send().await.unwrap_err();
    assert!(format!("{:?}", e).contains("public address"), "{e:?}");
    // The companion's and the app's own ports are never reachable.
    let own = gw.port.load(Ordering::Relaxed);
    assert_eq!(status(format!("{base}/v1/relay/lan/127.0.0.1:{own}/v1/status")).await, 403);
    assert_eq!(status(format!("{base}/v1/relay/lan/127.0.0.1:8790/api/companion")).await, 403);
    // Escapes.
    assert_eq!(status(format!("{base}/v1/relay/lan/127.0.0.1:{up}/..%2f..%2fx")).await, 400);
    assert_eq!(status(format!("{base}/v1/relay/lan/127.0.0.1:{up}/a?u=http://evil.test")).await, 400);
    assert_eq!(status(format!("{base}/v1/relay/lan/user@127.0.0.1/x")).await, 400);
    // Denied: refused now, and again at once (no second prompt) for a while.
    let req = {
        let (u, t) = (format!("{base}/v1/relay/lan/127.0.0.2:{up}/x"), t.clone());
        tokio::spawn(async move { client().get(u).header("origin", SITE).bearer_auth(&t).send().await.unwrap() })
    };
    let a = pending(&gw, "lan").await;
    gw.decide_approval(&a.id, false, "test").unwrap();
    let r = req.await.unwrap();
    assert_eq!(r.status(), 403);
    assert!(r.text().await.unwrap().contains("denied"));
    assert_eq!(status(format!("{base}/v1/relay/lan/127.0.0.2:{up}/y")).await, 403);
    assert!(gw.approvals.list().is_empty());
    // Methods.
    let r = c.request(reqwest::Method::from_bytes(b"PROPFIND").unwrap(), format!("{base}/v1/relay/lan/127.0.0.1:{up}/x")).header("origin", SITE).bearer_auth(&t).send().await.unwrap();
    assert_eq!(r.status(), 405);
    // The relay can be turned off.
    gw.settings.write().unwrap().relays.lan = false;
    assert_eq!(status(format!("{base}/v1/relay/lan/127.0.0.1:{up}/x")).await, 503);
}

#[tokio::test]
async fn lan_relay_self_signed_tls_only_with_insecure() {
    let tls = https_fake().await;
    let mut s = Settings::default();
    s.lan_hosts = vec!["127.0.0.1".into()];
    let (gw, base) = gateway(s, Store::memory(&[])).await;
    let t = pair(&gw, &base, &["relay.lan"]).await;
    let c = client();
    let r = c.get(format!("{base}/v1/relay/lan/127.0.0.1:{tls}/api/0/config?tls=1")).header("origin", SITE).bearer_auth(&t).send().await.unwrap();
    assert_eq!(r.status(), 502);
    let msg = r.text().await.unwrap();
    assert!(msg.to_lowercase().contains("certificate"), "{msg}");
    let r = c.get(format!("{base}/v1/relay/lan/127.0.0.1:{tls}/api/0/config?tls=1&insecure=1&a=b")).header("origin", SITE).bearer_auth(&t).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    assert_eq!((v["tls"].as_bool(), v["path"].as_str()), (Some(true), Some("/api/0/config?a=b")));
}

fn ws_req(url: String, proto: &str) -> tokio_tungstenite::tungstenite::handshake::client::Request {
    let mut req = url.into_client_request().unwrap();
    req.headers_mut().insert("origin", SITE.parse().unwrap());
    req.headers_mut().insert("sec-websocket-protocol", proto.parse().unwrap());
    req
}

async fn close_code(req: tokio_tungstenite::tungstenite::handshake::client::Request) -> (u16, String) {
    let (mut ws, resp) = tokio_tungstenite::connect_async(req).await.expect("upgrade accepted");
    assert_eq!(resp.headers()["sec-websocket-protocol"], "gw-companion.codes");
    match tokio::time::timeout(Duration::from_secs(10), ws.next()).await.unwrap().unwrap().unwrap() {
        Message::Close(Some(f)) => (u16::from(f.code), f.reason.to_string()),
        other => panic!("expected a close, got {other:?}"),
    }
}

#[tokio::test]
async fn ws_relay_plain_and_self_signed() {
    let plain = ws_echo(false).await;
    let secure = ws_echo(true).await;
    let (gw, base) = gateway(Settings::default(), Store::memory(&[])).await;
    let t = pair(&gw, &base, &["relay.ws"]).await;
    let ws_base = base.replace("http://", "ws://");
    // First connection to a new LAN host waits for the approval.
    let url = format!("{ws_base}/v1/relay/ws?target=ws://127.0.0.1:{plain}/ssap");
    let conn = tokio::spawn(tokio_tungstenite::connect_async(ws_req(url, &format!("gw-companion.v1, bearer.{t}"))));
    let a = pending(&gw, "lan").await;
    assert!(a.detail.contains("ws://127.0.0.1"), "{}", a.detail);
    gw.decide_approval(&a.code, true, "test").unwrap();
    let (mut ws, resp) = conn.await.unwrap().unwrap();
    assert_eq!(resp.headers()["sec-websocket-protocol"], "gw-companion.v1");
    ws.send(Message::Text("{\"type\":\"register\"}".into())).await.unwrap();
    let m = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.unwrap().unwrap().unwrap();
    assert_eq!(m.into_text().unwrap().as_str(), "{\"type\":\"register\"}");
    ws.send(Message::Binary(vec![1u8, 2, 3].into())).await.unwrap();
    let m = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.unwrap().unwrap().unwrap();
    assert_eq!(m.into_data().to_vec(), vec![1u8, 2, 3]);
    // The page's own subprotocol goes upstream and the upstream's pick comes back.
    let (_ws, resp) = tokio_tungstenite::connect_async(ws_req(format!("{ws_base}/v1/relay/ws?target=ws://127.0.0.1:{plain}/mqtt"), &format!("mqtt, gw-companion.v1, bearer.{t}"))).await.unwrap();
    assert_eq!(resp.headers()["sec-websocket-protocol"], "mqtt");
    // Self-signed wss: refused without insecure=1, works with it.
    let codes = format!("gw-companion.v1, gw-companion.codes, bearer.{t}");
    let (code, why) = close_code(ws_req(format!("{ws_base}/v1/relay/ws?target=wss://127.0.0.1:{secure}/"), &codes)).await;
    assert_eq!(code, 4502);
    assert!(why.contains("TLS"), "{why}");
    let (mut ws, _) = tokio_tungstenite::connect_async(ws_req(format!("{ws_base}/v1/relay/ws?target=wss://127.0.0.1:{secure}/&insecure=1"), &codes)).await.unwrap();
    ws.send(Message::Text("hi".into())).await.unwrap();
    let m = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.unwrap().unwrap().unwrap();
    assert_eq!(m.into_text().unwrap().as_str(), "hi");
    // Public targets: ws:// never; wss:// only when on ws_allow, never insecure.
    let (code, why) = close_code(ws_req(format!("{ws_base}/v1/relay/ws?target=ws://8.8.8.8/"), &codes)).await;
    assert_eq!(code, 4403);
    assert!(why.contains("public"), "{why}");
    gw.settings.write().unwrap().ws_allow = vec!["8.8.8.8".into()];
    let (code, _) = close_code(ws_req(format!("{ws_base}/v1/relay/ws?target=wss://8.8.8.8/&insecure=1"), &codes)).await;
    assert_eq!(code, 4403);
    // Rebinding: a listed name resolving to a public address.
    gw.dns.write().unwrap().insert("tv.local".into(), vec!["93.184.216.34".parse().unwrap()]);
    gw.settings.write().unwrap().lan_hosts.push("tv.local".into());
    let (code, _) = close_code(ws_req(format!("{ws_base}/v1/relay/ws?target=ws://tv.local:3000/"), &codes)).await;
    assert_eq!(code, 4403);
    // Bad target, wrong scope, unknown token.
    let (code, _) = close_code(ws_req(format!("{ws_base}/v1/relay/ws?target=http://x/"), &codes)).await;
    assert_eq!(code, 4400);
    let (code, _) = close_code(ws_req(format!("{ws_base}/v1/relay/ws?target=ws://127.0.0.1:{plain}/"), "gw-companion.codes, bearer.nope")).await;
    assert_eq!(code, 4401);
    let lan_only = pair(&gw, &base, &["relay.lan"]).await;
    let (code, why) = close_code(ws_req(format!("{ws_base}/v1/relay/ws?target=ws://127.0.0.1:{plain}/"), &format!("gw-companion.codes, bearer.{lan_only}"))).await;
    assert_eq!(code, 4403);
    assert!(why.contains("scope"), "{why}");
}

#[tokio::test]
async fn tcp_first_use_approval_for_lan_targets() {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let echo_port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = l.accept().await.unwrap();
            tokio::spawn(async move {
                let (mut r, mut w) = s.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    let mut s = Settings::default();
    s.tcp_allow = vec![];
    let (gw, base) = gateway(s, Store::memory(&[])).await;
    let t = pair(&gw, &base, &["relay.tcp"]).await;
    let ws_base = base.replace("http://", "ws://");
    let conn = tokio::spawn(tokio_tungstenite::connect_async(ws_req(format!("{ws_base}/v1/relay/tcp?target=127.0.0.1:{echo_port}"), &format!("bearer.{t}"))));
    let a = pending(&gw, "tcp").await;
    assert_eq!(a.target, format!("127.0.0.1:{echo_port}"));
    gw.decide_approval(&a.code, true, "test").unwrap();
    let (mut ws, _) = conn.await.unwrap().unwrap();
    ws.send(Message::Binary(vec![9u8].into())).await.unwrap();
    let m = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.unwrap().unwrap().unwrap();
    assert_eq!(m.into_data().to_vec(), vec![9u8]);
    assert_eq!(gw.settings().tcp_allow, vec![format!("127.0.0.1:{echo_port}")]);
    // Public targets are not offered for approval; own ports are refused.
    let codes = format!("gw-companion.codes, bearer.{t}");
    let (code, why) = close_code(ws_req(format!("{ws_base}/v1/relay/tcp?target=8.8.8.8:5432"), &codes)).await;
    assert_eq!(code, 4403);
    assert!(why.contains("allow-list"), "{why}");
    let own = gw.port.load(Ordering::Relaxed);
    let (code, _) = close_code(ws_req(format!("{ws_base}/v1/relay/tcp?target=127.0.0.1:{own}"), &codes)).await;
    assert_eq!(code, 4403);
    assert!(gw.approvals.list().is_empty());
}

#[tokio::test]
async fn fake_s3_verifier_is_checked_against_aws_vectors() {
    verifier_accepts_aws_vector();
}

#[tokio::test]
async fn s3_relay_signs_every_request() {
    let st = Arc::new(S3State::default());
    let s3 = serve(s3_fake("AKMINIO", "SECRET/MINIO+KEY", st.clone())).await;
    let mut s = Settings::default();
    s.s3_profiles.insert("minio".into(), S3Profile { endpoint: format!("http://127.0.0.1:{s3}"), region: "us-east-1".into(), path_style: true, insecure: false });
    s.s3_profiles.insert("wrong".into(), S3Profile { endpoint: format!("http://127.0.0.1:{s3}"), region: "us-east-1".into(), path_style: true, insecure: false });
    s.s3_profiles.insert("nocreds".into(), S3Profile { endpoint: format!("http://127.0.0.1:{s3}"), region: "us-east-1".into(), path_style: true, insecure: false });
    let store = Store::memory(&[
        ("s3:minio:access_key_id", "AKMINIO"), ("s3:minio:secret_access_key", "SECRET/MINIO+KEY"),
        ("s3:wrong:access_key_id", "AKMINIO"), ("s3:wrong:secret_access_key", "not-it"),
    ]);
    let (gw, base) = gateway(s, store).await;
    let t = pair(&gw, &base, &["relay.s3"]).await;
    let c = client();
    let rq = |m: reqwest::Method, p: &str| c.request(m, format!("{base}/v1/relay/s3{p}")).header("origin", SITE).bearer_auth(&t);
    // Profiles, never secrets.
    let r = rq(reqwest::Method::GET, "").send().await.unwrap();
    let body = r.text().await.unwrap();
    assert!(!body.contains("SECRET") && !body.contains("AKMINIO"), "{body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    let minio = v["profiles"].as_array().unwrap().iter().find(|p| p["name"] == "minio").unwrap().clone();
    assert_eq!((minio["credentials"].as_bool(), minio["path_style"].as_bool()), (Some(true), Some(true)));
    // PutObject (streamed, signed with UNSIGNED-PAYLOAD), with metadata.
    let r = rq(reqwest::Method::PUT, "/minio/photos/2024/hello%20world.txt").header("content-type", "text/plain").header("x-amz-meta-camera", "x100")
        .body("hello, s3 world").send().await.unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    // GetObject with Range.
    let r = rq(reqwest::Method::GET, "/minio/photos/2024/hello%20world.txt").header("range", "bytes=0-4").send().await.unwrap();
    assert_eq!(r.status(), 206);
    assert_eq!(r.headers()["content-range"], "bytes 0-4/15");
    assert_eq!(r.headers()["x-amz-meta-camera"], "x100");
    assert_eq!(r.text().await.unwrap(), "hello");
    // HeadObject.
    let r = rq(reqwest::Method::HEAD, "/minio/photos/2024/hello%20world.txt").send().await.unwrap();
    assert_eq!((r.status().as_u16(), r.headers()["content-length"].to_str().unwrap()), (200, "15"));
    // ListObjectsV2 and ListBuckets.
    let r = rq(reqwest::Method::GET, "/minio/photos?prefix=2024/").send().await.unwrap();
    assert_eq!(r.headers()["x-relay-s3-op"], "ListObjectsV2");
    assert!(r.text().await.unwrap().contains("<Key>2024/hello world.txt</Key>"));
    assert!(rq(reqwest::Method::GET, "/minio").send().await.unwrap().text().await.unwrap().contains("<Name>photos</Name>"));
    // Every request reached the fake signed (it refuses bad signatures).
    let r = rq(reqwest::Method::GET, "/wrong/photos").send().await.unwrap();
    assert_eq!(r.status(), 403);
    assert!(r.text().await.unwrap().contains("SignatureDoesNotMatch"));
    assert_eq!(rq(reqwest::Method::GET, "/nocreds/photos").send().await.unwrap().status(), 503);
    assert_eq!(rq(reqwest::Method::GET, "/nope/photos").send().await.unwrap().status(), 404);
    // Presign: the URL goes straight to S3 and the fake accepts its signature.
    let v: Value = rq(reqwest::Method::POST, "/minio/presign").json(&json!({"method": "GET", "bucket": "photos", "key": "2024/hello world.txt", "expires": 600})).send().await.unwrap().json().await.unwrap();
    let url = v["url"].as_str().unwrap();
    assert!(url.contains("X-Amz-Expires=600") && url.contains("X-Amz-Signature=") && !url.contains("SECRET"), "{url}");
    assert_eq!(client().get(url).send().await.unwrap().text().await.unwrap(), "hello, s3 world");
    let v: Value = rq(reqwest::Method::POST, "/minio/presign").json(&json!({"method": "PUT", "bucket": "photos", "key": "up.bin"})).send().await.unwrap().json().await.unwrap();
    assert_eq!(client().put(v["url"].as_str().unwrap()).body("direct").send().await.unwrap().status(), 200);
    assert_eq!(st.objects.lock().unwrap()["photos/up.bin"].1, b"direct");
    let r = rq(reqwest::Method::POST, "/minio/presign").json(&json!({"method": "DELETE", "bucket": "photos", "key": "up.bin"})).send().await.unwrap();
    assert_eq!(r.status(), 400);
    // DeleteObject waits for the app's confirmation.
    let del = {
        let (u, t) = (format!("{base}/v1/relay/s3/minio/photos/up.bin"), t.clone());
        tokio::spawn(async move { client().delete(u).header("origin", SITE).bearer_auth(&t).send().await.unwrap() })
    };
    let a = pending(&gw, "s3-delete").await;
    assert_eq!(a.target, "minio/photos/up.bin");
    gw.decide_approval(&a.id, true, "test").unwrap();
    assert_eq!(del.await.unwrap().status(), 204);
    assert!(!st.objects.lock().unwrap().contains_key("photos/up.bin"));
    assert!(gw.settings().lan_hosts.is_empty(), "an S3 confirmation adds nothing to an allow-list");
    let del = {
        let (u, t) = (format!("{base}/v1/relay/s3/minio/photos/2024/hello%20world.txt"), t.clone());
        tokio::spawn(async move { client().delete(u).header("origin", SITE).bearer_auth(&t).send().await.unwrap() })
    };
    let a = pending(&gw, "s3-delete").await;
    gw.decide_approval(&a.id, false, "test").unwrap();
    assert_eq!(del.await.unwrap().status(), 403);
    assert!(st.objects.lock().unwrap().contains_key("photos/2024/hello world.txt"));
    // Refused shapes: chunked PUT (no Content-Length), bucket ops, sub-resources, bad keys.
    let stream = futures_util::stream::iter(vec![Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"x"))]);
    let r = rq(reqwest::Method::PUT, "/minio/photos/c.bin").body(reqwest::Body::wrap_stream(stream)).send().await.unwrap();
    assert_eq!(r.status(), 411);
    assert_eq!(rq(reqwest::Method::DELETE, "/minio/photos").send().await.unwrap().status(), 405);
    assert_eq!(rq(reqwest::Method::PUT, "/minio/photos/k?acl").body("x").send().await.unwrap().status(), 405);
    assert_eq!(rq(reqwest::Method::GET, "/minio/photos?policy").send().await.unwrap().status(), 400);
    assert_eq!(rq(reqwest::Method::GET, "/minio/photos/a%00b").send().await.unwrap().status(), 400);
    // Scope.
    let lan_only = pair(&gw, &base, &["relay.lan"]).await;
    assert_eq!(c.get(format!("{base}/v1/relay/s3")).header("origin", SITE).bearer_auth(&lan_only).send().await.unwrap().status(), 403);
    // No secret value in the status or audit.
    let all = format!("{} {:?}", gw.status(), gw.audit_tail(100));
    assert!(!all.contains("SECRET/MINIO"), "{all}");
}
