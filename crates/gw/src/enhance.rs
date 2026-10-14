//! Enhancements — reworking an existing post with Claude: operations shared by the CLI
//! (`gw story enhance …`), the REST API, the MCP server, and the app.
//!
//! The workspace and the rules (proposals, accepting, the header merge) are
//! [`gw_core::enhance`]; this layer adds JSON, the context Claude reads (the post, the
//! author's answers, the repository), and starting a Claude session from the app.

use crate::stories::{self, actor, story_json, PREVIEW_BASE};
use anyhow::{bail, Context, Result};
use gw_core::enhance::{self as core, ClaudeRun, KEY_QUESTIONS};
use gw_core::story::{ChangeNote, Story};
use gw_core::Site;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn modify<T>(site: &Site, slug: &str, f: impl FnOnce(&mut Story) -> Result<(T, ChangeNote)>) -> Result<(T, Story)> {
    Story::modify(site, slug, &actor(), f)
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { s.chars().take(n).collect::<String>() + "\n… (truncated)" }
}

/// What an enhancement adds to a story's JSON (the app's story page and `story_show`).
pub fn extend_json(site: &Site, s: &Story, v: &mut Value) {
    let Some(id) = &s.enhances else { return };
    v["is_enhancement"] = json!(true);
    if let Some(p) = s.item_path(site) {
        if let Ok(rel) = p.strip_prefix(site.content_dir()) {
            if let Ok(it) = gw_core::content::load_item(&p, rel) {
                v["enhances_item"] = json!({ "id": it.id, "url": it.url, "title": it.title, "draft": it.draft, "kind": it.kind.as_str(),
                    "path": p.display().to_string(), "preview_url": format!("{PREVIEW_BASE}{}", it.url) });
            }
        }
    }
    if v.get("enhances_item").is_none() {
        v["enhances_item"] = json!({ "id": id, "missing": true });
    }
    v["key_questions"] = json!(KEY_QUESTIONS.iter().map(|(k, q)| json!({ "id": k, "q": q, "a": s.key_answers.get(*k).cloned().unwrap_or_default() })).collect::<Vec<_>>());
    v["has_proposal"] = json!(s.proposal.is_some());
    v["interview_command"] = json!(format!("/enhance-post {}", s.slug));
    v["claude_running"] = json!(running(s));
}

// ------------------------------------------------------------------ starting, context, answers

/// Start (or reopen) the enhancement of a content item (id, slug, or URL).
pub fn start(site: &Site, item: &str) -> Result<Value> {
    let (s, created) = core::create(site, item)?;
    if created {
        s.record_created(site, &actor())?;
    }
    let mut v = story_json(site, &s);
    v["created"] = json!(created);
    Ok(v)
}

/// Save the author's answers to the standard questions (question id → answer).
pub fn key_answers(site: &Site, slug: &str, answers: &BTreeMap<String, String>) -> Result<Value> {
    let (changed, s) = modify(site, slug, |s| {
        if !s.is_enhancement() {
            bail!("story {slug} is not an enhancement");
        }
        let c = core::set_key_answers(site, s, answers)?;
        let what = if c.is_empty() { "saved the key questions (no change)".to_string() } else { format!("answered the key questions: {}", c.join(", ")) };
        Ok((c, ChangeNote::new("answer", what)))
    })?;
    Ok(json!({ "changed": changed, "key_answers": s.key_answers, "rev": s.rev }))
}

/// Point the enhancement at a repository (URL and/or local checkout).
pub fn set_repo(site: &Site, slug: &str, url: &str, path: Option<&str>) -> Result<Value> {
    let (_, s) = modify(site, slug, |s| {
        core::set_repo(s, url, path)?;
        Ok(((), ChangeNote::new("settings", format!("set the repository to {}", s.repo.as_ref().map(|r| r.path.clone().unwrap_or_else(|| r.url.clone())).unwrap_or_else(|| "none".into())))))
    })?;
    Ok(json!({ "repo": s.repo, "rev": s.rev }))
}

fn git(repo: &Path, args: &[&str]) -> Option<String> {
    let o = std::process::Command::new("git").arg("-C").arg(repo).args(args).output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).trim_end().to_string())
}

/// Everything Claude needs to start: the post, the author's answers and interview, the
/// repository (README, recent history, top-level files), and any proposal waiting.
pub fn context(site: &Site, slug: &str) -> Result<Value> {
    let s = Story::load(site, slug)?;
    if !s.is_enhancement() {
        bail!("story {slug} is not an enhancement");
    }
    let path = s.item_path(site).context("this enhancement has lost track of its post")?;
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let mut repo = json!(null);
    if let Some(r) = &s.repo {
        repo = json!({ "url": r.url, "path": r.path });
        if let Some(p) = r.path.as_deref().map(PathBuf::from).filter(|p| p.is_dir()) {
            let readme = ["README.md", "README.adoc", "README", "readme.md", "README.txt"].iter().map(|n| p.join(n)).find(|f| f.is_file());
            repo["readme_path"] = json!(readme.as_ref().map(|f| f.display().to_string()));
            repo["readme"] = json!(readme.and_then(|f| std::fs::read_to_string(f).ok()).map(|t| clip(&t, 12_000)));
            repo["git_log"] = json!(git(&p, &["log", "--oneline", "-40"]));
            repo["first_commit"] = json!(git(&p, &["log", "--reverse", "--format=%ad %s", "--date=short"]).and_then(|l| l.lines().next().map(String::from)));
            let mut files: Vec<String> = std::fs::read_dir(&p)
                .map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().to_string() + if e.path().is_dir() { "/" } else { "" }).filter(|n| !n.starts_with('.')).collect())
                .unwrap_or_default();
            files.sort();
            files.truncate(150);
            repo["top_level"] = json!(files);
        } else {
            repo["note"] = json!("No local checkout found. Ask the author for one, or use the URL.");
        }
    }
    let item = s.enhances.clone().unwrap_or_default();
    Ok(json!({
        "slug": s.slug, "rev": s.rev, "title": s.title, "kind": s.target,
        "item": { "id": item, "path": path.display().to_string(), "text": text },
        "key_questions": KEY_QUESTIONS.iter().map(|(k, q)| json!({ "id": k, "q": q, "a": s.key_answers.get(*k).cloned().unwrap_or_default() })).collect::<Vec<_>>(),
        "narrative": s.narrative, "notes": s.notes,
        "interview": s.interview(site),
        "pending": s.pending,
        "proposal": s.proposal_text(site),
        "repo": repo,
        "images": s.images.iter().filter(|i| i.is_selected()).map(|i| json!({ "id": i.id, "number": i.number, "caption": i.caption, "comment": i.notes })).collect::<Vec<_>>(),
        "how": "Write the full revised post with story_draft: it becomes a proposal the author reviews in the app and accepts or sends back. Keep the post's voice, facts, links, and images unless the author asks; use only the post, the repository, and the author's answers.",
    }))
}

// ------------------------------------------------------------------ proposals

/// Store a revised post as the proposal (story_draft on an enhancement).
pub fn propose(site: &Site, slug: &str, text: &str) -> Result<Value> {
    let by = actor();
    let (p, s) = modify(site, slug, |s| {
        let p = core::propose(site, s, text, &by)?;
        let what = format!("proposed a revision (round {}): “{}”", p.round, p.title);
        Ok((p, ChangeNote::new("draft", what)))
    })?;
    Ok(json!({ "proposal": p, "path": s.proposal_path(site).display().to_string(), "review_in_app": format!("{}{}", stories::APP_BASE, s.slug),
        "note": "Saved as a proposal; the post is unchanged until the author accepts it in the app (or tells you to accept it)." }))
}

/// The proposal next to the current post, with a line diff of what accepting would write.
pub fn proposal(site: &Site, slug: &str) -> Result<Value> {
    let s = Story::load(site, slug)?;
    let text = s.proposal_text(site).context("there is no proposal")?;
    let path = s.item_path(site).context("this enhancement has lost track of its post")?;
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    // Diff what accepting would write (the header merged), not the raw proposal.
    let result = core::merge_header(&current, &text, chrono::Local::now().date_naive());
    let diff = core::diff_lines(&current, &result);
    let added = diff.iter().filter(|d| d.op == '+').count();
    let removed = diff.iter().filter(|d| d.op == '-').count();
    Ok(json!({ "proposal": s.proposal, "text": text, "result": result, "current": current, "diff": diff, "added": added, "removed": removed }))
}

fn rebuild_preview(site: &Site, v: &mut Value) {
    match crate::server::build_preview(site) {
        Ok(b) => v["preview_built"] = json!(b.pages),
        Err(e) => v["preview_error"] = json!(format!("{e:#}")),
    }
}

/// Write the proposal over the post (header merged), then rebuild the preview.
pub fn accept(site: &Site, slug: &str) -> Result<Value> {
    let (r, _) = modify(site, slug, |s| {
        let r = core::accept(site, s)?;
        let what = format!("accepted the proposal into {}", r.url);
        Ok((r, ChangeNote::new("draft", what)))
    })?;
    let mut v = json!(r);
    v["preview_url"] = json!(format!("{PREVIEW_BASE}{}", r.url));
    rebuild_preview(site, &mut v);
    Ok(v)
}

pub fn discard(site: &Site, slug: &str) -> Result<Value> {
    let (_, s) = modify(site, slug, |s| {
        core::discard(site, s)?;
        Ok(((), ChangeNote::new("draft", "discarded the proposal")))
    })?;
    Ok(json!({ "discarded": true, "rev": s.rev }))
}

/// The author's feedback on the proposal; Claude picks it up and revises.
pub fn request_changes(site: &Site, slug: &str, feedback: &str) -> Result<Value> {
    let (n, s) = modify(site, slug, |s| {
        let n = core::request_changes(site, s, feedback)?;
        let one = feedback.split_whitespace().collect::<Vec<_>>().join(" ");
        Ok((n, ChangeNote::new("answer", format!("asked for changes to the proposal: {}", one.chars().take(300).collect::<String>()))))
    })?;
    Ok(json!({ "recorded": n, "status": s.status, "rev": s.rev }))
}

// ------------------------------------------------------------------ Claude, started from the app

/// The tools a Claude session started from the app may use: reading the site and the repo and
/// the story's interview tools — never accept, publish, or edit content directly.
const ALLOWED_TOOLS: &[&str] = &[
    "Skill", "Read", "Grep", "Glob",
    "Bash(git log:*)", "Bash(git show:*)", "Bash(git ls-files:*)", "Bash(git diff:*)", "Bash(git rev-list:*)",
    "Bash(git branch:*)", "Bash(git status:*)", "Bash(ls:*)", "Bash(cd:*)", "Bash(head:*)", "Bash(tail:*)", "Bash(wc:*)",
    "mcp__gitwebsite__site_info", "mcp__gitwebsite__list_areas", "mcp__gitwebsite__list_content", "mcp__gitwebsite__get_content",
    "mcp__gitwebsite__list_media", "mcp__gitwebsite__list_registry",
    "mcp__gitwebsite__story_show", "mcp__gitwebsite__story_context", "mcp__gitwebsite__story_ask", "mcp__gitwebsite__story_wait",
    "mcp__gitwebsite__story_pending", "mcp__gitwebsite__story_changes", "mcp__gitwebsite__story_answer", "mcp__gitwebsite__story_qa",
    "mcp__gitwebsite__story_note", "mcp__gitwebsite__story_outline", "mcp__gitwebsite__story_draft", "mcp__gitwebsite__story_proposal",
    "mcp__gitwebsite__story_review_images",
];

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

fn executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.is_file() && std::fs::metadata(p).map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

/// Directories to look in for programs, beyond PATH (the desktop app starts with a short PATH).
fn extra_dirs() -> Vec<PathBuf> {
    let h = home();
    vec![h.join(".local/bin"), PathBuf::from("/opt/homebrew/bin"), PathBuf::from("/usr/local/bin"), h.join(".claude/local"), h.join(".npm-global/bin"), h.join("bin")]
}

/// `env` if set, then the usual install locations, then PATH — skipping temporary directories
/// (a terminal session's shims there vanish when it ends, while the app keeps running).
fn find_program(name: &str, env: &str) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(env).map(PathBuf::from).filter(|p| executable(p)) {
        return Some(p);
    }
    let tmp = std::env::temp_dir();
    let path = std::env::var_os("PATH").unwrap_or_default();
    extra_dirs()
        .into_iter()
        .chain(std::env::split_paths(&path).filter(|d| !d.starts_with(&tmp) && !d.starts_with("/tmp") && !d.starts_with("/private/var/folders") && !d.starts_with("/var/folders")))
        .map(|d| d.join(name))
        .find(|p| executable(p))
}

/// The `claude` CLI: `GW_CLAUDE_BIN`, the usual install locations, then PATH.
pub fn claude_bin() -> Option<PathBuf> {
    find_program("claude", "GW_CLAUDE_BIN")
}

/// The `gw` binary for the session's MCP server: `GW_BIN`, this checkout's release build,
/// `~/.local/bin/gw`, PATH, or this process if it is `gw`.
fn gw_bin(site: &Site) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("GW_BIN").map(PathBuf::from).filter(|p| executable(p)) {
        return Some(p);
    }
    let local = site.root.parent().map(|r| r.join("target/release/gw")).filter(|p| executable(p));
    local.or_else(|| find_program("gw", "GW_BIN")).or_else(|| std::env::current_exe().ok().filter(|p| p.file_name().map(|n| n == "gw").unwrap_or(false)))
}

fn alive(pid: u32) -> bool {
    std::process::Command::new("kill").args(["-0", &pid.to_string()]).stderr(std::process::Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

fn running(s: &Story) -> bool {
    s.claude.as_ref().map(|c| c.finished.is_none() && alive(c.pid)).unwrap_or(false)
}

fn now() -> chrono::NaiveDateTime {
    use chrono::Timelike;
    let t = chrono::Local::now().naive_local();
    t.with_nanosecond(0).unwrap_or(t)
}

/// The prompt for a session started from the app (there is no one at a terminal).
fn prompt(site: &Site, slug: &str) -> String {
    format!(
        "Use the enhance-post skill to run the gitwebsite enhancement story `{slug}` (site: {}). \
You were started from the gitwebsite app and nobody is watching a terminal: the author answers in the app. \
Ask every question with story_ask and wait for the answer with story_wait (answer_only=true); never ask in plain text. \
Never accept the proposal or publish — the author does that in the app.",
        site.root.display()
    )
}

/// Start Claude on an enhancement, in the background, logging to the story's `claude.log`.
pub fn claude_start(site: &Site, slug: &str) -> Result<Value> {
    use std::os::unix::process::CommandExt;
    let s = Story::load(site, slug)?;
    if !s.is_enhancement() {
        bail!("Claude can be started from the app for enhancements; for a project story run /project-story {slug} in Claude Code");
    }
    if running(&s) {
        bail!("Claude is already working on this story (pid {})", s.claude.as_ref().map(|c| c.pid).unwrap_or(0));
    }
    let claude = claude_bin().context("can't find the claude CLI; install Claude Code or set GW_CLAUDE_BIN")?;
    let gw = gw_bin(site).context("can't find the gw binary for the MCP server; run `make install-gw` or set GW_BIN")?;
    let mcp = json!({ "mcpServers": { "gitwebsite": { "command": gw.display().to_string(), "args": ["--site", site.root.display().to_string(), "mcp"], "env": { "GW_ACTOR": "claude" } } } });
    let mut args: Vec<String> = vec![
        "-p".into(), prompt(site, slug),
        "--output-format".into(), "stream-json".into(), "--verbose".into(),
        "--permission-mode".into(), "dontAsk".into(),
        "--mcp-config".into(), mcp.to_string(), "--strict-mcp-config".into(),
        "--allowedTools".into(), ALLOWED_TOOLS.join(","),
    ];
    if let Some(p) = s.repo.as_ref().and_then(|r| r.path.clone()).filter(|p| Path::new(p).is_dir()) {
        args.push("--add-dir".into());
        args.push(p);
    }
    let cwd = site.root.parent().map(Path::to_path_buf).unwrap_or_else(|| site.root.clone());
    let log_path = s.claude_log_path(site);
    let log = std::fs::File::create(&log_path)?;
    let path_env = {
        let cur = std::env::var_os("PATH").unwrap_or_default();
        let mut dirs: Vec<PathBuf> = std::env::split_paths(&cur).collect();
        for d in extra_dirs().into_iter().chain(["/usr/bin", "/bin", "/usr/sbin", "/sbin"].map(PathBuf::from)) {
            if !dirs.contains(&d) {
                dirs.push(d);
            }
        }
        std::env::join_paths(dirs).unwrap_or(cur)
    };
    let shown = format!("{} -p \"…\" --permission-mode dontAsk --mcp-config gitwebsite{}", claude.display(), s.repo.as_ref().and_then(|r| r.path.as_ref()).map(|p| format!(" --add-dir {p}")).unwrap_or_default());
    let mut child = std::process::Command::new(&claude)
        .args(&args)
        .current_dir(&cwd)
        .env("PATH", path_env)
        .env("GW_ACTOR", "claude")
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .process_group(0)
        .spawn()
        .with_context(|| format!("starting {}", claude.display()))?;
    let pid = child.id();
    let run = ClaudeRun { pid, started: now(), finished: None, exit_code: None, command: shown };
    let (_, s) = modify(site, slug, |s| {
        s.claude = Some(run.clone());
        Ok(((), ChangeNote::new("settings", "started Claude from the app")))
    })?;
    // Record when it ends (while this process lives: the app's server does).
    let (root, slug2) = (site.root.clone(), slug.to_string());
    std::thread::spawn(move || {
        let code = child.wait().ok().and_then(|st| st.code());
        if let Ok(site) = Site::open(Some(&root)) {
            let _ = Story::modify(&site, &slug2, &actor(), |s| {
                if let Some(c) = s.claude.as_mut().filter(|c| c.pid == pid) {
                    c.finished = Some(now());
                    c.exit_code = code;
                }
                Ok(((), ChangeNote::new("settings", format!("Claude finished{}", code.map(|c| format!(" (exit {c})")).unwrap_or_default()))))
            });
        }
    });
    Ok(json!({ "started": true, "pid": pid, "log": log_path.display().to_string(), "claude": s.claude }))
}

/// After the author answers (or asks for changes) in the app: if Claude was started from the app
/// for this story and has since stopped, start it again so it picks up where its notes left
/// off. A session stops after waiting a while with nothing to do; the author may answer much
/// later. Does nothing while Claude is still running, or once nothing is left to answer.
pub fn resume_if_stopped(site: &Site, slug: &str) -> Result<Option<Value>> {
    let s = Story::load(site, slug)?;
    if !s.is_enhancement() || s.claude.is_none() || running(&s) || !s.pending.is_empty() {
        return Ok(None);
    }
    claude_start(site, slug).map(Some)
}

/// Stop the session started from the app.
pub fn claude_stop(site: &Site, slug: &str) -> Result<Value> {
    let s = Story::load(site, slug)?;
    let c = s.claude.clone().context("Claude hasn't been started for this story")?;
    let was = running(&s);
    if was {
        // The whole process group: claude and the MCP server it started.
        let _ = std::process::Command::new("kill").args(["-TERM", &format!("-{}", c.pid)]).status();
        let _ = std::process::Command::new("kill").args(["-TERM", &c.pid.to_string()]).status();
    }
    let (_, s) = modify(site, slug, |s| {
        if let Some(c) = s.claude.as_mut() {
            if c.finished.is_none() {
                c.finished = Some(now());
            }
        }
        Ok(((), ChangeNote::new("settings", "stopped Claude")))
    })?;
    Ok(json!({ "stopped": was, "claude": s.claude }))
}

/// One readable line per event in a stream-json log.
fn readable(line: &str) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return if line.trim().is_empty() { vec![] } else { vec![line.to_string()] };
    };
    let short = |s: &str, n: usize| {
        let one = s.split_whitespace().collect::<Vec<_>>().join(" ");
        if one.chars().count() > n { one.chars().take(n).collect::<String>() + "…" } else { one }
    };
    match v["type"].as_str().unwrap_or("") {
        "system" if v["subtype"] == "init" => vec![format!("● Session started ({})", v["model"].as_str().unwrap_or("Claude"))],
        "assistant" => v["message"]["content"]
            .as_array()
            .map(|cs| {
                cs.iter()
                    .filter_map(|c| match c["type"].as_str() {
                        Some("text") => c["text"].as_str().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()),
                        Some("tool_use") => {
                            let name = c["name"].as_str().unwrap_or("tool").trim_start_matches("mcp__gitwebsite__");
                            let input = &c["input"];
                            let what = ["q", "file_path", "pattern", "command", "skill", "id", "slug"].iter().find_map(|k| input[*k].as_str()).unwrap_or("");
                            Some(format!("→ {name} {}", short(what, 140)))
                        }
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        "result" => vec![format!(
            "● Finished{}{}",
            v["subtype"].as_str().filter(|s| *s != "success").map(|s| format!(" ({s})")).unwrap_or_default(),
            v["total_cost_usd"].as_f64().map(|c| format!(" — ${c:.2}")).unwrap_or_default()
        )],
        _ => vec![],
    }
}

/// Is Claude running, and what has it said? The last `lines` readable lines of the log.
pub fn claude_status(site: &Site, slug: &str, lines: usize) -> Result<Value> {
    let s = Story::load(site, slug)?;
    let log = std::fs::read_to_string(s.claude_log_path(site)).unwrap_or_default();
    let mut out: Vec<String> = log.lines().flat_map(readable).collect();
    if out.len() > lines {
        out.drain(..out.len() - lines);
    }
    Ok(json!({ "running": running(&s), "claude": s.claude, "log": out, "claude_bin": claude_bin().map(|p| p.display().to_string()) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_stream_json_log() {
        let lines = [
            r#"{"type":"system","subtype":"init","model":"claude-opus-5-5"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Reading the post."},{"type":"tool_use","name":"mcp__gitwebsite__story_ask","input":{"slug":"enhance-x","q":"What was the inspiration?"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#,
            r#"{"type":"result","subtype":"success","total_cost_usd":0.4213}"#,
            "plain stderr line",
        ];
        let out: Vec<String> = lines.iter().flat_map(|l| readable(l)).collect();
        assert_eq!(out, vec![
            "● Session started (claude-opus-5-5)".to_string(),
            "Reading the post.".to_string(),
            "→ story_ask What was the inspiration?".to_string(),
            "● Finished — $0.42".to_string(),
            "plain stderr line".to_string(),
        ]);
    }

    #[test]
    fn the_session_cannot_accept_or_publish() {
        for t in ALLOWED_TOOLS {
            assert!(!t.contains("accept") && !t.contains("publish") && !t.contains("update_content") && !t.contains("set_status") && *t != "Edit" && *t != "Write" && *t != "Bash", "{t}");
        }
    }
}
