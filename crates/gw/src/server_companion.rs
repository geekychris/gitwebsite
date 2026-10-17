//! The app's REST endpoints for the companion, the tool viewer and the tool catalogue:
//!
//! - `GET /api/tools[?category=&q=]`, `GET /api/tools/{slug}` — the catalogue (see [`crate::toolcat`])
//! - `GET /api/view`, `POST /api/view`, `DELETE /api/view/{id}`, `POST /api/view/{id}/screenshot`
//! - `GET /api/companion` (status), `PUT /api/companion/settings`, `GET /api/companion/audit`,
//!   `POST /api/companion/pair/{id or code}` {allow}, `DELETE /api/companion/tokens/{id}`,
//!   `PUT|DELETE /api/companion/secrets/{name}` (values are write-only)
//!
//! Changes are refused from any page but the app's own (Origin must match Host), so another
//! local web page cannot approve its own pairing or turn relays on.

use crate::companion::{self, view};
use axum::{
    extract::{Path as AxPath, Query},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU16, Ordering};

/// The UI server's port (set when it starts).
pub static UI_PORT: AtomicU16 = AtomicU16::new(0);

fn bad(code: StatusCode, msg: impl std::fmt::Display) -> Response {
    (code, Json(json!({ "error": msg.to_string() }))).into_response()
}

/// Only the app's own pages (or non-browser clients) may change companion state.
pub fn same_origin(h: &HeaderMap) -> Result<(), Response> {
    let Some(o) = h.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else { return Ok(()) };
    let host = h.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("");
    if o == format!("http://{host}") && companion::host_ok(host) {
        Ok(())
    } else {
        Err(bad(StatusCode::FORBIDDEN, format!("origin {o} may not change the companion")))
    }
}

fn gateway() -> Result<std::sync::Arc<companion::Gateway>, Response> {
    companion::global().ok_or_else(|| bad(StatusCode::SERVICE_UNAVAILABLE, "the companion is not running in this server (disabled, or gw serve --no-companion)"))
}

// ------------------------------------------------------------------ tools catalogue

#[derive(Deserialize)]
pub struct ToolsQuery {
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    q: Option<String>,
}

pub async fn tools_list(root: Option<PathBuf>, q: ToolsQuery) -> Response {
    let Some(root) = root else { return bad(StatusCode::CONFLICT, "no site") };
    match tokio::task::spawn_blocking(move || crate::toolcat::list(&root, q.category.as_deref(), q.q.as_deref())).await {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) => bad(StatusCode::BAD_REQUEST, format!("{e:#}")),
        Err(e) => bad(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

pub async fn tool_info(root: Option<PathBuf>, slug: String) -> Response {
    let Some(root) = root else { return bad(StatusCode::CONFLICT, "no site") };
    match tokio::task::spawn_blocking(move || crate::toolcat::info(&root, &slug)).await {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) => bad(StatusCode::NOT_FOUND, format!("{e:#}")),
        Err(e) => bad(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

pub fn tools_query(q: Query<ToolsQuery>) -> ToolsQuery {
    q.0
}

// ------------------------------------------------------------------ viewer

/// POST /api/view — the window is titled with the tool's name unless a title is given.
pub async fn view_open(root: Option<PathBuf>, h: HeaderMap, mut r: view::ViewRequest) -> Response {
    if let Err(e) = same_origin(&h) {
        return e;
    }
    let port = UI_PORT.load(Ordering::Relaxed);
    match tokio::task::spawn_blocking(move || {
        if r.title.as_deref().is_none_or(|t| t.trim().is_empty()) {
            if let Some(root) = &root {
                r.title = crate::toolcat::load(root).ok().and_then(|ts| ts.into_iter().find(|t| t.slug == r.tool)).map(|t| t.title);
            }
        }
        view::open(port, r)
    })
    .await
    {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) => bad(StatusCode::BAD_REQUEST, format!("{e:#}")),
        Err(e) => bad(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

pub async fn view_list() -> Response {
    Json(view::list()).into_response()
}

pub async fn view_close(h: HeaderMap, AxPath(id): AxPath<String>) -> Response {
    if let Err(e) = same_origin(&h) {
        return e;
    }
    match tokio::task::spawn_blocking(move || view::close(&id)).await {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) => bad(StatusCode::NOT_FOUND, format!("{e:#}")),
        Err(e) => bad(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

pub async fn view_screenshot(h: HeaderMap, AxPath(id): AxPath<String>) -> Response {
    if let Err(e) = same_origin(&h) {
        return e;
    }
    match tokio::task::spawn_blocking(move || view::screenshot(&id)).await {
        Ok(Ok(png)) => ([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "no-store")], png).into_response(),
        Ok(Err(e)) => {
            let msg = format!("{e:#}");
            let code = if msg.starts_with("not supported") { StatusCode::NOT_IMPLEMENTED } else if msg.starts_with("no window") { StatusCode::NOT_FOUND } else { StatusCode::INTERNAL_SERVER_ERROR };
            bad(code, msg)
        }
        Err(e) => bad(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

// ------------------------------------------------------------------ companion

pub async fn status() -> Response {
    match gateway() {
        Ok(g) => Json(tokio::task::spawn_blocking(move || g.status()).await.unwrap_or(Value::Null)).into_response(),
        Err(_) => Json(json!({ "running": false, "settings_file": companion::settings::path().display().to_string(),
            "settings": companion::settings::Settings::load().ok(), "view": view::available() })).into_response(),
    }
}

pub async fn audit(Query(q): Query<Value>) -> Response {
    let n = q.get("n").and_then(|v| v.as_str()).and_then(|v| v.parse().ok()).unwrap_or(200usize).min(2000);
    match gateway() {
        Ok(g) => Json(json!({ "entries": g.audit_tail(n) })).into_response(),
        Err(e) => e,
    }
}

pub async fn settings_put(h: HeaderMap, Json(patch): Json<Value>) -> Response {
    if let Err(e) = same_origin(&h) {
        return e;
    }
    let r = (|| -> anyhow::Result<Value> {
        let cur = companion::settings::Settings::load()?;
        let new = cur.merged(&patch)?;
        new.save()?;
        if let Some(g) = companion::global() {
            *g.settings.write().unwrap() = new.clone();
        }
        let restart = new.port != cur.port || new.enabled != cur.enabled;
        let keys: Vec<&String> = patch.as_object().map(|o| o.keys().collect()).unwrap_or_default();
        let ev = json!({"event": "settings", "keys": keys, "by": "app"});
        match companion::global() {
            Some(g) => g.audit_event(ev),
            None => companion::audit_append(ev),
        }
        Ok(json!({ "settings": new, "restart_needed": restart }))
    })();
    match r {
        Ok(v) => Json(v).into_response(),
        Err(e) => bad(StatusCode::BAD_REQUEST, format!("{e:#}")),
    }
}

#[derive(Deserialize)]
pub struct Decide {
    allow: bool,
}

pub async fn pair_decide(h: HeaderMap, AxPath(which): AxPath<String>, Json(d): Json<Decide>) -> Response {
    if let Err(e) = same_origin(&h) {
        return e;
    }
    let g = match gateway() {
        Ok(g) => g,
        Err(e) => return e,
    };
    match g.pairing.decide(&which, d.allow, &g.tokens) {
        Ok(r) => Json(json!({ "decided": r, "allowed": d.allow })).into_response(),
        Err(e) => bad(StatusCode::NOT_FOUND, format!("{e:#}")),
    }
}

#[derive(Deserialize)]
pub struct ApprovalQuery {
    #[serde(default)]
    kind: Option<String>,
}

/// POST /api/companion/approvals/{id, code, or (with ?kind=lan|tcp) target} {allow}
pub async fn approval_decide(h: HeaderMap, AxPath(which): AxPath<String>, Query(q): Query<ApprovalQuery>, Json(d): Json<Decide>) -> Response {
    if let Err(e) = same_origin(&h) {
        return e;
    }
    let g = match gateway() {
        Ok(g) => g,
        Err(e) => return e,
    };
    let which = urlencoding::decode(&which).map(|s| s.into_owned()).unwrap_or(which);
    let Some(req) = g.approvals.find(&which, q.kind.as_deref()) else {
        return bad(StatusCode::NOT_FOUND, format!("no pending approval {which:?} (see `gw companion approvals`)"));
    };
    let by = if h.get(header::ORIGIN).is_some() { "app" } else { "cli" };
    match tokio::task::spawn_blocking(move || g.decide_approval(&req.id, d.allow, by)).await {
        Ok(Ok(r)) => Json(json!({ "decided": r, "allowed": d.allow })).into_response(),
        Ok(Err(e)) => bad(StatusCode::BAD_REQUEST, format!("{e:#}")),
        Err(e) => bad(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

pub async fn token_revoke(h: HeaderMap, AxPath(id): AxPath<String>) -> Response {
    if let Err(e) = same_origin(&h) {
        return e;
    }
    let g = match gateway() {
        Ok(g) => g,
        Err(e) => return e,
    };
    let which = urlencoding::decode(&id).map(|s| s.into_owned()).unwrap_or(id);
    Json(json!({ "revoked": g.tokens.revoke(&which) })).into_response()
}

#[derive(Deserialize)]
pub struct SecretBody {
    value: String,
}

pub async fn secret_put(h: HeaderMap, AxPath(name): AxPath<String>, Json(b): Json<SecretBody>) -> Response {
    if let Err(e) = same_origin(&h) {
        return e;
    }
    let r = tokio::task::spawn_blocking(move || {
        let store = companion::secrets::Store::user();
        let action = if b.value.trim().is_empty() { "deleted" } else { "set" };
        let r = if b.value.trim().is_empty() { store.delete(&name) } else { store.set(&name, b.value.trim()) }.map(|_| store.presence());
        if r.is_ok() {
            secret_audit(&name, action);
        }
        r
    })
    .await;
    match r {
        Ok(Ok(v)) => Json(json!({ "secrets": v })).into_response(),
        Ok(Err(e)) => bad(StatusCode::BAD_REQUEST, format!("{e:#}")),
        Err(e) => bad(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

pub async fn secret_delete(h: HeaderMap, AxPath(name): AxPath<String>) -> Response {
    if let Err(e) = same_origin(&h) {
        return e;
    }
    let r = tokio::task::spawn_blocking(move || {
        let s = companion::secrets::Store::user();
        let r = s.delete(&name).map(|_| s.presence());
        if r.is_ok() {
            secret_audit(&name, "deleted");
        }
        r
    })
    .await;
    match r {
        Ok(Ok(v)) => Json(json!({ "secrets": v })).into_response(),
        Ok(Err(e)) => bad(StatusCode::BAD_REQUEST, format!("{e:#}")),
        Err(e) => bad(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

fn secret_audit(name: &str, action: &str) {
    let ev = json!({"event": "secret", "names": [name], "action": action, "by": "app"});
    match companion::global() {
        Some(g) => g.audit_event(ev),
        None => companion::audit_append(ev),
    }
}

// ------------------------------------------------------------------ discovery file

/// `<config dir>/server.json`: where the running app / `gw serve` listens, for `gw mcp` and
/// `gw companion …` to find it.
pub fn server_file() -> PathBuf {
    gw_core::userconfig::config_dir().join("server.json")
}

pub fn write_server_file(ui_port: u16, companion_port: Option<u16>) {
    let v = json!({
        "ui_port": ui_port, "companion_port": companion_port, "pid": std::process::id(),
        "started": chrono::Utc::now().to_rfc3339(),
    });
    let p = server_file();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(&p, serde_json::to_vec_pretty(&v).unwrap_or_default());
}
