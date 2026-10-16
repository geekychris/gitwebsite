//! The tool viewer: open one of the site's tools, from the app's own preview, in a window of
//! its own (`POST /api/view`, MCP `tool_open`). The desktop app installs a [`ViewHost`] that
//! makes Tauri windows; `gw serve` without the app opens the default browser instead.

use anyhow::{bail, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::sync::OnceLock;

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ViewRequest {
    /// Tool slug (/tools/<slug>/).
    pub tool: String,
    /// URL parameters, as documented in the tool's "Linking to an example" table.
    #[serde(default)]
    pub params: Map<String, Value>,
    /// A preset (`?example=`).
    #[serde(default)]
    pub example: Option<String>,
    /// The tab (`#hash`).
    #[serde(default)]
    pub hash: Option<String>,
    /// Full tool state, packed as the site kit's `?s=` (JSON, base64url).
    #[serde(default)]
    pub state: Option<Value>,
    /// Window title (default: the tool's name).
    #[serde(default)]
    pub title: Option<String>,
    /// "reuse" (default: the window already showing this tool), "new", or a window id.
    #[serde(default)]
    pub window: Option<String>,
    /// Leave the site chrome on (default: `bare=1`).
    #[serde(default)]
    pub chrome: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ViewWindow {
    pub id: String,
    pub tool: String,
    pub title: String,
    pub url: String,
}

/// What the app does with windows.
pub trait ViewHost: Send + Sync {
    /// Open `url` in a window titled `title`; `target` is "new", "reuse" or a window id.
    fn open(&self, tool: &str, url: &str, title: &str, target: &str) -> Result<ViewWindow>;
    fn list(&self) -> Vec<ViewWindow>;
    fn close(&self, id: &str) -> Result<()>;
    /// A PNG of the window's page.
    fn screenshot(&self, id: &str) -> Result<Vec<u8>>;
}

static HOST: OnceLock<Box<dyn ViewHost>> = OnceLock::new();

/// Called once by the desktop app.
pub fn set_view_host(h: impl ViewHost + 'static) {
    let _ = HOST.set(Box::new(h));
}

/// "window" when the app makes windows, else "browser".
pub fn available() -> &'static str {
    if HOST.get().is_some() {
        "window"
    } else {
        "browser"
    }
}

pub fn valid_slug(s: &str) -> bool {
    !s.is_empty() && s.len() <= 80 && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// `?…#…` for a request (without `bare`): params, then example, then the packed state.
pub fn query_and_hash(r: &ViewRequest) -> (String, String) {
    let mut parts: Vec<String> = vec![];
    for (k, v) in &r.params {
        if k == "bare" {
            continue;
        }
        parts.push(format!("{}={}", urlencoding::encode(k), urlencoding::encode(&scalar(v))));
    }
    if let Some(e) = r.example.as_deref().filter(|e| !e.is_empty()) {
        if !r.params.contains_key("example") {
            parts.push(format!("example={}", urlencoding::encode(e)));
        }
    }
    if let Some(s) = &r.state {
        let packed = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s.to_string());
        parts.push(format!("s=j{packed}"));
    }
    let hash = r.hash.as_deref().unwrap_or("").trim_start_matches('#').to_string();
    (parts.join("&"), hash)
}

/// The tool's URL under `base` (e.g. http://127.0.0.1:8790 or https://site.example).
pub fn tool_url(base: &str, r: &ViewRequest, bare: bool) -> String {
    let (mut q, hash) = query_and_hash(r);
    if bare {
        q = if q.is_empty() { "bare=1".into() } else { format!("{q}&bare=1") };
    }
    let mut u = format!("{}/tools/{}/", base.trim_end_matches('/'), r.tool);
    if !q.is_empty() {
        u.push('?');
        u.push_str(&q);
    }
    if !hash.is_empty() {
        u.push('#');
        u.push_str(&urlencoding::encode(&hash).replace("%2F", "/"));
    }
    u
}

/// Open a tool from the preview at http://127.0.0.1:<ui_port>.
pub fn open(ui_port: u16, r: ViewRequest) -> Result<Value> {
    if !valid_slug(&r.tool) {
        bail!("bad tool slug {:?}", r.tool);
    }
    if ui_port == 0 {
        bail!("the gitwebsite server is not running here");
    }
    let url = tool_url(&format!("http://127.0.0.1:{ui_port}"), &r, !r.chrome);
    let title = r.title.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| r.tool.clone());
    match HOST.get() {
        Some(h) => {
            let target = r.window.clone().unwrap_or_else(|| "reuse".into());
            let w = h.open(&r.tool, &url, &title, &target)?;
            Ok(json!({ "opened": "window", "window": w, "url": url }))
        }
        None => {
            open::that(&url)?;
            Ok(json!({ "opened": "browser", "url": url, "note": "no gitwebsite app window host here (gw serve): opened the default browser" }))
        }
    }
}

pub fn list() -> Value {
    match HOST.get() {
        Some(h) => json!({ "host": "window", "windows": h.list() }),
        None => json!({ "host": "browser", "windows": [], "note": "windows are only tracked in the gitwebsite app" }),
    }
}

pub fn close(id: &str) -> Result<Value> {
    match HOST.get() {
        Some(h) => {
            h.close(id)?;
            Ok(json!({ "closed": id }))
        }
        None => bail!("windows are only managed in the gitwebsite app"),
    }
}

pub fn screenshot(id: &str) -> Result<Vec<u8>> {
    match HOST.get() {
        Some(h) => h.screenshot(id),
        None => bail!("not supported: screenshots need the gitwebsite app"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn urls() {
        let mut r = ViewRequest { tool: "diagrams".into(), ..Default::default() };
        r.params.insert("mmd".into(), json!("graph TD; A-->B"));
        r.hash = Some("#mermaid".into());
        assert_eq!(tool_url("http://127.0.0.1:8790", &r, true), "http://127.0.0.1:8790/tools/diagrams/?mmd=graph%20TD%3B%20A--%3EB&bare=1#mermaid");
        let r2 = ViewRequest { tool: "redis".into(), example: Some("leaderboard".into()), ..Default::default() };
        assert_eq!(tool_url("https://x.test/", &r2, false), "https://x.test/tools/redis/?example=leaderboard");
        let r3 = ViewRequest { tool: "graph".into(), state: Some(json!({"a": 1})), ..Default::default() };
        assert!(tool_url("http://h", &r3, false).contains("?s=j"));
        assert!(!valid_slug("../x") && !valid_slug("") && valid_slug("circuit-sim"));
    }
}
