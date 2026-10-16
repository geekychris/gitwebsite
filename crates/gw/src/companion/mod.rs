//! The companion gateway: a small, local-only HTTP/WebSocket service that the site's one-page
//! tools call for what a browser cannot do on its own — reach APIs without CORS headers, open
//! TCP connections, read the Docker socket or a Kubernetes API, clone over git smart-HTTP. It
//! replaces the per-tool relay scripts (aviation-relay.py, container-relay.py, …).
//!
//! Listens on 127.0.0.1:7317 (fallbacks 7318–7320), separate from the app's UI port. Protocol v1:
//! see docs/companion.adoc. Every /v1 request passes [`gate`]: Host must be this machine, Origin
//! must be allowed (the site, http://127.0.0.1:*, http://localhost:*, user-added), preflights are
//! answered (CORS and Private Network Access). Everything except `/v1/hello` and pairing needs a
//! token from pairing, with the scope of the relay used.

pub mod pairing;
pub mod relay_docker;
pub mod relay_git;
pub mod relay_http;
pub mod relay_kube;
pub mod relay_tcp;
pub mod secrets;
pub mod settings;
pub mod view;

use axum::{
    body::Body,
    extract::{Path as AxPath, Request, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{any, get, post},
    Json, Router,
};
use pairing::{Decision, Pairing, Tokens, TokenRecord};
use serde::Deserialize;
use serde_json::{json, Value};
use settings::Settings;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

pub const VERSION: &str = "1";
pub const NAME: &str = "gitwebsite-companion";

/// Called when a pairing request needs the user's decision (the app shows a dialog).
type PairHook = Box<dyn Fn(pairing::PairRequest) + Send + Sync>;

pub struct Gateway {
    pub settings: RwLock<Settings>,
    pub tokens: Tokens,
    pub pairing: Pairing,
    pub secrets: secrets::Store,
    /// The app's own UI/preview port; pages served from it are paired without asking.
    pub ui_port: AtomicU16,
    /// The port the gateway is listening on (0 before it binds).
    pub port: AtomicU16,
    audit: Audit,
    pub(crate) http: reqwest::Client,
    pub(crate) cache: relay_http::Cache,
    pub(crate) supplier_tokens: Mutex<std::collections::HashMap<String, (String, Instant)>>,
    pub(crate) kube: relay_kube::Contexts,
    /// Tests: replace a fixed upstream's base URL (e.g. "digikey" → a fake server).
    pub(crate) upstream_override: RwLock<std::collections::HashMap<String, String>>,
    pair_hook: RwLock<Option<PairHook>>,
}

impl Gateway {
    /// The gateway with the user's settings, token file, audit log and keychain.
    pub fn from_user_config() -> anyhow::Result<Self> {
        let s = Settings::load()?;
        let dir = settings::dir();
        Ok(Self::new(s, Some(dir.join("companion-tokens.json")), Some(dir.join("companion-audit.log")), secrets::Store::System))
    }

    pub fn new(s: Settings, tokens: Option<PathBuf>, audit: Option<PathBuf>, secrets: secrets::Store) -> Self {
        Gateway {
            settings: RwLock::new(s),
            tokens: Tokens::open(tokens),
            pairing: Pairing::default(),
            secrets,
            ui_port: AtomicU16::new(0),
            port: AtomicU16::new(0),
            audit: Audit { path: audit, recent: Mutex::new(std::collections::VecDeque::new()) },
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("http client"),
            cache: Default::default(),
            supplier_tokens: Default::default(),
            kube: Default::default(),
            upstream_override: Default::default(),
            pair_hook: RwLock::new(None),
        }
    }

    pub fn settings(&self) -> Settings {
        self.settings.read().unwrap().clone()
    }

    /// Replace a fixed upstream's base URL (tests, or a regional API endpoint).
    pub fn set_upstream_override(&self, id: &str, base: &str) {
        self.upstream_override.write().unwrap().insert(id.into(), base.into());
    }

    pub fn set_pair_hook(&self, f: impl Fn(pairing::PairRequest) + Send + Sync + 'static) {
        *self.pair_hook.write().unwrap() = Some(Box::new(f));
    }

    /// The app's preview origins (auto-paired).
    fn own_origin(&self, origin: &str) -> bool {
        let p = self.ui_port.load(Ordering::Relaxed);
        p != 0 && (origin == format!("http://127.0.0.1:{p}") || origin == format!("http://localhost:{p}"))
    }

    pub fn origin_allowed(&self, o: &str) -> bool {
        origin_ok(o, &self.settings.read().unwrap().extra_origins)
    }

    /// Status for the app's Companion page and `companion_status` (no secrets, no tokens).
    pub fn status(&self) -> Value {
        let s = self.settings();
        json!({
            "name": NAME, "version": VERSION,
            "running": self.port.load(Ordering::Relaxed) != 0,
            "port": self.port.load(Ordering::Relaxed),
            "url": format!("http://127.0.0.1:{}", self.port.load(Ordering::Relaxed)),
            "settings_file": settings::path().display().to_string(),
            "settings": s,
            "origins": allowed_origins(&s.extra_origins),
            "paired": self.tokens.list(),
            "pending": self.pairing.list(),
            "secrets": self.secrets.presence(),
            "docker": relay_docker::find_socket(&s.docker_host),
            "kube": relay_kube::describe(&s.kubeconfig),
            "upstreams": relay_http::upstream_ids(),
            "view": view::available(),
        })
    }

    pub fn audit_tail(&self, n: usize) -> Vec<Value> {
        self.audit.tail(n)
    }
}

// ------------------------------------------------------------------ the running gateway

static GATEWAY: OnceLock<Arc<Gateway>> = OnceLock::new();

/// The gateway started in this process, if any.
pub fn global() -> Option<Arc<Gateway>> {
    GATEWAY.get().cloned()
}

#[derive(Debug, Clone, Default)]
pub struct StartOptions {
    /// The app's UI/preview port (its pages are paired without asking).
    pub ui_port: Option<u16>,
    /// Override the configured port (`gw serve --companion-port`, `GW_COMPANION_PORT`).
    pub port: Option<u16>,
    /// Approve every pairing request (headless use).
    pub auto_approve: bool,
}

/// Start the gateway on this runtime: binds the first free port of port..port+3. Returns the
/// port, or `None` when it is disabled in the settings.
pub async fn start(opts: StartOptions) -> anyhow::Result<Option<u16>> {
    let gw = match global() {
        Some(g) => g,
        None => {
            let g = Arc::new(Gateway::from_user_config()?);
            let _ = GATEWAY.set(g);
            global().unwrap()
        }
    };
    if opts.auto_approve {
        gw.settings.write().unwrap().auto_approve = true;
    }
    if let Some(p) = opts.ui_port {
        gw.ui_port.store(p, Ordering::Relaxed);
    }
    let s = gw.settings();
    if !s.enabled {
        eprintln!("companion: disabled in {}", settings::path().display());
        return Ok(None);
    }
    let first = opts
        .port
        .or_else(|| std::env::var("GW_COMPANION_PORT").ok().and_then(|p| p.parse().ok()))
        .unwrap_or(s.port);
    let mut listener = None;
    for p in first..first.saturating_add(4) {
        if let Ok(l) = tokio::net::TcpListener::bind(("127.0.0.1", p)).await {
            listener = Some(l);
            break;
        }
    }
    let Some(listener) = listener else {
        anyhow::bail!("companion: ports {first}–{} are all taken", first + 3);
    };
    let port = listener.local_addr()?.port();
    gw.port.store(port, Ordering::Relaxed);
    let app = router(gw.clone());
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            eprintln!("companion: stopped: {e}");
        }
    });
    eprintln!("companion: http://127.0.0.1:{port}/v1/hello");
    Ok(Some(port))
}

// ------------------------------------------------------------------ origins, hosts, CORS

pub fn allowed_origins(extra: &[String]) -> Vec<String> {
    let mut v: Vec<String> = settings::DEFAULT_ORIGINS.iter().map(|s| s.to_string()).collect();
    v.push("http://127.0.0.1:*".into());
    v.push("http://localhost:*".into());
    v.extend(extra.iter().cloned());
    v
}

/// The site, local previews (http://127.0.0.1:*, http://localhost:*, http://[::1]:*), and
/// user-added origins. Exact matches only — no prefixes, so http://127.0.0.1.evil.test fails.
pub fn origin_ok(o: &str, extra: &[String]) -> bool {
    if settings::DEFAULT_ORIGINS.contains(&o) || extra.iter().any(|e| e == o) {
        return true;
    }
    for local in ["http://127.0.0.1", "http://localhost", "http://[::1]"] {
        if let Some(rest) = o.strip_prefix(local) {
            if rest.is_empty() || (rest.starts_with(':') && rest.len() > 1 && rest[1..].bytes().all(|b| b.is_ascii_digit())) {
                return true;
            }
        }
    }
    false
}

/// Host must name this machine (defeats DNS rebinding).
pub fn host_ok(h: &str) -> bool {
    let h = h.to_ascii_lowercase();
    let name = if h.starts_with('[') {
        h.split(']').next().map(|s| format!("{s}]")).unwrap_or_default()
    } else {
        h.split(':').next().unwrap_or("").to_string()
    };
    let port_ok = match h.rsplit_once(']') {
        Some((_, rest)) => rest.is_empty() || rest.strip_prefix(':').is_some_and(|p| p.parse::<u16>().is_ok()),
        None => h.split_once(':').map(|(_, p)| p.parse::<u16>().is_ok()).unwrap_or(true),
    };
    matches!(name.as_str(), "127.0.0.1" | "localhost" | "[::1]") && port_ok
}

fn hv(s: &str) -> HeaderValue {
    HeaderValue::from_str(s).unwrap_or_else(|_| HeaderValue::from_static(""))
}

fn cors_headers(h: &mut HeaderMap, origin: &str) {
    h.insert("access-control-allow-origin", hv(origin));
    h.append(header::VARY, HeaderValue::from_static("Origin"));
    h.insert("access-control-allow-private-network", HeaderValue::from_static("true"));
    h.insert(
        "access-control-expose-headers",
        HeaderValue::from_static("Content-Type, X-Relay-Upstream, X-Relay-Cache, X-Rate-Limit-Remaining, X-RateLimit-Remaining, X-RateLimit-Limit, X-BurstLimit-Remaining, Retry-After, Api-Version, Docker-Experimental, Server"),
    );
}

pub fn err(code: StatusCode, msg: impl std::fmt::Display) -> Response {
    (code, Json(json!({ "error": msg.to_string() }))).into_response()
}

/// The page's origin, or "" for a non-browser client (curl, the CLI), which sends none.
#[derive(Clone)]
pub struct Origin(pub String);

/// Which token served a request (for the audit log).
#[derive(Clone)]
pub struct AuditTool(pub String);

/// Host, Origin, preflight and CORS for every /v1 request; then the audit log.
async fn gate(State(gw): State<Arc<Gateway>>, mut req: Request, next: Next) -> Response {
    let host = req.headers().get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    if !host_ok(&host) {
        return err(StatusCode::FORBIDDEN, format!("Host {host:?} not allowed: use http://127.0.0.1:{}/", gw.port.load(Ordering::Relaxed)));
    }
    let origin = req.headers().get(header::ORIGIN).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    if !origin.is_empty() && !gw.origin_allowed(&origin) {
        return err(StatusCode::FORBIDDEN, format!("origin {origin} is not allowed (add it in the app's Companion settings)"));
    }
    if req.method() == Method::OPTIONS {
        let mut r = StatusCode::NO_CONTENT.into_response();
        let h = r.headers_mut();
        if !origin.is_empty() {
            cors_headers(h, &origin);
        }
        h.insert("access-control-allow-methods", HeaderValue::from_static("GET, POST, PUT, DELETE, OPTIONS"));
        let asked = req.headers().get("access-control-request-headers").cloned();
        h.insert(
            "access-control-allow-headers",
            asked.unwrap_or_else(|| HeaderValue::from_static("Authorization, Content-Type, Accept, X-Companion-Token")),
        );
        h.insert("access-control-max-age", HeaderValue::from_static("600"));
        return r;
    }
    let t0 = Instant::now();
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    req.extensions_mut().insert(Origin(origin.clone()));
    let mut res = next.run(req).await;
    if !origin.is_empty() {
        cors_headers(res.headers_mut(), &origin);
    }
    if path != "/v1/hello" {
        let tool = res.extensions().get::<AuditTool>().map(|t| t.0.clone()).unwrap_or_default();
        gw.audit.log(json!({
            "at": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "origin": origin, "tool": tool, "method": method.as_str(), "path": path,
            "status": res.status().as_u16(), "ms": t0.elapsed().as_millis() as u64,
        }));
    }
    res
}

/// Where a request's pairing token comes from: `X-Companion-Token`, else `Authorization: Bearer`,
/// else (WebSocket) a `bearer.<token>` subprotocol. Query-string tokens are never accepted.
pub struct Presented {
    pub token: String,
    /// The token came in Authorization (so that header must not be forwarded upstream).
    pub via_authorization: bool,
    /// The subprotocol to echo on a WebSocket upgrade.
    pub subprotocol: Option<String>,
}

pub fn presented(h: &HeaderMap) -> Option<Presented> {
    if let Some(t) = h.get("x-companion-token").and_then(|v| v.to_str().ok()).filter(|t| !t.is_empty()) {
        return Some(Presented { token: t.trim().to_string(), via_authorization: false, subprotocol: None });
    }
    if let Some(t) = h.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")) {
        return Some(Presented { token: t.trim().to_string(), via_authorization: true, subprotocol: None });
    }
    for v in h.get_all(header::SEC_WEBSOCKET_PROTOCOL) {
        for p in v.to_str().unwrap_or("").split(',').map(str::trim) {
            if let Some(t) = p.strip_prefix("bearer.") {
                return Some(Presented { token: t.to_string(), via_authorization: false, subprotocol: Some(p.to_string()) });
            }
        }
    }
    None
}

/// Check the request's token for `scope`. Err is the response to send.
pub fn authorize(gw: &Gateway, h: &HeaderMap, origin: &str, scope: &str) -> Result<(TokenRecord, Presented), Response> {
    let Some(p) = presented(h) else {
        return Err(err(StatusCode::UNAUTHORIZED, "pair first: POST /v1/pair, then send Authorization: Bearer <token>"));
    };
    let Some(rec) = gw.tokens.check(&p.token, origin) else {
        return Err(err(StatusCode::UNAUTHORIZED, "unknown or revoked token for this origin: pair again"));
    };
    if !rec.scopes.iter().any(|s| s == scope) {
        return Err(err(StatusCode::FORBIDDEN, format!("this token does not have the {scope} scope: pair again asking for it")));
    }
    Ok((rec, p))
}

/// Attach the token's tool to a response for the audit log.
pub fn tagged(mut r: Response, rec: &TokenRecord) -> Response {
    r.extensions_mut().insert(AuditTool(rec.tool.clone()));
    r
}

// ------------------------------------------------------------------ audit log

struct Audit {
    path: Option<PathBuf>,
    recent: Mutex<std::collections::VecDeque<Value>>,
}

const AUDIT_MAX: u64 = 2 * 1024 * 1024;

impl Audit {
    fn log(&self, v: Value) {
        {
            let mut r = self.recent.lock().unwrap();
            r.push_back(v.clone());
            while r.len() > 500 {
                r.pop_front();
            }
        }
        let Some(p) = &self.path else { return };
        if std::fs::metadata(p).map(|m| m.len() > AUDIT_MAX).unwrap_or(false) {
            let _ = std::fs::rename(p, p.with_extension("log.1"));
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
            let _ = writeln!(f, "{v}");
        }
    }

    /// The last `n` entries, newest first (from the file when there is one).
    fn tail(&self, n: usize) -> Vec<Value> {
        let mut v: Vec<Value> = match &self.path {
            Some(p) => std::fs::read_to_string(p)
                .unwrap_or_default()
                .lines()
                .rev()
                .take(n)
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect(),
            None => self.recent.lock().unwrap().iter().rev().take(n).cloned().collect(),
        };
        v.truncate(n);
        v
    }
}

// ------------------------------------------------------------------ /v1 handlers

async fn hello(State(gw): State<Arc<Gateway>>, axum::Extension(Origin(origin)): axum::Extension<Origin>, h: HeaderMap) -> Response {
    let s = gw.settings();
    let mut caps = vec![];
    for (on, c) in [(s.relays.http, "relay.http"), (s.relays.tcp, "relay.tcp"), (s.relays.docker, "relay.docker"), (s.relays.kube, "relay.kube"), (s.relays.git, "relay.git")] {
        if on {
            caps.push(c);
        }
    }
    caps.push("view");
    let paired = presented(&h).and_then(|p| gw.tokens.check(&p.token, &origin));
    Json(json!({
        "name": NAME, "version": VERSION, "capabilities": caps,
        "paired": paired.is_some(),
        "scopes": paired.map(|r| r.scopes).unwrap_or_default(),
        "app": gw.own_origin(&origin),
    }))
    .into_response()
}

#[derive(Deserialize, Default)]
struct PairBody {
    #[serde(default)]
    tool: String,
    #[serde(default)]
    scopes: Vec<String>,
}

fn clean_tool(t: &str) -> String {
    t.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(64).collect()
}

/// POST /v1/pair {tool, scopes} → 200 {token} (the app's own pages, or auto-approve), else
/// 202 {id, code, poll} and the app asks the user.
async fn pair(State(gw): State<Arc<Gateway>>, axum::Extension(Origin(origin)): axum::Extension<Origin>, body: Option<Json<PairBody>>) -> Response {
    let b = body.map(|b| b.0).unwrap_or_default();
    let tool = clean_tool(&b.tool);
    let scopes = match pairing::clean_scopes(&b.scopes) {
        Ok(s) => s,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    let who = if origin.is_empty() { "(no origin)".to_string() } else { origin.clone() };
    if gw.own_origin(&origin) || gw.settings.read().unwrap().auto_approve {
        let (token, rec) = gw.tokens.issue(&origin, &tool, scopes);
        return tagged(Json(json!({ "token": token, "scopes": rec.scopes, "id": rec.id })).into_response(), &rec);
    }
    let req = gw.pairing.create(&origin, &tool, scopes);
    eprintln!("companion: pairing request from {who} for {tool:?} {:?} — code {} (approve in the app or `gw companion approve {}`)", req.scopes, req.code, req.code);
    if let Some(f) = gw.pair_hook.read().unwrap().as_ref() {
        f(req.clone());
    }
    let mut r = (
        StatusCode::ACCEPTED,
        Json(json!({ "id": req.id, "code": req.code, "poll": format!("/v1/pair/{}", req.id), "expires_in": pairing::PAIR_TTL.as_secs() })),
    )
        .into_response();
    r.extensions_mut().insert(AuditTool(tool));
    r
}

/// GET /v1/pair/{id} — long-polls (≤ 50 s): 200 {token}, 403 denied, 202 still pending.
async fn pair_poll(State(gw): State<Arc<Gateway>>, axum::Extension(Origin(origin)): axum::Extension<Origin>, AxPath(id): AxPath<String>) -> Response {
    match gw.pairing.wait(&id, &origin, Duration::from_secs(50)).await {
        None => err(StatusCode::NOT_FOUND, "no such pairing request (expired, or asked from another origin)"),
        Some(Decision::Pending) => (StatusCode::ACCEPTED, Json(json!({ "pending": true }))).into_response(),
        Some(Decision::Denied) => err(StatusCode::FORBIDDEN, "pairing denied"),
        Some(Decision::Approved { token, scopes }) => Json(json!({ "token": token, "scopes": scopes })).into_response(),
    }
}

/// GET /v1/status — what the relays are connected to (no secrets). Any valid token.
async fn status(State(gw): State<Arc<Gateway>>, axum::Extension(Origin(origin)): axum::Extension<Origin>, h: HeaderMap) -> Response {
    let Some(p) = presented(&h) else { return err(StatusCode::UNAUTHORIZED, "pair first") };
    let Some(rec) = gw.tokens.check(&p.token, &origin) else { return err(StatusCode::UNAUTHORIZED, "unknown or revoked token") };
    let s = gw.settings();
    let r = Json(json!({
        "name": NAME, "version": VERSION, "tool": rec.tool, "scopes": rec.scopes,
        "relays": s.relays,
        "http": { "upstreams": relay_http::upstream_ids(), "receiver": !s.receiver.is_empty(), "kafka_rest": !s.kafka_rest.is_empty(),
                  "redpanda_proxy": !s.redpanda_proxy.is_empty(), "redpanda_admin": !s.redpanda_admin.is_empty(),
                  "secrets": { "digikey": gw.secrets.holds("digikey"), "mouser": gw.secrets.holds("mouser"), "nexar": gw.secrets.holds("nexar") } },
        "tcp": { "allow": s.tcp_allow },
        "docker": relay_docker::find_socket(&s.docker_host),
        "kube": relay_kube::describe(&s.kubeconfig),
        "git": { "hosts": relay_git::hosts(&s), "push": s.git_allow_push },
    }))
    .into_response();
    tagged(r, &rec)
}

/// POST /v1/view {tool, params, …} — open a tool in an app window (scope `view`).
async fn v1_view(State(gw): State<Arc<Gateway>>, axum::Extension(Origin(origin)): axum::Extension<Origin>, h: HeaderMap, Json(b): Json<view::ViewRequest>) -> Response {
    let (rec, _) = match authorize(&gw, &h, &origin, "view") {
        Ok(x) => x,
        Err(r) => return r,
    };
    let ui = gw.ui_port.load(Ordering::Relaxed);
    let r = match tokio::task::spawn_blocking(move || view::open(ui, b)).await {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) => err(StatusCode::BAD_REQUEST, format!("{e:#}")),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    tagged(r, &rec)
}

pub fn router(gw: Arc<Gateway>) -> Router {
    Router::new()
        .route("/v1/hello", get(hello))
        .route("/v1/pair", post(pair))
        .route("/v1/pair/{id}", get(pair_poll))
        .route("/v1/status", get(status))
        .route("/v1/view", post(v1_view))
        .route("/v1/relay/http/{*rest}", any(relay_http::handle))
        .route("/v1/relay/tcp", get(relay_tcp::handle))
        .route("/v1/relay/docker", get(relay_docker::handle))
        .route("/v1/relay/docker/{*rest}", any(relay_docker::handle))
        .route("/v1/relay/kube", get(relay_kube::handle))
        .route("/v1/relay/kube/{*rest}", any(relay_kube::handle))
        .route("/v1/relay/git/{*rest}", any(relay_git::handle))
        .fallback(|| async { err(StatusCode::NOT_FOUND, "no such companion endpoint (see /v1/hello)") })
        .layer(middleware::from_fn_with_state(gw.clone(), gate))
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024 * 1024))
        .with_state(gw)
}

/// A response body from bytes with a content type.
pub fn bytes_response(code: StatusCode, ctype: &str, body: impl Into<Body>) -> Response {
    let mut r = Response::new(body.into());
    *r.status_mut() = code;
    r.headers_mut().insert(header::CONTENT_TYPE, hv(ctype));
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

/// Reject paths that could escape their route: `..`, `//`, backslashes, encoded slashes or
/// dots, NULs and control characters.
pub fn bad_path(p: &str) -> bool {
    let l = p.to_ascii_lowercase();
    l.contains("..") || l.contains("//") || l.contains('\\') || l.contains("%2f") || l.contains("%5c") || l.contains("%2e") || l.contains("%00")
        || l.bytes().any(|b| b < 0x20 || b == 0x7f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins() {
        assert!(origin_ok("https://geekychris.github.io", &[]));
        assert!(origin_ok("http://127.0.0.1:8790", &[]));
        assert!(origin_ok("http://localhost:3000", &[]));
        assert!(origin_ok("http://localhost", &[]));
        assert!(!origin_ok("http://127.0.0.1.evil.test", &[]));
        assert!(!origin_ok("http://localhost:80@evil.test", &[]));
        assert!(!origin_ok("https://geekychris.github.io.evil.test", &[]));
        assert!(!origin_ok("https://evil.test", &[]));
        assert!(!origin_ok("null", &[]));
        assert!(origin_ok("https://me.test", &["https://me.test".into()]));
    }

    #[test]
    fn hosts() {
        for ok in ["127.0.0.1:7317", "localhost:7317", "[::1]:7317", "127.0.0.1", "LOCALHOST:1"] {
            assert!(host_ok(ok), "{ok}");
        }
        for bad in ["evil.test:7317", "127.0.0.1.evil.test", "", "127.0.0.1:abc", "localhost.evil:7317", "[::2]:1"] {
            assert!(!host_ok(bad), "{bad}");
        }
    }

    #[test]
    fn paths() {
        assert!(bad_path("/a/../b") && bad_path("/a//b") && bad_path("/a%2Fb") && bad_path("/a\\b") && bad_path("/%2e%2e/x"));
        assert!(!bad_path("/v2/aircraft/json"));
    }
}
