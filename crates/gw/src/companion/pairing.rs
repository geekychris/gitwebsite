//! Pairing: a page asks for a token (`POST /v1/pair`), the user approves it in the app (or with
//! `gw companion approve <code>`), and the page collects the token (`GET /v1/pair/<id>`).
//! Tokens are random 256-bit values shown once; only their SHA-256 is kept, with the origin
//! they were issued to (a token works only from that origin), the tool and the scopes.

use anyhow::{bail, Context, Result};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::sync::watch;

pub const SCOPES: &[&str] = &["relay.http", "relay.tcp", "relay.docker", "relay.kube", "relay.git", "view"];
pub const PAIR_TTL: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TokenRecord {
    /// Short public id (first 12 hex digits of the hash), for listing and revoking.
    pub id: String,
    pub hash: String,
    pub origin: String,
    pub tool: String,
    pub scopes: Vec<String>,
    pub created: String,
    #[serde(default)]
    pub last_used: Option<String>,
}

pub fn hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn new_token() -> String {
    let mut b = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut b);
    hex::encode(b)
}

/// Normalise requested scopes: known ones only, deduplicated; empty means all relays (not view).
pub fn clean_scopes(asked: &[String]) -> Result<Vec<String>> {
    let mut out: Vec<String> = vec![];
    for s in asked {
        if !SCOPES.contains(&s.as_str()) {
            bail!("unknown scope {s:?}; known: {}", SCOPES.join(", "));
        }
        if !out.contains(s) {
            out.push(s.clone());
        }
    }
    if out.is_empty() {
        out = SCOPES.iter().filter(|s| s.starts_with("relay.")).map(|s| s.to_string()).collect();
    }
    Ok(out)
}

/// The token file (hashes only), or memory for tests.
pub struct Tokens {
    path: Option<PathBuf>,
    list: Mutex<Vec<TokenRecord>>,
}

impl Tokens {
    pub fn open(path: Option<PathBuf>) -> Self {
        let list = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        Tokens { path, list: Mutex::new(list) }
    }

    fn save(&self, list: &[TokenRecord]) {
        let Some(p) = &self.path else { return };
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let tmp = p.with_extension("json.tmp");
        if std::fs::write(&tmp, serde_json::to_vec_pretty(list).unwrap_or_default()).is_ok() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
            }
            let _ = std::fs::rename(&tmp, p);
        }
    }

    /// Issue a token; returns (token, record). The token itself is never stored.
    pub fn issue(&self, origin: &str, tool: &str, scopes: Vec<String>) -> (String, TokenRecord) {
        let t = new_token();
        let h = hash(&t);
        let rec = TokenRecord { id: h[..12].to_string(), hash: h, origin: origin.into(), tool: tool.into(), scopes, created: now(), last_used: None };
        let mut l = self.list.lock().unwrap();
        // One live token per origin + tool: re-pairing replaces the old one.
        l.retain(|r| !(r.origin == rec.origin && r.tool == rec.tool));
        l.push(rec.clone());
        self.save(&l);
        (t, rec)
    }

    /// The record for a presented token, if it exists and was issued to this origin.
    pub fn check(&self, token: &str, origin: &str) -> Option<TokenRecord> {
        let h = hash(token);
        let mut l = self.list.lock().unwrap();
        let r = l.iter_mut().find(|r| r.hash == h && r.origin == origin)?;
        // Write last_used at most once a minute.
        let stamp = now();
        let stale = r.last_used.as_deref().map(|u| u.get(..16) != stamp.get(..16)).unwrap_or(true);
        r.last_used = Some(stamp);
        let rec = r.clone();
        if stale {
            let snapshot = l.clone();
            drop(l);
            self.save(&snapshot);
        }
        Some(rec)
    }

    pub fn list(&self) -> Vec<Value> {
        self.list
            .lock()
            .unwrap()
            .iter()
            .map(|r| json!({"id": r.id, "origin": r.origin, "tool": r.tool, "scopes": r.scopes, "created": r.created, "last_used": r.last_used}))
            .collect()
    }

    /// Revoke a presented token (the page's own "Forget"); returns its record.
    pub fn revoke_token(&self, token: &str, origin: &str) -> Option<TokenRecord> {
        let h = hash(token);
        let mut l = self.list.lock().unwrap();
        let i = l.iter().position(|r| r.hash == h && r.origin == origin)?;
        let rec = l.remove(i);
        self.save(&l);
        Some(rec)
    }

    /// Revoke by id, or every token of an origin.
    pub fn revoke(&self, which: &str) -> usize {
        let mut l = self.list.lock().unwrap();
        let before = l.len();
        l.retain(|r| r.id != which && r.origin != which);
        let n = before - l.len();
        self.save(&l);
        n
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Pending,
    Approved { token: String, scopes: Vec<String> },
    Denied,
}

#[derive(Debug, Clone, Serialize)]
pub struct PairRequest {
    pub id: String,
    pub code: String,
    pub origin: String,
    pub tool: String,
    pub scopes: Vec<String>,
    pub created: String,
}

struct Pending {
    req: PairRequest,
    at: Instant,
    tx: watch::Sender<Decision>,
}

#[derive(Default)]
pub struct Pairing {
    pending: Mutex<HashMap<String, Pending>>,
}

impl Pairing {
    fn sweep(map: &mut HashMap<String, Pending>) {
        map.retain(|_, p| p.at.elapsed() < PAIR_TTL);
    }

    pub fn create(&self, origin: &str, tool: &str, scopes: Vec<String>) -> PairRequest {
        let mut b = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut b);
        let code = format!("{:06}", rand::random::<u32>() % 1_000_000);
        let req = PairRequest { id: hex::encode(b), code, origin: origin.into(), tool: tool.into(), scopes, created: now() };
        let (tx, _) = watch::channel(Decision::Pending);
        let mut m = self.pending.lock().unwrap();
        Self::sweep(&mut m);
        m.insert(req.id.clone(), Pending { req: req.clone(), at: Instant::now(), tx });
        req
    }

    pub fn list(&self) -> Vec<PairRequest> {
        let mut m = self.pending.lock().unwrap();
        Self::sweep(&mut m);
        let mut v: Vec<_> = m.values().filter(|p| *p.tx.borrow() == Decision::Pending).map(|p| p.req.clone()).collect();
        v.sort_by(|a, b| a.created.cmp(&b.created));
        v
    }

    /// Find a pending request by id or 6-digit code.
    pub fn find(&self, which: &str) -> Option<PairRequest> {
        self.list().into_iter().find(|r| r.id == which || r.code == which)
    }

    /// Approve or deny; approving issues the token.
    pub fn decide(&self, which: &str, allow: bool, tokens: &Tokens) -> Result<PairRequest> {
        let req = self.find(which).with_context(|| format!("no pending pairing request {which:?}"))?;
        let m = self.pending.lock().unwrap();
        let p = m.get(&req.id).context("pairing request expired")?;
        let d = if allow {
            let (t, _) = tokens.issue(&req.origin, &req.tool, req.scopes.clone());
            Decision::Approved { token: t, scopes: req.scopes.clone() }
        } else {
            Decision::Denied
        };
        p.tx.send_replace(d);
        Ok(req)
    }

    /// Wait (up to `wait`) for a decision on request `id` from `origin`. `None`: no such request.
    /// A decided request is collected once and then forgotten.
    pub async fn wait(&self, id: &str, origin: &str, wait: Duration) -> Option<Decision> {
        let mut rx = {
            let m = self.pending.lock().unwrap();
            let p = m.get(id).filter(|p| p.req.origin == origin && p.at.elapsed() < PAIR_TTL)?;
            p.tx.subscribe()
        };
        let _ = tokio::time::timeout(wait, rx.wait_for(|d| *d != Decision::Pending)).await;
        let d = rx.borrow().clone();
        if d != Decision::Pending {
            self.pending.lock().unwrap().remove(id);
        }
        Some(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_bound_to_origin_and_hashed() {
        let d = std::env::temp_dir().join(format!("gw-tok-{}", std::process::id()));
        let p = d.join("t.json");
        let t = Tokens::open(Some(p.clone()));
        let (tok, rec) = t.issue("https://a.test", "redis", vec!["relay.tcp".into()]);
        assert!(t.check(&tok, "https://a.test").is_some());
        assert!(t.check(&tok, "https://b.test").is_none());
        assert!(t.check("nope", "https://a.test").is_none());
        let file = std::fs::read_to_string(&p).unwrap();
        assert!(!file.contains(&tok) && file.contains(&rec.hash));
        let again = Tokens::open(Some(p.clone()));
        assert!(again.check(&tok, "https://a.test").is_some());
        assert_eq!(again.revoke(&rec.id), 1);
        assert!(again.check(&tok, "https://a.test").is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn scopes() {
        assert_eq!(clean_scopes(&[]).unwrap().len(), 5);
        assert!(clean_scopes(&["root".into()]).is_err());
        assert_eq!(clean_scopes(&["view".into(), "view".into()]).unwrap(), vec!["view"]);
    }

    #[tokio::test]
    async fn approve_and_deny() {
        let p = Pairing::default();
        let t = Tokens::open(None);
        let r = p.create("https://a.test", "redis", vec!["relay.tcp".into()]);
        assert_eq!(p.wait(&r.id, "https://a.test", Duration::from_millis(10)).await, Some(Decision::Pending));
        assert_eq!(p.wait(&r.id, "https://evil.test", Duration::from_millis(10)).await, None);
        p.decide(&r.code, true, &t).unwrap();
        match p.wait(&r.id, "https://a.test", Duration::from_secs(1)).await {
            Some(Decision::Approved { token, .. }) => assert!(t.check(&token, "https://a.test").is_some()),
            other => panic!("{other:?}"),
        }
        let r2 = p.create("https://a.test", "kafka", vec![]);
        p.decide(&r2.id, false, &t).unwrap();
        assert_eq!(p.wait(&r2.id, "https://a.test", Duration::from_secs(1)).await, Some(Decision::Denied));
        assert!(p.decide("000000", true, &t).is_err() || p.list().is_empty());
    }
}
