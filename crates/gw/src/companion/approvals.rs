//! First-use approvals: a paired page asks for something not yet allowed — a new LAN host
//! (`lan`, for relay/lan and LAN ws:// targets), a new LAN TCP target (`tcp`), or an S3 object
//! deletion (`s3-delete`). The request waits while the app asks the user (a dialog; also on the
//! Companion page and in `gw companion approvals`, decided with `gw companion approve <code>`).
//! Approving a `lan` or `tcp` request adds the target to its allow-list (saved); `s3-delete`
//! approves that one deletion. A denial is remembered for [`DENY_TTL`] so retries don't
//! re-prompt.

use rand::RngCore;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::sync::watch;

pub const APPROVAL_TTL: Duration = Duration::from_secs(300);
pub const DENY_TTL: Duration = Duration::from_secs(300);
/// How long a relay request waits in place for the user's decision.
pub const WAIT: Duration = Duration::from_secs(90);

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ApprovalRequest {
    pub id: String,
    pub code: String,
    /// `lan`, `tcp` or `s3-delete`.
    pub kind: String,
    /// What gets allowed: a host (`lan`), `host:port` (`tcp`), `profile/bucket/key` (`s3-delete`).
    pub target: String,
    pub origin: String,
    pub tool: String,
    /// What the page asked for, for the dialog (e.g. `GET https://tv.local:3001/…`).
    pub detail: String,
    pub created: String,
}

struct Pending {
    req: ApprovalRequest,
    at: Instant,
    tx: watch::Sender<Option<bool>>,
}

#[derive(Default)]
pub struct Approvals {
    pending: Mutex<HashMap<String, Pending>>,
    denied: Mutex<HashMap<(String, String, String), Instant>>,
}

pub enum Ask {
    /// A new request (call the app's hook) or an existing one for the same target.
    Wait { req: ApprovalRequest, rx: watch::Receiver<Option<bool>>, new: bool },
    /// Denied recently.
    Denied,
}

impl Approvals {
    fn sweep(m: &mut HashMap<String, Pending>) {
        m.retain(|_, p| p.at.elapsed() < APPROVAL_TTL && p.tx.borrow().is_none());
    }

    /// Ask (or join the pending question) about `kind`/`target` from `origin`.
    pub fn ask(&self, kind: &str, target: &str, origin: &str, tool: &str, detail: &str) -> Ask {
        let key = (kind.to_string(), target.to_string(), origin.to_string());
        {
            let mut d = self.denied.lock().unwrap();
            d.retain(|_, at| at.elapsed() < DENY_TTL);
            if d.contains_key(&key) {
                return Ask::Denied;
            }
        }
        let mut m = self.pending.lock().unwrap();
        Self::sweep(&mut m);
        if let Some(p) = m.values().find(|p| p.req.kind == kind && p.req.target == target && p.req.origin == origin) {
            return Ask::Wait { req: p.req.clone(), rx: p.tx.subscribe(), new: false };
        }
        let mut b = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut b);
        let req = ApprovalRequest {
            id: hex::encode(b),
            code: format!("{:06}", rand::random::<u32>() % 1_000_000),
            kind: kind.into(),
            target: target.into(),
            origin: origin.into(),
            tool: tool.into(),
            detail: detail.chars().take(300).collect(),
            created: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        };
        let (tx, rx) = watch::channel(None);
        m.insert(req.id.clone(), Pending { req: req.clone(), at: Instant::now(), tx });
        Ask::Wait { req, rx, new: true }
    }

    pub fn list(&self) -> Vec<ApprovalRequest> {
        let mut m = self.pending.lock().unwrap();
        Self::sweep(&mut m);
        let mut v: Vec<_> = m.values().map(|p| p.req.clone()).collect();
        v.sort_by(|a, b| a.created.cmp(&b.created));
        v
    }

    /// Find a pending request by id, code, or (when `kind` is given) its target.
    pub fn find(&self, which: &str, kind: Option<&str>) -> Option<ApprovalRequest> {
        let w = which.to_ascii_lowercase();
        self.list().into_iter().find(|r| r.id == which || r.code == which || kind.is_some_and(|k| r.kind == k && r.target.eq_ignore_ascii_case(&w)))
    }

    /// Record the decision; the caller adds the target to its allow-list first when allowing.
    pub fn decide(&self, id: &str, allow: bool) -> Option<ApprovalRequest> {
        let mut m = self.pending.lock().unwrap();
        let p = m.remove(id)?;
        if !allow {
            self.denied.lock().unwrap().insert((p.req.kind.clone(), p.req.target.clone(), p.req.origin.clone()), Instant::now());
        }
        p.tx.send_replace(Some(allow));
        Some(p.req)
    }
}

/// Wait for a decision: `Some(true/false)`, or `None` if still undecided after `wait`.
pub async fn wait(mut rx: watch::Receiver<Option<bool>>, wait: Duration) -> Option<bool> {
    let _ = tokio::time::timeout(wait, rx.wait_for(|d| d.is_some())).await;
    let d = *rx.borrow();
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn ask_join_decide_deny() {
        let a = Approvals::default();
        let Ask::Wait { req, rx, new } = a.ask("lan", "tv.local", "https://s", "t", "GET /") else { panic!() };
        assert!(new);
        let Ask::Wait { new: again, .. } = a.ask("lan", "tv.local", "https://s", "t", "GET /x") else { panic!() };
        assert!(!again);
        assert_eq!(a.find("tv.local", Some("lan")).unwrap().id, req.id);
        assert_eq!(wait(rx.clone(), Duration::from_millis(10)).await, None);
        a.decide(&req.id, true);
        assert_eq!(wait(rx, Duration::from_secs(1)).await, Some(true));
        let Ask::Wait { req, .. } = a.ask("tcp", "10.0.0.2:5555", "https://s", "t", "") else { panic!() };
        a.decide(&req.code.clone(), false); // by code: not found (decide takes ids)
        a.decide(&req.id, false);
        assert!(matches!(a.ask("tcp", "10.0.0.2:5555", "https://s", "t", ""), Ask::Denied));
        assert!(a.list().is_empty());
    }
}
