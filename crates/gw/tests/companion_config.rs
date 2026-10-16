//! A companion with a scratch config directory (GW_CONFIG_DIR): its supplier secrets live in a
//! file there (never the Keychain), and `upstream_override` in its companion.toml points the
//! supplier relays at a mock, end to end. Its own test binary, since it sets the environment.

use axum::{extract::Request, routing::any, Router};
use gw::companion::{secrets::Store, Gateway};
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

const SITE: &str = "https://geekychris.github.io";

#[tokio::test]
async fn scratch_config_uses_file_secrets_and_mock_suppliers() {
    for v in ["DIGIKEY_CLIENT_ID", "DIGIKEY_CLIENT_SECRET", "MOUSER_API_KEY", "NEXAR_CLIENT_ID", "NEXAR_CLIENT_SECRET", "GW_COMPANION_TEST_UPSTREAMS"] {
        std::env::remove_var(v);
    }
    // A mock Digi-Key: an OAuth token endpoint, and an echo for everything else.
    let mock = Router::new().fallback(any(|req: Request| async move {
        let (parts, body) = req.into_parts();
        let body = axum::body::to_bytes(body, 1 << 20).await.unwrap();
        if parts.uri.path() == "/v1/oauth2/token" {
            let form = String::from_utf8_lossy(&body).to_string();
            assert!(form.contains("client_id=CID") && form.contains("client_secret=CSECRET"), "{form}");
            return axum::Json(json!({"access_token": "MOCK-AT", "expires_in": 600}));
        }
        let h = |k: &str| parts.headers.get(k).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        axum::Json(json!({"path": parts.uri.path(), "authorization": h("authorization"), "client": h("x-digikey-client-id")}))
    }));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mock_port = l.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(l, mock).await.unwrap() });

    let dir = std::env::temp_dir().join(format!("gw-cfg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("companion.toml"), format!("[upstream_override]\ndigikey = \"http://127.0.0.1:{mock_port}\"\n")).unwrap();
    std::env::set_var("GW_CONFIG_DIR", &dir);
    assert!(!gw_core::userconfig::is_default_config_dir());

    // Secrets go to <dir>/companion-secrets.json, not the Keychain.
    let store = Store::user();
    assert_eq!(store.backend(), format!("file:{}", dir.join("companion-secrets.json").display()));
    store.set("digikey_client_id", "CID").unwrap();
    store.set("digikey_client_secret", "CSECRET").unwrap();
    assert!(std::fs::read_to_string(dir.join("companion-secrets.json")).unwrap().contains("CSECRET"));

    let gw = Arc::new(Gateway::from_user_config().unwrap());
    assert!(gw.status()["secrets_store"].as_str().unwrap().starts_with("file:"));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    gw.port.store(port, Ordering::Relaxed);
    let app = gw::companion::router(gw.clone());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    let base = format!("http://127.0.0.1:{port}");
    let c = reqwest::Client::builder().timeout(Duration::from_secs(20)).build().unwrap();

    // Pair (approving as the app would).
    let v: Value = c.post(format!("{base}/v1/pair")).header("origin", SITE).json(&json!({"tool": "parts", "scopes": ["relay.http"]})).send().await.unwrap().json().await.unwrap();
    let poll = format!("{base}{}", v["poll"].as_str().unwrap());
    let waiter = tokio::spawn({
        let c = c.clone();
        async move { c.get(poll).header("origin", SITE).send().await.unwrap().json::<Value>().await.unwrap() }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    gw.pairing.decide(v["code"].as_str().unwrap(), true, &gw.tokens).unwrap();
    let t = waiter.await.unwrap()["token"].as_str().unwrap().to_string();
    assert!(dir.join("companion-tokens.json").exists());

    // The relay signs in to the mock with the file's secrets and injects the token.
    let r = c.get(format!("{base}/v1/relay/http/digikey/products/v4/search/x/productdetails")).header("origin", SITE).bearer_auth(&t).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["path"], "/products/v4/search/x/productdetails");
    assert_eq!(v["authorization"], "Bearer MOCK-AT");
    assert_eq!(v["client"], "CID");
    let _ = std::fs::remove_dir_all(&dir);
}
