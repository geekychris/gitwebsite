//! `/v1/relay/git/<host>/<repo path>/(info/refs|git-upload-pack|git-receive-pack)` — a CORS
//! proxy for isomorphic-git's smart-HTTP clone and fetch (as git-relay.py). Only allow-listed
//! hosts (github.com, gitlab.com, codeberg.org, bitbucket.org, plus user-added), always https
//! upstream; only `GET info/refs?service=git-upload-pack` and `POST git-upload-pack`; push
//! (`git-receive-pack`) is refused unless `git_allow_push` is on. The page's own Authorization
//! (a token for a private repository) is passed to the git host when the pairing token comes in
//! `X-Companion-Token`; the companion never stores it.

use super::{authorize, err, settings::Settings, tagged, Gateway, Origin};
use axum::{
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri},
    response::Response,
};
use futures_util::TryStreamExt;
use regex::Regex;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

pub fn hosts(s: &Settings) -> Vec<String> {
    let mut v: Vec<String> = super::settings::DEFAULT_GIT_HOSTS.iter().map(|h| h.to_string()).collect();
    v.extend(s.git_hosts.iter().map(|h| h.to_ascii_lowercase()));
    v
}

/// The upstream URL for a relay path, or the reason it is refused.
pub fn target(s: &Settings, method: &Method, path: &str, query: &str) -> Result<String, (StatusCode, String)> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"^/([A-Za-z0-9.-]+(?::\d+)?)(/[A-Za-z0-9._~%/+-]+?)/(info/refs|git-upload-pack|git-receive-pack)$").unwrap());
    let Some(m) = re.captures(path) else {
        return Err((StatusCode::FORBIDDEN, "only git smart-HTTP fetch requests are relayed".into()));
    };
    let host = m[1].to_ascii_lowercase();
    let (repo, svc) = (&m[2], &m[3]);
    let plain_http = s.git_http_hosts.iter().any(|h| h.eq_ignore_ascii_case(&host));
    if !hosts(s).contains(&host) && !plain_http {
        return Err((StatusCode::FORBIDDEN, format!("host {host} is not allowed (add it in the app's Companion settings)")));
    }
    if repo.contains("..") || repo.contains("//") || repo.to_ascii_lowercase().contains("%2f") || repo.to_ascii_lowercase().contains("%2e") {
        return Err((StatusCode::FORBIDDEN, "bad path".into()));
    }
    let push = svc == "git-receive-pack" || (svc == "info/refs" && query.contains("service=git-receive-pack"));
    if push && !s.git_allow_push {
        return Err((StatusCode::FORBIDDEN, "push (git-receive-pack) is turned off; enable it in the app's Companion settings".into()));
    }
    if svc == "info/refs" {
        if *method != Method::GET || !(query == "service=git-upload-pack" || query == "service=git-receive-pack") {
            return Err((StatusCode::FORBIDDEN, "info/refs needs GET ?service=git-upload-pack".into()));
        }
    } else if *method != Method::POST {
        return Err((StatusCode::FORBIDDEN, format!("{svc} needs POST")));
    }
    let scheme = if plain_http { "http" } else { "https" };
    Ok(format!("{scheme}://{host}{repo}/{svc}{}", if query.is_empty() { String::new() } else { format!("?{query}") }))
}

pub async fn handle(
    State(gw): State<Arc<Gateway>>,
    axum::Extension(Origin(origin)): axum::Extension<Origin>,
    method: Method,
    uri: Uri,
    h: HeaderMap,
    body: Bytes,
) -> Response {
    let s = gw.settings();
    if !s.relays.git {
        return err(StatusCode::SERVICE_UNAVAILABLE, "the git relay is turned off in the companion settings");
    }
    let (rec, presented) = match authorize(&gw, &h, &origin, "relay.git") {
        Ok(x) => x,
        Err(r) => return r,
    };
    let path = uri.path().strip_prefix("/v1/relay/git").unwrap_or("");
    let url = match target(&s, &method, path, uri.query().unwrap_or("")) {
        Ok(u) => u,
        Err((code, msg)) => return tagged(err(code, msg), &rec),
    };
    let mut rb = gw.http.request(method.clone(), &url).header("user-agent", "git/2.45 gitwebsite-companion/1").timeout(Duration::from_secs(300));
    for k in ["content-type", "accept", "git-protocol"] {
        if let Some(v) = h.get(k) {
            rb = rb.header(k, v.clone());
        }
    }
    if !presented.via_authorization {
        if let Some(v) = h.get("authorization") {
            rb = rb.header("authorization", v.clone());
        }
    }
    if method == Method::POST {
        rb = rb.body(body);
    }
    let resp = match rb.send().await {
        Ok(r) => r,
        Err(e) => return tagged(err(StatusCode::BAD_GATEWAY, format!("upstream error: {e}")), &rec),
    };
    let status = resp.status();
    let ctype = resp.headers().get("content-type").cloned().unwrap_or(HeaderValue::from_static("application/octet-stream"));
    let host = url.split('/').nth(2).unwrap_or("").to_string();
    let mut out = Response::new(Body::from_stream(resp.bytes_stream().map_err(std::io::Error::other)));
    *out.status_mut() = status;
    let hd = out.headers_mut();
    hd.insert("content-type", ctype);
    hd.insert("cache-control", HeaderValue::from_static("no-store"));
    if let Ok(v) = HeaderValue::from_str(&host) {
        hd.insert("x-relay-upstream", v);
    }
    tagged(out, &rec)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fetch_only() {
        let s = Settings::default();
        let g = Method::GET;
        let p = Method::POST;
        assert_eq!(
            target(&s, &g, "/github.com/o/r.git/info/refs", "service=git-upload-pack").unwrap(),
            "https://github.com/o/r.git/info/refs?service=git-upload-pack"
        );
        assert!(target(&s, &p, "/github.com/o/r.git/git-upload-pack", "").is_ok());
        // push refused
        assert!(target(&s, &p, "/github.com/o/r.git/git-receive-pack", "").is_err());
        assert!(target(&s, &g, "/github.com/o/r.git/info/refs", "service=git-receive-pack").is_err());
        // not an open proxy
        assert!(target(&s, &g, "/evil.test/o/r.git/info/refs", "service=git-upload-pack").is_err());
        assert!(target(&s, &g, "/github.com/o/../../r/info/refs", "service=git-upload-pack").is_err());
        assert!(target(&s, &g, "/github.com/o/r.git/objects/pack", "").is_err());
        assert!(target(&s, &p, "/github.com/o/r.git/info/refs", "service=git-upload-pack").is_err());
        assert!(target(&s, &g, "/github.com/o/r.git/git-upload-pack", "").is_err());
        let mut s2 = Settings::default();
        s2.git_allow_push = true;
        s2.git_hosts.push("git.example.com".into());
        assert!(target(&s2, &p, "/github.com/o/r.git/git-receive-pack", "").is_ok());
        assert!(target(&s2, &g, "/git.example.com/o/r.git/info/refs", "service=git-upload-pack").is_ok());
    }
}
