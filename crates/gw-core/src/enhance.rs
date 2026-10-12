//! Enhancements: reworking an existing post with Claude.
//!
//! An enhancement is a [`Story`] whose `enhances` field names an existing content item. It uses
//! the story machinery (the interview, questions answered in the app or the terminal, the change
//! feed, photos) with three differences:
//!
//! - it starts from the item: title, kind, areas, tags, and project come from it, `draft` points
//!   at its file, and the code repository behind it is detected ([`detect_repo`]);
//! - the author can answer a standard set of questions ([`KEY_QUESTIONS`]) at any time — their
//!   answers land in the interview transcript like any other answer;
//! - a draft is a *proposal* (`proposed.adoc` in the story), never written over the post until the
//!   author accepts it ([`accept`]), which keeps the post's path, `:status:`, `:date:`, and any
//!   header attributes the proposal left out, and stamps `:updated:`.
//!
//! ```text
//! stories/enhance-amiga-devbench/
//!   story.yaml      … enhances: articles/amiga-devbench, repo: {url, path}, key_answers, proposal
//!   interview.md    key-question answers and the follow-up interview
//!   proposed.adoc   the revised post, while one is waiting
//!   claude.log      output of the last Claude session started from the app
//! ```

use crate::config::expand_home;
use crate::content::{self, slugify, Item, Kind};
use crate::story::{self, story_dir, DraftResult, NoNanos, Story};
use crate::Site;
use anyhow::{bail, Context, Result};
use chrono::{Local, NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The standard questions: (id, question). Asked of every enhancement; all optional.
pub const KEY_QUESTIONS: &[(&str, &str)] = &[
    ("inspiration", "What was the inspiration? What got you started on this?"),
    ("why", "What problem does it solve, or why did it need to exist?"),
    ("how", "How does it work — what are the interesting bits worth explaining?"),
    ("hardest", "What was the hardest part, and how did you get past it?"),
    ("learned", "What did you learn, or what would you do differently?"),
    ("next", "What's next for it?"),
    ("audience", "Who is it for — who should read this?"),
];

pub const PROPOSAL_FILE: &str = "proposed.adoc";
pub const CLAUDE_LOG: &str = "claude.log";

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct RepoRef {
    /// Where the code is published (https://github.com/…), if known.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    /// A local checkout, if one was found (absolute).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Proposal {
    pub at: NaiveDateTime,
    pub by: String,
    pub title: String,
    /// Revision of the proposal: 1 for the first, +1 each time Claude revises it.
    #[serde(default)]
    pub round: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClaudeRun {
    pub pid: u32,
    pub started: NaiveDateTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished: Option<NaiveDateTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// The command line, for the log and the UI.
    #[serde(default)]
    pub command: String,
}

fn now() -> NaiveDateTime {
    Local::now().naive_local().with_nanosecond_zero()
}

impl Story {
    pub fn is_enhancement(&self) -> bool {
        self.enhances.is_some()
    }
    pub fn proposal_path(&self, site: &Site) -> PathBuf {
        self.dir(site).join(PROPOSAL_FILE)
    }
    /// The revised post waiting for the author, if any.
    pub fn proposal_text(&self, site: &Site) -> Option<String> {
        self.proposal.as_ref()?;
        std::fs::read_to_string(self.proposal_path(site)).ok()
    }
    pub fn claude_log_path(&self, site: &Site) -> PathBuf {
        self.dir(site).join(CLAUDE_LOG)
    }
    /// The enhanced item's file (absolute).
    pub fn item_path(&self, site: &Site) -> Option<PathBuf> {
        self.enhances.as_ref()?;
        self.draft.as_ref().map(|d| site.root.join(d))
    }
}

/// The story slug for enhancing an item: `enhance-<slug>`.
pub fn slug_for(item: &Item) -> String {
    let s = slugify(&item.slug);
    format!("enhance-{}", if s.is_empty() { slugify(&item.title) } else { s })
}

/// Start (or reopen) the enhancement of a content item. Returns the story and whether it was
/// created now. An open enhancement of the same item is reused; one for a different item that
/// happens to share the slug gets a numbered slug.
pub fn create(site: &Site, item_id: &str) -> Result<(Story, bool)> {
    let item = site.item(item_id)?;
    if item.id.starts_with("ext:") {
        bail!("{} lives in another repository ({}); enhance it there", item.title, item.path.display());
    }
    let rel = item.path.strip_prefix(&site.root).with_context(|| format!("{} is not inside the site", item.path.display()))?.to_string_lossy().to_string();
    let base = slug_for(&item);
    let mut slug = base.clone();
    let mut i = 2;
    while story_dir(site, &slug).join("story.yaml").exists() {
        let s = Story::load(site, &slug)?;
        if s.enhances.as_deref() == Some(item.id.as_str()) {
            return Ok((s, false));
        }
        slug = format!("{base}-{i}");
        i += 1;
    }
    let t = now();
    let mut s = Story {
        title: item.title.clone(),
        slug,
        target: item.kind.as_str().into(),
        project: if item.kind == Kind::Update { item.project.clone() } else { None },
        areas: item.areas.clone(),
        tags: item.tags.clone(),
        notes: String::new(),
        status: "interviewing".into(),
        created: t,
        updated: t,
        images: vec![],
        draft: Some(rel),
        draft_files: vec![],
        narrative: String::new(),
        narrative_rev: 0,
        rev: 1,
        pending: vec![],
        asked: 0,
        enhances: Some(item.id.clone()),
        repo: detect_repo(site, &item),
        key_answers: BTreeMap::new(),
        proposal: None,
        claude: None,
    };
    std::fs::create_dir_all(s.dir(site).join("images"))?;
    s.save(site)?;
    Ok((s, true))
}

// ------------------------------------------------------------------ repository detection

/// `github.com/owner/name` from any form of a git URL, lowercased, for comparing.
pub fn repo_key(url: &str) -> Option<String> {
    let u = url.trim().trim_end_matches('/').trim_end_matches(".git");
    let u = u.split("://").nth(1).unwrap_or(u);
    let u = u.strip_prefix("git@").unwrap_or(u).replacen(':', "/", 1);
    let u = u.split('@').next_back().unwrap_or(&u).to_string();
    let parts: Vec<&str> = u.split('/').filter(|p| !p.is_empty()).collect();
    (parts.len() >= 3 && parts[0].contains('.')).then(|| format!("{}/{}/{}", parts[0], parts[1], parts[2]).to_ascii_lowercase())
}

/// A registry `origin` may read "https://github.com/x/y (~/code/y)": the URL and a local path.
fn split_origin(origin: &str) -> (String, Option<String>) {
    let o = origin.trim();
    if let (Some(a), Some(b)) = (o.find('('), o.rfind(')')) {
        if a < b {
            return (o[..a].trim().to_string(), Some(o[a + 1..b].trim().to_string()));
        }
    }
    (o.to_string(), None)
}

/// Directories searched for a local checkout: `GW_REPO_ROOTS` (`:`-separated), the user config's
/// `repo_roots`, or the usual places (`~/code`, `~/src`, …) that exist.
fn repo_roots() -> Vec<PathBuf> {
    crate::userconfig::UserConfig::load().unwrap_or_default().repo_roots()
}

/// The remote URLs in a checkout's git config (`.git` may be a file pointing elsewhere, as in a
/// submodule).
fn remotes(dir: &Path) -> Vec<String> {
    let dotgit = dir.join(".git");
    let gitdir = if dotgit.is_dir() {
        dotgit
    } else if let Ok(t) = std::fs::read_to_string(&dotgit) {
        match t.trim().strip_prefix("gitdir:") {
            Some(p) => {
                let p = Path::new(p.trim());
                if p.is_absolute() { p.to_path_buf() } else { dir.join(p) }
            }
            None => return vec![],
        }
    } else {
        return vec![];
    };
    // Only `[remote "…"]` sections: a superproject's config also lists its submodules' URLs.
    let mut in_remote = false;
    let mut out = vec![];
    for l in std::fs::read_to_string(gitdir.join("config")).unwrap_or_default().lines() {
        let l = l.trim();
        if l.starts_with('[') {
            in_remote = l.starts_with("[remote ");
        } else if in_remote {
            if let Some(r) = l.strip_prefix("url") {
                out.push(r.trim_start().trim_start_matches('=').trim().to_string());
            }
        }
    }
    out
}

/// Find a local checkout of `url` under the repo roots (two levels deep).
pub fn find_checkout(url: &str) -> Option<PathBuf> {
    let key = repo_key(url)?;
    let matches = |d: &Path| remotes(d).iter().any(|r| repo_key(r).as_deref() == Some(key.as_str()));
    for root in repo_roots() {
        let Ok(rd) = std::fs::read_dir(&root) else { continue };
        let mut level1: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        level1.sort();
        for d in &level1 {
            if matches(d) {
                return Some(d.clone());
            }
        }
        for d in &level1 {
            if !d.join(".git").exists() && !d.join(".gitmodules").exists() {
                continue;
            }
            if let Ok(rd2) = std::fs::read_dir(d) {
                for e in rd2.flatten() {
                    let p = e.path();
                    if p.is_dir() && p.join(".git").exists() && matches(&p) {
                        return Some(p);
                    }
                }
            }
        }
    }
    None
}

/// The repository behind an item: its `:repo:`; else its registry entry (`:source:`, or the
/// entry whose `content` is this item); else, for an update, its project's. The local path comes
/// from the registry origin's "(~/path)" or a checkout found under the repo roots.
pub fn detect_repo(site: &Site, item: &Item) -> Option<RepoRef> {
    let reg = site.registry().ok();
    let entry_for = |it: &Item| -> Option<crate::registry::Entry> {
        let reg = reg.as_ref()?;
        it.source.as_deref().and_then(|s| reg.get(s).cloned()).or_else(|| reg.entries.iter().find(|e| e.content == it.id).cloned())
    };
    let from_item = |it: &Item| -> Option<(String, Option<String>)> {
        let e = entry_for(it);
        let (o_url, o_path) = e.as_ref().map(|e| split_origin(&e.origin)).unwrap_or_default();
        let url = it.repo.clone().or_else(|| e.as_ref().map(|e| e.url.clone()).filter(|u| repo_key(u).is_some())).or_else(|| (repo_key(&o_url).is_some()).then_some(o_url))?;
        Some((url, o_path))
    };
    let mut found = from_item(item);
    if found.is_none() && item.kind == Kind::Update {
        if let Some(p) = &item.project {
            if let Ok(pi) = site.item(&format!("projects/{p}")) {
                found = from_item(&pi);
            }
        }
    }
    let (url, hint) = found?;
    let path = hint
        .map(|h| expand_home(&h))
        .filter(|p| p.is_dir())
        .or_else(|| find_checkout(&url))
        .map(|p| p.display().to_string());
    Some(RepoRef { url, path })
}

/// Set the repository by hand (from the app). An empty path means "look for a checkout".
pub fn set_repo(s: &mut Story, url: &str, path: Option<&str>) -> Result<()> {
    let url = url.trim().to_string();
    let path = match path.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => {
            let d = expand_home(p);
            if !d.is_dir() {
                bail!("{} is not a directory", d.display());
            }
            Some(d.display().to_string())
        }
        None => find_checkout(&url).map(|p| p.display().to_string()),
    };
    s.repo = if url.is_empty() && path.is_none() { None } else { Some(RepoRef { url, path }) };
    Ok(())
}

// ------------------------------------------------------------------ key questions

pub fn key_question(id: &str) -> Option<&'static str> {
    KEY_QUESTIONS.iter().find(|(k, _)| *k == id).map(|(_, q)| *q)
}

/// Save the author's answers (question id → answer). Changed, non-empty answers are added to
/// the interview transcript (a revised answer is recorded again, marked as revised). Returns the
/// ids that changed.
pub fn set_key_answers(site: &Site, s: &mut Story, answers: &BTreeMap<String, String>) -> Result<Vec<String>> {
    let mut changed = vec![];
    for (id, a) in answers {
        let q = key_question(id).with_context(|| format!("no key question {id:?}"))?;
        let a = a.trim();
        let old = s.key_answers.get(id).map(|x| x.trim().to_string()).unwrap_or_default();
        if a == old {
            continue;
        }
        if a.is_empty() {
            s.key_answers.remove(id);
        } else {
            let q = if old.is_empty() { q.to_string() } else { format!("{q} (revised answer)") };
            s.add_qa(site, &q, a)?;
            s.key_answers.insert(id.clone(), a.to_string());
        }
        changed.push(id.clone());
    }
    Ok(changed)
}

// ------------------------------------------------------------------ proposals

/// Store a revised post as the proposal. The post itself is not touched.
pub fn propose(site: &Site, s: &mut Story, text: &str, by: &str) -> Result<Proposal> {
    if !s.is_enhancement() {
        bail!("story {} is not an enhancement; use story_draft", s.slug);
    }
    let (title, attrs) = content::parse_header(text);
    let title = title.context("the proposal must start with a `= Title` line")?;
    if let Some(st) = attrs.get("status") {
        if !matches!(st.to_ascii_lowercase().as_str(), "draft" | "published") {
            bail!(":status: must be draft or published");
        }
    }
    if let Some(t) = attrs.get("type") {
        Kind::parse(t).with_context(|| format!("unknown :type: {t}"))?;
    }
    for id in story::referenced_ids(text) {
        s.image(&id).with_context(|| format!("the proposal uses story:{id}, which is not an image in this story"))?;
    }
    story::atomic_write(&s.proposal_path(site), text.as_bytes())?;
    let round = s.proposal.as_ref().map(|p| p.round).unwrap_or(0) + 1;
    let p = Proposal { at: now(), by: by.into(), title, round };
    s.proposal = Some(p.clone());
    s.status = "drafted".into();
    Ok(p)
}

/// The header lines (title first) and the body of an AsciiDoc document.
fn split_doc(text: &str) -> (Vec<String>, String) {
    let mut head = vec![];
    let mut lines = text.lines();
    let mut started = false;
    for l in lines.by_ref() {
        if !started {
            if l.trim().is_empty() || l.starts_with("//") {
                continue;
            }
            started = true;
            head.push(l.to_string());
            continue;
        }
        if l.trim().is_empty() {
            break;
        }
        head.push(l.to_string());
    }
    let body: Vec<&str> = lines.collect();
    (head, body.join("\n"))
}

fn attr_name(line: &str) -> Option<&str> {
    let l = line.strip_prefix(':')?;
    let end = l.find(':')?;
    Some(&l[..end])
}

/// The proposal with the post's header kept where it matters: `:status:`, `:date:`, `:type:`,
/// `:project:`, `:source:`, and `:slug:` come from the existing post; any attribute the proposal
/// left out is carried over; `:updated:` is set to `today`.
pub fn merge_header(existing: &str, proposed: &str, today: NaiveDate) -> String {
    const KEEP: &[&str] = &["status", "date", "type", "project", "source", "slug"];
    let (_, old) = content::parse_header(existing);
    let (head, body) = split_doc(proposed);
    let mut out: Vec<String> = vec![];
    let mut seen = std::collections::BTreeSet::new();
    for (i, l) in head.iter().enumerate() {
        if i == 0 {
            out.push(l.clone());
            continue;
        }
        match attr_name(l) {
            Some(n) if KEEP.contains(&n) => {
                seen.insert(n.to_string());
                match old.get(n) {
                    Some(v) => out.push(format!(":{n}: {v}")),
                    None if n == "status" || n == "type" || n == "slug" || n == "source" => {} // not set on the post: leave unset
                    None => out.push(l.clone()),
                }
            }
            Some("updated") => {
                seen.insert("updated".into());
            }
            Some(n) => {
                seen.insert(n.to_string());
                out.push(l.clone());
            }
            None => out.push(l.clone()),
        }
    }
    // Carried-over attributes keep the post's order.
    for l in split_doc(existing).0.iter().skip(1) {
        if let Some(n) = attr_name(l) {
            if !seen.contains(n) && n != "updated" {
                if let Some(v) = old.get(n) {
                    out.push(format!(":{n}: {v}"));
                    seen.insert(n.to_string());
                }
            }
        }
    }
    out.push(format!(":updated: {today}"));
    let body = body.trim_start_matches('\n');
    format!("{}\n\n{}\n", out.join("\n"), body.trim_end())
}

/// Accept the proposal: write it over the post (header merged, images resolved), keep the post
/// where it is, and clear the proposal.
pub fn accept(site: &Site, s: &mut Story) -> Result<DraftResult> {
    let text = s.proposal_text(site).context("there is no proposal to accept")?;
    let path = s.item_path(site).context("this enhancement has lost track of its post")?;
    let existing = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let merged = merge_header(&existing, &text, Local::now().date_naive());
    let r = story::draft(site, s, &merged)?;
    let _ = std::fs::remove_file(s.proposal_path(site));
    s.proposal = None;
    s.status = "released".into();
    Ok(r)
}

/// Throw the proposal away.
pub fn discard(site: &Site, s: &mut Story) -> Result<()> {
    if s.proposal.is_none() {
        bail!("there is no proposal to discard");
    }
    let _ = std::fs::remove_file(s.proposal_path(site));
    s.proposal = None;
    s.status = "interviewing".into();
    Ok(())
}

/// The author's feedback on the proposal, recorded in the transcript so Claude revises it.
pub fn request_changes(site: &Site, s: &mut Story, feedback: &str) -> Result<usize> {
    let round = s.proposal.as_ref().map(|p| p.round).unwrap_or(0);
    let q = if round > 0 { format!("What should change in proposal {round}?") } else { "What should change?".to_string() };
    let n = s.add_qa(site, &q, feedback)?;
    s.status = "drafting".into();
    Ok(n)
}

// ------------------------------------------------------------------ a line diff

/// One line of a diff: ' ' kept, '-' removed (current post), '+' added (proposal).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DiffLine {
    pub op: char,
    pub text: String,
}

/// Line diff (longest common subsequence), with long unchanged stretches kept whole; callers
/// fold them for display.
pub fn diff_lines(old: &str, new: &str) -> Vec<DiffLine> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let (n, m) = (a.len(), b.len());
    // Trim the common prefix and suffix so the table stays small for typical edits.
    let mut pre = 0;
    while pre < n && pre < m && a[pre] == b[pre] {
        pre += 1;
    }
    let mut suf = 0;
    while suf < n - pre && suf < m - pre && a[n - 1 - suf] == b[m - 1 - suf] {
        suf += 1;
    }
    let (aa, bb) = (&a[pre..n - suf], &b[pre..m - suf]);
    let mut out: Vec<DiffLine> = a[..pre].iter().map(|l| DiffLine { op: ' ', text: l.to_string() }).collect();
    if aa.len() * bb.len() > 25_000_000 {
        // Too big to align: show it as replaced.
        out.extend(aa.iter().map(|l| DiffLine { op: '-', text: l.to_string() }));
        out.extend(bb.iter().map(|l| DiffLine { op: '+', text: l.to_string() }));
    } else {
        let (x, y) = (aa.len(), bb.len());
        let mut t = vec![vec![0u32; y + 1]; x + 1];
        for i in (0..x).rev() {
            for j in (0..y).rev() {
                t[i][j] = if aa[i] == bb[j] { t[i + 1][j + 1] + 1 } else { t[i + 1][j].max(t[i][j + 1]) };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < x || j < y {
            if i < x && j < y && aa[i] == bb[j] {
                out.push(DiffLine { op: ' ', text: aa[i].to_string() });
                i += 1;
                j += 1;
            } else if i < x && (j == y || t[i + 1][j] >= t[i][j + 1]) {
                // removals before additions, so a changed line reads old then new
                out.push(DiffLine { op: '-', text: aa[i].to_string() });
                i += 1;
            } else {
                out.push(DiffLine { op: '+', text: bb[j].to_string() });
                j += 1;
            }
        }
    }
    out.extend(a[n - suf..].iter().map(|l| DiffLine { op: ' ', text: l.to_string() }));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::story::NewStory;

    fn site_with_article() -> (Site, PathBuf) {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("gw-enhance-test-{}-{n}-{}", std::process::id(), Local::now().timestamp_nanos_opt().unwrap()));
        std::fs::create_dir_all(root.join("content/articles/widget")).unwrap();
        std::fs::create_dir_all(root.join("content/projects/bench")).unwrap();
        std::fs::write(
            root.join("content/articles/widget/index.adoc"),
            "= The Widget\n:type: article\n:date: 2026-01-02\n:status: published\n:areas: retro\n:tags: c, amiga\n:hero: hero.jpg\n:summary: Old summary.\n:repo: https://github.com/someone/widget\n\nThe original body.\n\nSecond paragraph.\n",
        )
        .unwrap();
        std::fs::write(root.join("content/projects/bench/index.adoc"), "= Bench\n:type: project\n:source: repo-bench\n\nBody.\n").unwrap();
        std::fs::write(
            root.join("site.yaml"),
            "site:\n  title: T\n  base_url: https://example.org\n  author: A\nareas:\n  - id: retro\n    title: Retro\n",
        )
        .unwrap();
        std::fs::write(
            root.join("registry.yaml"),
            "entries:\n- id: repo-bench\n  kind: repo\n  title: Bench\n  url: https://github.com/someone/bench\n  origin: https://github.com/someone/bench (/nonexistent/bench)\n  content: projects/bench\n",
        )
        .unwrap();
        (Site::open(Some(&root)).unwrap(), root)
    }

    #[test]
    fn creates_an_enhancement_from_an_item_and_reuses_it() {
        let (site, _root) = site_with_article();
        let (s, created) = create(&site, "articles/widget").unwrap();
        assert!(created);
        assert_eq!(s.slug, "enhance-widget");
        assert_eq!(s.title, "The Widget");
        assert_eq!(s.target, "article");
        assert_eq!(s.enhances.as_deref(), Some("articles/widget"));
        assert_eq!(s.draft.as_deref(), Some("content/articles/widget/index.adoc"));
        assert_eq!(s.areas, vec!["retro".to_string()]);
        assert_eq!(s.tags, vec!["c".to_string(), "amiga".to_string()]);
        assert_eq!(s.repo.as_ref().unwrap().url, "https://github.com/someone/widget");
        let (again, created) = create(&site, "/articles/widget/").unwrap();
        assert!(!created);
        assert_eq!(again.slug, s.slug);
        // An ordinary story with the same slug is not taken over.
        let other = Story::create(&site, &NewStory { title: "x".into(), slug: Some("enhance-bench".into()), ..Default::default() }).unwrap();
        let (b, _) = create(&site, "projects/bench").unwrap();
        assert_ne!(b.slug, other.slug);
        assert_eq!(b.slug, "enhance-bench-2");
    }

    #[test]
    fn detects_the_repo_from_the_registry() {
        let (site, _root) = site_with_article();
        let it = site.item("projects/bench").unwrap();
        let r = detect_repo(&site, &it).unwrap();
        assert_eq!(r.url, "https://github.com/someone/bench");
        assert_eq!(r.path, None, "the origin's path does not exist");
        assert_eq!(repo_key("git@github.com:Someone/Bench.git").as_deref(), Some("github.com/someone/bench"));
        assert_eq!(repo_key("https://github.com/someone/bench/").as_deref(), Some("github.com/someone/bench"));
        assert_eq!(repo_key("not a url"), None);
    }

    #[test]
    fn finds_a_local_checkout_by_its_remote() {
        let base = std::env::temp_dir().join(format!("gw-enhance-roots-{}", std::process::id()));
        let co = base.join("group").join("widget");
        std::fs::create_dir_all(base.join("group/.git")).unwrap();
        std::fs::create_dir_all(co.join(".git")).unwrap();
        std::fs::write(co.join(".git/config"), "[remote \"origin\"]\n\turl = git@github.com:someone/widget.git\n").unwrap();
        // A superproject that has widget as a submodule must not be taken for it.
        let sup = base.join("aaa-super");
        std::fs::create_dir_all(sup.join(".git")).unwrap();
        std::fs::write(sup.join(".git/config"), "[remote \"origin\"]\n\turl = https://github.com/someone/super\n[submodule \"widget\"]\n\turl = https://github.com/someone/widget\n").unwrap();
        // ...and a submodule checkout (a .git file) is found through its gitdir.
        let sub = sup.join("examples");
        std::fs::create_dir_all(sup.join(".git/modules/examples")).unwrap();
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join(".git"), "gitdir: ../.git/modules/examples\n").unwrap();
        std::fs::write(sup.join(".git/modules/examples/config"), "[remote \"origin\"]\n\turl = https://github.com/someone/examples\n").unwrap();
        std::env::set_var("GW_REPO_ROOTS", base.display().to_string());
        assert_eq!(find_checkout("https://github.com/someone/widget"), Some(co));
        assert_eq!(find_checkout("https://github.com/someone/super"), Some(sup.clone()));
        assert_eq!(find_checkout("https://github.com/someone/examples"), Some(sub));
        assert_eq!(find_checkout("https://github.com/someone/other"), None);
    }

    #[test]
    fn key_answers_land_in_the_transcript() {
        let (site, _root) = site_with_article();
        let (mut s, _) = create(&site, "articles/widget").unwrap();
        let mut a = BTreeMap::new();
        a.insert("inspiration".to_string(), "A rainy weekend.".to_string());
        a.insert("next".to_string(), "  ".to_string());
        assert_eq!(set_key_answers(&site, &mut s, &a).unwrap(), vec!["inspiration".to_string()]);
        assert!(s.interview(&site).contains("What was the inspiration?"));
        assert!(s.interview(&site).contains("A rainy weekend."));
        // Unchanged: nothing recorded again; revised: recorded as revised.
        assert!(set_key_answers(&site, &mut s, &a).unwrap().is_empty());
        a.insert("inspiration".to_string(), "A snowy weekend.".to_string());
        set_key_answers(&site, &mut s, &a).unwrap();
        assert!(s.interview(&site).contains("(revised answer)"));
        assert_eq!(s.key_answers["inspiration"], "A snowy weekend.");
        a.insert("bogus".to_string(), "x".to_string());
        assert!(set_key_answers(&site, &mut s, &a).is_err());
    }

    #[test]
    fn a_proposal_does_not_touch_the_post_and_accept_keeps_its_header() {
        let (site, root) = site_with_article();
        let post = root.join("content/articles/widget/index.adoc");
        let before = std::fs::read_to_string(&post).unwrap();
        let (mut s, _) = create(&site, "articles/widget").unwrap();
        // A proposal that changes the title and summary, drops :hero: and :repo:, and tries to
        // move the date and unpublish the post.
        let text = "= The Widget, Revisited\n:type: article\n:date: 2030-01-01\n:status: draft\n:summary: New summary.\n:tags: c, amiga, retro\n\nA better body.\n\nSecond paragraph.\n";
        let p = propose(&site, &mut s, text, "mcp").unwrap();
        assert_eq!(p.title, "The Widget, Revisited");
        assert_eq!(std::fs::read_to_string(&post).unwrap(), before, "the post is untouched");
        assert_eq!(s.proposal_text(&site).unwrap(), text);
        let r = accept(&site, &mut s).unwrap();
        assert_eq!(r.path, "content/articles/widget/index.adoc");
        let after = std::fs::read_to_string(&post).unwrap();
        let (title, attrs) = content::parse_header(&after);
        assert_eq!(title.as_deref(), Some("The Widget, Revisited"));
        assert_eq!(attrs["status"], "published");
        assert_eq!(attrs["date"], "2026-01-02");
        assert_eq!(attrs["summary"], "New summary.");
        assert_eq!(attrs["tags"], "c, amiga, retro");
        assert_eq!(attrs["hero"], "hero.jpg");
        assert_eq!(attrs["repo"], "https://github.com/someone/widget");
        assert_eq!(attrs["updated"], Local::now().date_naive().to_string());
        assert!(after.contains("A better body."));
        assert!(s.proposal.is_none());
        assert!(!s.proposal_path(&site).exists());
        let it = site.item("articles/widget").unwrap();
        assert_eq!(it.url, "/articles/widget/");
        assert!(!it.draft);
    }

    #[test]
    fn discard_and_request_changes() {
        let (site, root) = site_with_article();
        let post = root.join("content/articles/widget/index.adoc");
        let before = std::fs::read_to_string(&post).unwrap();
        let (mut s, _) = create(&site, "articles/widget").unwrap();
        assert!(discard(&site, &mut s).is_err());
        propose(&site, &mut s, "= W\n\nBody.\n", "mcp").unwrap();
        request_changes(&site, &mut s, "Keep the original title.").unwrap();
        assert!(s.interview(&site).contains("Keep the original title."));
        assert_eq!(s.status, "drafting");
        discard(&site, &mut s).unwrap();
        assert!(s.proposal.is_none());
        assert_eq!(std::fs::read_to_string(&post).unwrap(), before);
        assert!(propose(&site, &mut s, "no title here", "mcp").is_err());
        let mut plain = Story::create(&site, &NewStory { title: "Plain".into(), ..Default::default() }).unwrap();
        assert!(propose(&site, &mut plain, "= P\n\nx\n", "mcp").is_err());
    }

    #[test]
    fn line_diff() {
        let d = diff_lines("a\nb\nc\nd\n", "a\nB\nc\nd\ne\n");
        let ops: String = d.iter().map(|l| l.op).collect();
        assert_eq!(ops, " -+  +");
        assert_eq!(d[2].text, "B");
    }

    #[test]
    fn merge_header_adds_updated_once_and_keeps_missing_attrs() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 12).unwrap();
        let m = merge_header("= A\n:hero: h.jpg\n:date: 2026-01-01\n:updated: 2026-02-02\n\nx\n", "= B\n:updated: 1999-01-01\n\ny\n", today);
        assert_eq!(m, "= B\n:hero: h.jpg\n:date: 2026-01-01\n:updated: 2026-10-12\n\ny\n");
    }
}
