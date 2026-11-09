//! /v1/native/mac through the real Swift helper (target/gw-mac-helper, built by the app's
//! build.rs or `make mac-helper`). Skipped when the helper has not been built.

use futures_util::{SinkExt, StreamExt};
use gw::companion::{secrets::Store, settings::Settings, Gateway};
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};

const APP: &str = "http://127.0.0.1:1";

async fn gateway() -> String {
    let gw = Arc::new(Gateway::new(Settings::default(), None, None, Store::memory(&[])));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let p = l.local_addr().unwrap().port();
    let r = gw::companion::router(gw.clone());
    tokio::spawn(async move { axum::serve(l, r).await.unwrap() });
    gw.port.store(p, Ordering::Relaxed);
    gw.ui_port.store(1, Ordering::Relaxed); // the app's own pages: http://127.0.0.1:1
    format!("http://127.0.0.1:{p}")
}

/// The app's own pages are paired without asking.
async fn token(base: &str, scopes: &[&str]) -> String {
    let r = reqwest::Client::new().post(format!("{base}/v1/pair")).header("origin", APP).json(&json!({"tool": "t", "scopes": scopes})).send().await.unwrap();
    assert_eq!(r.status(), 200);
    r.json::<Value>().await.unwrap()["token"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn helper_ops_calls_and_sessions() {
    if gw::companion::native::mac::helper_path().is_none() {
        eprintln!("skipped: no gw-mac-helper (make mac-helper)");
        return;
    }
    let base = gateway().await;
    let c = reqwest::Client::new();
    let hello: Value = c.get(format!("{base}/v1/hello")).header("origin", APP).send().await.unwrap().json().await.unwrap();
    let caps = hello["capabilities"].as_array().unwrap();
    assert!(caps.contains(&json!("native.mac")) && caps.contains(&json!("native.mac.ping")), "{caps:?}");

    // Needs the scope.
    let relay = token(&base, &["relay.http"]).await;
    let r = c.get(format!("{base}/v1/native/mac")).header("origin", APP).bearer_auth(&relay).send().await.unwrap();
    assert_eq!(r.status(), 403);

    let t = token(&base, &["native.mac"]).await;
    let list: Value = c.get(format!("{base}/v1/native/mac")).header("origin", APP).bearer_auth(&t).send().await.unwrap().json().await.unwrap();
    assert!(list["ops"].as_array().unwrap().iter().any(|o| o["op"] == "echo" && o["session"] == true));

    let r = c.post(format!("{base}/v1/native/mac/ping")).header("origin", APP).bearer_auth(&t).header("x-native-args", r#"{"n":2}"#).body("abc").send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["content-type"], "application/json");
    let v: Value = r.json().await.unwrap();
    assert_eq!((v["pong"].clone(), v["bytes"].clone(), v["args"]["n"].clone()), (json!(true), json!(3), json!(2)));

    let r = c.post(format!("{base}/v1/native/mac/nope")).header("origin", APP).bearer_auth(&t).send().await.unwrap();
    assert_eq!(r.status(), 404);
    let r = c.post(format!("{base}/v1/native/mac/ping")).header("origin", APP).bearer_auth(&t).header("x-native-args", "[1]").send().await.unwrap();
    assert_eq!(r.status(), 400);

    // A session: arguments first, then lines both ways.
    let mut req = format!("{}/v1/native/mac/session/echo", base.replace("http", "ws")).into_client_request().unwrap();
    req.headers_mut().insert("origin", APP.parse().unwrap());
    req.headers_mut().insert("sec-websocket-protocol", format!("gw-companion.v1, bearer.{t}").parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    ws.send(Message::Text("{}".into())).await.unwrap();
    ws.send(Message::Text(r#"{"hi":1}"#.into())).await.unwrap();
    let m = ws.next().await.unwrap().unwrap();
    let v: Value = serde_json::from_str(m.to_text().unwrap()).unwrap();
    assert_eq!(v["echo"]["hi"], 1);
}
