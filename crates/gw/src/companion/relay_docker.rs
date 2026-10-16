//! `/v1/relay/docker/<api path>` — the Docker Engine API on the local socket, read-only (as
//! container-relay.py): GET only, an allow-list of read endpoints (no exec, attach, archive,
//! export or image downloads), streaming reads (logs follow, stats, events) passed through.
//! The socket is auto-detected: $DOCKER_HOST (unix://), /var/run/docker.sock, Rancher Desktop
//! (~/.rd/docker.sock), Docker Desktop, Colima, OrbStack; or set `docker_host`.

use super::{authorize, bad_path, err, tagged, Gateway, Origin};
use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    Json,
};
use regex::Regex;
use serde_json::{json, Value};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

fn docker_res() -> &'static [Regex] {
    static R: OnceLock<Vec<Regex>> = OnceLock::new();
    R.get_or_init(|| {
        [
            r"^/_ping$", r"^/version$", r"^/info$", r"^/system/df$", r"^/events$",
            r"^/containers/json$", r"^/containers/[\w.-]+/(json|logs|stats|top|changes)$",
            r"^/images/json$", r"^/images/[\w.:@/-]+/(json|history)$",
            r"^/volumes$", r"^/volumes/[\w.-]+$", r"^/networks$", r"^/networks/[\w.-]+$",
        ]
        .iter()
        .map(|p| Regex::new(p).unwrap())
        .collect()
    })
}

/// A read-only Docker API path (an optional /v1.xx version prefix is allowed).
pub fn path_ok(path: &str) -> bool {
    static V: OnceLock<Regex> = OnceLock::new();
    let v = V.get_or_init(|| Regex::new(r"^/v\d+\.\d+/").unwrap());
    let p = if v.is_match(path) { &path[path[1..].find('/').map(|i| i + 1).unwrap_or(0)..] } else { path };
    !bad_path(path) && docker_res().iter().any(|r| r.is_match(p))
}

/// `unix:///path` or `tcp://host:port`: the configured one, $DOCKER_HOST, or the first socket found.
pub fn find_socket(configured: &str) -> Option<String> {
    if !configured.is_empty() {
        return Some(configured.to_string());
    }
    if let Ok(e) = std::env::var("DOCKER_HOST") {
        if e.starts_with("unix://") {
            return Some(e);
        }
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let candidates = [
        "/var/run/docker.sock".to_string(),
        format!("{home}/.rd/docker.sock"),
        format!("{home}/.docker/run/docker.sock"),
        format!("{home}/.colima/default/docker.sock"),
        format!("{home}/.orbstack/run/docker.sock"),
    ];
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        for c in candidates {
            if std::fs::metadata(&c).map(|m| m.file_type().is_socket()).unwrap_or(false) {
                return Some(format!("unix://{c}"));
            }
        }
    }
    #[cfg(not(unix))]
    let _ = candidates;
    None
}

/// Send one GET to the Docker API over its socket; the response body streams.
pub async fn get(host: &str, path_q: &str, accept: &str, timeout: Option<Duration>) -> anyhow::Result<hyper::Response<hyper::body::Incoming>> {
    use http_body_util::Empty;
    use hyper_util::rt::TokioIo;
    let req = hyper::Request::builder()
        .method("GET")
        .uri(path_q)
        .header("host", "docker")
        .header("accept", accept)
        .header("user-agent", "gitwebsite-companion/1")
        .body(Empty::<bytes::Bytes>::new())?;
    macro_rules! send {
        ($stream:expr) => {{
            let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new($stream)).await?;
            tokio::spawn(async move {
                let _ = conn.await;
            });
            let fut = sender.send_request(req);
            match timeout {
                Some(t) => tokio::time::timeout(t, fut).await.map_err(|_| anyhow::anyhow!("Docker did not answer in time"))??,
                None => fut.await?,
            }
        }};
    }
    if let Some(p) = host.strip_prefix("unix://") {
        #[cfg(unix)]
        {
            let s = tokio::net::UnixStream::connect(p).await?;
            return Ok(send!(s));
        }
        #[cfg(not(unix))]
        anyhow::bail!("unix sockets are not supported here: {p}");
    }
    if let Some(a) = host.strip_prefix("tcp://").or_else(|| host.strip_prefix("http://")) {
        let s = tokio::net::TcpStream::connect(a.trim_end_matches('/')).await?;
        return Ok(send!(s));
    }
    anyhow::bail!("unsupported docker_host {host:?} (use unix:///path or tcp://host:port)")
}

pub async fn handle(State(gw): State<Arc<Gateway>>, axum::Extension(Origin(origin)): axum::Extension<Origin>, method: Method, uri: Uri, h: HeaderMap) -> Response {
    let s = gw.settings();
    if !s.relays.docker {
        return err(StatusCode::SERVICE_UNAVAILABLE, "the Docker relay is turned off in the companion settings");
    }
    let (rec, _) = match authorize(&gw, &h, &origin, "relay.docker") {
        Ok(x) => x,
        Err(r) => return r,
    };
    if method != Method::GET {
        let mut r = err(StatusCode::METHOD_NOT_ALLOWED, format!("{method} refused: the Docker relay is read-only (GET only)"));
        r.headers_mut().insert("allow", HeaderValue::from_static("GET, OPTIONS"));
        return tagged(r, &rec);
    }
    if h.contains_key("upgrade") {
        return tagged(err(StatusCode::BAD_REQUEST, "upgrade requests are not relayed (no exec or attach)"), &rec);
    }
    let Some(sock) = find_socket(&s.docker_host) else {
        return tagged(err(StatusCode::SERVICE_UNAVAILABLE, "no Docker socket found (set docker_host in the companion settings)"), &rec);
    };
    let path = uri.path().strip_prefix("/v1/relay/docker").unwrap_or("");
    if path.is_empty() || path == "/" {
        return tagged(Json(status(&sock).await).into_response(), &rec);
    }
    if !path_ok(path) {
        return tagged(err(StatusCode::FORBIDDEN, format!("not a read-only Docker endpoint this relay forwards: {path}")), &rec);
    }
    let query = uri.query().unwrap_or("");
    let follow = is_follow(path, query);
    let target = if query.is_empty() { path.to_string() } else { format!("{path}?{query}") };
    let accept = h.get("accept").and_then(|v| v.to_str().ok()).unwrap_or("*/*").to_string();
    let r = match get(&sock, &target, &accept, if follow { None } else { Some(Duration::from_secs(30)) }).await {
        Ok(r) => r,
        Err(e) => return tagged(err(StatusCode::BAD_GATEWAY, format!("docker: {e:#}")), &rec),
    };
    tagged(stream_response(r, "docker"), &rec)
}

/// Streaming reads (no timeout): follow/watch/stream, stats (unless stream=0), events.
pub fn is_follow(path: &str, query: &str) -> bool {
    let q: Vec<&str> = query.split('&').collect();
    let on = |k: &str| q.iter().any(|p| *p == format!("{k}=1") || *p == format!("{k}=true"));
    on("follow") || on("watch") || on("stream")
        || (path.ends_with("/stats") && !q.iter().any(|p| *p == "stream=0" || *p == "stream=false"))
        || path.ends_with("/events")
}

/// Pass an upstream hyper response through (status, content type, Api-Version; body streams).
pub fn stream_response(r: hyper::Response<hyper::body::Incoming>, upstream: &'static str) -> Response {
    let (parts, body) = r.into_parts();
    let mut out = Response::new(Body::new(body));
    *out.status_mut() = parts.status;
    let hd = out.headers_mut();
    for k in ["content-type", "api-version", "docker-experimental"] {
        if let Some(v) = parts.headers.get(k) {
            hd.insert(k, v.clone());
        }
    }
    hd.insert("cache-control", HeaderValue::from_static("no-store"));
    hd.insert("x-relay-upstream", HeaderValue::from_static(upstream));
    out
}

async fn status(sock: &str) -> Value {
    match get(sock, "/_ping", "*/*", Some(Duration::from_secs(3))).await {
        Ok(r) => json!({
            "host": sock, "ok": r.status().is_success(), "readOnly": true,
            "apiVersion": r.headers().get("api-version").and_then(|v| v.to_str().ok()),
            "os": r.headers().get("ostype").and_then(|v| v.to_str().ok()),
        }),
        Err(e) => json!({ "host": sock, "ok": false, "readOnly": true, "error": format!("{e:#}") }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn read_only_paths() {
        for ok in ["/_ping", "/v1.43/containers/json", "/containers/abc/logs", "/images/nginx:latest/json", "/events", "/networks"] {
            assert!(path_ok(ok), "{ok}");
        }
        for bad in ["/containers/abc/exec", "/containers/abc/attach", "/containers/abc/archive", "/containers/abc/export",
            "/images/get", "/images/x/get", "/containers/create", "/containers/../info", "/v1.43/exec/x/start", "/build", "/secrets"] {
            assert!(!path_ok(bad), "{bad}");
        }
        assert!(is_follow("/containers/x/logs", "follow=1&stdout=1"));
        assert!(is_follow("/containers/x/stats", ""));
        assert!(!is_follow("/containers/x/stats", "stream=0"));
        assert!(!is_follow("/containers/json", "all=1"));
    }
}
