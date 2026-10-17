//! `/v1/relay/s3/<profile>/<bucket>/<key…>` — S3-compatible storage (AWS S3, MinIO, Ceph,
//! Garage, R2, B2, …), signed server-side with SigV4 from a named profile (endpoint, region,
//! path-style; the access key id and secret live in the secret store and never reach a page).
//!
//! | Request | S3 operation |
//! |---|---|
//! | `GET /v1/relay/s3` | the profiles (names, endpoints, whether credentials are set — never secrets) |
//! | `GET /v1/relay/s3/<p>` | ListBuckets |
//! | `GET /v1/relay/s3/<p>/<bucket>[?prefix=&delimiter=&continuation-token=&start-after=&max-keys=]` | ListObjectsV2 |
//! | `GET /v1/relay/s3/<p>/<bucket>/<key>` (Range passed) | GetObject |
//! | `HEAD …/<bucket>/<key>` | HeadObject |
//! | `PUT …/<bucket>/<key>` (Content-Length required, ≤ 100 MB, streamed) | PutObject |
//! | `DELETE …/<bucket>/<key>` (the app asks first, unless `s3_confirm_delete = false`) | DeleteObject |
//! | `POST /v1/relay/s3/<p>/presign` `{method: GET\|PUT\|HEAD, bucket, key, expires}` | a presigned URL |

use super::{authorize, err, lan, limited_body, sigv4, stream_back, tagged, Gateway, Origin};
use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub const MAX_PUT: usize = 100 * 1024 * 1024;
const LIST_PARAMS: &[&str] = &["prefix", "delimiter", "continuation-token", "start-after", "max-keys", "fetch-owner", "encoding-type", "list-type"];
const OBJECT_GET_PARAMS: &[&str] = &["versionId", "partNumber", "response-content-type", "response-content-disposition", "response-cache-control"];
const GET_HEADERS: &[&str] = &["range", "if-match", "if-none-match", "if-modified-since", "if-unmodified-since"];
const PUT_HEADERS: &[&str] = &["content-type", "content-md5", "cache-control", "content-disposition", "content-encoding", "content-language", "expires"];
const KEEP: &[&str] = &[
    "content-type", "content-length", "content-range", "accept-ranges", "etag", "last-modified", "content-disposition", "content-encoding",
    "x-amz-request-id", "x-amz-version-id", "x-amz-delete-marker", "x-amz-storage-class",
];

/// The profiles, for `GET /v1/relay/s3` and the status page (no secrets).
pub fn profiles(gw: &Gateway) -> Vec<Value> {
    gw.settings()
        .s3_profiles
        .iter()
        .map(|(n, p)| json!({"name": n, "endpoint": p.endpoint, "region": p.region, "path_style": p.path_style, "insecure": p.insecure,
            "credentials": gw.secrets.holds(&format!("s3:{n}"))}))
        .collect()
}

pub fn bucket_ok(b: &str) -> bool {
    (1..=255).contains(&b.len()) && b.bytes().all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c)) && !b.starts_with('.') && b != ".."
}

/// A decoded object key: no `.`/`..` segments (URLs would collapse them), no control characters.
pub fn key_ok(k: &str) -> bool {
    !k.is_empty() && k.len() <= 1024 && !k.split('/').any(|s| s == "." || s == "..") && !k.chars().any(|c| c.is_control())
}

pub struct Target {
    pub url: String,
    pub host: String,
    pub path: String,
}

/// Where a request for `bucket`/`key` goes: path-style `endpoint/bucket/key`, else
/// `bucket.endpoint/key`. `path` is the canonical (encoded) path.
pub fn target(endpoint: &str, path_style: bool, bucket: Option<&str>, key: Option<&str>) -> Option<Target> {
    let u = reqwest::Url::parse(endpoint).ok()?;
    let h = u.host_str()?.to_string();
    let enc_key = key.map(|k| sigv4::uri_encode(k, true)).unwrap_or_default();
    let (host, path) = match bucket {
        None => (h, "/".to_string()),
        Some(b) if path_style || lan::literal(&h).is_some() => {
            let p = match key {
                Some(_) => format!("/{}/{enc_key}", sigv4::uri_encode(b, false)),
                None => format!("/{}", sigv4::uri_encode(b, false)),
            };
            (h, p)
        }
        Some(b) => (format!("{b}.{h}"), format!("/{enc_key}")),
    };
    let authority = match u.port() {
        Some(p) => format!("{host}:{p}"),
        None => host.clone(),
    };
    Some(Target { url: format!("{}://{authority}{path}", u.scheme()), host: authority, path })
}

#[derive(Deserialize)]
struct PresignBody {
    #[serde(default)]
    method: String,
    bucket: String,
    key: String,
    #[serde(default)]
    expires: Option<u64>,
}

pub async fn handle(
    State(gw): State<Arc<Gateway>>,
    axum::Extension(Origin(origin)): axum::Extension<Origin>,
    method: Method,
    uri: Uri,
    h: HeaderMap,
    body: Body,
) -> Response {
    if !gw.settings().relays.s3 {
        return err(StatusCode::SERVICE_UNAVAILABLE, "the S3 relay is turned off in the companion settings");
    }
    let (rec, _) = match authorize(&gw, &h, &origin, "relay.s3") {
        Ok(x) => x,
        Err(r) => return r,
    };
    let r = relay(&gw, &origin, &rec.tool, method, &uri, &h, body).await;
    tagged(r, &rec)
}

#[allow(clippy::too_many_arguments)]
async fn relay(gw: &Gateway, origin: &str, tool: &str, method: Method, uri: &Uri, h: &HeaderMap, body: Body) -> Response {
    let after = uri.path().strip_prefix("/v1/relay/s3").unwrap_or("").trim_start_matches('/');
    if after.is_empty() {
        return if method == Method::GET { Json(json!({ "profiles": profiles(gw) })).into_response() } else { err(StatusCode::METHOD_NOT_ALLOWED, "GET only") };
    }
    let mut parts = after.splitn(3, '/');
    let pname = parts.next().unwrap_or("");
    let bucket_raw = parts.next().filter(|b| !b.is_empty());
    let key_raw = parts.next().filter(|k| !k.is_empty());
    let dec = |s: &str| urlencoding::decode(s).map(|c| c.into_owned()).ok();
    let Some(pname) = dec(pname) else { return err(StatusCode::BAD_REQUEST, "bad profile name") };
    let s = gw.settings();
    let Some(profile) = s.s3_profiles.get(&pname).cloned() else {
        return (StatusCode::NOT_FOUND, Json(json!({"error": format!("no S3 profile {pname:?} (add one in the app's Companion settings or with `gw companion s3 add`)"), "profiles": s.s3_profiles.keys().collect::<Vec<_>>()}))).into_response();
    };
    let creds = match (gw.secrets.get(&format!("s3:{pname}:access_key_id")), gw.secrets.get(&format!("s3:{pname}:secret_access_key"))) {
        (Some(a), Some(k)) => sigv4::Creds { access_key: a, secret_key: k },
        _ => return err(StatusCode::SERVICE_UNAVAILABLE, format!("S3 profile {pname} has no credentials: `gw companion secret set s3:{pname}` or the Companion page")),
    };
    // LAN endpoints go through the LAN client (checked addresses; self-signed allowed with
    // `insecure`); others through the public one.
    let ep = reqwest::Url::parse(&profile.endpoint).ok();
    let ep_host = ep.as_ref().and_then(|u| u.host_str()).unwrap_or("").to_string();
    let ep_port = ep.as_ref().and_then(|u| u.port_or_known_default()).unwrap_or(443);
    let on_lan = match lan::resolve(&ep_host, &gw.dns).await {
        Ok(a) => {
            if lan::own_port_refused(&a, ep_port, &gw.own_ports()) {
                return err(StatusCode::FORBIDDEN, "the profile's endpoint is the companion's or the app's own port");
            }
            true
        }
        Err(_) => false,
    };
    if profile.insecure && !on_lan {
        return err(StatusCode::FORBIDDEN, format!("S3 profile {pname}: insecure TLS is only allowed for LAN endpoints"));
    }
    let client = match (on_lan, profile.insecure) {
        (true, true) => &gw.lan_http_insecure,
        (true, false) => &gw.lan_http,
        _ => &gw.http,
    };

    // Presign.
    if method == Method::POST && bucket_raw == Some("presign") && key_raw.is_none() {
        let b = match axum::body::to_bytes(body, 64 * 1024).await.ok().and_then(|b| serde_json::from_slice::<PresignBody>(&b).ok()) {
            Some(b) => b,
            None => return err(StatusCode::BAD_REQUEST, "body: {\"method\":\"GET|PUT|HEAD\", \"bucket\":…, \"key\":…, \"expires\": seconds}"),
        };
        let m = if b.method.is_empty() { "GET".to_string() } else { b.method.to_ascii_uppercase() };
        if !["GET", "PUT", "HEAD"].contains(&m.as_str()) {
            return err(StatusCode::BAD_REQUEST, "presign: GET, PUT or HEAD only (DELETE needs the app's confirmation)");
        }
        if !bucket_ok(&b.bucket) || !key_ok(&b.key) {
            return err(StatusCode::BAD_REQUEST, "bad bucket or key");
        }
        let expires = b.expires.unwrap_or(900).clamp(1, 604_800);
        let Some(t) = target(&profile.endpoint, profile.path_style, Some(&b.bucket), Some(&b.key)) else { return err(StatusCode::BAD_REQUEST, "bad endpoint") };
        let r = sigv4::Request { method: &m, path: &t.path, query: vec![], headers: vec![("host".into(), t.host.clone())], payload_hash: sigv4::UNSIGNED,
            region: &profile.region, service: "s3", time: chrono::Utc::now() };
        let q = sigv4::presign_query(&r, &creds, expires);
        return Json(json!({ "url": format!("{}?{q}", t.url), "method": m, "expires": expires })).into_response();
    }

    let bucket = match bucket_raw.map(dec) {
        None => None,
        Some(Some(b)) if bucket_ok(&b) => Some(b),
        _ => return err(StatusCode::BAD_REQUEST, "bad bucket name"),
    };
    let key = match key_raw.map(dec) {
        None => None,
        Some(Some(k)) if key_ok(&k) => Some(k),
        _ => return err(StatusCode::BAD_REQUEST, "bad object key (no . or .. segments, no control characters)"),
    };
    let page_q = sigv4::parse_query(uri.query().unwrap_or(""));
    let allowed = |names: &[&str]| page_q.iter().all(|(k, _)| names.contains(&k.as_str()));
    let (op, mut query): (&str, Vec<(String, String)>) = match (&method, &bucket, &key) {
        (&Method::GET, None, None) => ("ListBuckets", vec![]),
        (&Method::GET, Some(_), None) => {
            if !allowed(LIST_PARAMS) {
                return err(StatusCode::BAD_REQUEST, format!("ListObjectsV2 parameters: {}", LIST_PARAMS.join(", ")));
            }
            let mut q: Vec<_> = page_q.iter().filter(|(k, _)| k != "list-type").cloned().collect();
            q.push(("list-type".into(), "2".into()));
            ("ListObjectsV2", q)
        }
        (&Method::GET, Some(_), Some(_)) | (&Method::HEAD, Some(_), Some(_)) => {
            if !allowed(OBJECT_GET_PARAMS) {
                return err(StatusCode::BAD_REQUEST, format!("GetObject parameters: {}", OBJECT_GET_PARAMS.join(", ")));
            }
            (if method == Method::GET { "GetObject" } else { "HeadObject" }, page_q.clone())
        }
        (&Method::PUT, Some(_), Some(_)) if page_q.is_empty() => ("PutObject", vec![]),
        (&Method::DELETE, Some(_), Some(_)) if allowed(&["versionId"]) => ("DeleteObject", page_q.clone()),
        _ => {
            return err(StatusCode::METHOD_NOT_ALLOWED, "supported: GET (ListBuckets, ListObjectsV2, GetObject), HEAD, PUT (PutObject), DELETE (DeleteObject), POST <profile>/presign");
        }
    };
    query.sort();
    let Some(t) = target(&profile.endpoint, profile.path_style, bucket.as_deref(), key.as_deref()) else { return err(StatusCode::BAD_REQUEST, "bad endpoint") };
    let content_length = h.get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<usize>().ok());
    if op == "PutObject" {
        match content_length {
            None => return err(StatusCode::LENGTH_REQUIRED, "PutObject needs a Content-Length (send a Blob or ArrayBuffer)"),
            Some(n) if n > MAX_PUT => return err(StatusCode::PAYLOAD_TOO_LARGE, "PutObject through the relay is limited to 100 MB (use a presigned PUT for larger objects)"),
            _ => {}
        }
    }
    if op == "DeleteObject" && s.s3_confirm_delete {
        let what = format!("{pname}/{}/{}", bucket.as_deref().unwrap_or(""), key.as_deref().unwrap_or(""));
        if let Err((code, msg)) = gw.ask_approval("s3-delete", &what, origin, tool, &format!("DeleteObject s3://{what}")).await {
            return err(code, msg);
        }
    }
    // Sign.
    let now = chrono::Utc::now();
    let payload = if op == "PutObject" { sigv4::UNSIGNED } else { sigv4::EMPTY_SHA256 };
    let mut signed: Vec<(String, String)> = vec![
        ("host".into(), t.host.clone()),
        ("x-amz-date".into(), sigv4::amz_date(&now)),
        ("x-amz-content-sha256".into(), payload.into()),
    ];
    let mut unsigned: Vec<(String, HeaderValue)> = vec![];
    let pass: &[&str] = if op == "PutObject" { PUT_HEADERS } else if op == "GetObject" || op == "HeadObject" { GET_HEADERS } else { &[] };
    for n in pass {
        if let Some(v) = h.get(*n) {
            if matches!(*n, "content-type" | "content-md5") {
                signed.push((n.to_string(), v.to_str().unwrap_or("").to_string()));
            } else {
                unsigned.push((n.to_string(), v.clone()));
            }
        }
    }
    if op == "PutObject" {
        for (k, v) in h.iter() {
            if k.as_str().starts_with("x-amz-meta-") {
                if let Ok(v) = v.to_str() {
                    signed.push((k.as_str().to_string(), v.to_string()));
                }
            }
        }
    }
    let req = sigv4::Request { method: method.as_str(), path: &t.path, query: query.clone(), headers: signed.clone(), payload_hash: payload, region: &profile.region, service: "s3", time: now };
    let auth = sigv4::authorization(&req, &creds);
    let cq = sigv4::canonical_query(&query);
    let url = if cq.is_empty() { t.url.clone() } else { format!("{}?{cq}", t.url) };
    let mut out = reqwest::header::HeaderMap::new();
    for (k, v) in &signed {
        if k == "host" {
            continue;
        }
        if let (Ok(n), Ok(v)) = (reqwest::header::HeaderName::from_bytes(k.as_bytes()), HeaderValue::from_str(v)) {
            out.insert(n, v);
        }
    }
    for (k, v) in unsigned {
        if let Ok(n) = reqwest::header::HeaderName::from_bytes(k.as_bytes()) {
            out.insert(n, v);
        }
    }
    out.insert("authorization", HeaderValue::from_str(&auth).unwrap_or(HeaderValue::from_static("")));
    out.insert("user-agent", HeaderValue::from_static("gitwebsite-companion/1.2"));
    let mut rb = client.request(method.clone(), &url).headers(out);
    if op == "PutObject" {
        let n = content_length.unwrap_or(0);
        rb = rb.header("content-length", n.to_string()).body(limited_body(body, n.min(MAX_PUT)));
    }
    match rb.send().await {
        Ok(resp) => {
            let meta: Vec<(String, HeaderValue)> = resp
                .headers()
                .iter()
                .filter(|(k, _)| k.as_str().starts_with("x-amz-meta-"))
                .filter_map(|(k, v)| HeaderValue::from_bytes(v.as_bytes()).ok().map(|v| (k.as_str().to_string(), v)))
                .collect();
            let mut r = stream_back(resp, "s3", KEEP);
            for (k, v) in meta {
                if let Ok(n) = axum::http::HeaderName::from_bytes(k.as_bytes()) {
                    r.headers_mut().insert(n, v);
                }
            }
            r.headers_mut().insert("x-relay-s3-op", HeaderValue::from_static(op));
            r
        }
        Err(e) => err(StatusCode::BAD_GATEWAY, format!("S3 {pname}: {}", super::relay_lan::error_chain(&e))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn targets() {
        let t = target("http://nas.local:9000", true, Some("photos"), Some("2024/a b.jpg")).unwrap();
        assert_eq!((t.url.as_str(), t.host.as_str(), t.path.as_str()), ("http://nas.local:9000/photos/2024/a%20b.jpg", "nas.local:9000", "/photos/2024/a%20b.jpg"));
        let t = target("https://s3.us-east-1.amazonaws.com", false, Some("b"), Some("k")).unwrap();
        assert_eq!((t.url.as_str(), t.host.as_str()), ("https://b.s3.us-east-1.amazonaws.com/k", "b.s3.us-east-1.amazonaws.com"));
        let t = target("http://192.168.1.5:9000", false, Some("b"), None).unwrap();
        assert_eq!(t.url, "http://192.168.1.5:9000/b");
        assert!(key_ok("a/b.txt") && !key_ok("a/../b") && !key_ok("./x") && !key_ok(""));
        assert!(bucket_ok("my-bucket.1") && !bucket_ok("a/b") && !bucket_ok(""));
    }
}
