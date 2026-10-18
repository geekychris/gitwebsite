//! Meshy.ai: keep a local git repository of 3D models in step with the user's Meshy account.
//!
//! gitwebsite does not talk to Meshy's task API itself. The models repository has its own sync
//! tool (`<repo>/tools/sync`): it lists the Meshy tasks, downloads new models while Meshy's
//! signed URLs are live (free, about three days), recovers expired ones only when asked
//! (`--recover --max-credits N`, 5 credits each, never below a minimum balance), lays out build
//! plates, renders images, regenerates the catalogue and commits. This module drives that tool:
//!
//! - `gw meshy status | sync | models | config` (the CLI),
//! - a scheduler inside `gw serve` and the desktop app ([`start_scheduler`]): every
//!   `interval_hours` (default 24) a sync *without* `--recover` — free downloads only,
//! - the REST routes under `/api/meshy` and the MCP tools `meshy_status`, `meshy_sync`,
//!   `meshy_models`, which call the functions here.
//!
//! Runs never overlap, across processes: a lock file holds the running `gw` process's id.
//! Each run's output goes to `<config dir>/meshy/logs/<run>.log`; the last [`KEEP_RUNS`] results
//! are kept in `<config dir>/meshy/history.json`. When new models arrive the user gets a macOS
//! notification and the app shows them.
//!
//! The Meshy API key lives in the Keychain (service `gitwebsite-meshy`, account `api-key`). It is
//! only read here to ask Meshy for the credit balance and is never logged or returned.

use super::keychain;
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration as CDuration, Utc};
use gw_core::userconfig::{config_dir, MeshyConfig, UserConfig};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

pub const KEYCHAIN_SERVICE: &str = "gitwebsite-meshy";
const BALANCE_URL: &str = "https://api.meshy.ai/openapi/v1/balance";
/// Credits Meshy charges to recover (remesh) one expired model.
pub const REMESH_COST: u64 = 5;
/// Results (and their logs) kept in history.json.
pub const KEEP_RUNS: usize = 20;
/// After a failed run, try again this soon (or at the interval, if that is shorter).
const RETRY_AFTER_FAILURE_HOURS: i64 = 1;
/// The first scheduled check after `gw serve` starts.
const STARTUP_DELAY_SECS: u64 = 90;
/// How often the scheduler looks at the clock.
const TICK_SECS: u64 = 300;

// ------------------------------------------------------------------ context

/// Where things are: the models repository and gitwebsite's own Meshy folder.
#[derive(Debug, Clone)]
pub struct Ctx {
    pub cfg: MeshyConfig,
    pub repo: Option<PathBuf>,
    /// `<config dir>/meshy`: history.json, logs/, the lock.
    pub data: PathBuf,
    /// Show a macOS notification when new models arrive.
    pub notify: bool,
}

impl Ctx {
    pub fn load() -> Result<Ctx> {
        let cfg = UserConfig::load()?.meshy();
        let repo = cfg.repo_path();
        Ok(Ctx { cfg, repo, data: config_dir().join("meshy"), notify: cfg!(target_os = "macos") })
    }
    fn history_path(&self) -> PathBuf {
        self.data.join("history.json")
    }
    fn logs(&self) -> PathBuf {
        self.data.join("logs")
    }
    fn lock_path(&self) -> PathBuf {
        self.data.join("sync.lock")
    }
    fn seen_path(&self) -> PathBuf {
        self.data.join("seen.json")
    }
    /// The repository, checked: it must exist and hold `tools/sync`.
    pub fn repo_checked(&self) -> Result<PathBuf> {
        let r = self.repo.clone().context("no Meshy models repository: set one with `gw meshy config --repo <path>`")?;
        if !r.join("tools/sync").exists() {
            bail!("{} has no tools/sync — is it the Meshy models repository? (`gw meshy config --repo <path>`)", r.display());
        }
        Ok(r)
    }
}

// ------------------------------------------------------------------ the sync tool's summary

/// A model as the sync tool names it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ModelRef {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub project: String,
    #[serde(default)]
    pub slug: String,
    #[serde(default)]
    pub name: String,
    /// Expired models: no paid remesh exists yet, so recovering costs [`REMESH_COST`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_remesh: Option<bool>,
}

/// What a run found and did: `tools/sync --json-summary` (version 1), or — with an older sync
/// tool — what could be read from its log ([`parse_log`]).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Summary {
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub recover: bool,
    #[serde(default)]
    pub tasks: u64,
    #[serde(default)]
    pub models: u64,
    #[serde(default)]
    pub in_repo: u64,
    /// New models that could be downloaded free when the run started.
    #[serde(default)]
    pub available_free: u64,
    /// Models not in the repository whose Meshy download links have expired.
    #[serde(default)]
    pub expired: u64,
    /// Of those, the ones that need a paid remesh (the rest have one already and resume free).
    #[serde(default)]
    pub needs_remesh: u64,
    #[serde(default)]
    pub recover_cost: u64,
    #[serde(default)]
    pub balance_before: Option<u64>,
    #[serde(default)]
    pub available_models: Vec<ModelRef>,
    /// Expired models still missing after the run.
    #[serde(default)]
    pub expired_models: Vec<ModelRef>,
    /// Models added by this run.
    #[serde(default)]
    pub new: u64,
    #[serde(default)]
    pub new_models: Vec<ModelRef>,
    #[serde(default)]
    pub failures: Vec<String>,
    #[serde(default)]
    pub credits_spent: u64,
    #[serde(default)]
    pub committed: Option<String>,
    /// "json" (from --json-summary) or "log" (read from the output).
    #[serde(default)]
    pub source: String,
}

pub fn parse_summary_json(text: &str) -> Result<Summary> {
    let mut s: Summary = serde_json::from_str(text).context("the sync summary is not valid JSON")?;
    s.source = "json".into();
    Ok(s)
}

/// Read what an older sync tool (without `--json-summary`) printed:
/// `206 tasks, 102 models; 102 already in repo; 0 downloadable free; 0 expired (0 need a remesh
/// = 0 credits); balance 334`, and `committed: Sync: 3 new models (…)`.
pub fn parse_log(log: &str) -> Option<Summary> {
    let re = regex::Regex::new(
        r"(\d+) tasks, (\d+) models; (\d+) already in repo; (\d+) downloadable free; (\d+) expired \((\d+) need a remesh = (\d+) credits\); balance (\d+)",
    )
    .unwrap();
    let c = re.captures_iter(log).last()?;
    let n = |i: usize| c[i].parse::<u64>().unwrap_or(0);
    let mut s = Summary {
        tasks: n(1), models: n(2), in_repo: n(3), available_free: n(4), expired: n(5), needs_remesh: n(6),
        recover_cost: n(7), balance_before: Some(n(8)), source: "log".into(), ..Default::default()
    };
    let commit = regex::Regex::new(r"committed: (Sync: (\d+) new models?.*)").unwrap();
    if let Some(c) = commit.captures_iter(log).last() {
        s.committed = Some(c[1].trim().to_string());
        s.new = c[2].parse().unwrap_or(0);
    }
    if let Some(i) = log.find("FAILURES:") {
        s.failures = log[i + 9..].lines().skip(1).map(str::trim).take_while(|l| !l.is_empty() && !l.chars().next().is_some_and(|c| c.is_ascii_digit())).map(str::to_string).collect();
    }
    Some(s)
}

// ------------------------------------------------------------------ history

/// One sync run, kept in history.json.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct RunRecord {
    /// Also the log's file name (`logs/<id>.log`).
    pub id: String,
    pub started: DateTime<Utc>,
    #[serde(default)]
    pub finished: Option<DateTime<Utc>>,
    /// schedule | app | cli | mcp
    pub trigger: String,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub recover: bool,
    #[serde(default)]
    pub max_credits: u64,
    pub ok: bool,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub summary: Option<Summary>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct History {
    #[serde(default)]
    pub runs: Vec<RunRecord>,
}

impl History {
    pub fn load(ctx: &Ctx) -> History {
        std::fs::read_to_string(ctx.history_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }
    fn save(&self, ctx: &Ctx) -> Result<()> {
        std::fs::create_dir_all(&ctx.data)?;
        let p = ctx.history_path();
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, &p)?;
        Ok(())
    }
    /// Add a run (newest last), keep the last [`KEEP_RUNS`], and delete the logs of the rest.
    fn push(&mut self, ctx: &Ctx, r: RunRecord) {
        self.runs.push(r);
        while self.runs.len() > KEEP_RUNS {
            let old = self.runs.remove(0);
            let _ = std::fs::remove_file(ctx.logs().join(format!("{}.log", old.id)));
        }
    }
    /// The last run that actually synced (dry runs only look).
    pub fn last_real(&self) -> Option<&RunRecord> {
        self.runs.iter().rev().find(|r| !r.dry_run)
    }
    pub fn last(&self) -> Option<&RunRecord> {
        self.runs.last()
    }
}

// ------------------------------------------------------------------ scheduling

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// Scheduled syncs are turned off (or there is no repository).
    Disabled,
    /// A sync is running now.
    Busy,
    /// Start one now.
    Run,
    /// Next one due then.
    Wait(DateTime<Utc>),
}

/// When the next scheduled sync is due: `interval` after the last real run (manual ones count),
/// or [`RETRY_AFTER_FAILURE_HOURS`] after a failed one; at once when there has never been one.
pub fn next_due(last: Option<&RunRecord>, interval_hours: u32) -> Option<DateTime<Utc>> {
    let r = last?;
    let at = r.finished.unwrap_or(r.started);
    let interval = CDuration::hours(interval_hours.max(1) as i64);
    let gap = if r.ok { interval } else { interval.min(CDuration::hours(RETRY_AFTER_FAILURE_HOURS)) };
    Some(at + gap)
}

/// Should the scheduler start a sync at `now`?
pub fn decide(now: DateTime<Utc>, enabled: bool, running: bool, last: Option<&RunRecord>, interval_hours: u32) -> Decision {
    if !enabled {
        return Decision::Disabled;
    }
    if running {
        return Decision::Busy;
    }
    match next_due(last, interval_hours) {
        Some(due) if due > now => Decision::Wait(due),
        _ => Decision::Run,
    }
}

// ------------------------------------------------------------------ the lock (no overlapping runs)

fn pid_alive(pid: u32) -> bool {
    if pid == std::process::id() {
        return true;
    }
    #[cfg(unix)]
    {
        Command::new("/bin/kill").args(["-0", &pid.to_string()]).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

/// Held while a sync runs; removing the file releases it.
pub struct Lock {
    path: PathBuf,
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// In this process, one run at a time (the lock file is shared with other `gw` processes).
static IN_PROCESS: Mutex<()> = Mutex::new(());

/// What the lock file says: `{pid, run, started}` of the run in progress, if its process lives.
pub fn running(ctx: &Ctx) -> Option<Value> {
    let t = std::fs::read_to_string(ctx.lock_path()).ok()?;
    let v: Value = serde_json::from_str(&t).ok()?;
    let pid = v["pid"].as_u64()? as u32;
    pid_alive(pid).then_some(v)
}

/// Take the lock for run `id`, or fail if another run (in any `gw` process) holds it. A lock
/// left by a process that has died is taken over.
pub fn acquire(ctx: &Ctx, id: &str) -> Result<Lock> {
    std::fs::create_dir_all(&ctx.data)?;
    let path = ctx.lock_path();
    let body = json!({ "pid": std::process::id(), "run": id, "started": Utc::now() }).to_string();
    for _ in 0..2 {
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                f.write_all(body.as_bytes())?;
                return Ok(Lock { path });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if let Some(v) = running(ctx) {
                    bail!("a Meshy sync is already running (run {}, since {})", v["run"].as_str().unwrap_or("?"), v["started"].as_str().unwrap_or("?"));
                }
                // Unreadable, or its process is gone: stale.
                let _ = std::fs::remove_file(&path);
            }
            Err(e) => return Err(e.into()),
        }
    }
    bail!("could not take the Meshy sync lock {}", path.display())
}

// ------------------------------------------------------------------ running a sync

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct SyncOptions {
    /// Pay to recover expired models (needs `max_credits`).
    #[serde(default)]
    pub recover: bool,
    #[serde(default)]
    pub max_credits: Option<u64>,
    #[serde(default)]
    pub dry_run: bool,
}

impl SyncOptions {
    pub fn validate(&self) -> Result<()> {
        if self.recover && self.max_credits.unwrap_or(0) == 0 {
            bail!("recovering expired models spends Meshy credits ({REMESH_COST} each): give max_credits, the most this run may spend");
        }
        if !self.recover && self.max_credits.is_some_and(|c| c > 0) {
            bail!("max_credits only applies with recover");
        }
        Ok(())
    }
    pub fn args(&self) -> Vec<String> {
        let mut a = vec![];
        if self.dry_run {
            a.push("--dry-run".into());
        }
        if self.recover {
            a.push("--recover".into());
            a.push("--max-credits".into());
            a.push(self.max_credits.unwrap_or(0).to_string());
        }
        a
    }
}

/// Does this repository's sync tool understand `--json-summary`?
fn supports_json(repo: &Path) -> bool {
    std::fs::read_to_string(repo.join("tools/sync.py")).is_ok_and(|t| t.contains("--json-summary"))
}

/// A new run id (sortable; also the log's name).
pub fn new_run_id(trigger: &str) -> String {
    format!("{}-{}", Utc::now().format("%Y%m%d-%H%M%S"), trigger)
}

/// Run `tools/sync` to completion: stream its output to `on_line` and the run's log, record the
/// result in history.json, and notify about new models. Fails at once if a run is in progress.
pub fn run_sync(ctx: &Ctx, opts: &SyncOptions, trigger: &str, on_line: &mut dyn FnMut(&str)) -> Result<RunRecord> {
    opts.validate()?;
    let repo = ctx.repo_checked()?;
    let _guard = IN_PROCESS.try_lock().map_err(|_| anyhow::anyhow!("a Meshy sync is already running"))?;
    let id = new_run_id(trigger);
    let _lock = acquire(ctx, &id)?;
    std::fs::create_dir_all(ctx.logs())?;
    let log_path = ctx.logs().join(format!("{id}.log"));
    let mut log = std::fs::File::create(&log_path)?;
    let mut rec = RunRecord {
        id: id.clone(), started: Utc::now(), trigger: trigger.into(), dry_run: opts.dry_run, recover: opts.recover,
        max_credits: opts.max_credits.unwrap_or(0), ..Default::default()
    };
    let mut args = opts.args();
    let summary_path = ctx.data.join(format!("{id}.summary.json"));
    let json = supports_json(&repo);
    if json {
        args.push("--json-summary".into());
        args.push(summary_path.display().to_string());
    }
    let head = format!("# gw meshy sync ({trigger}) {} — tools/sync {}\n", rec.started.to_rfc3339(), opts.args().join(" "));
    log.write_all(head.as_bytes())?;
    on_line(head.trim_end());

    // One pipe for stdout and stderr, in order.
    let path = format!("/opt/homebrew/bin:/usr/local/bin:{}", std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin:/usr/sbin:/sbin".into()));
    let spawned = Command::new("/bin/sh")
        .arg("-c")
        .arg("exec \"$0\" \"$@\" 2>&1")
        .arg(repo.join("tools/sync"))
        .args(&args)
        .current_dir(&repo)
        .env("PATH", path)
        .env("PYTHONUNBUFFERED", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn();
    let mut text = String::new();
    match spawned {
        Err(e) => {
            rec.error = Some(format!("could not start tools/sync: {e}"));
        }
        Ok(mut child) => {
            let out = child.stdout.take().unwrap();
            for line in BufReader::new(out).lines() {
                let Ok(line) = line else { break };
                let _ = writeln!(log, "{line}");
                let _ = log.flush();
                on_line(&line);
                text.push_str(&line);
                text.push('\n');
            }
            let st = child.wait()?;
            rec.exit_code = st.code();
            rec.ok = st.success();
            if !st.success() {
                rec.error = Some(match st.code() {
                    Some(1) => "the sync finished with failures (see the log)".into(),
                    Some(c) => format!("tools/sync exited with status {c}"),
                    None => "tools/sync was killed".into(),
                });
            }
        }
    }
    rec.finished = Some(Utc::now());
    rec.summary = if json {
        std::fs::read_to_string(&summary_path).ok().and_then(|t| parse_summary_json(&t).ok()).or_else(|| parse_log(&text))
    } else {
        parse_log(&text)
    };
    let _ = std::fs::remove_file(&summary_path);
    let tail = format!("# finished {} — {}\n", rec.finished.unwrap().to_rfc3339(), if rec.ok { "ok" } else { rec.error.as_deref().unwrap_or("failed") });
    let _ = log.write_all(tail.as_bytes());
    on_line(tail.trim_end());

    let mut h = History::load(ctx);
    h.push(ctx, rec.clone());
    h.save(ctx)?;
    if ctx.notify && !rec.dry_run {
        if let Some(s) = &rec.summary {
            if s.new > 0 {
                notify_new(s);
            }
        }
    }
    Ok(rec)
}

/// Start a sync on a background thread (the app, the MCP server). Fails at once when one is
/// running; otherwise returns the run id to follow with [`log_since`].
pub fn spawn_sync(ctx: Ctx, opts: SyncOptions, trigger: &str) -> Result<Value> {
    opts.validate()?;
    ctx.repo_checked()?;
    if let Some(v) = running(&ctx) {
        bail!("conflict: a Meshy sync is already running (run {})", v["run"].as_str().unwrap_or("?"));
    }
    let data_ctx = ctx.clone();
    let trigger = trigger.to_string();
    let (tx, rx) = std::sync::mpsc::channel::<Result<String, String>>();
    std::thread::spawn(move || {
        let mut sent = false;
        let tx2 = tx.clone();
        let mut first = |_: &str| {
            if !sent {
                sent = true;
                // The lock is held by now; the newest log is ours.
                let _ = tx2.send(Ok(String::new()));
            }
        };
        if let Err(e) = run_sync(&ctx, &opts, &trigger, &mut first) {
            let _ = tx.send(Err(format!("{e:#}")));
            eprintln!("meshy: sync failed to run: {e:#}");
        }
    });
    match rx.recv_timeout(std::time::Duration::from_secs(10)) {
        Ok(Err(e)) => bail!("{}", if e.contains("already running") { format!("conflict: {e}") } else { e }),
        _ => Ok(json!({ "started": true, "run": running(&data_ctx).map(|v| v["run"].clone()) })),
    }
}

fn notify_new(s: &Summary) {
    let names: Vec<&str> = s.new_models.iter().map(|m| m.name.as_str()).take(4).collect();
    let body = if names.is_empty() {
        format!("{} new model{} in the models repository", s.new, if s.new == 1 { "" } else { "s" })
    } else {
        format!("{}{}", names.join(", "), if s.new as usize > names.len() { format!(" and {} more", s.new as usize - names.len()) } else { String::new() })
    };
    let title = format!("{} new Meshy model{}", s.new, if s.new == 1 { "" } else { "s" });
    let q = |t: &str| t.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!("display notification \"{}\" with title \"{}\" subtitle \"gitwebsite\"", q(&body), q(&title));
    let _ = Command::new("/usr/bin/osascript").arg("-e").arg(script).stdout(Stdio::null()).stderr(Stdio::null()).status();
}

// ------------------------------------------------------------------ the scheduler (gw serve)

/// Start the scheduled sync in `gw serve` / the app: a first look shortly after start, then every
/// few minutes; a sync runs when [`decide`] says so. Never `--recover`.
pub fn start_scheduler() {
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(STARTUP_DELAY_SECS));
        loop {
            if let Ok(ctx) = Ctx::load() {
                let enabled = ctx.cfg.enabled && ctx.repo_checked().is_ok();
                let h = History::load(&ctx);
                match decide(Utc::now(), enabled, running(&ctx).is_some(), h.last_real(), ctx.cfg.interval_hours) {
                    Decision::Run => {
                        eprintln!("meshy: scheduled sync starting");
                        match run_sync(&ctx, &SyncOptions::default(), "schedule", &mut |_| {}) {
                            Ok(r) => eprintln!("meshy: scheduled sync {} ({} new)", if r.ok { "ok" } else { "failed" }, r.summary.map(|s| s.new).unwrap_or(0)),
                            Err(e) => eprintln!("meshy: scheduled sync not run: {e:#}"),
                        }
                    }
                    Decision::Busy | Decision::Disabled | Decision::Wait(_) => {}
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(TICK_SECS));
        }
    });
}

// ------------------------------------------------------------------ balance

static BALANCE: Mutex<Option<(std::time::Instant, u64)>> = Mutex::new(None);

/// The account's credit balance (`GET /openapi/v1/balance`), cached for a minute.
pub fn balance() -> Result<u64> {
    if let Some((at, b)) = *BALANCE.lock().unwrap() {
        if at.elapsed().as_secs() < 60 {
            return Ok(b);
        }
    }
    let key = keychain::get(KEYCHAIN_SERVICE, "api-key")
        .or_else(|| std::env::var("MESHY_API_KEY").ok().filter(|k| !k.is_empty()))
        .context("no Meshy API key: store it with `security add-generic-password -s gitwebsite-meshy -a api-key -w` (it prompts)")?;
    let r = reqwest::blocking::Client::builder()
        .user_agent("gitwebsite/0.1")
        .timeout(std::time::Duration::from_secs(20))
        .build()?
        .get(BALANCE_URL)
        .bearer_auth(&key)
        .send()
        .context("asking Meshy for the balance")?;
    let status = r.status();
    let v: Value = r.json().with_context(|| format!("Meshy balance: not JSON (HTTP {status})"))?;
    if !status.is_success() {
        bail!("Meshy balance: HTTP {status}: {}", v["message"].as_str().unwrap_or(""));
    }
    let b = v["balance"].as_u64().context("Meshy balance: no `balance` in the reply")?;
    *BALANCE.lock().unwrap() = Some((std::time::Instant::now(), b));
    Ok(b)
}

// ------------------------------------------------------------------ status

/// Last sync, next due, balance, counts. `with_balance: false` skips the network call.
pub fn status(ctx: &Ctx, with_balance: bool) -> Result<Value> {
    let h = History::load(ctx);
    let repo_ok = ctx.repo_checked();
    let run = running(ctx);
    let last = h.last();
    let last_real = h.last_real();
    let enabled = ctx.cfg.enabled && repo_ok.is_ok();
    let due = if enabled { Some(next_due(last_real, ctx.cfg.interval_hours).unwrap_or_else(Utc::now)) } else { None };
    // What is still waiting, from the newest summary (a dry run's counts are current; after a real
    // run the free ones it found were downloaded).
    let latest = h.runs.iter().rev().find_map(|r| r.summary.as_ref().map(|s| (r, s)));
    let (available, expired, expired_models, needs) = match latest {
        Some((r, s)) => {
            let avail = if r.dry_run { s.available_free } else { s.available_free.saturating_sub(s.new) };
            let (exp, needs) = if s.source == "json" && !r.dry_run {
                (s.expired_models.len() as u64, s.expired_models.iter().filter(|m| m.needs_remesh != Some(false)).count() as u64)
            } else {
                (s.expired, s.needs_remesh)
            };
            (avail, exp, s.expired_models.clone(), needs)
        }
        None => (0, 0, vec![], 0),
    };
    let models = ctx.repo.as_deref().map(|r| model_dirs(r).len()).unwrap_or(0);
    let (balance, balance_error) = if with_balance {
        match balance() {
            Ok(b) => (Some(b), None),
            Err(e) => (None, Some(format!("{e:#}"))),
        }
    } else {
        (None, None)
    };
    let seen = seen_at(ctx);
    let unseen: Vec<&ModelRef> = h
        .runs
        .iter()
        .filter(|r| !r.dry_run && seen.is_none_or(|s| r.started > s))
        .filter_map(|r| r.summary.as_ref())
        .flat_map(|s| s.new_models.iter())
        .collect();
    let unseen_count: u64 = h.runs.iter().filter(|r| !r.dry_run && seen.is_none_or(|s| r.started > s)).filter_map(|r| r.summary.as_ref()).map(|s| s.new).sum();
    Ok(json!({
        "repo": ctx.repo.as_ref().map(|p| p.display().to_string()),
        "repo_ok": repo_ok.is_ok(),
        "repo_error": repo_ok.err().map(|e| format!("{e:#}")),
        "enabled": ctx.cfg.enabled,
        "interval_hours": ctx.cfg.interval_hours,
        "running": run,
        "last": last,
        "last_sync": last_real,
        "next_due": due,
        "balance": balance,
        "balance_error": balance_error,
        "models": models,
        "new_available": available,
        "expired_unrecovered": expired,
        "expired_needing_remesh": needs,
        "recover_cost": needs * REMESH_COST,
        "remesh_cost": REMESH_COST,
        "expired_models": expired_models,
        "unseen_new": unseen_count,
        "unseen_models": unseen,
        "counts_from": latest.map(|(r, _)| json!({ "run": r.id, "at": r.finished.unwrap_or(r.started), "dry_run": r.dry_run })),
        "data_dir": ctx.data.display().to_string(),
    }))
}

fn seen_at(ctx: &Ctx) -> Option<DateTime<Utc>> {
    let t = std::fs::read_to_string(ctx.seen_path()).ok()?;
    serde_json::from_str::<Value>(&t).ok()?["at"].as_str()?.parse().ok()
}

/// The user has seen the new models (the app's banner is dismissed).
pub fn mark_seen(ctx: &Ctx) -> Result<Value> {
    std::fs::create_dir_all(&ctx.data)?;
    let at = Utc::now();
    std::fs::write(ctx.seen_path(), json!({ "at": at }).to_string())?;
    Ok(json!({ "seen": at }))
}

// ------------------------------------------------------------------ logs

/// A run's log from byte `offset` (default: the running run, else the newest).
pub fn log_since(ctx: &Ctx, run: Option<&str>, offset: u64) -> Result<Value> {
    let h = History::load(ctx);
    let current = running(ctx).and_then(|v| v["run"].as_str().map(str::to_string));
    let id = match run {
        Some(r) => r.to_string(),
        None => match current.clone().or_else(|| h.last().map(|r| r.id.clone())) {
            Some(r) => r,
            None => return Ok(json!({ "run": null, "text": "", "offset": 0, "running": false })),
        },
    };
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        bail!("bad run id");
    }
    let p = ctx.logs().join(format!("{id}.log"));
    let bytes = std::fs::read(&p).unwrap_or_default();
    let from = (offset as usize).min(bytes.len());
    // Only whole lines, so a reader never sees half of one.
    let end = if current.as_deref() == Some(id.as_str()) {
        bytes[from..].iter().rposition(|b| *b == b'\n').map(|i| from + i + 1).unwrap_or(from)
    } else {
        bytes.len()
    };
    Ok(json!({
        "run": id,
        "text": String::from_utf8_lossy(&bytes[from..end]),
        "offset": end,
        "running": current.as_deref() == Some(id.as_str()),
        "record": h.runs.iter().find(|r| r.id == id),
    }))
}

// ------------------------------------------------------------------ models

/// `<project>/<slug>/` folders holding a meshy.json.
fn model_dirs(repo: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    let Ok(rd) = std::fs::read_dir(repo) else { return out };
    let mut projects: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    projects.sort();
    for p in projects {
        let n = p.file_name().unwrap_or_default().to_string_lossy().to_string();
        if n.starts_with('.') || n == "tools" {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&p) else { continue };
        let mut ms: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|m| m.join("meshy.json").is_file()).collect();
        ms.sort();
        out.extend(ms);
    }
    out
}

const IMAGE_ORDER: &[&str] = &["hero", "front", "side", "top", "plate", "preview", "thumb"];

fn model_entry(repo: &Path, dir: &Path, with_readme: bool) -> Option<Value> {
    let m: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("meshy.json")).ok()?).ok()?;
    let rel = |p: &Path| p.strip_prefix(repo).ok().map(|r| r.to_string_lossy().replace('\\', "/"));
    let mut images: Vec<(usize, String, String)> = std::fs::read_dir(dir.join("images"))
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let p = e.path();
                    let ext = p.extension()?.to_str()?.to_ascii_lowercase();
                    if !IMAGE_EXTS.contains(&ext.as_str()) {
                        return None;
                    }
                    let stem = p.file_stem()?.to_string_lossy().to_string();
                    let rank = IMAGE_ORDER.iter().position(|s| stem == *s || stem.starts_with(&format!("{s}-"))).unwrap_or(IMAGE_ORDER.len());
                    Some((rank, stem, rel(&p)?))
                })
                .collect()
        })
        .unwrap_or_default();
    images.sort();
    let pick = |s: &str| images.iter().find(|i| i.1 == s).map(|i| i.2.clone());
    let hero = pick("hero").or_else(|| pick("preview")).or_else(|| images.first().map(|i| i.2.clone()));
    let thumb = pick("thumb").or_else(|| hero.clone());
    let print = &m["print"];
    // Committed print files: <slug>-print.stl (one part) or <slug>-plate*.stl (build plates). The
    // full-resolution <slug>.stl is local-only (git-ignored), like the .glb/.3mf/.usdz/.blend.
    let slug = m["slug"].as_str().unwrap_or("");
    let mut print_files: Vec<String> = print["print_file"].as_str().into_iter().map(str::to_string).collect();
    print_files.extend(print["plate_files"].as_array().into_iter().flatten().filter_map(|f| f.as_str()).filter(|f| f.ends_with(".stl")).map(str::to_string));
    print_files.retain(|f| dir.join(f).is_file());
    let full_res = format!("{slug}.stl");
    let full_res = (!slug.is_empty() && dir.join(&full_res).is_file()).then_some(full_res);
    let description = m["description"].as_str().unwrap_or("");
    let mut v = json!({
        "project": m["project"], "slug": m["slug"], "name": m["name"],
        "summary": m["summary"], "description": description,
        "first_line": description.lines().next().unwrap_or("").trim(),
        "created": m["meshy"]["created"], "type": m["meshy"]["type"],
        "triangles": m["mesh"]["triangles"],
        "dimensions_mm": print["dimensions_mm"],
        "parts": print["parts"], "multi_part": print["kind"] == "multi",
        "plates": print["plates"], "textured": m["textured"],
        "watertight": m["mesh"]["watertight"],
        "source": m["recovery"]["source"],
        "print_files": print_files, "full_res_stl": full_res,
        "dir": rel(dir), "path": dir.display().to_string(),
        "hero": hero, "thumb": thumb,
        "images": images.iter().map(|i| json!({ "name": i.1, "path": i.2 })).collect::<Vec<_>>(),
    });
    if with_readme {
        v["readme"] = json!(std::fs::read_to_string(dir.join("README.md")).unwrap_or_default());
        v["prompts"] = json!({ "object": m["meshy"]["object_prompt"], "texture": m["meshy"]["texture_prompt"], "style": m["meshy"]["style_prompt"] });
        v["files"] = json!(std::fs::read_dir(dir).map(|rd| {
            let mut f: Vec<Value> = rd.flatten().filter(|e| e.path().is_file()).map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                json!({ "name": name, "bytes": e.metadata().map(|m| m.len()).unwrap_or(0), "in_git": in_git(&name) })
            }).collect();
            f.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
            f
        }).unwrap_or_default());
    }
    Some(v)
}

/// Is a model-folder file versioned? Print files, README and meta are; the full-resolution
/// `<slug>.stl` and Meshy's other formats are kept locally only (the repository's .gitignore).
pub fn in_git(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    if n.ends_with(".stl") {
        return n.ends_with("-print.stl") || n.rsplit_once("-plate").is_some_and(|(_, rest)| rest.ends_with(".stl"));
    }
    !matches!(n.rsplit('.').next(), Some("glb" | "3mf" | "usdz" | "blend" | "blend1" | "part" | "tmp"))
}

/// Every model in the repository, with the projects (and their descriptions from projects.yaml).
pub fn models(ctx: &Ctx, project: Option<&str>) -> Result<Value> {
    let repo = ctx.repo.clone().context("no Meshy models repository: `gw meshy config --repo <path>`")?;
    let all: Vec<Value> = model_dirs(&repo).iter().filter_map(|d| model_entry(&repo, d, false)).collect();
    let descr: serde_yaml::Value = std::fs::read_to_string(repo.join("projects.yaml")).ok().and_then(|t| serde_yaml::from_str(&t).ok()).unwrap_or_default();
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    for m in &all {
        *counts.entry(m["project"].as_str().unwrap_or("").to_string()).or_default() += 1;
    }
    let projects: Vec<Value> = counts
        .iter()
        .map(|(p, n)| json!({ "id": p, "count": n, "description": descr["projects"][p.as_str()].as_str().unwrap_or("") }))
        .collect();
    let shown: Vec<Value> = all.into_iter().filter(|m| project.is_none_or(|p| m["project"] == p)).collect();
    Ok(json!({ "repo": repo.display().to_string(), "projects": projects, "count": shown.len(), "models": shown }))
}

pub(crate) fn safe_segment(s: &str) -> bool {
    !s.is_empty() && s != "." && s != ".." && !s.starts_with('.') && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// One model with its README, prompts and file list.
pub fn model(ctx: &Ctx, project: &str, slug: &str) -> Result<Value> {
    let repo = ctx.repo.clone().context("no Meshy models repository")?;
    if !safe_segment(project) || !safe_segment(slug) {
        bail!("bad project or slug");
    }
    let dir = repo.join(project).join(slug);
    model_entry(&repo, &dir, true).with_context(|| format!("no model {project}/{slug}"))
}

/// Show a model's folder in the Finder.
pub fn reveal(ctx: &Ctx, project: &str, slug: &str) -> Result<Value> {
    let repo = ctx.repo.clone().context("no Meshy models repository")?;
    if !safe_segment(project) || !safe_segment(slug) {
        bail!("bad project or slug");
    }
    let dir = repo.join(project).join(slug);
    if !dir.join("meshy.json").is_file() {
        bail!("no model {project}/{slug}");
    }
    let target = std::fs::read_dir(&dir).ok().and_then(|rd| rd.flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|e| e == "stl"))).unwrap_or(dir.clone());
    if cfg!(target_os = "macos") {
        Command::new("/usr/bin/open").arg("-R").arg(&target).status().context("running open -R")?;
    } else {
        open::that_detached(&dir).context("opening the folder")?;
    }
    Ok(json!({ "revealed": target.display().to_string() }))
}

/// Image types `GET /api/meshy/file` serves.
pub const IMAGE_EXTS: &[&str] = &["webp", "png", "jpg", "jpeg", "gif"];

/// Resolve `rel` (as in a model's `images` list) to a file inside `repo`, or `None` when it is
/// absolute, climbs out (`..`), goes through a dot folder (`.git`), is not an image, or — after
/// following symlinks — lies outside the repository.
pub fn safe_file(repo: &Path, rel: &str) -> Option<PathBuf> {
    if rel.is_empty() || rel.contains('\0') || rel.contains('\\') {
        return None;
    }
    let p = Path::new(rel);
    for c in p.components() {
        match c {
            Component::Normal(s) if !s.to_string_lossy().starts_with('.') => {}
            _ => return None,
        }
    }
    let ext = p.extension()?.to_str()?.to_ascii_lowercase();
    if !IMAGE_EXTS.contains(&ext.as_str()) {
        return None;
    }
    let root = repo.canonicalize().ok()?;
    let full = root.join(p).canonicalize().ok()?;
    (full.starts_with(&root) && full.is_file()).then_some(full)
}

// ------------------------------------------------------------------ config

pub fn set_config(repo: Option<&Path>, interval: Option<u32>, enable: Option<bool>) -> Result<Value> {
    let mut uc = UserConfig::load()?;
    let mut m = uc.meshy();
    if let Some(r) = repo {
        let r = gw_core::userconfig::norm(&r.to_string_lossy());
        if !Path::new(&r).join("tools/sync").exists() {
            bail!("{r} has no tools/sync — is it the Meshy models repository?");
        }
        m.repo = Some(r);
    }
    if let Some(i) = interval {
        if i == 0 {
            bail!("--interval is in hours and must be at least 1");
        }
        m.interval_hours = i;
    }
    if let Some(e) = enable {
        m.enabled = e;
    }
    uc.meshy = Some(m);
    uc.save()?;
    config_value()
}

pub fn config_value() -> Result<Value> {
    let ctx = Ctx::load()?;
    Ok(json!({
        "repo": ctx.repo.as_ref().map(|p| p.display().to_string()),
        "repo_set": ctx.cfg.repo.is_some(),
        "repo_ok": ctx.repo_checked().is_ok(),
        "interval_hours": ctx.cfg.interval_hours,
        "enabled": ctx.cfg.enabled,
        "data_dir": ctx.data.display().to_string(),
        "config": gw_core::userconfig::config_path().display().to_string(),
    }))
}

// ------------------------------------------------------------------ CLI

pub fn run_cli(c: super::MeshyCmd, json_out: bool) -> Result<Value> {
    use super::MeshyCmd::*;
    match c {
        Config { repo, interval, enable, disable } => {
            let en = if enable { Some(true) } else if disable { Some(false) } else { None };
            if repo.is_none() && interval.is_none() && en.is_none() {
                config_value()
            } else {
                set_config(repo.as_deref(), interval, en)
            }
        }
        Status => {
            let ctx = Ctx::load()?;
            let v = status(&ctx, true)?;
            if !json_out {
                print_status(&v);
                return Ok(Value::Null);
            }
            Ok(v)
        }
        Models { project } => {
            let ctx = Ctx::load()?;
            let v = models(&ctx, project.as_deref())?;
            if !json_out {
                for m in v["models"].as_array().into_iter().flatten() {
                    let dims = m["dimensions_mm"].as_array().map(|d| d.iter().map(|x| format!("{:.0}", x.as_f64().unwrap_or(0.0))).collect::<Vec<_>>().join("×")).unwrap_or_default();
                    let parts = if m["multi_part"] == true { format!("{} parts", m["parts"]) } else { String::new() };
                    println!("{:<52} {:>11} tris  {:>12} mm  {:<9} {}", m["dir"].as_str().unwrap_or(""), m["triangles"].as_u64().map(|t| t.to_string()).unwrap_or_default(), dims, parts, m["name"].as_str().unwrap_or(""));
                }
                println!("{} models", v["count"]);
                return Ok(Value::Null);
            }
            Ok(v)
        }
        Sync { recover, max_credits, dry_run } => {
            let ctx = Ctx::load()?;
            let opts = SyncOptions { recover, max_credits, dry_run };
            let r = run_sync(&ctx, &opts, "cli", &mut |l| if !json_out { println!("{l}") })?;
            if !json_out {
                if let Some(s) = &r.summary {
                    println!(
                        "\n{}: {} new, {} free downloads were available, {} expired ({} need a remesh = {} credits), {} credits spent",
                        if r.ok { "Done" } else { "Finished with problems" },
                        s.new, s.available_free, s.expired, s.needs_remesh, s.needs_remesh * REMESH_COST, s.credits_spent
                    );
                }
                if !r.ok {
                    bail!("{}", r.error.unwrap_or_else(|| "the sync failed".into()));
                }
                return Ok(Value::Null);
            }
            Ok(serde_json::to_value(r)?)
        }
    }
}

fn ago(t: &DateTime<Utc>) -> String {
    let local = t.with_timezone(&chrono::Local);
    let d = Utc::now().signed_duration_since(*t);
    let rel = if d.num_seconds() < 0 {
        let m = -d.num_minutes();
        if m < 90 { format!("in {m} min") } else { format!("in {} h", (m + 30) / 60) }
    } else if d.num_minutes() < 90 {
        format!("{} min ago", d.num_minutes())
    } else if d.num_hours() < 48 {
        format!("{} h ago", d.num_hours())
    } else {
        format!("{} days ago", d.num_days())
    };
    format!("{} ({rel})", local.format("%Y-%m-%d %H:%M"))
}

fn print_status(v: &Value) {
    println!("Models repository: {}{}", v["repo"].as_str().unwrap_or("(none)"), if v["repo_ok"] == true { "" } else { "  — not usable" });
    if let Some(e) = v["repo_error"].as_str() {
        println!("  {e}");
    }
    let t = |k: &str| v[k].as_str().and_then(|s| s.parse::<DateTime<Utc>>().ok());
    match v["last_sync"].as_object() {
        Some(l) => {
            let at = l.get("finished").and_then(|f| f.as_str()).and_then(|s| s.parse::<DateTime<Utc>>().ok());
            let res = if l.get("ok") == Some(&Value::Bool(true)) { "ok".to_string() } else { l.get("error").and_then(|e| e.as_str()).unwrap_or("failed").to_string() };
            let new = l.get("summary").and_then(|s| s.get("new")).and_then(|n| n.as_u64()).unwrap_or(0);
            println!("Last sync:         {} — {res}, {new} new ({})", at.map(|a| ago(&a)).unwrap_or_default(), l.get("trigger").and_then(|x| x.as_str()).unwrap_or(""));
        }
        None => println!("Last sync:         never"),
    }
    if let Some(r) = v["running"].as_object() {
        println!("Running now:       run {}", r.get("run").and_then(|x| x.as_str()).unwrap_or(""));
    }
    if v["enabled"] == true {
        let due = t("next_due").map(|a| if a <= Utc::now() { "due now".to_string() } else { ago(&a) }).unwrap_or_else(|| "—".into());
        println!("Next scheduled:    {due} (every {} h, while `gw serve` or the app runs)", v["interval_hours"]);
    } else {
        println!("Scheduled sync:    off (`gw meshy config --enable`)");
    }
    match v["balance"].as_u64() {
        Some(b) => println!("Credit balance:    {b}"),
        None => println!("Credit balance:    unknown ({})", v["balance_error"].as_str().unwrap_or("")),
    }
    println!("Models:            {}", v["models"]);
    if v["counts_from"].is_null() {
        println!("(no sync recorded yet: run `gw meshy sync --dry-run` to see what is waiting on Meshy)");
    }
    println!("New available:     {} (free; the next sync downloads them)", v["new_available"]);
    println!(
        "Expired:           {} not recovered ({} need a remesh = {} credits; `gw meshy sync --recover --max-credits N`)",
        v["expired_unrecovered"], v["expired_needing_remesh"], v["recover_cost"]
    );
    if v["unseen_new"].as_u64().unwrap_or(0) > 0 {
        println!("Arrived recently:  {} new model(s)", v["unseen_new"]);
    }
}

// ------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("gw-meshy-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn rec(ok: bool, finished: DateTime<Utc>) -> RunRecord {
        RunRecord { id: "r".into(), started: finished - CDuration::minutes(3), finished: Some(finished), trigger: "schedule".into(), ok, ..Default::default() }
    }

    #[test]
    fn scheduling_decisions() {
        let now: DateTime<Utc> = "2026-10-17T12:00:00Z".parse().unwrap();
        // Never synced: run now.
        assert_eq!(decide(now, true, false, None, 24), Decision::Run);
        // Disabled or running wins over everything.
        assert_eq!(decide(now, false, false, None, 24), Decision::Disabled);
        assert_eq!(decide(now, true, true, None, 24), Decision::Busy);
        let old = rec(true, now - CDuration::hours(30));
        assert_eq!(decide(now, true, true, Some(&old), 24), Decision::Busy, "never two at once");
        // Older than the interval: due.
        assert_eq!(decide(now, true, false, Some(&old), 24), Decision::Run);
        // Recent and ok: wait for the interval.
        let recent = rec(true, now - CDuration::hours(2));
        assert_eq!(decide(now, true, false, Some(&recent), 24), Decision::Wait(now + CDuration::hours(22)));
        assert_eq!(decide(now, true, false, Some(&recent), 1), Decision::Run);
        // Exactly at the interval: due.
        let edge = rec(true, now - CDuration::hours(24));
        assert_eq!(decide(now, true, false, Some(&edge), 24), Decision::Run);
        // Failed: retry after an hour, not a day.
        let failed = rec(false, now - CDuration::minutes(20));
        assert_eq!(decide(now, true, false, Some(&failed), 24), Decision::Wait(now + CDuration::minutes(40)));
        let failed = rec(false, now - CDuration::minutes(61));
        assert_eq!(decide(now, true, false, Some(&failed), 24), Decision::Run);
        // A zero interval is treated as one hour.
        assert_eq!(next_due(Some(&recent), 0), Some(now - CDuration::hours(1)));
        // Dry runs don't count as a sync.
        let mut h = History::default();
        h.runs.push(rec(true, now - CDuration::hours(30)));
        h.runs.push(RunRecord { dry_run: true, ..rec(true, now - CDuration::minutes(5)) });
        assert_eq!(h.last_real().unwrap().finished, Some(now - CDuration::hours(30)));
        assert_eq!(decide(now, true, false, h.last_real(), 24), Decision::Run);
    }

    #[test]
    fn lock_prevents_overlap_and_recovers_stale() {
        let d = tmp("lock");
        let ctx = Ctx { cfg: MeshyConfig::default(), repo: None, data: d.clone(), notify: false };
        let l = acquire(&ctx, "one").unwrap();
        assert!(running(&ctx).is_some());
        let e = acquire(&ctx, "two").err().unwrap().to_string();
        assert!(e.contains("already running"), "{e}");
        drop(l);
        assert!(running(&ctx).is_none());
        // A lock left by a dead process is taken over.
        std::fs::write(d.join("sync.lock"), json!({ "pid": 999_999_u32, "run": "dead", "started": Utc::now() }).to_string()).unwrap();
        assert!(running(&ctx).is_none());
        let l = acquire(&ctx, "three").unwrap();
        assert_eq!(running(&ctx).unwrap()["run"], "three");
        drop(l);
        // Garbage in the lock file is stale too.
        std::fs::write(d.join("sync.lock"), "not json").unwrap();
        drop(acquire(&ctx, "four").unwrap());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn file_route_is_path_safe() {
        let d = tmp("files");
        let repo = d.join("repo");
        std::fs::create_dir_all(repo.join("amiga/juggler/images")).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join("amiga/juggler/images/hero.webp"), b"RIFF").unwrap();
        std::fs::write(repo.join("amiga/juggler/meshy.json"), b"{}").unwrap();
        std::fs::write(repo.join(".git/x.png"), b"x").unwrap();
        std::fs::write(d.join("secret.png"), b"s").unwrap();
        assert!(safe_file(&repo, "amiga/juggler/images/hero.webp").is_some());
        for bad in [
            "", "../secret.png", "amiga/../../secret.png", "amiga/juggler/../../../secret.png", "/etc/hosts",
            &d.join("secret.png").display().to_string(), ".git/x.png", "amiga/juggler/meshy.json",
            "amiga/juggler/images/nope.webp", "amiga\\..\\..\\secret.png", "./amiga/juggler/images/hero.webp",
            "amiga/juggler/images", "amiga/juggler/images/hero.webp\0.png",
        ] {
            assert!(safe_file(&repo, bad).is_none(), "{bad:?} must be refused");
        }
        // A symlink pointing out of the repository is refused.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(d.join("secret.png"), repo.join("amiga/juggler/images/link.png")).unwrap();
            assert!(safe_file(&repo, "amiga/juggler/images/link.png").is_none());
        }
        assert!(in_git("x-print.stl") && in_git("x-plate.stl") && in_git("x-plate-2.stl") && in_git("README.md") && in_git("meshy.json"));
        assert!(!in_git("x.stl") && !in_git("x.glb") && !in_git("x-plate.3mf") && !in_git("x.blend"));
        assert!(!safe_segment("..") && !safe_segment(".git") && !safe_segment("a/b") && safe_segment("v-series"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn parses_json_summary() {
        let t = r#"{"version": 1, "at": "2026-10-18T04:12:51+00:00", "dry_run": false, "recover": false, "max_credits": 0,
          "tasks": 206, "models": 104, "in_repo": 102, "available_free": 2, "expired": 1, "needs_remesh": 1,
          "remesh_cost": 5, "recover_cost": 5, "balance_before": 334, "projects": {"amiga": 14},
          "available_models": [{"id": "a", "project": "amiga", "slug": "boing", "name": "Boing Ball"}],
          "expired_models": [{"id": "b", "project": "space", "slug": "eagle", "name": "Eagle", "needs_remesh": true}],
          "new": 2, "new_models": [{"id": "a", "project": "amiga", "slug": "boing", "name": "Boing Ball"},
                                   {"id": "c", "project": "amiga", "slug": "kick", "name": "Kickstart"}],
          "failures": [], "credits_spent": 0, "committed": "Sync: 2 new models (Boing Ball, Kickstart)", "extra": 1}"#;
        let s = parse_summary_json(t).unwrap();
        assert_eq!((s.tasks, s.models, s.in_repo, s.available_free, s.expired, s.needs_remesh), (206, 104, 102, 2, 1, 1));
        assert_eq!(s.balance_before, Some(334));
        assert_eq!(s.new, 2);
        assert_eq!(s.new_models[1].name, "Kickstart");
        assert_eq!(s.expired_models[0].needs_remesh, Some(true));
        assert_eq!(s.committed.as_deref(), Some("Sync: 2 new models (Boing Ball, Kickstart)"));
        assert_eq!(s.source, "json");
        // Missing fields default; garbage is an error.
        let s = parse_summary_json(r#"{"version": 1, "dry_run": true, "tasks": 3}"#).unwrap();
        assert!(s.dry_run && s.tasks == 3 && s.new_models.is_empty() && s.balance_before.is_none());
        assert!(parse_summary_json("not json").is_err());
    }

    #[test]
    fn parses_log_fallback() {
        let log = "21:14:02 listing Meshy tasks…\n\
            21:14:05 206 tasks, 104 models; 102 already in repo; 2 downloadable free; 3 expired (1 need a remesh = 5 credits); balance 334\n\
            21:14:05 projects: amiga 14\n\
            21:15:10 FAILURES:\n  eagle: remesh FAILED: boom\n  hawk: download failed: x\n\
            21:16:00 committed: Sync: 2 new models (Boing Ball, Kickstart)\n";
        let s = parse_log(log).unwrap();
        assert_eq!((s.tasks, s.models, s.in_repo, s.available_free, s.expired, s.needs_remesh, s.recover_cost), (206, 104, 102, 2, 3, 1, 5));
        assert_eq!(s.balance_before, Some(334));
        assert_eq!(s.new, 2);
        assert_eq!(s.failures, vec!["eagle: remesh FAILED: boom", "hawk: download failed: x"]);
        assert_eq!(s.source, "log");
        assert!(parse_log("nothing here").is_none());
        let s = parse_log("x 1 tasks, 1 models; 1 already in repo; 0 downloadable free; 0 expired (0 need a remesh = 0 credits); balance 9\nnothing to commit\n").unwrap();
        assert_eq!((s.new, s.committed), (0, None));
    }

    #[test]
    fn sync_options_need_explicit_credits() {
        assert!(SyncOptions { recover: true, max_credits: None, dry_run: false }.validate().is_err());
        assert!(SyncOptions { recover: true, max_credits: Some(0), dry_run: false }.validate().is_err());
        assert!(SyncOptions { recover: false, max_credits: Some(10), dry_run: false }.validate().is_err());
        let o = SyncOptions { recover: true, max_credits: Some(15), dry_run: false };
        o.validate().unwrap();
        assert_eq!(o.args(), vec!["--recover", "--max-credits", "15"]);
        assert_eq!(SyncOptions::default().args(), Vec::<String>::new());
        assert_eq!(SyncOptions { dry_run: true, ..Default::default() }.args(), vec!["--dry-run"]);
    }

    /// A fake models repository whose tools/sync prints a log and writes a JSON summary.
    #[cfg(unix)]
    #[test]
    fn runs_the_sync_tool_and_records_history() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmp("run");
        let repo = d.join("repo");
        std::fs::create_dir_all(repo.join("tools")).unwrap();
        std::fs::create_dir_all(repo.join("amiga/boing/images")).unwrap();
        std::fs::write(repo.join("amiga/boing/meshy.json"), r#"{"name": "Boing Ball", "slug": "boing", "project": "amiga",
            "description": "A red and white ball.\nSecond line.", "meshy": {"created": "2026-10-01T00:00:00+00:00"},
            "mesh": {"triangles": 1200}, "print": {"dimensions_mm": [100, 100, 100], "parts": 1, "kind": "single", "print_file": "boing-print.stl"}}"#).unwrap();
        std::fs::write(repo.join("amiga/boing/boing-print.stl"), b"solid").unwrap();
        std::fs::write(repo.join("amiga/boing/images/hero.webp"), b"RIFF").unwrap();
        std::fs::write(repo.join("amiga/boing/images/front.webp"), b"RIFF").unwrap();
        std::fs::write(repo.join("amiga/boing/README.md"), "# Boing Ball\n").unwrap();
        // The script sees its args; it writes the summary to the --json-summary path (sync.py
        // advertises the flag so gw passes it).
        std::fs::write(repo.join("tools/sync.py"), "# --json-summary\n").unwrap();
        let script = r#"#!/bin/sh
echo "args: $*"
echo "1 tasks, 1 models; 0 already in repo; 1 downloadable free; 0 expired (0 need a remesh = 0 credits); balance 300"
echo "oops" >&2
while [ $# -gt 0 ]; do
  if [ "$1" = "--json-summary" ]; then
    printf '{"version":1,"tasks":1,"models":1,"available_free":1,"new":1,"new_models":[{"id":"x","project":"amiga","slug":"boing","name":"Boing Ball"}],"balance_before":300}' > "$2"
  fi
  shift
done
"#;
        std::fs::write(repo.join("tools/sync"), script).unwrap();
        std::fs::set_permissions(repo.join("tools/sync"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let ctx = Ctx { cfg: MeshyConfig::default(), repo: Some(repo.clone()), data: d.join("data"), notify: false };
        let mut lines = vec![];
        let r = run_sync(&ctx, &SyncOptions::default(), "test", &mut |l| lines.push(l.to_string())).unwrap();
        assert!(r.ok, "{r:?}");
        assert!(lines.iter().any(|l| l == "oops"), "stderr is captured: {lines:?}");
        assert!(lines.iter().any(|l| l.starts_with("args: --json-summary ")), "{lines:?}");
        let s = r.summary.clone().unwrap();
        assert_eq!((s.source.as_str(), s.new), ("json", 1));
        assert!(running(&ctx).is_none(), "lock released");
        let h = History::load(&ctx);
        assert_eq!(h.runs.len(), 1);
        let log = log_since(&ctx, None, 0).unwrap();
        assert!(log["text"].as_str().unwrap().contains("downloadable free"));
        let st = status(&ctx, false).unwrap();
        assert_eq!(st["models"], 1);
        assert_eq!(st["unseen_new"], 1);
        assert_eq!(st["new_available"], 0);
        mark_seen(&ctx).unwrap();
        assert_eq!(status(&ctx, false).unwrap()["unseen_new"], 0);
        let ms = models(&ctx, None).unwrap();
        assert_eq!(ms["models"][0]["first_line"], "A red and white ball.");
        assert_eq!(ms["models"][0]["hero"], "amiga/boing/images/hero.webp");
        assert_eq!(ms["models"][0]["images"][1]["name"], "front");
        assert_eq!(ms["models"][0]["print_files"], json!(["boing-print.stl"]));
        assert_eq!(ms["models"][0]["full_res_stl"], Value::Null, "the local full-resolution STL is optional");
        assert!(model(&ctx, "amiga", "boing").unwrap()["readme"].as_str().unwrap().contains("Boing"));
        assert!(model(&ctx, "..", "boing").is_err());
        // While a run holds the lock, another is refused.
        let l = acquire(&ctx, "other").unwrap();
        assert!(run_sync(&ctx, &SyncOptions::default(), "test", &mut |_| {}).is_err());
        drop(l);
        // History keeps the last KEEP_RUNS and prunes their logs.
        for _ in 0..KEEP_RUNS + 2 {
            let mut h = History::load(&ctx);
            h.push(&ctx, RunRecord { id: format!("x{}", h.runs.len()), started: Utc::now(), trigger: "t".into(), ok: true, ..Default::default() });
            h.save(&ctx).unwrap();
        }
        assert_eq!(History::load(&ctx).runs.len(), KEEP_RUNS);
        assert!(!ctx.logs().join(format!("{}.log", r.id)).exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
