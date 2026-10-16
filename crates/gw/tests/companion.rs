//! The companion gateway end to end: a gateway on a random port, fake upstreams (HTTP, TCP,
//! a Docker unix socket, a Kubernetes API, a git host), and real HTTP/WebSocket clients.

use axum::{extract::Request, routing::any, Router};
use futures_util::{SinkExt, StreamExt};
use gw::companion::{secrets::Store, settings::Settings, Gateway};
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

const SITE: &str = "https://geekychris.github.io";

/// A fake upstream that echoes the request (method, path, query, selected headers, body).
fn echo() -> Router {
    Router::new().fallback(any(|req: Request| async move {
        let (parts, body) = req.into_parts();
        let body = axum::body::to_bytes(body, 1 << 20).await.unwrap();
        let h = |k: &str| parts.headers.get(k).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        axum::Json(json!({
            "method": parts.method.as_str(), "path": parts.uri.path(), "query": parts.uri.query().unwrap_or(""),
            "authorization": h("authorization"), "cookie": h("cookie"), "origin": h("origin"),
            "body": String::from_utf8_lossy(&body),
        }))
    }))
}

async fn serve(r: Router) -> u16 {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let p = l.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(l, r).await.unwrap() });
    p
}

async fn gateway(s: Settings, secrets: Store) -> (Arc<Gateway>, String) {
    s.validate().unwrap();
    let gw = Arc::new(Gateway::new(s, None, None, secrets));
    let p = serve(gw::companion::router(gw.clone())).await;
    gw.port.store(p, Ordering::Relaxed);
    gw.ui_port.store(1, Ordering::Relaxed); // the "app" origin: http://127.0.0.1:1
    (gw, format!("http://127.0.0.1:{p}"))
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().timeout(Duration::from_secs(20)).build().unwrap()
}

/// Pair from the site origin (approving like the app would) and return the token.
async fn pair(gw: &Gateway, base: &str, scopes: &[&str]) -> String {
    let c = client();
    let r = c.post(format!("{base}/v1/pair")).header("origin", SITE).json(&json!({"tool": "test", "scopes": scopes})).send().await.unwrap();
    assert_eq!(r.status(), 202);
    let v: Value = r.json().await.unwrap();
    let code = v["code"].as_str().unwrap().to_string();
    assert_eq!(code.len(), 6);
    let poll = format!("{base}{}", v["poll"].as_str().unwrap());
    let waiter = tokio::spawn(async move { client().get(poll).header("origin", SITE).send().await.unwrap() });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(gw.pairing.list().len(), 1);
    gw.pairing.decide(&code, true, &gw.tokens).unwrap();
    let r = waiter.await.unwrap();
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    v["token"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn hello_origin_host_and_preflight() {
    let (_gw, base) = gateway(Settings::default(), Store::memory(&[])).await;
    let c = client();
    let r = c.get(format!("{base}/v1/hello")).header("origin", SITE).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["access-control-allow-origin"], SITE);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["name"], "gitwebsite-companion");
    assert_eq!(v["version"], "1");
    assert_eq!(v["paired"], false);
    assert!(v["capabilities"].as_array().unwrap().contains(&json!("relay.tcp")));

    // Other web pages are refused, and get no CORS headers.
    let r = c.get(format!("{base}/v1/hello")).header("origin", "https://evil.test").send().await.unwrap();
    assert_eq!(r.status(), 403);
    assert!(r.headers().get("access-control-allow-origin").is_none());
    // DNS rebinding: a foreign Host.
    let r = c.get(format!("{base}/v1/hello")).header("host", "evil.test:7317").send().await.unwrap();
    assert_eq!(r.status(), 403);

    // CORS + Private Network Access preflight.
    let r = c
        .request(reqwest::Method::OPTIONS, format!("{base}/v1/relay/http/adsblol/v2/x"))
        .header("origin", SITE)
        .header("access-control-request-method", "GET")
        .header("access-control-request-private-network", "true")
        .header("access-control-request-headers", "authorization")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    assert_eq!(r.headers()["access-control-allow-private-network"], "true");
    assert_eq!(r.headers()["access-control-allow-origin"], SITE);
    assert_eq!(r.headers()["access-control-allow-headers"], "authorization");
    let r = c.request(reqwest::Method::OPTIONS, format!("{base}/v1/hello")).header("origin", "https://evil.test").send().await.unwrap();
    assert_eq!(r.status(), 403);
}

#[tokio::test]
async fn pairing_tokens_and_scopes() {
    let up = serve(echo()).await;
    let mut s = Settings::default();
    s.receiver = format!("http://127.0.0.1:{up}");
    let (gw, base) = gateway(s, Store::memory(&[])).await;
    let c = client();
    // No token: refused.
    let r = c.get(format!("{base}/v1/relay/http/receiver/data/aircraft.json")).header("origin", SITE).send().await.unwrap();
    assert_eq!(r.status(), 401);
    // A token for the TCP relay only does not open the HTTP relay.
    let tcp_only = pair(&gw, &base, &["relay.tcp"]).await;
    let r = c.get(format!("{base}/v1/relay/http/receiver/x")).header("origin", SITE).bearer_auth(&tcp_only).send().await.unwrap();
    assert_eq!(r.status(), 403);
    let t = pair(&gw, &base, &["relay.http"]).await;
    // hello reports the pairing.
    let v: Value = c.get(format!("{base}/v1/hello")).header("origin", SITE).bearer_auth(&t).send().await.unwrap().json().await.unwrap();
    assert_eq!(v["paired"], true);
    // The token works only from the origin it was issued to.
    let r = c.get(format!("{base}/v1/relay/http/receiver/x")).header("origin", "http://localhost:3000").bearer_auth(&t).send().await.unwrap();
    assert_eq!(r.status(), 401);
    // Denied pairing.
    let r = c.post(format!("{base}/v1/pair")).header("origin", SITE).json(&json!({"tool": "x"})).send().await.unwrap();
    let v: Value = r.json().await.unwrap();
    gw.pairing.decide(v["code"].as_str().unwrap(), false, &gw.tokens).unwrap();
    // Denied comes back as a quiet 200 (no console error in the page).
    let r = c.get(format!("{base}{}", v["poll"].as_str().unwrap())).header("origin", SITE).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let d: Value = r.json().await.unwrap();
    assert_eq!((d["status"].as_str(), d["denied"].as_bool()), (Some("denied"), Some(true)));
    assert!(d.get("token").is_none());
    // Another origin cannot collect someone else's pairing.
    let r = c.post(format!("{base}/v1/pair")).header("origin", SITE).json(&json!({"tool": "y"})).send().await.unwrap();
    let v: Value = r.json().await.unwrap();
    let r = c.get(format!("{base}{}", v["poll"].as_str().unwrap())).header("origin", "http://localhost:9").send().await.unwrap();
    assert_eq!(r.status(), 200);
    let d: Value = r.json().await.unwrap();
    assert_eq!((d["status"].as_str(), d["expired"].as_bool()), (Some("expired"), Some(true)));
    assert!(d.get("token").is_none());
    // Pending polls stay 202.
    let r = c.post(format!("{base}/v1/pair")).header("origin", SITE).json(&json!({"tool": "z"})).send().await.unwrap();
    let v: Value = r.json().await.unwrap();
    let id = v["id"].as_str().unwrap().to_string();
    let poll = tokio::spawn({
        let u = format!("{base}/v1/pair/{id}");
        async move { client().get(u).header("origin", SITE).send().await.unwrap() }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    gw.pairing.decide(&id, true, &gw.tokens).unwrap();
    let r = poll.await.unwrap();
    assert_eq!(r.status(), 200);
    let d: Value = r.json().await.unwrap();
    assert_eq!(d["status"], "approved");
    let zt = d["token"].as_str().unwrap().to_string();
    // Self-revoke: DELETE /v1/token with the token itself.
    let r = c.delete(format!("{base}/v1/token")).header("origin", "http://localhost:9").bearer_auth(&zt).send().await.unwrap();
    assert_eq!(r.status(), 401, "another origin cannot revoke it");
    let r = c.delete(format!("{base}/v1/token")).header("origin", SITE).bearer_auth(&zt).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["access-control-allow-origin"], SITE);
    assert_eq!(r.json::<Value>().await.unwrap()["revoked"], 1);
    let v: Value = c.get(format!("{base}/v1/hello")).header("origin", SITE).bearer_auth(&zt).send().await.unwrap().json().await.unwrap();
    assert_eq!(v["paired"], false);
    assert_eq!(c.delete(format!("{base}/v1/token")).header("origin", SITE).bearer_auth(&zt).send().await.unwrap().status(), 401);
    assert!(gw.audit_tail(5).iter().any(|e| e["note"] == "self-revoked" && e["tool"] == "z" && e["path"] == "/v1/token"));
    // The app's own preview pages pair without asking.
    let r = c.post(format!("{base}/v1/pair")).header("origin", "http://127.0.0.1:1").json(&json!({"tool": "redis", "scopes": ["relay.tcp"]})).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.json::<Value>().await.unwrap()["token"].as_str().is_some());
    // Revocation.
    let id = gw.tokens.list().iter().find(|r| r["scopes"] == json!(["relay.http"])).unwrap()["id"].as_str().unwrap().to_string();
    gw.tokens.revoke(&id);
    let r = c.get(format!("{base}/v1/relay/http/receiver/x")).header("origin", SITE).bearer_auth(&t).send().await.unwrap();
    assert_eq!(r.status(), 401);
}

#[tokio::test]
async fn http_relay_is_not_an_open_proxy() {
    let up = serve(echo()).await;
    let mut s = Settings::default();
    s.receiver = format!("http://127.0.0.1:{up}");
    s.kafka_rest = format!("http://127.0.0.1:{up}");
    let (gw, base) = gateway(s, Store::memory(&[("mouser_api_key", "MK")])).await;
    gw.set_upstream_override("mouser", &format!("http://127.0.0.1:{up}"));
    let t = pair(&gw, &base, &["relay.http"]).await;
    let c = client();
    let get = |p: &str| c.get(format!("{base}{p}")).header("origin", SITE).bearer_auth(&t).header("cookie", "a=b");

    let r = get("/v1/relay/http/receiver/data/aircraft.json?x=1").send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["x-relay-upstream"], "receiver");
    assert_eq!(r.headers()["x-relay-cache"], "miss");
    assert_eq!(r.headers()["access-control-allow-origin"], SITE);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["path"], "/data/aircraft.json");
    assert_eq!(v["query"], "x=1");
    // Neither the pairing token, nor cookies, nor the page's origin go upstream.
    assert_eq!(v["authorization"], "");
    assert_eq!(v["cookie"], "");
    assert_eq!(v["origin"], "");
    // Cached for 2 s.
    let r = get("/v1/relay/http/receiver/data/aircraft.json?x=1").send().await.unwrap();
    assert_eq!(r.headers()["x-relay-cache"], "hit");

    // Unknown upstreams and escapes are refused.
    assert_eq!(get("/v1/relay/http/evil.test/x").send().await.unwrap().status(), 404);
    assert_eq!(get("/v1/relay/http/receiver/..%2f..%2fetc/passwd").send().await.unwrap().status(), 400);
    assert_eq!(get("/v1/relay/http/receiver/@evil.test/x").send().await.unwrap().status(), 400);
    assert_eq!(get("/v1/relay/http/receiver/x?u=http://evil.test").send().await.unwrap().status(), 400);
    // Unconfigured user upstream.
    assert_eq!(get("/v1/relay/http/redpanda-admin/v1/brokers").send().await.unwrap().status(), 503);
    // Aviation upstreams are GET only.
    let r = c.post(format!("{base}/v1/relay/http/receiver/x")).header("origin", SITE).bearer_auth(&t).body("{}").send().await.unwrap();
    assert_eq!(r.status(), 405);
    // Kafka REST: POST passes, X-Method-Override turns a POST into a GET with a body.
    let r = c.post(format!("{base}/v1/relay/http/kafka-rest/topics/t")).header("origin", SITE).bearer_auth(&t).body("{\"records\":[]}").send().await.unwrap();
    assert_eq!(r.json::<Value>().await.unwrap()["method"], "POST");
    let r = c.post(format!("{base}/v1/relay/http/kafka-rest/groups/g/offsets")).header("origin", SITE).bearer_auth(&t)
        .header("x-method-override", "GET").body("{\"p\":1}").send().await.unwrap();
    let v: Value = r.json().await.unwrap();
    assert_eq!((v["method"].as_str(), v["body"].as_str()), (Some("GET"), Some("{\"p\":1}")));
    // With X-Companion-Token, the page's own Authorization goes to kafka.
    let r = c.get(format!("{base}/v1/relay/http/kafka-rest/topics")).header("origin", SITE).header("x-companion-token", &t)
        .header("authorization", "Basic dTpw").send().await.unwrap();
    assert_eq!(r.json::<Value>().await.unwrap()["authorization"], "Basic dTpw");
    // Stored supplier secrets are injected server-side.
    let v: Value = get("/v1/relay/http/mouser/api/v1/search/keyword?x=1").send().await.unwrap().json().await.unwrap();
    assert_eq!(v["query"], "x=1&apiKey=MK");
}

#[tokio::test]
async fn tcp_relay_allow_list() {
    let echo_l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let echo_port = echo_l.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = echo_l.accept().await.unwrap();
            tokio::spawn(async move {
                let (mut r, mut w) = s.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    let mut s = Settings::default();
    s.tcp_allow = vec![format!("127.0.0.1:{echo_port}")];
    let (gw, base) = gateway(s, Store::memory(&[])).await;
    let t = pair(&gw, &base, &["relay.tcp"]).await;
    let ws_base = base.replace("http://", "ws://");
    let connect = |target: String, proto: String| {
        let url = format!("{ws_base}/v1/relay/tcp?target={target}");
        async move {
            use tokio_tungstenite::tungstenite::client::IntoClientRequest;
            let mut req = url.into_client_request().unwrap();
            req.headers_mut().insert("origin", SITE.parse().unwrap());
            req.headers_mut().insert("sec-websocket-protocol", proto.parse().unwrap());
            tokio_tungstenite::connect_async(req).await
        }
    };
    let (mut ws, resp) = connect(format!("localhost:{echo_port}"), format!("bearer.{t}")).await.unwrap();
    assert_eq!(resp.headers()["sec-websocket-protocol"], format!("bearer.{t}"));
    use tokio_tungstenite::tungstenite::Message;
    ws.send(Message::Binary(vec![0u8, 1, 2, 255].into())).await.unwrap();
    let m = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.unwrap().unwrap().unwrap();
    assert_eq!(m.into_data().to_vec(), vec![0u8, 1, 2, 255]);
    // The gw-companion.v1 subprotocol is preferred when offered (the token is not echoed).
    let (_ws2, resp) = connect(format!("127.0.0.1:{echo_port}"), format!("gw-companion.v1, bearer.{t}")).await.unwrap();
    assert_eq!(resp.headers()["sec-websocket-protocol"], "gw-companion.v1");
    // Not on the allow-list.
    let e = connect("127.0.0.1:22".into(), format!("bearer.{t}")).await.unwrap_err();
    assert!(e.to_string().contains("403"), "{e}");
    // Tokens in the URL are not accepted.
    let e = connect(format!("127.0.0.1:{echo_port}&token={t}"), "x".into()).await.unwrap_err();
    assert!(e.to_string().contains("401"), "{e}");

    // With `gw-companion.codes` offered, refusals are an accepted upgrade (selecting that
    // subprotocol) and then a close with an application code and a reason.
    let refused = |target: String, proto: String| {
        let c = connect(target, proto);
        async move {
            let (mut ws, resp) = c.await.expect("upgrade accepted");
            assert_eq!(resp.headers()["sec-websocket-protocol"], "gw-companion.codes");
            let m = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.unwrap().unwrap().unwrap();
            match m {
                Message::Close(Some(f)) => (u16::from(f.code), f.reason.to_string()),
                other => panic!("expected a close, got {other:?}"),
            }
        }
    };
    let codes = |extra: &str| format!("gw-companion.v1, gw-companion.codes{extra}");
    let (code, why) = refused("127.0.0.1:22".into(), codes(&format!(", bearer.{t}"))).await;
    assert_eq!(code, 4403);
    assert!(why.contains("allow-list"), "{why}");
    let (code, _) = refused(format!("127.0.0.1:{echo_port}"), codes(", bearer.nope")).await;
    assert_eq!(code, 4401);
    let (code, _) = refused(format!("127.0.0.1:{echo_port}"), codes("")).await;
    assert_eq!(code, 4401);
    let (code, _) = refused(String::new(), codes(&format!(", bearer.{t}"))).await;
    assert_eq!(code, 4400);
    // Allowed, but nothing listening: 4502.
    let dead = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    gw.settings.write().unwrap().tcp_allow.push(format!("127.0.0.1:{dead}"));
    let (code, why) = refused(format!("127.0.0.1:{dead}"), codes(&format!(", bearer.{t}"))).await;
    assert_eq!(code, 4502);
    assert!(why.contains("cannot connect"), "{why}");
    assert!(gw.audit_tail(10).iter().any(|e| e["close"] == 4502));
    // Success with codes offered still selects gw-companion.v1.
    let (mut ws3, resp) = connect(format!("127.0.0.1:{echo_port}"), codes(&format!(", bearer.{t}"))).await.unwrap();
    assert_eq!(resp.headers()["sec-websocket-protocol"], "gw-companion.v1");
    ws3.send(Message::Binary(vec![7u8].into())).await.unwrap();
    let m = tokio::time::timeout(Duration::from_secs(5), ws3.next()).await.unwrap().unwrap().unwrap();
    assert_eq!(m.into_data().to_vec(), vec![7u8]);
    // A token without the relay.tcp scope (re-pairing "test" replaces the token above).
    let http_only = pair(&gw, &base, &["relay.http"]).await;
    let (code, why) = refused(format!("127.0.0.1:{echo_port}"), codes(&format!(", bearer.{http_only}"))).await;
    assert_eq!(code, 4403);
    assert!(why.contains("scope"), "{why}");
    // A foreign Origin is still a plain HTTP 403 before any upgrade.
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut req = format!("{ws_base}/v1/relay/tcp?target=127.0.0.1:{echo_port}").into_client_request().unwrap();
    req.headers_mut().insert("origin", "https://evil.test".parse().unwrap());
    req.headers_mut().insert("sec-websocket-protocol", codes(&format!(", bearer.{http_only}")).parse().unwrap());
    let e = tokio_tungstenite::connect_async(req).await.unwrap_err();
    assert!(e.to_string().contains("403"), "{e}");
}

#[cfg(unix)]
#[tokio::test]
async fn docker_relay_is_get_only() {
    let dir = std::env::temp_dir().join(format!("gw-dock-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("d.sock");
    let _ = std::fs::remove_file(&sock);
    let l = tokio::net::UnixListener::bind(&sock).unwrap();
    tokio::spawn(async move { axum::serve(l, echo()).await.unwrap() });
    let mut s = Settings::default();
    s.docker_host = format!("unix://{}", sock.display());
    let (gw, base) = gateway(s, Store::memory(&[])).await;
    let t = pair(&gw, &base, &["relay.docker"]).await;
    let c = client();
    let r = c.get(format!("{base}/v1/relay/docker/v1.43/containers/json?all=1")).header("origin", SITE).bearer_auth(&t).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    assert_eq!((v["method"].as_str(), v["path"].as_str(), v["query"].as_str()), (Some("GET"), Some("/v1.43/containers/json"), Some("all=1")));
    for m in [reqwest::Method::POST, reqwest::Method::DELETE, reqwest::Method::PUT] {
        let r = c.request(m, format!("{base}/v1/relay/docker/containers/x/stop")).header("origin", SITE).bearer_auth(&t).send().await.unwrap();
        assert_eq!(r.status(), 405);
    }
    for p in ["/containers/x/exec", "/containers/x/archive", "/images/x/get", "/containers/..%2f/x"] {
        let r = c.get(format!("{base}/v1/relay/docker{p}")).header("origin", SITE).bearer_auth(&t).send().await.unwrap();
        assert!(r.status() == 403 || r.status() == 400, "{p}: {}", r.status());
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn kube_relay_is_get_only_and_keeps_credentials() {
    let up = serve(echo()).await;
    let dir = std::env::temp_dir().join(format!("gw-kube-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let kc = dir.join("config");
    std::fs::write(
        &kc,
        format!(
            "apiVersion: v1\nkind: Config\ncurrent-context: other\nclusters:\n- name: c\n  cluster:\n    server: http://127.0.0.1:{up}\ncontexts:\n- name: lab/one\n  context:\n    cluster: c\n    user: u\n- name: other\n  context:\n    cluster: c\n    user: u\nusers:\n- name: u\n  user:\n    token: KUBE-SECRET\n"
        ),
    )
    .unwrap();
    let before = std::fs::read_to_string(&kc).unwrap();
    let mut s = Settings::default();
    s.kubeconfig = kc.display().to_string();
    let (gw, base) = gateway(s, Store::memory(&[])).await;
    let t = pair(&gw, &base, &["relay.kube"]).await;
    let c = client();
    let get = |p: &str| c.get(format!("{base}/v1/relay/kube{p}")).header("origin", SITE).bearer_auth(&t);
    let v: Value = get("/lab%2Fone/api/v1/namespaces/default/pods?watch=1").send().await.unwrap().json().await.unwrap();
    assert_eq!(v["path"], "/api/v1/namespaces/default/pods");
    assert_eq!(v["authorization"], "Bearer KUBE-SECRET");
    let list = get("").send().await.unwrap().text().await.unwrap();
    assert!(list.contains("lab/one") && !list.contains("KUBE-SECRET"));
    assert_eq!(get("/lab%2Fone/api/v1/namespaces/default/secrets").send().await.unwrap().status(), 403);
    assert_eq!(get("/lab%2Fone/api/v1/namespaces/default/pods/p/exec").send().await.unwrap().status(), 403);
    let r = c.delete(format!("{base}/v1/relay/kube/lab%2Fone/api/v1/namespaces/default/pods/p")).header("origin", SITE).bearer_auth(&t).send().await.unwrap();
    assert_eq!(r.status(), 405);
    assert_eq!(get("/nope/api/v1/pods").send().await.unwrap().status(), 503);
    assert_eq!(std::fs::read_to_string(&kc).unwrap(), before, "kubeconfig must not change");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn git_relay_refuses_push() {
    let up = serve(echo()).await;
    let mut s = Settings::default();
    s.git_http_hosts = vec![format!("127.0.0.1:{up}")];
    let (gw, base) = gateway(s, Store::memory(&[])).await;
    let t = pair(&gw, &base, &["relay.git"]).await;
    let c = client();
    // Fetch works (the page's own Authorization goes to the git host with X-Companion-Token).
    let r = c.get(format!("{base}/v1/relay/git/127.0.0.1:{up}/o/r.git/info/refs?service=git-upload-pack"))
        .header("origin", SITE).header("x-companion-token", &t).header("authorization", "Basic Z2l0OnRvaw==").send().await.unwrap();
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    assert_eq!((v["path"].as_str(), v["authorization"].as_str()), (Some("/o/r.git/info/refs"), Some("Basic Z2l0OnRvaw==")));
    let r = c.post(format!("{base}/v1/relay/git/127.0.0.1:{up}/o/r.git/git-upload-pack")).header("origin", SITE).bearer_auth(&t).body("0000").send().await.unwrap();
    assert_eq!(r.json::<Value>().await.unwrap()["body"], "0000");
    // Push is refused.
    let r = c.post(format!("{base}/v1/relay/git/github.com/o/r.git/git-receive-pack")).header("origin", SITE).bearer_auth(&t).body("x").send().await.unwrap();
    assert_eq!(r.status(), 403);
    let r = c.get(format!("{base}/v1/relay/git/github.com/o/r.git/info/refs?service=git-receive-pack")).header("origin", SITE).bearer_auth(&t).send().await.unwrap();
    assert_eq!(r.status(), 403);
    // Not an allow-listed host.
    let r = c.get(format!("{base}/v1/relay/git/evil.test/o/r.git/info/refs?service=git-upload-pack")).header("origin", SITE).bearer_auth(&t).send().await.unwrap();
    assert_eq!(r.status(), 403);
}

#[tokio::test]
async fn receiver_base_with_path_is_not_doubled() {
    let up = serve(echo()).await;
    let mut s = Settings::default();
    s.receiver = format!("http://127.0.0.1:{up}/tar1090");
    let (gw, base) = gateway(s, Store::memory(&[])).await;
    let t = pair(&gw, &base, &["relay.http"]).await;
    let c = client();
    for p in ["tar1090/data/aircraft.json", "data/aircraft.json"] {
        let v: Value = c.get(format!("{base}/v1/relay/http/receiver/{p}")).header("origin", SITE).bearer_auth(&t).send().await.unwrap().json().await.unwrap();
        assert_eq!(v["path"], "/tar1090/data/aircraft.json", "{p}");
    }
}

#[tokio::test]
async fn git_info_refs_is_sent_whole() {
    // A git host whose info/refs comes in several chunks (as a real server streams it).
    let adv = "001e# service=git-upload-pack\n0000003f0123456789012345678901234567890123456789 refs/heads/main\n0000";
    let host = Router::new().fallback(any(move || async move {
        let chunks: Vec<Result<axum::body::Bytes, std::io::Error>> = adv.as_bytes().chunks(16).map(|c| Ok(axum::body::Bytes::copy_from_slice(c))).collect();
        let mut r = axum::response::Response::new(axum::body::Body::from_stream(futures_util::stream::iter(chunks)));
        r.headers_mut().insert("content-type", "application/x-git-upload-pack-advertisement".parse().unwrap());
        r
    }));
    let up = serve(host).await;
    let mut s = Settings::default();
    s.git_http_hosts = vec![format!("127.0.0.1:{up}")];
    let (gw, base) = gateway(s, Store::memory(&[])).await;
    let t = pair(&gw, &base, &["relay.git"]).await;
    let r = client().get(format!("{base}/v1/relay/git/127.0.0.1:{up}/o/r.git/info/refs?service=git-upload-pack"))
        .header("origin", SITE).bearer_auth(&t).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["content-length"], adv.len().to_string());
    assert!(r.headers().get("transfer-encoding").is_none());
    assert_eq!(r.headers()["content-type"], "application/x-git-upload-pack-advertisement");
    assert_eq!(r.text().await.unwrap(), adv);
}
