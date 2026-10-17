//! Companion settings: `companion.toml` beside the user config (`gw config`), editable in the
//! app's Companion page, with `gw companion set`, or by hand.
//!
//! ```toml
//! enabled = true
//! port = 7317                       # fallbacks 7318–7320
//! extra_origins = ["https://example.org"]
//! tcp_allow = ["127.0.0.1:5432", "127.0.0.1:6379", "piaware.local:30005"]
//! receiver = "http://piaware.local:8080"
//! kafka_rest = "http://localhost:8082"
//! git_allow_push = false
//! [relays]
//! http = true
//! tcp = true
//! docker = true
//! kube = true
//! git = true
//! ```

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const DEFAULT_PORT: u16 = 7317;
pub const DEFAULT_ORIGINS: &[&str] = &["https://geekychris.github.io"];
pub const DEFAULT_TCP: &[&str] = &["127.0.0.1:5432", "127.0.0.1:6379"];
pub const DEFAULT_GIT_HOSTS: &[&str] = &["github.com", "gitlab.com", "codeberg.org", "bitbucket.org"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Relays {
    pub http: bool,
    pub tcp: bool,
    pub docker: bool,
    pub kube: bool,
    pub git: bool,
    pub lan: bool,
    pub ws: bool,
    pub s3: bool,
}

impl Default for Relays {
    fn default() -> Self {
        Relays { http: true, tcp: true, docker: true, kube: true, git: true, lan: true, ws: true, s3: true }
    }
}

/// An S3-compatible endpoint for `relay/s3/<name>/…`. The access key id and secret are in the
/// secret store (`s3:<name>:access_key_id`, `s3:<name>:secret_access_key`), never here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct S3Profile {
    /// `https://s3.us-east-1.amazonaws.com`, `http://nas.local:9000` (MinIO), … — scheme://host[:port].
    pub endpoint: String,
    pub region: String,
    /// `endpoint/bucket/key` (MinIO, most self-hosted stores) instead of `bucket.endpoint/key`.
    pub path_style: bool,
    /// Accept a self-signed certificate (LAN endpoints only).
    pub insecure: bool,
}

/// A profile name: letters, digits, `-`, `_` (1–64), not `presign`.
pub fn s3_name_ok(n: &str) -> bool {
    !n.is_empty() && n.len() <= 64 && n != "presign" && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A `relay/lan` allow-list entry: `host` (any port) or `host:port`, lower-cased.
pub fn parse_host_entry(e: &str) -> Option<(String, Option<u16>)> {
    let e = e.trim().to_ascii_lowercase();
    if let Some((h, p)) = parse_target(&e) {
        if super::lan::host_syntax_ok(&h) || super::lan::literal(&h).is_some() {
            return Some((h, Some(p)));
        }
    }
    let h = e.trim_start_matches('[').trim_end_matches(']').to_string();
    (super::lan::host_syntax_ok(&h)).then_some((h, None))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Run the gateway at all.
    pub enabled: bool,
    /// First port tried; the next three are fallbacks.
    pub port: u16,
    /// Page origins allowed besides the site, http://127.0.0.1:* and http://localhost:*.
    pub extra_origins: Vec<String>,
    /// `host:port` targets the TCP-over-WebSocket relay may open.
    pub tcp_allow: Vec<String>,
    /// ADS-B receiver web base (e.g. http://piaware.local:8080) for `relay/http/receiver`.
    pub receiver: String,
    /// Kafka REST proxy (Confluent or Redpanda HTTP Proxy) for `relay/http/kafka-rest`.
    pub kafka_rest: String,
    /// Redpanda HTTP Proxy for `relay/http/redpanda-proxy`.
    pub redpanda_proxy: String,
    /// Redpanda Admin API for `relay/http/redpanda-admin` (GET only).
    pub redpanda_admin: String,
    /// Git hosts besides github.com, gitlab.com, codeberg.org and bitbucket.org.
    pub git_hosts: Vec<String>,
    /// Testing only: git hosts reached over plain http (`host:port`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub git_http_hosts: Vec<String>,
    /// Relay `git-receive-pack` (push). Off by default.
    pub git_allow_push: bool,
    /// Docker Engine socket (`unix:///path` or `tcp://host:port`); empty: auto-detect.
    pub docker_host: String,
    /// kubeconfig to read (never written); empty: $KUBECONFIG, else ~/.kube/config.
    pub kubeconfig: String,
    /// LAN hosts `relay/lan` and LAN `ws://`/`wss://` targets may reach: `host` (any port) or
    /// `host:port`; IPs, names and mDNS `.local`. A paired page's first request to a new LAN
    /// host asks the user, and approving adds it here.
    pub lan_hosts: Vec<String>,
    /// Public `wss://` hosts (`host` or `host:port`) `relay/ws` may reach (added by hand only).
    pub ws_allow: Vec<String>,
    /// Ask in the app before each S3 DeleteObject.
    pub s3_confirm_delete: bool,
    /// Approve every pairing request without asking (headless `gw serve --companion-auto-approve`).
    pub auto_approve: bool,
    pub relays: Relays,
    /// S3-compatible endpoints for `relay/s3/<name>/…`.
    pub s3_profiles: BTreeMap<String, S3Profile>,
    /// Testing only: replace a fixed upstream's base URL, e.g. `digikey = "http://127.0.0.1:9999"`,
    /// so supplier relays can be tested end to end against a mock. Honoured only for a
    /// non-default config directory (`GW_CONFIG_DIR`) or with `GW_COMPANION_TEST_UPSTREAMS=1`;
    /// ignored (with a warning) for the user's real config.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub upstream_override: BTreeMap<String, String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            enabled: true,
            port: DEFAULT_PORT,
            extra_origins: vec![],
            tcp_allow: DEFAULT_TCP.iter().map(|s| s.to_string()).collect(),
            receiver: String::new(),
            kafka_rest: String::new(),
            redpanda_proxy: String::new(),
            redpanda_admin: String::new(),
            git_hosts: vec![],
            git_http_hosts: vec![],
            git_allow_push: false,
            docker_host: String::new(),
            kubeconfig: String::new(),
            lan_hosts: vec![],
            ws_allow: vec![],
            s3_confirm_delete: true,
            auto_approve: false,
            relays: Relays::default(),
            s3_profiles: BTreeMap::new(),
            upstream_override: BTreeMap::new(),
        }
    }
}

pub fn dir() -> PathBuf {
    gw_core::userconfig::config_dir()
}

pub fn path() -> PathBuf {
    dir().join("companion.toml")
}

impl Settings {
    pub fn load() -> Result<Self> {
        let p = path();
        if !p.exists() {
            return Ok(Self::default());
        }
        let t = std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
        toml::from_str(&t).with_context(|| format!("parsing {}", p.display()))
    }

    pub fn save(&self) -> Result<()> {
        let p = path();
        std::fs::create_dir_all(p.parent().unwrap())?;
        let body = format!("# gitwebsite companion settings — see docs/companion.adoc.\n{}", toml::to_string_pretty(self)?);
        let tmp = p.with_extension("toml.tmp");
        std::fs::write(&tmp, body)?;
        std::fs::rename(&tmp, &p)?;
        Ok(())
    }

    /// Apply a JSON object of changes (unknown keys are an error) and validate the result.
    pub fn merged(&self, patch: &serde_json::Value) -> Result<Self> {
        let mut v = serde_json::to_value(self)?;
        let obj = patch.as_object().context("settings must be a JSON object")?;
        for (k, val) in obj {
            if v.get(k).is_none() {
                anyhow::bail!("unknown companion setting {k:?}");
            }
            if k == "relays" {
                for (rk, rv) in val.as_object().context("relays must be an object")? {
                    if v["relays"].get(rk).is_none() {
                        anyhow::bail!("unknown relay {rk:?}");
                    }
                    v["relays"][rk] = rv.clone();
                }
            } else {
                v[k] = val.clone();
            }
        }
        let s: Settings = serde_json::from_value(v).context("bad companion settings")?;
        s.validate()?;
        Ok(s)
    }

    pub fn validate(&self) -> Result<()> {
        for t in &self.tcp_allow {
            parse_target(t).with_context(|| format!("tcp_allow entry {t:?} must be host:port"))?;
        }
        for o in &self.extra_origins {
            if !(o.starts_with("https://") || o.starts_with("http://")) || o.ends_with('/') || o.contains('*') {
                anyhow::bail!("origin {o:?} must look like https://host[:port] (no path, no wildcard)");
            }
        }
        for (name, u) in [("receiver", &self.receiver), ("kafka_rest", &self.kafka_rest), ("redpanda_proxy", &self.redpanda_proxy), ("redpanda_admin", &self.redpanda_admin)] {
            if !u.is_empty() {
                check_base(u).with_context(|| format!("{name} {u:?}"))?;
            }
        }
        for e in &self.lan_hosts {
            let (h, _) = parse_host_entry(e).with_context(|| format!("lan_hosts entry {e:?} must be host or host:port"))?;
            if let Some(ip) = super::lan::literal(&h) {
                if !super::lan::ip_lan(ip) {
                    anyhow::bail!("lan_hosts entry {e:?} is a public address (LAN means RFC 1918, link-local, loopback or .local)");
                }
            }
        }
        for e in &self.ws_allow {
            parse_host_entry(e).with_context(|| format!("ws_allow entry {e:?} must be host or host:port"))?;
        }
        for (n, p) in &self.s3_profiles {
            if !s3_name_ok(n) {
                anyhow::bail!("S3 profile name {n:?}: letters, digits, - and _ only (not \"presign\")");
            }
            check_base(&p.endpoint).with_context(|| format!("S3 profile {n}: endpoint {:?}", p.endpoint))?;
            if reqwest::Url::parse(&p.endpoint).map(|u| u.path() != "/").unwrap_or(true) {
                anyhow::bail!("S3 profile {n}: endpoint must be scheme://host[:port] with no path");
            }
            if p.region.is_empty() || !p.region.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
                anyhow::bail!("S3 profile {n}: region {:?} (e.g. us-east-1; MinIO: us-east-1)", p.region);
            }
        }
        for (id, u) in &self.upstream_override {
            check_base(u).with_context(|| format!("upstream_override.{id} {u:?}"))?;
        }
        Ok(())
    }
}

/// A relay base URL: `http(s)://host[:port]`, optionally with a path prefix (kept, but the
/// request path is not doubled — see relay_http), no query, fragment or credentials.
pub fn check_base(u: &str) -> Result<()> {
    if !(u.starts_with("http://") || u.starts_with("https://")) {
        anyhow::bail!("must be an http(s) URL like http://piaware.local:8080");
    }
    let url = reqwest::Url::parse(u).context("not a URL")?;
    if url.host_str().is_none_or(|h| h.is_empty()) {
        anyhow::bail!("has no host");
    }
    if url.query().is_some() || url.fragment().is_some() || !url.username().is_empty() || url.password().is_some() {
        anyhow::bail!("must be scheme://host[:port] only (no query, fragment or user:password@)");
    }
    Ok(())
}

/// `host:port` → (lower-cased host, port). IPv6 as `[::1]:5432`.
pub fn parse_target(t: &str) -> Option<(String, u16)> {
    let t = t.trim();
    let (h, p) = t.rsplit_once(':')?;
    let port: u16 = p.parse().ok().filter(|p| *p > 0)?;
    let h = h.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    if h.is_empty() || h.contains('/') || h.contains(' ') {
        return None;
    }
    Some((h, port))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn merge_and_validate() {
        let s = Settings::default();
        let m = s.merged(&serde_json::json!({"tcp_allow": ["pi.local:30005"], "relays": {"git": false}})).unwrap();
        assert_eq!(m.tcp_allow, vec!["pi.local:30005"]);
        assert!(!m.relays.git && m.relays.http);
        assert!(s.merged(&serde_json::json!({"nope": 1})).is_err());
        assert!(s.merged(&serde_json::json!({"tcp_allow": ["nohost"]})).is_err());
        assert!(s.merged(&serde_json::json!({"extra_origins": ["https://x.test/"]})).is_err());
        let round: Settings = toml::from_str(&toml::to_string_pretty(&m).unwrap()).unwrap();
        assert_eq!(round, m);
        assert_eq!(parse_target("[::1]:5432"), Some(("::1".into(), 5432)));
        assert!(s.merged(&serde_json::json!({"receiver": "http://pi.local:8080"})).is_ok());
        assert!(s.merged(&serde_json::json!({"receiver": "http://pi.local:8080/"})).is_ok());
        assert!(s.merged(&serde_json::json!({"receiver": "pi.local:8080"})).is_err());
        assert!(s.merged(&serde_json::json!({"receiver": "http://pi.local:8080/?x=1"})).is_err());
        assert!(s.merged(&serde_json::json!({"receiver": "http://u:p@pi.local"})).is_err());
        // upstream_override is hand-edited only (not settable through the app API).
        assert!(s.merged(&serde_json::json!({"upstream_override": {"digikey": "http://127.0.0.1:1"}})).is_err());
        let t: Settings = toml::from_str("[upstream_override]\ndigikey = \"http://127.0.0.1:9\"\n").unwrap();
        assert_eq!(t.upstream_override["digikey"], "http://127.0.0.1:9");
    }
}
