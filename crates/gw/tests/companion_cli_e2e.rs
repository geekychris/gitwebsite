//! The real `gw serve` with a scratch config directory (GW_CONFIG_DIR — file-backed secrets,
//! never the Keychain; never the user's ports), driven by the `gw companion …` CLI: pairing
//! approval, LAN and TCP first-use approvals by code and by host, S3 profiles and secrets,
//! and the audit log.

mod common;

use common::*;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::Arc;
use std::time::Duration;

struct Kill(Child);
impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn gw(dir: &Path, ui: u16, args: &[&str]) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_gw"))
        .env("GW_CONFIG_DIR", dir)
        .env("GW_APP_PORT", ui.to_string())
        .env_remove("GW_COMPANION_PORT")
        .arg("--json")
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "gw {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
}

async fn wait_for<T>(mut f: impl FnMut() -> Option<T>) -> T {
    for _ in 0..300 {
        if let Some(v) = f() {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out");
}

#[tokio::test(flavor = "multi_thread")]
async fn gw_serve_scratch_config_cli_approvals() {
    let dir: PathBuf = std::env::temp_dir().join(format!("gw-c12-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (ui, cp) = (free_port(), free_port());
    assert!(ui != 8790 && cp != 7317);
    let child = Command::new(env!("CARGO_BIN_EXE_gw"))
        .env("GW_CONFIG_DIR", &dir)
        .current_dir(&dir)
        .args(["serve", "--port", &ui.to_string(), "--companion-port", &cp.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let _k = Kill(child);
    let base = format!("http://127.0.0.1:{cp}");
    let c = reqwest::Client::builder().timeout(Duration::from_secs(60)).build().unwrap();
    for _ in 0..200 {
        if c.get(format!("{base}/v1/hello")).send().await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let hello: Value = c.get(format!("{base}/v1/hello")).header("origin", SITE).send().await.unwrap().json().await.unwrap();
    assert_eq!(hello["version"], "1.2");

    // Pairing, approved with the CLI.
    let v: Value = c.post(format!("{base}/v1/pair")).header("origin", SITE).json(&json!({"tool": "ha", "scopes": ["relay.lan", "relay.tcp", "relay.s3"]}))
        .send().await.unwrap().json().await.unwrap();
    let code = v["code"].as_str().unwrap().to_string();
    let poll = tokio::spawn({
        let u = format!("{base}{}", v["poll"].as_str().unwrap());
        async move { reqwest::Client::new().get(u).header("origin", SITE).send().await.unwrap().json::<Value>().await.unwrap() }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    gw(&dir, ui, &["companion", "approve", &code]);
    let t = poll.await.unwrap()["token"].as_str().unwrap().to_string();

    // relay/lan: the first request waits; `gw companion lan approve <host>` lets it through.
    let up = serve(echo()).await;
    let req = tokio::spawn({
        let (u, t) = (format!("{base}/v1/relay/lan/127.0.0.1:{up}/api/states"), t.clone());
        async move { reqwest::Client::new().get(u).header("origin", SITE).bearer_auth(&t).header("x-upstream-authorization", "Bearer LL").send().await.unwrap() }
    });
    let pending = wait_for(|| gw(&dir, ui, &["companion", "approvals"]).as_array().filter(|a| !a.is_empty()).cloned()).await;
    assert_eq!((pending[0]["kind"].as_str(), pending[0]["target"].as_str()), (Some("lan"), Some("127.0.0.1")));
    gw(&dir, ui, &["companion", "lan", "approve", "127.0.0.1"]);
    let r = req.await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.json::<Value>().await.unwrap()["authorization"], "Bearer LL");
    let toml = std::fs::read_to_string(dir.join("companion.toml")).unwrap();
    assert!(toml.contains("lan_hosts = [\"127.0.0.1\"]"), "{toml}");
    assert_eq!(gw(&dir, ui, &["companion", "lan", "list"]), json!(["127.0.0.1"]));

    // relay/tcp: a new LAN target, approved by its code with the generic `approve`.
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ep = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = l.accept().await.unwrap();
            tokio::spawn(async move {
                let (mut r, mut w) = s.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    let conn = tokio::spawn({
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut req = format!("ws://127.0.0.1:{cp}/v1/relay/tcp?target=127.0.0.1:{ep}").into_client_request().unwrap();
        req.headers_mut().insert("origin", SITE.parse().unwrap());
        req.headers_mut().insert("sec-websocket-protocol", format!("gw-companion.v1, bearer.{t}").parse().unwrap());
        tokio_tungstenite::connect_async(req)
    });
    let a = wait_for(|| gw(&dir, ui, &["companion", "approvals"]).as_array().and_then(|a| a.iter().find(|x| x["kind"] == "tcp").cloned())).await;
    gw(&dir, ui, &["companion", "approve", a["code"].as_str().unwrap()]);
    let (mut ws, _) = conn.await.unwrap().unwrap();
    ws.send(tokio_tungstenite::tungstenite::Message::Binary(vec![4u8, 2].into())).await.unwrap();
    let m = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.unwrap().unwrap().unwrap();
    assert_eq!(m.into_data().to_vec(), vec![4u8, 2]);
    assert!(gw(&dir, ui, &["companion", "tcp", "list"]).as_array().unwrap().contains(&json!(format!("127.0.0.1:{ep}"))));

    // S3: profile and credentials with the CLI (file store in the scratch dir), then a signed request.
    let st = Arc::new(S3State::default());
    let s3 = serve(s3_fake("AKCLI", "cli/secret", st.clone())).await;
    let v = gw(&dir, ui, &["companion", "s3", "add", "nas", "--endpoint", &format!("http://127.0.0.1:{s3}"), "--region", "us-east-1", "--path-style"]);
    assert!(v["next"].as_str().unwrap().contains("secret set s3:nas"));
    gw(&dir, ui, &["companion", "secret", "set", "s3:nas:access_key_id", "--value", "AKCLI"]);
    gw(&dir, ui, &["companion", "secret", "set", "s3:nas:secret_access_key", "--value", "cli/secret"]);
    let list = gw(&dir, ui, &["companion", "s3", "list"]);
    assert_eq!(list[0]["credentials"], true, "{list}");
    let secrets_file = dir.join("companion-secrets.json");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&secrets_file).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let r = c.put(format!("{base}/v1/relay/s3/nas/bkt/k.txt")).header("origin", SITE).bearer_auth(&t).body("via cli").send().await.unwrap();
    assert_eq!(r.status(), 200);
    let r = c.get(format!("{base}/v1/relay/s3/nas/bkt/k.txt")).header("origin", SITE).bearer_auth(&t).send().await.unwrap();
    assert_eq!(r.text().await.unwrap(), "via cli");
    let st_json = gw(&dir, ui, &["companion", "status"]).to_string();
    assert!(!st_json.contains("cli/secret") && st_json.contains("file:"), "{st_json}");

    // Audit: approvals, settings and secret changes (names only).
    let audit = gw(&dir, ui, &["companion", "audit", "--n", "200"]).to_string();
    assert!(audit.contains("\"event\":\"approval\"") && audit.contains("\"event\":\"settings\"") && audit.contains("s3:nas:secret_access_key"), "{audit}");
    assert!(!audit.contains("cli/secret"));

    // Remove.
    gw(&dir, ui, &["companion", "lan", "remove", "127.0.0.1"]);
    assert_eq!(gw(&dir, ui, &["companion", "lan", "list"]), json!([]));
    gw(&dir, ui, &["companion", "s3", "remove", "nas"]);
    assert_eq!(gw(&dir, ui, &["companion", "s3", "list"]), json!([]));
    drop(_k);
    let _ = std::fs::remove_dir_all(&dir);
}
