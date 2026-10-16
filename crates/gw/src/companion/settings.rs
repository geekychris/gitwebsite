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
}

impl Default for Relays {
    fn default() -> Self {
        Relays { http: true, tcp: true, docker: true, kube: true, git: true }
    }
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
    /// Approve every pairing request without asking (headless `gw serve --companion-auto-approve`).
    pub auto_approve: bool,
    pub relays: Relays,
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
            auto_approve: false,
            relays: Relays::default(),
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
            if !u.is_empty() && !(u.starts_with("http://") || u.starts_with("https://")) {
                anyhow::bail!("{name} must be an http(s) URL");
            }
        }
        Ok(())
    }
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
    }
}
