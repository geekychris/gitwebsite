//! A client for the running gitwebsite app (or `gw serve`): finds its port from
//! `<config dir>/server.json` (written when it starts), else tries 8790, and calls its REST API.
//! Used by `gw mcp` (tool windows) and `gw companion …` (approvals).

use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::time::Duration;

fn reachable(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(&std::net::SocketAddr::from(([127, 0, 0, 1], port)), Duration::from_millis(400)).is_ok()
}

/// The UI/API port of a running app, if one answers.
pub fn find_port() -> Option<u16> {
    if let Some(p) = std::env::var("GW_APP_PORT").ok().and_then(|p| p.parse().ok()) {
        return reachable(p).then_some(p);
    }
    let file = std::fs::read_to_string(crate::server_companion::server_file()).ok();
    let from_file = file.and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v["ui_port"].as_u64()).map(|p| p as u16);
    // A scratch config directory (GW_CONFIG_DIR) never falls back to 8790, the real app's port.
    let fallback = gw_core::userconfig::is_default_config_dir().then_some(8790);
    from_file.into_iter().chain(fallback).find(|p| reachable(*p))
}

pub fn base() -> Result<String> {
    match find_port() {
        Some(p) => Ok(format!("http://127.0.0.1:{p}")),
        None => bail!("the gitwebsite app is not running (start it, or run `gw serve`)"),
    }
}

fn client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder().timeout(Duration::from_secs(60)).build().expect("http client")
}

fn decode(r: reqwest::blocking::Response) -> Result<Value> {
    let status = r.status();
    let v: Value = r.json().unwrap_or(Value::Null);
    if !status.is_success() {
        bail!("{}", v["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("HTTP {status}")));
    }
    Ok(v)
}

pub fn get(path: &str) -> Result<Value> {
    decode(client().get(format!("{}{path}", base()?)).send().context("calling the app")?)
}

pub fn send(method: &str, path: &str, body: &Value) -> Result<Value> {
    let m = reqwest::Method::from_bytes(method.as_bytes())?;
    decode(client().request(m, format!("{}{path}", base()?)).json(body).send().context("calling the app")?)
}

/// POST returning raw bytes (screenshots).
pub fn post_bytes(path: &str) -> Result<Vec<u8>> {
    let r = client().post(format!("{}{path}", base()?)).send().context("calling the app")?;
    if !r.status().is_success() {
        let v: Value = r.json().unwrap_or(Value::Null);
        bail!("{}", v["error"].as_str().unwrap_or("screenshot failed"));
    }
    Ok(r.bytes()?.to_vec())
}
