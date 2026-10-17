//! `/v1/relay/lan/<host[:port]>/<path>[?tls=1][&insecure=1]` — HTTP(S) to a device on the
//! user's LAN (Home Assistant, Shelly, Tasmota, Hue, Roku ECP, WebHDFS, MinIO, Tizen REST).
//!
//! * The host must be on the LAN (see [`super::lan`]): public addresses are refused, names are
//!   resolved and every address checked, and the connection goes only to checked addresses.
//! * It must be on `lan_hosts`; a paired page's first request to a new one waits while the app
//!   asks the user, and approving adds the host.
//! * `?tls=1` → https; `?insecure=1` also accepts a self-signed certificate (LAN only, which
//!   this relay always is). Both are removed from the query sent upstream.
//! * Headers passed: Content-Type, Accept, Range, the vendor headers `hue-application-key` and
//!   `X-Auth-Token`, and Authorization — from `X-Upstream-Authorization` (so it doesn't clash
//!   with the companion's own `Authorization: Bearer`), or the page's `Authorization` when the
//!   token came in `X-Companion-Token`. Cookies and the Origin never go upstream.

use super::{authorize, err, lan, limited_body, relay_http::upstream_url, stream_back, tagged, Gateway, Origin};
use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri},
    response::Response,
};
use std::sync::Arc;

pub const MAX_BODY: usize = 64 * 1024 * 1024;
const PASS: &[&str] = &["content-type", "accept", "range", "hue-application-key", "x-auth-token"];
const KEEP: &[&str] = &["content-type", "content-length", "content-range", "accept-ranges", "etag", "last-modified", "retry-after", "server"];

/// Split `host[:port]` (IPv6 in brackets). Returns (host without brackets, port, URL authority).
pub fn split_authority(a: &str) -> Option<(String, Option<u16>, String)> {
    let a = a.to_ascii_lowercase();
    if let Some(rest) = a.strip_prefix('[') {
        let (h, tail) = rest.split_once(']')?;
        lan::literal(h)?;
        let port = match tail.strip_prefix(':') {
            Some(p) => Some(p.parse::<u16>().ok().filter(|p| *p > 0)?),
            None if tail.is_empty() => None,
            None => return None,
        };
        return Some((h.to_string(), port, a.clone()));
    }
    let (h, port) = match a.split_once(':') {
        Some((h, p)) => (h.to_string(), Some(p.parse::<u16>().ok().filter(|p| *p > 0)?)),
        None => (a.clone(), None),
    };
    lan::host_syntax_ok(&h).then_some((h.clone(), port, a))
}

fn canon(h: &str) -> &str {
    if h == "localhost" || h == "::1" {
        "127.0.0.1"
    } else {
        h
    }
}

/// Is `host`:`port` on the LAN allow-list (`host` = any port, `host:port` = that port)?
pub fn listed(host: &str, port: u16, list: &[String]) -> bool {
    list.iter().filter_map(|e| super::settings::parse_host_entry(e)).any(|(h, p)| canon(&h) == canon(host) && p.is_none_or(|p| p == port))
}

/// Everything a LAN target must pass before a connection: LAN addresses only, not the
/// companion's or the app's own ports, and on the allow-list (or approved now).
pub async fn admit(gw: &Gateway, host: &str, port: u16, origin: &str, tool: &str, detail: &str) -> Result<Vec<std::net::IpAddr>, (StatusCode, String)> {
    let addrs = lan::resolve(host, &gw.dns).await.map_err(|e| (StatusCode::FORBIDDEN, e))?;
    if lan::own_port_refused(&addrs, port, &gw.own_ports()) {
        return Err((StatusCode::FORBIDDEN, format!("{host}:{port} is the companion's or the app's own port")));
    }
    if !listed(host, port, &gw.settings.read().unwrap().lan_hosts) {
        let target = if host.contains(':') { format!("[{host}]") } else { host.to_string() };
        gw.ask_approval("lan", &target, origin, tool, detail).await?;
    }
    Ok(addrs)
}

pub async fn handle(
    State(gw): State<Arc<Gateway>>,
    axum::Extension(Origin(origin)): axum::Extension<Origin>,
    method: Method,
    uri: Uri,
    h: HeaderMap,
    body: Body,
) -> Response {
    if !gw.settings().relays.lan {
        return err(StatusCode::SERVICE_UNAVAILABLE, "the LAN relay is turned off in the companion settings");
    }
    let (rec, presented) = match authorize(&gw, &h, &origin, "relay.lan") {
        Ok(x) => x,
        Err(r) => return r,
    };
    let r = relay(&gw, &origin, &rec.tool, method, &uri, &h, body, presented.via_authorization).await;
    tagged(r, &rec)
}

#[allow(clippy::too_many_arguments)]
async fn relay(gw: &Gateway, origin: &str, tool: &str, method: Method, uri: &Uri, h: &HeaderMap, body: Body, auth_is_ours: bool) -> Response {
    if ![Method::GET, Method::HEAD, Method::POST, Method::PUT, Method::PATCH, Method::DELETE].contains(&method) {
        return err(StatusCode::METHOD_NOT_ALLOWED, format!("{method} refused: GET, HEAD, POST, PUT, PATCH, DELETE only"));
    }
    let after = uri.path().strip_prefix("/v1/relay/lan/").unwrap_or("");
    let (authority, path) = after.split_once('/').unwrap_or((after, ""));
    let Some((host, port, auth)) = split_authority(authority) else {
        return err(StatusCode::BAD_REQUEST, format!("bad host {authority:?}: use /v1/relay/lan/<host[:port]>/<path>"));
    };
    // Our own query parameters; the rest go upstream as sent.
    let (mut tls, mut insecure) = (false, false);
    let mut q = vec![];
    for p in uri.query().unwrap_or("").split('&').filter(|p| !p.is_empty()) {
        match p {
            "tls=1" | "tls=true" => tls = true,
            "insecure=1" | "insecure=true" => insecure = true,
            "tls=0" | "insecure=0" => {}
            _ => q.push(p),
        }
    }
    let scheme = if tls { "https" } else { "http" };
    let port = port.unwrap_or(if tls { 443 } else { 80 });
    let base = format!("{scheme}://{auth}");
    let rest = if q.is_empty() { path.to_string() } else { format!("{path}?{}", q.join("&")) };
    if super::bad_path(&format!("/{path}")) {
        return err(StatusCode::BAD_REQUEST, "path refused");
    }
    let Some(url) = upstream_url(&base, &rest) else {
        return err(StatusCode::BAD_REQUEST, "path refused: URL-safe characters only, no .., //, @ or scheme");
    };
    let detail = format!("{method} {scheme}://{auth}/{}", path.chars().take(120).collect::<String>());
    if let Err((code, msg)) = admit(gw, &host, port, origin, tool, &detail).await {
        return err(code, msg);
    }
    if h.get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<usize>().ok()).is_some_and(|n| n > MAX_BODY) {
        return err(StatusCode::PAYLOAD_TOO_LARGE, "request body over 64 MB");
    }
    let mut out = reqwest::header::HeaderMap::new();
    out.insert("user-agent", HeaderValue::from_static("gitwebsite-companion/1.2"));
    for n in PASS {
        if let Some(v) = h.get(*n) {
            out.insert(*n, v.clone());
        }
    }
    if let Some(v) = h.get("x-upstream-authorization") {
        out.insert("authorization", v.clone());
    } else if !auth_is_ours {
        if let Some(v) = h.get("authorization") {
            out.insert("authorization", v.clone());
        }
    }
    if let Some(v) = h.get("content-length") {
        out.insert("content-length", v.clone());
    }
    let client = if tls && insecure { &gw.lan_http_insecure } else { &gw.lan_http };
    let mut rb = client.request(method.clone(), &url).headers(out);
    if ![Method::GET, Method::HEAD].contains(&method) {
        rb = rb.body(limited_body(body, MAX_BODY));
    }
    match rb.send().await {
        Ok(resp) => stream_back(resp, "lan", KEEP),
        Err(e) => {
            let msg = format!("{host}:{port}: {}", error_chain(&e));
            err(if e.is_timeout() { StatusCode::GATEWAY_TIMEOUT } else { StatusCode::BAD_GATEWAY }, msg)
        }
    }
}

/// reqwest errors with their causes (a certificate failure is several levels down).
pub fn error_chain(e: &dyn std::error::Error) -> String {
    let mut s = e.to_string();
    let mut cur = e.source();
    while let Some(c) = cur {
        s.push_str(": ");
        s.push_str(&c.to_string());
        cur = c.source();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authority_and_list() {
        assert_eq!(split_authority("TV.local:3001"), Some(("tv.local".into(), Some(3001), "tv.local:3001".into())));
        assert_eq!(split_authority("[fe80::1]:80").unwrap().0, "fe80::1");
        assert!(split_authority("a@b").is_none() && split_authority("x:0").is_none() && split_authority("[nope]").is_none());
        let l = vec!["hue.local".to_string(), "192.168.1.5:8123".to_string(), "localhost:9000".to_string()];
        assert!(listed("hue.local", 443, &l) && listed("192.168.1.5", 8123, &l) && !listed("192.168.1.5", 22, &l));
        assert!(listed("127.0.0.1", 9000, &l) && !listed("evil.local", 80, &l));
    }
}
