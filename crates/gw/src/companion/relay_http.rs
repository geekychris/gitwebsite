//! `/v1/relay/http/<upstream>/<path>` — HTTP relay to a fixed set of upstreams (never an open
//! proxy): public ADS-B and aviation-weather APIs (as aviation-relay.py), the parts suppliers
//! (as parts-relay.py, with secrets injected from the keychain), and user-configured ones (an
//! ADS-B receiver, a Kafka REST proxy, Redpanda's HTTP Proxy and Admin API).

use super::{authorize, bytes_response, err, tagged, Gateway, Origin};
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const UA: &str = "gitwebsite-companion/1 (+https://geekychris.github.io/tools/)";
const CACHE_TTL: Duration = Duration::from_secs(2);
const PARTS_MAX_BODY: usize = 64 * 1024;
const KAFKA_MAX_BODY: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    /// GET only, cached 2 s.
    Aviation,
    /// GET and POST, header allow-list, 64 KB bodies, secrets.
    Parts,
    /// GET, POST, PUT, DELETE (and POST + X-Method-Override: GET).
    Kafka,
    /// GET only (and POST + X-Method-Override: GET).
    Admin,
}

const FIXED: &[(&str, &str, Kind)] = &[
    ("adsblol", "https://api.adsb.lol", Kind::Aviation),
    ("adsbfi", "https://opendata.adsb.fi", Kind::Aviation),
    ("opensky", "https://opensky-network.org", Kind::Aviation),
    ("awc", "https://aviationweather.gov", Kind::Aviation),
    ("digikey", "https://api.digikey.com", Kind::Parts),
    ("mouser", "https://api.mouser.com", Kind::Parts),
    ("nexar", "https://api.nexar.com", Kind::Parts),
    ("nexar-id", "https://identity.nexar.com", Kind::Parts),
];

const PARTS_HEADERS: &[&str] = &[
    "content-type", "accept", "x-digikey-client-id", "x-digikey-locale-site", "x-digikey-locale-language",
    "x-digikey-locale-currency", "x-digikey-locale-shiptocountry", "x-digikey-customer-id",
];
const KEEP_HEADERS: &[&str] = &["x-rate-limit-remaining", "x-ratelimit-remaining", "x-ratelimit-limit", "x-burstlimit-remaining", "retry-after", "server"];

pub fn upstream_ids() -> Vec<&'static str> {
    let mut v: Vec<&str> = FIXED.iter().map(|f| f.0).collect();
    v.extend(["receiver", "kafka-rest", "redpanda-proxy", "redpanda-admin"]);
    v
}

/// Base URL and kind of an upstream id (user-configured ones only when set).
fn upstream(gw: &Gateway, id: &str) -> Option<(String, Kind)> {
    let s = gw.settings();
    let (base, kind) = match id {
        "receiver" => (s.receiver, Kind::Aviation),
        "kafka-rest" => (s.kafka_rest, Kind::Kafka),
        "redpanda-proxy" => (s.redpanda_proxy, Kind::Kafka),
        "redpanda-admin" => (s.redpanda_admin, Kind::Admin),
        _ => FIXED.iter().find(|f| f.0 == id).map(|f| (f.1.to_string(), f.2))?,
    };
    if base.is_empty() {
        return None;
    }
    let base = gw
        .upstream_override
        .read()
        .unwrap()
        .get(id)
        .cloned()
        .or_else(|| gw.settings_overrides.then(|| gw.settings.read().unwrap().upstream_override.get(id).cloned()).flatten())
        .unwrap_or(base);
    Some((base, kind))
}

/// The upstream URL for `rest` (path and query after the upstream id), or None when unsafe:
/// only URL-safe characters, no `..`, `.`, `//`, `@`, `\`, no scheme, and the host must stay
/// the upstream's.
pub fn upstream_url(base: &str, rest: &str) -> Option<String> {
    let ok = |c: char| c.is_ascii_alphanumeric() || "._~!$&'()*+,;=:@%/?-".contains(c);
    if !rest.chars().all(ok) {
        return None;
    }
    let path = rest.split('?').next().unwrap_or("");
    if path.starts_with('/') || path.contains("//") || path.contains('@') {
        return None;
    }
    let decoded = urlencoding::decode(rest).ok()?;
    if decoded.contains("://") || decoded.contains('\\') {
        return None;
    }
    let dpath = urlencoding::decode(path).ok()?;
    if dpath.split('/').any(|s| s == ".." || s == ".") || dpath.contains("//") {
        return None;
    }
    let url = format!("{}/{}", base.trim_end_matches('/'), rest);
    let (a, b) = (reqwest::Url::parse(&url).ok()?, reqwest::Url::parse(base).ok()?);
    (a.host_str() == b.host_str() && a.port_or_known_default() == b.port_or_known_default() && a.scheme() == b.scheme()).then_some(url)
}

/// A configured base should be scheme://host[:port] only, but if it carries a path (e.g.
/// `http://piaware.local:8080/tar1090`) and the page's path already starts with that same path,
/// strip it so it is not doubled (`…/tar1090/tar1090/data/aircraft.json`).
pub fn dedupe_base_path<'a>(base: &str, rest: &'a str) -> &'a str {
    let Ok(u) = reqwest::Url::parse(base) else { return rest };
    let bp = u.path().trim_matches('/');
    if bp.is_empty() {
        return rest;
    }
    match rest.strip_prefix(bp) {
        Some("") => "",
        Some(r) if r.starts_with('/') => &r[1..],
        Some(r) if r.starts_with('?') => r,
        _ => rest,
    }
}

#[derive(Clone)]
struct Cached {
    status: u16,
    ctype: String,
    keep: Vec<(String, String)>,
    body: Bytes,
}

#[derive(Default)]
pub struct Cache(Mutex<HashMap<String, (Instant, Cached)>>);

pub async fn handle(
    State(gw): State<Arc<Gateway>>,
    axum::Extension(Origin(origin)): axum::Extension<Origin>,
    method: Method,
    uri: Uri,
    h: HeaderMap,
    body: Bytes,
) -> Response {
    if !gw.settings().relays.http {
        return err(StatusCode::SERVICE_UNAVAILABLE, "the HTTP relay is turned off in the companion settings");
    }
    let (rec, presented) = match authorize(&gw, &h, &origin, "relay.http") {
        Ok(x) => x,
        Err(r) => return r,
    };
    let r = relay(&gw, method, &uri, &h, body, presented.via_authorization).await;
    tagged(r, &rec)
}

async fn relay(gw: &Gateway, method: Method, uri: &Uri, h: &HeaderMap, body: Bytes, auth_is_ours: bool) -> Response {
    let pq = uri.path_and_query().map(|p| p.as_str()).unwrap_or("");
    let after = pq.strip_prefix("/v1/relay/http/").unwrap_or("");
    let (id, rest) = after.split_once('/').unwrap_or((after.split('?').next().unwrap_or(""), ""));
    let Some((base, kind)) = upstream(gw, id) else {
        let configurable = ["receiver", "kafka-rest", "redpanda-proxy", "redpanda-admin"].contains(&id);
        return if configurable {
            err(StatusCode::SERVICE_UNAVAILABLE, format!("{id} is not configured: set it in the app's Companion settings"))
        } else {
            (StatusCode::NOT_FOUND, axum::Json(json!({"error": format!("unknown upstream {id:?}"), "upstreams": upstream_ids()}))).into_response()
        };
    };
    let rest = dedupe_base_path(&base, rest);
    let Some(mut url) = upstream_url(&base, rest) else {
        return err(StatusCode::BAD_REQUEST, "path refused: this is not an open proxy");
    };
    // Methods.
    let override_get = h.get("x-method-override").and_then(|v| v.to_str().ok()).map(|v| v.eq_ignore_ascii_case("GET")).unwrap_or(false);
    let send_method = match (kind, &method) {
        (_, &Method::GET) => Method::GET,
        (Kind::Kafka | Kind::Admin, &Method::POST) if override_get => Method::GET,
        (Kind::Parts, &Method::POST) => Method::POST,
        (Kind::Kafka, m) if [Method::POST, Method::PUT, Method::DELETE].contains(m) => m.clone(),
        _ => {
            let allowed = match kind {
                Kind::Aviation => "GET",
                Kind::Parts => "GET, POST",
                Kind::Kafka => "GET, POST, PUT, DELETE",
                Kind::Admin => "GET",
            };
            let mut r = err(StatusCode::METHOD_NOT_ALLOWED, format!("{method} refused for {id}: {allowed} only"));
            r.headers_mut().insert("allow", HeaderValue::from_static(allowed));
            return r;
        }
    };
    let limit = if kind == Kind::Parts { PARTS_MAX_BODY } else { KAFKA_MAX_BODY };
    if body.len() > limit {
        return err(StatusCode::PAYLOAD_TOO_LARGE, "request body too large");
    }
    // Aviation GETs are cached briefly, so several tabs don't multiply the load.
    if kind == Kind::Aviation {
        if let Some((at, c)) = gw.cache.0.lock().unwrap().get(&url).cloned() {
            if at.elapsed() < CACHE_TTL {
                return cached_response(id, &c, "hit");
            }
        }
    }
    // Headers.
    let mut out = reqwest::header::HeaderMap::new();
    out.insert("user-agent", HeaderValue::from_static(UA));
    let pass = |name: &str| h.get(name).cloned();
    match kind {
        Kind::Aviation => {
            out.insert("accept", HeaderValue::from_static("*/*"));
        }
        Kind::Parts => {
            for n in PARTS_HEADERS {
                if let Some(v) = pass(n) {
                    out.insert(*n, v);
                }
            }
            if !auth_is_ours {
                if let Some(v) = pass("authorization") {
                    out.insert("authorization", v);
                }
            }
        }
        Kind::Kafka | Kind::Admin => {
            for n in ["content-type", "accept"] {
                if let Some(v) = pass(n) {
                    out.insert(n, v);
                }
            }
            if !auth_is_ours {
                if let Some(v) = pass("authorization") {
                    out.insert("authorization", v);
                }
            }
        }
    }
    // Secrets held by the companion.
    if kind == Kind::Parts {
        if let Err(e) = sign(gw, id, rest, &mut url, &mut out).await {
            return err(StatusCode::BAD_GATEWAY, format!("could not sign in with the stored secrets: {e:#}"));
        }
    }
    let mut rb = gw.http.request(send_method.clone(), &url).headers(out).timeout(Duration::from_secs(if kind == Kind::Kafka { 60 } else { 25 }));
    if send_method != Method::GET && !body.is_empty() {
        rb = rb.body(body);
    } else if send_method == Method::GET && kind != Kind::Aviation && !body.is_empty() {
        // Redpanda's committed-offsets call is a GET with a body (sent as POST + X-Method-Override).
        rb = rb.body(body);
    }
    let resp = match rb.send().await {
        Ok(r) => r,
        Err(e) => return err(StatusCode::BAD_GATEWAY, format!("upstream {id}: {e}")),
    };
    let status = resp.status().as_u16();
    let ctype = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("application/octet-stream").to_string();
    let keep: Vec<(String, String)> = KEEP_HEADERS
        .iter()
        .filter_map(|k| resp.headers().get(*k).and_then(|v| v.to_str().ok()).map(|v| (k.to_string(), v.to_string())))
        .collect();
    let body = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_GATEWAY, format!("upstream {id}: {e}")),
    };
    let c = Cached { status, ctype, keep, body };
    if kind == Kind::Aviation {
        let mut m = gw.cache.0.lock().unwrap();
        m.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(60));
        m.insert(url, (Instant::now(), c.clone()));
    }
    cached_response(id, &c, "miss")
}

fn cached_response(id: &str, c: &Cached, cache: &str) -> Response {
    let mut r = bytes_response(StatusCode::from_u16(c.status).unwrap_or(StatusCode::BAD_GATEWAY), &c.ctype, c.body.clone());
    let hd = r.headers_mut();
    hd.insert("x-relay-upstream", HeaderValue::from_str(id).unwrap_or(HeaderValue::from_static("?")));
    hd.insert("x-relay-cache", HeaderValue::from_str(cache).unwrap_or(HeaderValue::from_static("?")));
    for (k, v) in &c.keep {
        if let (Ok(k), Ok(v)) = (axum::http::HeaderName::from_bytes(k.as_bytes()), HeaderValue::from_str(v)) {
            hd.insert(k, v);
        }
    }
    r
}

/// Inject the companion's own supplier credentials (as parts-relay.py does).
async fn sign(gw: &Gateway, id: &str, rest: &str, url: &mut String, out: &mut reqwest::header::HeaderMap) -> anyhow::Result<()> {
    let s = &gw.secrets;
    match id {
        "digikey" if s.holds("digikey") && !rest.starts_with("v1/oauth2/") => {
            let tok = supplier_token(gw, "digikey").await?;
            out.remove("authorization");
            out.remove("x-digikey-client-id");
            out.insert("authorization", HeaderValue::from_str(&format!("Bearer {tok}"))?);
            out.insert("x-digikey-client-id", HeaderValue::from_str(&s.get("digikey_client_id").unwrap_or_default())?);
        }
        "mouser" if s.holds("mouser") && !url.to_ascii_lowercase().contains("apikey=") => {
            let k = s.get("mouser_api_key").unwrap_or_default();
            url.push(if url.contains('?') { '&' } else { '?' });
            url.push_str(&format!("apiKey={}", urlencoding::encode(&k)));
        }
        "nexar" if s.holds("nexar") && !out.contains_key("authorization") => {
            let tok = supplier_token(gw, "nexar").await?;
            out.insert("authorization", HeaderValue::from_str(&format!("Bearer {tok}"))?);
        }
        _ => {}
    }
    Ok(())
}

/// An OAuth client-credentials token for digikey or nexar, cached until a minute before expiry.
async fn supplier_token(gw: &Gateway, which: &str) -> anyhow::Result<String> {
    if let Some((t, exp)) = gw.supplier_tokens.lock().unwrap().get(which).cloned() {
        if Instant::now() + Duration::from_secs(60) < exp {
            return Ok(t);
        }
    }
    let s = &gw.secrets;
    let (url, id, secret, extra) = if which == "digikey" {
        let base = upstream(gw, "digikey").map(|u| u.0).unwrap_or_default();
        (format!("{base}/v1/oauth2/token"), s.get("digikey_client_id"), s.get("digikey_client_secret"), "")
    } else {
        let base = upstream(gw, "nexar-id").map(|u| u.0).unwrap_or_default();
        (format!("{base}/connect/token"), s.get("nexar_client_id"), s.get("nexar_client_secret"), "&scope=supply.domain")
    };
    let form = format!(
        "client_id={}&client_secret={}&grant_type=client_credentials{extra}",
        urlencoding::encode(&id.unwrap_or_default()),
        urlencoding::encode(&secret.unwrap_or_default())
    );
    let r = gw
        .http
        .post(&url)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("accept", "application/json")
        .header("user-agent", UA)
        .body(form)
        .timeout(Duration::from_secs(20))
        .send()
        .await?;
    let j: serde_json::Value = r.json().await?;
    let tok = j["access_token"].as_str().ok_or_else(|| anyhow::anyhow!("{which}: no access token in the response"))?.to_string();
    let ttl = j["expires_in"].as_f64().unwrap_or(599.0);
    gw.supplier_tokens.lock().unwrap().insert(which.into(), (tok.clone(), Instant::now() + Duration::from_secs_f64(ttl)));
    Ok(tok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_path_not_doubled() {
        assert_eq!(dedupe_base_path("http://pi:8080", "data/aircraft.json"), "data/aircraft.json");
        assert_eq!(dedupe_base_path("http://pi:8080/", "data/aircraft.json"), "data/aircraft.json");
        assert_eq!(dedupe_base_path("http://pi:8080/tar1090", "tar1090/data/aircraft.json"), "data/aircraft.json");
        assert_eq!(dedupe_base_path("http://pi:8080/tar1090/", "tar1090/data/aircraft.json"), "data/aircraft.json");
        assert_eq!(dedupe_base_path("http://pi:8080/tar1090", "data/aircraft.json"), "data/aircraft.json");
        assert_eq!(dedupe_base_path("http://pi:8080/tar1090", "tar1090x/a"), "tar1090x/a");
        assert_eq!(dedupe_base_path("http://pi:8080/a/b", "a/b/c?x=1"), "c?x=1");
        assert_eq!(upstream_url("http://pi:8080/tar1090", dedupe_base_path("http://pi:8080/tar1090", "tar1090/data/aircraft.json")).as_deref(),
            Some("http://pi:8080/tar1090/data/aircraft.json"));
    }

    #[test]
    fn not_an_open_proxy() {
        let b = "https://api.adsb.lol";
        assert_eq!(upstream_url(b, "v2/lat/1/lon/2/dist/5").as_deref(), Some("https://api.adsb.lol/v2/lat/1/lon/2/dist/5"));
        assert_eq!(upstream_url(b, "v2/x?a=1&b=2").as_deref(), Some("https://api.adsb.lol/v2/x?a=1&b=2"));
        for bad in [
            "/evil.test/x", "//evil.test/x", "@evil.test/x", "..%2f..%2fx", "a/../b", "a/./b", "a/%2e%2e/b",
            "x?u=https://evil.test", "a//b", "a\\b", "a b", "x#frag",
        ] {
            assert!(upstream_url(b, bad).is_none(), "{bad}");
        }
        // The host can't change through userinfo or a port.
        assert!(upstream_url("http://127.0.0.1:8080", ":9999/x").is_none() || upstream_url("http://127.0.0.1:8080", ":9999/x").unwrap().starts_with("http://127.0.0.1:8080/"));
    }
}
