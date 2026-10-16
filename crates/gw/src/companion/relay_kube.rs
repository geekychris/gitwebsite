//! `/v1/relay/kube/<context>/<api path>` — a Kubernetes API, read-only, through the user's
//! kubeconfig and a named context (as container-relay.py). The kubeconfig is read, never
//! written, and its current context is never changed; credentials stay in this process.
//! GET only (watch and log follow stream through); exec, attach, port-forward, the API server's
//! proxy subresources and Secrets are refused. `GET /v1/relay/kube` lists the contexts.
//! Context names may contain `/` or `:` — percent-encode them in the path.

use super::{authorize, bad_path, err, tagged, Gateway, Origin};
use anyhow::{bail, Context as _, Result};
use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    Json,
};
use base64::Engine;
use futures_util::TryStreamExt;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// Read-only Kubernetes API paths: discovery, version/health, and resources — not exec, attach,
/// portforward or proxy subresources, not Secrets.
pub fn path_ok(path: &str) -> bool {
    if bad_path(path) {
        return false;
    }
    let p = path.trim_end_matches('/');
    if matches!(p, "/version" | "/healthz" | "/readyz" | "/livez" | "/api" | "/apis") || p.starts_with("/openapi/v2") || p.starts_with("/openapi/v3") {
        return true;
    }
    let segs: Vec<&str> = p.split('/').filter(|s| !s.is_empty()).collect();
    let rest: &[&str] = match segs.as_slice() {
        ["api", "v1", rest @ ..] => rest,
        ["apis", _group] => return true,
        ["apis", _group, _version, rest @ ..] => rest,
        _ => return false,
    };
    let mut rest = rest;
    if rest.first() == Some(&"watch") {
        rest = &rest[1..];
    }
    if rest.len() == 2 && rest[0] == "namespaces" {
        return true;
    }
    if rest.len() >= 3 && rest[0] == "namespaces" {
        rest = &rest[2..];
    }
    if rest.is_empty() {
        return true;
    }
    if rest[0] == "secrets" {
        return false;
    }
    !(rest.len() >= 3 && ["exec", "attach", "portforward", "proxy"].contains(&rest[2]))
}

// ------------------------------------------------------------------ kubeconfig

fn kubeconfig_paths(configured: &str) -> Vec<PathBuf> {
    if !configured.is_empty() {
        return vec![gw_core::config::expand_home(configured)];
    }
    if let Ok(k) = std::env::var("KUBECONFIG") {
        let v: Vec<PathBuf> = k.split(':').filter(|s| !s.is_empty()).map(PathBuf::from).collect();
        if !v.is_empty() {
            return v;
        }
    }
    vec![gw_core::config::expand_home("~/.kube/config")]
}

/// Merged kubeconfig (first file wins for a name, as kubectl does) and the files' mtimes.
fn load(configured: &str) -> Result<(Value, Vec<(PathBuf, Option<SystemTime>)>)> {
    let mut merged = json!({"contexts": [], "clusters": [], "users": [], "current-context": null});
    let mut stamps = vec![];
    let mut any = false;
    for p in kubeconfig_paths(configured) {
        let m = std::fs::metadata(&p).ok().and_then(|m| m.modified().ok());
        stamps.push((p.clone(), m));
        let Ok(t) = std::fs::read_to_string(&p) else { continue };
        any = true;
        let mut v: Value = serde_yaml::from_str(&t).with_context(|| format!("parsing {}", p.display()))?;
        let dir = p.parent().map(|d| d.to_path_buf()).unwrap_or_default();
        for key in ["contexts", "clusters", "users"] {
            if let Some(items) = v.get_mut(key).and_then(|x| x.as_array_mut()) {
                for it in items.iter_mut() {
                    // Remember where relative file references are resolved from.
                    it["_dir"] = json!(dir.display().to_string());
                    let name = it["name"].clone();
                    let list = merged[key].as_array_mut().unwrap();
                    if !list.iter().any(|x| x["name"] == name) {
                        list.push(it.clone());
                    }
                }
            }
        }
        if merged["current-context"].is_null() {
            if let Some(c) = v.get("current-context").filter(|c| c.as_str().is_some_and(|s| !s.is_empty())) {
                merged["current-context"] = c.clone();
            }
        }
    }
    if !any {
        bail!("no kubeconfig found (looked at {})", kubeconfig_paths(configured).iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "));
    }
    Ok((merged, stamps))
}

fn find<'a>(cfg: &'a Value, key: &str, name: &str) -> Option<&'a Value> {
    cfg[key].as_array()?.iter().find(|x| x["name"].as_str() == Some(name))
}

/// Contexts for the Companion page and `/v1/relay/kube` (no credentials).
pub fn describe(configured: &str) -> Value {
    match load(configured) {
        Err(e) => json!({ "ok": false, "error": format!("{e:#}") }),
        Ok((cfg, stamps)) => {
            let contexts: Vec<Value> = cfg["contexts"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|c| {
                    let cluster = c["context"]["cluster"].as_str().unwrap_or("");
                    let user = c["context"]["user"].as_str().unwrap_or("");
                    json!({
                        "name": c["name"], "cluster": cluster, "namespace": c["context"]["namespace"],
                        "server": find(&cfg, "clusters", cluster).map(|x| x["cluster"]["server"].clone()),
                        "auth": find(&cfg, "users", user).map(|u| auth_kind(&u["user"])).unwrap_or("none"),
                    })
                })
                .collect();
            json!({
                "ok": true, "readOnly": true,
                "kubeconfig": stamps.iter().map(|(p, _)| p.display().to_string()).collect::<Vec<_>>(),
                "current_context": cfg["current-context"], "contexts": contexts,
            })
        }
    }
}

fn auth_kind(u: &Value) -> &'static str {
    if u["client-certificate-data"].is_string() || u["client-certificate"].is_string() {
        "client certificate"
    } else if u["token"].is_string() {
        "bearer token"
    } else if u["tokenFile"].is_string() {
        "token file"
    } else if u["username"].is_string() {
        "basic"
    } else if u["exec"].is_object() {
        "exec plugin"
    } else if u["auth-provider"].is_object() {
        "auth-provider (unsupported: use kubectl proxy)"
    } else {
        "none"
    }
}

// ------------------------------------------------------------------ clients per context

#[derive(Clone)]
struct Ctx {
    client: reqwest::Client,
    base: String,
    headers: Vec<(String, String)>,
    token_file: Option<PathBuf>,
    exec: Option<Value>,
    stamps: Vec<(PathBuf, Option<SystemTime>)>,
}

#[derive(Default)]
pub struct Contexts {
    clients: Mutex<HashMap<String, Ctx>>,
    exec_tokens: Mutex<HashMap<String, (String, Instant)>>,
}

fn rel(dir: &Value, p: &str) -> PathBuf {
    let pb = PathBuf::from(p);
    if pb.is_absolute() {
        pb
    } else {
        PathBuf::from(dir.as_str().unwrap_or("")).join(pb)
    }
}

fn data_or_file(obj: &Value, data_key: &str, file_key: &str) -> Result<Option<Vec<u8>>> {
    if let Some(d) = obj[data_key].as_str() {
        return Ok(Some(base64::engine::general_purpose::STANDARD.decode(d.trim()).with_context(|| format!("{data_key} is not base64"))?));
    }
    if let Some(f) = obj[file_key].as_str() {
        let p = rel(&obj["_dir"], f);
        return Ok(Some(std::fs::read(&p).with_context(|| format!("reading {}", p.display()))?));
    }
    Ok(None)
}

fn build(configured: &str, name: &str) -> Result<Ctx> {
    let (cfg, stamps) = load(configured)?;
    let c = find(&cfg, "contexts", name).with_context(|| format!("no context named {name:?} in the kubeconfig"))?;
    let cl_name = c["context"]["cluster"].as_str().unwrap_or("");
    let us_name = c["context"]["user"].as_str().unwrap_or("");
    let cl_entry = find(&cfg, "clusters", cl_name).with_context(|| format!("context {name:?}: no cluster {cl_name:?}"))?;
    let mut cl = cl_entry["cluster"].clone();
    cl["_dir"] = cl_entry["_dir"].clone();
    let mut us = find(&cfg, "users", us_name).map(|u| u["user"].clone()).unwrap_or(json!({}));
    us["_dir"] = find(&cfg, "users", us_name).map(|u| u["_dir"].clone()).unwrap_or(Value::Null);
    let server = cl["server"].as_str().filter(|s| !s.is_empty()).with_context(|| format!("context {name:?} has no cluster server"))?;
    let mut url = reqwest::Url::parse(server).context("bad cluster server URL")?;

    let mut b = reqwest::Client::builder().connect_timeout(Duration::from_secs(10)).redirect(reqwest::redirect::Policy::none());
    if let Some(ca) = data_or_file(&cl, "certificate-authority-data", "certificate-authority")? {
        for cert in reqwest::Certificate::from_pem_bundle(&ca).context("reading the cluster CA")? {
            b = b.add_root_certificate(cert);
        }
    }
    if cl["insecure-skip-tls-verify"].as_bool() == Some(true) {
        b = b.danger_accept_invalid_certs(true);
    }
    // tls-server-name: connect to the server's address but verify (and send SNI) for this name.
    if let Some(sni) = cl["tls-server-name"].as_str() {
        let host = url.host_str().unwrap_or("").to_string();
        let port = url.port_or_known_default().unwrap_or(443);
        let addrs: Vec<std::net::SocketAddr> = std::net::ToSocketAddrs::to_socket_addrs(&(host.as_str(), port)).map(|a| a.collect()).unwrap_or_default();
        if let Some(a) = addrs.first() {
            b = b.resolve(sni, *a);
            let _ = url.set_host(Some(sni));
        }
    }
    let cert = data_or_file(&us, "client-certificate-data", "client-certificate")?;
    let key = data_or_file(&us, "client-key-data", "client-key")?;
    if let (Some(mut c), Some(k)) = (cert, key) {
        c.push(b'\n');
        c.extend(k);
        b = b.identity(reqwest::Identity::from_pem(&c).context("reading the client certificate and key")?);
    }
    let mut headers = vec![];
    let mut token_file = None;
    let mut exec = None;
    if let Some(t) = us["token"].as_str() {
        headers.push(("authorization".into(), format!("Bearer {t}")));
    } else if let Some(f) = us["tokenFile"].as_str() {
        token_file = Some(rel(&us["_dir"], f));
    } else if let (Some(u), Some(p)) = (us["username"].as_str(), us["password"].as_str()) {
        headers.push(("authorization".into(), format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{u}:{p}")))));
    } else if us["exec"].is_object() {
        exec = Some(us["exec"].clone());
    } else if us["auth-provider"].is_object() {
        bail!("context {name:?} uses an auth-provider; run `kubectl proxy --context {name}` and use a kubeconfig context pointing at it");
    }
    Ok(Ctx { client: b.build()?, base: url.as_str().trim_end_matches('/').to_string(), headers, token_file, exec, stamps })
}

impl Contexts {
    fn get(&self, configured: &str, name: &str) -> Result<Ctx> {
        let key = format!("{configured}\u{0}{name}");
        if let Some(c) = self.clients.lock().unwrap().get(&key) {
            let fresh = c.stamps.iter().all(|(p, m)| std::fs::metadata(p).ok().and_then(|x| x.modified().ok()) == *m);
            if fresh {
                return Ok(c.clone());
            }
        }
        let c = build(configured, name)?;
        self.clients.lock().unwrap().insert(key, c.clone());
        Ok(c)
    }

    async fn auth_headers(&self, name: &str, c: &Ctx) -> Result<Vec<(String, String)>> {
        let mut h = c.headers.clone();
        if let Some(f) = &c.token_file {
            let t = std::fs::read_to_string(f).with_context(|| format!("reading {}", f.display()))?;
            h.push(("authorization".into(), format!("Bearer {}", t.trim())));
        }
        if let Some(e) = &c.exec {
            if let Some((t, exp)) = self.exec_tokens.lock().unwrap().get(name).cloned() {
                if Instant::now() + Duration::from_secs(60) < exp {
                    h.push(("authorization".into(), format!("Bearer {t}")));
                    return Ok(h);
                }
            }
            let e = e.clone();
            let (tok, ttl) = tokio::task::spawn_blocking(move || run_exec(&e)).await??;
            self.exec_tokens.lock().unwrap().insert(name.into(), (tok.clone(), Instant::now() + ttl));
            h.push(("authorization".into(), format!("Bearer {tok}")));
        }
        Ok(h)
    }
}

/// Run a client-go exec credential plugin (aws eks get-token, gke-gcloud-auth-plugin, …).
fn run_exec(e: &Value) -> Result<(String, Duration)> {
    let cmd = e["command"].as_str().context("exec plugin without a command")?;
    let args: Vec<String> = e["args"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
    let mut c = std::process::Command::new(cmd);
    c.args(&args);
    for v in e["env"].as_array().cloned().unwrap_or_default() {
        if let (Some(n), Some(val)) = (v["name"].as_str(), v["value"].as_str()) {
            c.env(n, val);
        }
    }
    c.env("KUBERNETES_EXEC_INFO", json!({"apiVersion": e["apiVersion"], "kind": "ExecCredential", "spec": {"interactive": false}}).to_string());
    let out = c.stdin(std::process::Stdio::null()).output().with_context(|| format!("running {cmd}"))?;
    if !out.status.success() {
        bail!("{cmd} failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let v: Value = serde_json::from_slice(&out.stdout).context("exec plugin output is not JSON")?;
    let tok = v["status"]["token"].as_str().context("exec plugin returned no token (client certificates from plugins are not supported)")?;
    let ttl = v["status"]["expirationTimestamp"]
        .as_str()
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .map(|t| (t.with_timezone(&chrono::Utc) - chrono::Utc::now()).to_std().unwrap_or_default())
        .unwrap_or(Duration::from_secs(300));
    Ok((tok.to_string(), ttl))
}

// ------------------------------------------------------------------ handler

pub async fn handle(State(gw): State<Arc<Gateway>>, axum::Extension(Origin(origin)): axum::Extension<Origin>, method: Method, uri: Uri, h: HeaderMap) -> Response {
    let s = gw.settings();
    if !s.relays.kube {
        return err(StatusCode::SERVICE_UNAVAILABLE, "the Kubernetes relay is turned off in the companion settings");
    }
    let (rec, _) = match authorize(&gw, &h, &origin, "relay.kube") {
        Ok(x) => x,
        Err(r) => return r,
    };
    if method != Method::GET {
        let mut r = err(StatusCode::METHOD_NOT_ALLOWED, format!("{method} refused: the Kubernetes relay is read-only (GET only)"));
        r.headers_mut().insert("allow", HeaderValue::from_static("GET, OPTIONS"));
        return tagged(r, &rec);
    }
    if h.contains_key("upgrade") {
        return tagged(err(StatusCode::BAD_REQUEST, "upgrade requests are not relayed (no exec, attach or port-forward)"), &rec);
    }
    let rest = uri.path().strip_prefix("/v1/relay/kube").unwrap_or("").trim_start_matches('/');
    if rest.is_empty() {
        return tagged(Json(describe(&s.kubeconfig)).into_response(), &rec);
    }
    let (ctx_enc, api) = rest.split_once('/').unwrap_or((rest, ""));
    let ctx = urlencoding::decode(ctx_enc).map(|c| c.into_owned()).unwrap_or_default();
    let api = format!("/{api}");
    if !path_ok(&api) {
        return tagged(err(StatusCode::FORBIDDEN, format!("not a read-only Kubernetes path this relay forwards: {api}")), &rec);
    }
    let r = forward(&gw, &s.kubeconfig, &ctx, &api, uri.query().unwrap_or(""), &h).await;
    tagged(r, &rec)
}

async fn forward(gw: &Gateway, configured: &str, ctx: &str, api: &str, query: &str, h: &HeaderMap) -> Response {
    let c = match gw.kube.get(configured, ctx) {
        Ok(c) => c,
        Err(e) => return err(StatusCode::SERVICE_UNAVAILABLE, format!("{e:#}")),
    };
    let auth = match gw.kube.auth_headers(ctx, &c).await {
        Ok(a) => a,
        Err(e) => return err(StatusCode::BAD_GATEWAY, format!("credentials for {ctx}: {e:#}")),
    };
    let follow = super::relay_docker::is_follow(api, query);
    let url = format!("{}{}{}", c.base, api, if query.is_empty() { String::new() } else { format!("?{query}") });
    let mut rb = c
        .client
        .get(&url)
        .header("accept", h.get("accept").and_then(|v| v.to_str().ok()).unwrap_or("application/json"))
        .header("user-agent", "gitwebsite-companion/1");
    for (k, v) in auth {
        rb = rb.header(k, v);
    }
    if !follow {
        rb = rb.timeout(Duration::from_secs(30));
    }
    let resp = match rb.send().await {
        Ok(r) => r,
        Err(e) => return err(StatusCode::BAD_GATEWAY, format!("kubernetes ({ctx}): {e}")),
    };
    let status = resp.status();
    let ctype = resp.headers().get("content-type").cloned();
    let mut out = Response::new(Body::from_stream(resp.bytes_stream().map_err(std::io::Error::other)));
    *out.status_mut() = status;
    let hd = out.headers_mut();
    if let Some(ct) = ctype {
        hd.insert("content-type", ct);
    }
    hd.insert("cache-control", HeaderValue::from_static("no-store"));
    hd.insert("x-relay-upstream", HeaderValue::from_static("kube"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn read_only_paths() {
        for ok in ["/version", "/api", "/apis", "/api/v1/pods", "/api/v1/namespaces", "/api/v1/namespaces/default",
            "/api/v1/namespaces/default/pods", "/api/v1/namespaces/default/pods/web-1", "/api/v1/namespaces/default/pods/web-1/log",
            "/apis/apps/v1/deployments", "/apis/apps/v1/namespaces/x/deployments/y", "/api/v1/watch/pods", "/apis/apps", "/openapi/v3"] {
            assert!(path_ok(ok), "{ok}");
        }
        for bad in ["/api/v1/secrets", "/api/v1/namespaces/default/secrets", "/api/v1/namespaces/default/secrets/tls",
            "/api/v1/namespaces/default/pods/web-1/exec", "/api/v1/namespaces/default/pods/web-1/attach",
            "/api/v1/namespaces/default/pods/web-1/portforward", "/api/v1/namespaces/default/services/s/proxy",
            "/api/v1/nodes/n1/proxy", "/api/v1/namespaces/../secrets", "/metrics", "/logs", "/api/v1/namespaces/x/pods/y%2Fexec"] {
            assert!(!path_ok(bad), "{bad}");
        }
    }

    #[test]
    fn kubeconfig_contexts_without_secrets() {
        let d = std::env::temp_dir().join(format!("gw-kc-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("config");
        std::fs::write(&p, "apiVersion: v1\nkind: Config\ncurrent-context: a\nclusters:\n- name: c1\n  cluster:\n    server: http://127.0.0.1:1\ncontexts:\n- name: a\n  context: {cluster: c1, user: u1, namespace: ns}\n- name: arn:aws:eks:x/y\n  context: {cluster: c1, user: u1}\nusers:\n- name: u1\n  user:\n    token: SECRET-TOKEN\n").unwrap();
        let v = describe(p.to_str().unwrap());
        assert_eq!(v["current_context"], "a");
        assert_eq!(v["contexts"].as_array().unwrap().len(), 2);
        assert!(!v.to_string().contains("SECRET-TOKEN"));
        let c = build(p.to_str().unwrap(), "arn:aws:eks:x/y").unwrap();
        assert_eq!(c.base, "http://127.0.0.1:1");
        assert!(build(p.to_str().unwrap(), "missing").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
