//! The code catalog from the gitwebsite side: the site's git projects (`code.yaml`, shown at
//! `/code/`) kept in step with GitHub, edited in the app's Code page, `gw code`, or over MCP.
//!
//! - **sync** reads the owner's repositories with the GitHub CLI (`gh`), adds new public ones
//!   (forks and archived ones hidden), refreshes each project's `github:` facts, and finds each
//!   project's logo — an image named or placed as a logo in its README, else a `logo.*` file in
//!   the repository — and copies it, scaled down, into `static/code-logos/`.
//! - **logo prompts**: projects without a logo get an image prompt, handed out in batches to paste
//!   into ChatGPT; the rendered images are **imported** by file name, and a **logo PR** adds one to
//!   its repository (`assets/logo.png`) and to the top of the README.
//! - **enhance** asks Claude (the `claude` CLI, no tools) for a better title, summary,
//!   description, tags, category, related projects and logo idea for one project, from its README;
//!   **suggest** asks for a category scheme and tags for the whole catalog. Both are proposals:
//!   nothing is saved until they are applied.
//!
//! Long operations run as a background **job** (one at a time) that the app follows.
//! See [`gw_core::codecat`] for the file and the pages.

use anyhow::{bail, Context, Result};
use base64::Engine;
use gw_core::codecat::{self, Catalog, Category, GitHub, Project};
use gw_core::Site;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

/// One read-modify-write of code.yaml at a time.
static LOCK: Mutex<()> = Mutex::new(());

/// Logos on the website are at most this big (they show at 56 and 112 px).
const SITE_PX: u32 = 256;
/// Logos committed to repositories.
const REPO_PX: u32 = 512;
/// The branch a logo PR is made from.
const PR_BRANCH: &str = "gitwebsite/add-logo";

fn today() -> String {
    chrono::Local::now().date_naive().to_string()
}

fn load(site: &Site) -> Result<Catalog> {
    Ok(Catalog::load(&site.root)?.unwrap_or_else(|| Catalog { title: codecat::DEFAULT_TITLE.into(), ..Default::default() }))
}

/// Read, change and save code.yaml under the lock.
fn modify<T>(site: &Site, f: impl FnOnce(&mut Catalog) -> Result<T>) -> Result<T> {
    let _l = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut c = load(site)?;
    let out = f(&mut c)?;
    c.save(&site.root)?;
    Ok(out)
}

fn logo_dir(site: &Site) -> PathBuf {
    site.static_dir().join(codecat::LOGO_DIR)
}
/// Full-size imported logos (for the repository PR), outside the published site.
fn source_dir(site: &Site) -> PathBuf {
    site.cache_dir().join("code-logos")
}

// ------------------------------------------------------------------ gh

fn gh_bin() -> Result<PathBuf> {
    crate::enhance::find_program("gh", "GW_GH_BIN").context("can't find the GitHub CLI (gh): install it (brew install gh) and run `gh auth login`")
}

fn gh_raw(args: &[&str], input: Option<&[u8]>) -> Result<Vec<u8>> {
    use std::io::Write;
    let mut cmd = Command::new(gh_bin()?);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() });
    let mut child = cmd.spawn().context("running gh")?;
    if let Some(data) = input {
        child.stdin.take().context("gh stdin")?.write_all(data)?;
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let body = String::from_utf8_lossy(&out.stdout);
        let msg = serde_json::from_str::<Value>(&body).ok().and_then(|v| v["message"].as_str().map(str::to_string)).unwrap_or_default();
        bail!("gh {}: {} {}", args.iter().take(3).cloned().collect::<Vec<_>>().join(" "), err.trim(), msg);
    }
    Ok(out.stdout)
}

fn gh_json(args: &[&str]) -> Result<Value> {
    let out = gh_raw(args, None)?;
    serde_json::from_slice(&out).context("gh: not JSON")
}

/// `gh api -X <method> <path> --input -` with a JSON body. GitHub limits how fast an account
/// may write (its "secondary rate limit", about 80 writes a minute and 500 an hour); a write
/// refused for that waits a minute and tries again, for up to an hour.
fn gh_send(method: &str, path: &str, body: &Value) -> Result<Value> {
    let mut tries = 0;
    loop {
        match gh_raw(&["api", "-X", method, path, "--input", "-"], Some(body.to_string().as_bytes())) {
            Ok(out) => return Ok(serde_json::from_slice(&out).unwrap_or(Value::Null)),
            Err(e) if tries < 60 && is_rate_limited(&format!("{e:#}")) => {
                tries += 1;
                eprintln!("GitHub rate limit; waiting a minute ({tries})");
                std::thread::sleep(std::time::Duration::from_secs(60));
            }
            Err(e) => return Err(e),
        }
    }
}

fn is_rate_limited(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    e.contains("secondary rate limit") || e.contains("rate limit exceeded") || e.contains("abuse")
}

/// A GET that may 404: `None` then.
fn gh_get_opt(path: &str) -> Result<Option<Value>> {
    match gh_json(&["api", path]) {
        Ok(v) => Ok(Some(v)),
        // Not there, or an empty repository.
        Err(e) if ["404", "Not Found", "409", "is empty"].iter().any(|s| format!("{e:#}").contains(s)) => Ok(None),
        Err(e) => Err(e),
    }
}

fn enc_path(p: &str) -> String {
    p.split('/').map(|s| urlencoding::encode(s).into_owned()).collect::<Vec<_>>().join("/")
}

/// A file's bytes from a repository.
fn gh_file(repo: &str, path: &str, branch: &str) -> Result<Vec<u8>> {
    let api = format!("repos/{repo}/contents/{}?ref={}", enc_path(path), urlencoding::encode(branch));
    gh_raw(&["api", "-H", "Accept: application/vnd.github.raw", &api], None)
}

fn gh_login() -> Result<String> {
    let v = gh_json(&["api", "user"])?;
    v["login"].as_str().map(str::to_string).context("gh api user: no login")
}

// ------------------------------------------------------------------ jobs

#[derive(Clone, Default)]
struct Job {
    id: u64,
    kind: String,
    target: String,
    started: String,
    finished: Option<String>,
    log: Vec<String>,
    result: Option<Value>,
    error: Option<String>,
}

static JOB: Mutex<Option<Job>> = Mutex::new(None);

fn job_json(j: &Job) -> Value {
    json!({ "id": j.id, "kind": j.kind, "target": j.target, "started": j.started, "finished": j.finished,
        "running": j.finished.is_none(), "log": j.log, "result": j.result, "error": j.error })
}

/// The current (or last) job.
pub fn job() -> Value {
    JOB.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(job_json).unwrap_or(json!({ "running": false }))
}

fn job_log(line: String) {
    if let Some(j) = JOB.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        j.log.push(line);
        if j.log.len() > 2000 {
            j.log.drain(..500);
        }
    }
}

/// Run `f` in the background as the job; refuses while another is running.
fn start_job(site: &Site, kind: &str, target: &str, f: impl FnOnce(&Site, &dyn Fn(String)) -> Result<Value> + Send + 'static) -> Result<Value> {
    let id = {
        let mut g = JOB.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(j) = g.as_ref().filter(|j| j.finished.is_none()) {
            bail!("already busy: {} {} (started {})", j.kind, j.target, j.started);
        }
        let id = g.as_ref().map(|j| j.id + 1).unwrap_or(1);
        *g = Some(Job { id, kind: kind.into(), target: target.into(), started: chrono::Local::now().format("%H:%M:%S").to_string(), ..Default::default() });
        id
    };
    let root = site.root.clone();
    std::thread::spawn(move || {
        let r = Site::open(Some(&root)).and_then(|s| f(&s, &|l| job_log(l)));
        let mut g = JOB.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(j) = g.as_mut().filter(|j| j.id == id) {
            j.finished = Some(chrono::Local::now().format("%H:%M:%S").to_string());
            match r {
                Ok(v) => j.result = Some(v),
                Err(e) => j.error = Some(format!("{e:#}")),
            }
        }
    });
    Ok(job())
}

// ------------------------------------------------------------------ state

/// The catalog as the app shows it.
pub fn state(site: &Site) -> Result<Value> {
    let exists = Catalog::path(&site.root).exists();
    let c = load(site)?;
    let dir = logo_dir(site);
    // Write-ups found by their :repo:.
    let items = site.items().unwrap_or_default();
    let norm = |u: &str| u.trim().trim_end_matches('/').trim_end_matches(".git").to_ascii_lowercase();
    let projects: Vec<Value> = c
        .projects
        .iter()
        .map(|p| {
            let mut v = serde_json::to_value(p).unwrap_or_default();
            v["display_title"] = json!(p.display_title());
            v["display_summary"] = json!(p.display_summary());
            v["listed"] = json!(p.listed());
            v["url"] = json!(format!("{}{}/", codecat::URL, p.slug));
            v["repo_url"] = json!(p.repo_url());
            let shown = c.logo_of(p);
            v["logo_url"] = json!(shown.map(|(f, _)| logo_api_url(&dir, f)).unwrap_or_default());
            v["logo_from_family"] = json!(shown.is_some_and(|(_, fam)| fam));
            v["related_back"] = json!(c.projects.iter().filter(|q| q.related.contains(&p.slug)).map(|q| q.slug.clone()).collect::<Vec<_>>());
            let post = if p.post.is_empty() {
                items.iter().find(|it| it.repo.as_deref().is_some_and(|r| norm(r) == norm(&p.repo_url())))
            } else {
                items.iter().find(|it| it.id == p.post)
            };
            v["post_item"] = post.map(|it| json!({ "id": it.id, "title": it.title, "url": it.url, "draft": it.draft })).unwrap_or(Value::Null);
            v
        })
        .collect();
    let listed = c.projects.iter().filter(|p| p.listed()).count();
    let need_logo = codecat::logo_batches(&c, 1000).concat().len();
    let open_prs = pr_candidates(&c).len();
    let families: Vec<Value> = c
        .families
        .iter()
        .map(|f| {
            let mut v = serde_json::to_value(f).unwrap_or_default();
            v["members"] = json!(c.projects.iter().filter(|p| p.family == f.id).map(|p| p.slug.clone()).collect::<Vec<_>>());
            v["logo_url"] = json!(if f.logo.is_empty() { String::new() } else { logo_api_url(&dir, &f.logo) });
            v["prompt"] = json!(codecat::family_prompt(&c, f));
            v
        })
        .collect();
    Ok(json!({
        "file": Catalog::path(&site.root).display().to_string(), "exists": exists,
        "title": c.title, "intro": c.intro, "owner": c.owner, "logo_style": c.logo_style, "default_logo_style": codecat::DEFAULT_LOGO_STYLE,
        "categories": c.categories, "families": families, "projects": projects,
        "counts": { "all": c.projects.len(), "listed": listed, "need_logo": need_logo, "awaiting_pr": open_prs },
        "url": format!("{}{}", site.config.site.base_url.trim_end_matches('/'), codecat::URL),
        "job": job(),
    }))
}

fn logo_api_url(dir: &Path, file: &str) -> String {
    if !dir.join(file).is_file() {
        return String::new();
    }
    let t = std::fs::metadata(dir.join(file)).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    format!("/api/code/logo/{file}?v={t}")
}

/// Projects whose logo was made here (imported, or their family's mark) and is not yet in their
/// repository by pull request.
fn pr_candidates(c: &Catalog) -> Vec<String> {
    c.projects
        .iter()
        .filter(|p| p.listed() && p.logo_pr.is_empty())
        .filter(|p| match c.logo_of(p) {
            Some((_, true)) => true,
            Some((_, false)) => p.logo_source == "imported",
            None => false,
        })
        .map(|p| p.slug.clone())
        .collect()
}

/// One project with everything the app's editor shows.
pub fn show(site: &Site, slug: &str) -> Result<Value> {
    let st = state(site)?;
    let p = st["projects"].as_array().and_then(|a| a.iter().find(|p| p["slug"] == slug)).cloned().with_context(|| format!("no project {slug} in {}", codecat::FILE))?;
    let c = load(site)?;
    let mut v = p;
    let p = c.get(slug).unwrap();
    v["logo_prompt_effective"] = json!(match c.family(&p.family) {
        Some(f) if p.logo.is_empty() => codecat::family_prompt(&c, f),
        _ => codecat::logo_prompt(&c, p),
    });
    Ok(v)
}

// ------------------------------------------------------------------ editing

/// The fields of a project the app and CLI may change (missing = unchanged).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Patch {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub category: Option<String>,
    pub tags: Option<Vec<String>>,
    pub related: Option<Vec<String>>,
    pub hidden: Option<bool>,
    pub featured: Option<bool>,
    pub order: Option<Option<i64>>,
    pub logo_prompt: Option<String>,
    pub post: Option<String>,
    /// A logo family's id ("" to leave it).
    pub family: Option<String>,
    /// Set by an applied enhancement: stamps `enhanced`.
    pub enhanced: Option<bool>,
}

fn clean_tags(tags: &[String]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    tags.iter()
        .map(|t| t.trim().trim_start_matches('#').to_lowercase().split_whitespace().collect::<Vec<_>>().join("-"))
        .filter(|t| !t.is_empty() && t.len() <= 40 && seen.insert(t.clone()))
        .collect()
}

pub fn update(site: &Site, slug: &str, p: &Patch) -> Result<Value> {
    modify(site, |c| {
        let slugs: BTreeSet<String> = c.projects.iter().map(|x| x.slug.clone()).collect();
        let cats: BTreeSet<String> = c.categories.iter().map(|x| x.id.clone()).collect();
        let it = c.get_mut(slug).with_context(|| format!("no project {slug}"))?;
        if let Some(v) = &p.title { it.title = v.trim().to_string(); }
        if let Some(v) = &p.summary { it.summary = v.trim().to_string(); }
        if let Some(v) = &p.description { it.description = v.trim().to_string(); }
        if let Some(v) = &p.category {
            let v = v.trim();
            if !v.is_empty() && !cats.contains(v) {
                bail!("no category {v}: add it first (gw code category {v} --title …)");
            }
            it.category = v.to_string();
        }
        if let Some(v) = &p.tags { it.tags = clean_tags(v); }
        if let Some(v) = &p.related {
            let mut seen = BTreeSet::new();
            let mut out = vec![];
            for r in v.iter().map(|r| r.trim()).filter(|r| !r.is_empty()) {
                if r == slug { continue; }
                if !slugs.contains(r) { bail!("no project {r} to relate to"); }
                if seen.insert(r.to_string()) { out.push(r.to_string()); }
            }
            it.related = out;
        }
        if let Some(v) = p.hidden { it.hidden = v; }
        if let Some(v) = p.featured { it.featured = v; }
        if let Some(v) = p.order { it.order = v; }
        if let Some(v) = &p.logo_prompt { it.logo_prompt = v.trim().to_string(); }
        if let Some(v) = &p.post { it.post = v.trim().to_string(); }
        if let Some(v) = &p.family { it.family = v.trim().to_string(); }
        if p.enhanced == Some(true) { it.enhanced = today(); }
        Ok(())
    })?;
    show(site, slug)
}

/// Change the catalog's own settings.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub title: Option<String>,
    pub intro: Option<String>,
    pub owner: Option<String>,
    pub logo_style: Option<String>,
    /// The whole list, in display order. Projects in a removed category lose their category.
    pub categories: Option<Vec<Category>>,
}

pub fn settings(site: &Site, s: &Settings) -> Result<Value> {
    modify(site, |c| {
        if let Some(v) = &s.title { c.title = v.trim().into(); }
        if let Some(v) = &s.intro { c.intro = v.trim().into(); }
        if let Some(v) = &s.owner { c.owner = v.trim().into(); }
        if let Some(v) = &s.logo_style { c.logo_style = v.trim().into(); }
        if let Some(v) = &s.categories {
            c.categories = v.iter().map(|x| Category { id: codecat::slug_for(&x.id), title: x.title.trim().into(), description: x.description.trim().into() }).collect();
            let ids: BTreeSet<String> = c.categories.iter().map(|x| x.id.clone()).collect();
            for p in c.projects.iter_mut().filter(|p| !p.category.is_empty() && !ids.contains(&p.category)) {
                p.category.clear();
            }
        }
        Ok(())
    })?;
    state(site)
}

/// Add or change a logo family (`remove` drops it and frees its members), and add members: the
/// given slugs and every project in the given categories.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct FamilyChange {
    pub id: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub concept: Option<String>,
    pub logo_prompt: Option<String>,
    pub add: Vec<String>,
    pub categories: Vec<String>,
    pub drop: Vec<String>,
    pub remove: bool,
}

pub fn family(site: &Site, ch: &FamilyChange) -> Result<Value> {
    let id = codecat::slug_for(&ch.id);
    let added = modify(site, |c| {
        if ch.remove {
            c.families.retain(|f| f.id != id);
            for p in c.projects.iter_mut().filter(|p| p.family == id) {
                p.family.clear();
            }
            return Ok(0);
        }
        let f = match c.families.iter_mut().position(|f| f.id == id) {
            Some(i) => &mut c.families[i],
            None => {
                c.families.push(codecat::Family { id: id.clone(), title: codecat::title_for(&id), ..Default::default() });
                c.families.last_mut().unwrap()
            }
        };
        if let Some(v) = &ch.title { f.title = v.trim().into(); }
        if let Some(v) = &ch.description { f.description = v.trim().into(); }
        if let Some(v) = &ch.concept { f.concept = v.trim().into(); }
        if let Some(v) = &ch.logo_prompt { f.logo_prompt = v.trim().into(); }
        let mut n = 0;
        for s in &ch.add {
            let p = c.get_mut(s.trim()).with_context(|| format!("no project {s}"))?;
            if p.family != id { p.family = id.clone(); n += 1; }
        }
        for p in c.projects.iter_mut().filter(|p| ch.categories.contains(&p.category)) {
            if p.family != id { p.family = id.clone(); n += 1; }
        }
        for s in &ch.drop {
            if let Some(p) = c.get_mut(s.trim()).filter(|p| p.family == id) { p.family.clear(); }
        }
        Ok(n)
    })?;
    let mut v = state(site)?;
    v["added"] = json!(added);
    Ok(v)
}

/// Add or change one category (`remove` drops it).
pub fn category(site: &Site, id: &str, title: Option<&str>, description: Option<&str>, remove: bool) -> Result<Value> {
    let id = codecat::slug_for(id);
    modify(site, |c| {
        if remove {
            c.categories.retain(|x| x.id != id);
            for p in c.projects.iter_mut().filter(|p| p.category == id) {
                p.category.clear();
            }
            return Ok(());
        }
        match c.categories.iter_mut().find(|x| x.id == id) {
            Some(x) => {
                if let Some(t) = title { x.title = t.trim().into(); }
                if let Some(d) = description { x.description = d.trim().into(); }
            }
            None => c.categories.push(Category { id: id.clone(), title: title.map(|t| t.trim().to_string()).unwrap_or_else(|| codecat::title_for(&id)), description: description.unwrap_or("").trim().into() }),
        }
        Ok(())
    })?;
    state(site)
}

// ------------------------------------------------------------------ sync

#[derive(Debug, Default, serde::Serialize)]
pub struct SyncReport {
    pub owner: String,
    pub repositories: usize,
    pub added: Vec<String>,
    pub added_hidden: Vec<String>,
    pub updated: usize,
    pub gone: Vec<String>,
    pub logos_found: Vec<String>,
    pub problems: Vec<String>,
}

fn facts(r: &Value) -> GitHub {
    let s = |v: &Value| v.as_str().unwrap_or("").trim().to_string();
    GitHub {
        description: s(&r["description"]),
        language: s(&r["primaryLanguage"]["name"]),
        topics: r["repositoryTopics"].as_array().map(|a| a.iter().filter_map(|t| t["name"].as_str().or(t.as_str()).map(str::to_string)).collect()).unwrap_or_default(),
        stars: r["stargazerCount"].as_u64().unwrap_or(0),
        homepage: s(&r["homepageUrl"]),
        pushed: s(&r["pushedAt"]).chars().take(10).collect(),
        created: s(&r["createdAt"]).chars().take(10).collect(),
        license: { let l = s(&r["licenseInfo"]["spdxId"]); if l == "NOASSERTION" { s(&r["licenseInfo"]["name"]) } else { l } },
        default_branch: s(&r["defaultBranchRef"]["name"]),
        fork: r["isFork"].as_bool().unwrap_or(false),
        archived: r["isArchived"].as_bool().unwrap_or(false),
        private: r["isPrivate"].as_bool().unwrap_or(false),
        gone: false,
        logo_path: String::new(),
    }
}

/// Read the owner's repositories and bring code.yaml up to date; with `logos`, look for logos
/// of listed projects that have none (`refresh_logos`: of all listed projects not imported).
pub fn sync(site: &Site, logos: bool, refresh_logos: bool, log: &dyn Fn(String)) -> Result<SyncReport> {
    let mut r = SyncReport::default();
    let owner = { let c = load(site)?; if c.owner.is_empty() { gh_login()? } else { c.owner.clone() } };
    r.owner = owner.clone();
    log(format!("Reading {owner}'s repositories from GitHub…"));
    let list = gh_json(&["repo", "list", &owner, "--limit", "2000", "--json",
        "name,nameWithOwner,description,isPrivate,isFork,isArchived,primaryLanguage,repositoryTopics,pushedAt,createdAt,stargazerCount,homepageUrl,licenseInfo,defaultBranchRef"])?;
    let repos = list.as_array().cloned().unwrap_or_default();
    r.repositories = repos.len();
    log(format!("{} repositories", repos.len()));
    modify(site, |c| {
        if c.owner.is_empty() { c.owner = owner.clone(); }
        let seen: BTreeSet<String> = repos.iter().filter_map(|x| x["nameWithOwner"].as_str()).map(|s| s.to_ascii_lowercase()).collect();
        for x in &repos {
            let Some(full) = x["nameWithOwner"].as_str() else { continue };
            if codecat::parse_repo(full).is_err() { continue; }
            let f = facts(x);
            match c.projects.iter_mut().find(|p| p.repo.eq_ignore_ascii_case(full)) {
                Some(p) => {
                    let logo_path = std::mem::take(&mut p.github.logo_path);
                    p.github = GitHub { logo_path, ..f };
                    r.updated += 1;
                }
                None if f.private => {}
                None => {
                    let name = x["name"].as_str().unwrap_or(full);
                    let hide = f.fork || f.archived;
                    let slug = c.new_slug(name);
                    if hide { r.added_hidden.push(slug.clone()); } else { r.added.push(slug.clone()); }
                    c.projects.push(Project { slug, repo: full.to_string(), hidden: hide, added: today(), github: f, ..Default::default() });
                }
            }
        }
        for p in c.projects.iter_mut().filter(|p| p.repo.split('/').next().is_some_and(|o| o.eq_ignore_ascii_case(&owner))) {
            let gone = !seen.contains(&p.repo.to_ascii_lowercase());
            if gone && !p.github.gone { r.gone.push(p.slug.clone()); }
            p.github.gone = gone;
        }
        Ok(())
    })?;
    log(format!("{} new ({} hidden: forks or archived), {} refreshed, {} gone", r.added.len() + r.added_hidden.len(), r.added_hidden.len(), r.updated, r.gone.len()));
    if logos {
        find_logos(site, refresh_logos, &mut r, log)?;
    }
    Ok(r)
}

/// Look for logos in the repositories (in parallel) and copy those found into the site.
fn find_logos(site: &Site, refresh: bool, r: &mut SyncReport, log: &dyn Fn(String)) -> Result<()> {
    let c = load(site)?;
    let todo: Vec<Project> = c.projects.iter().filter(|p| p.listed() && p.logo_source != "imported" && (refresh || p.logo.is_empty())).cloned().collect();
    if todo.is_empty() {
        return Ok(());
    }
    log(format!("Looking for logos in {} repositories…", todo.len()));
    let queue = Mutex::new(todo.into_iter());
    let found: Mutex<Vec<(String, String, Vec<u8>)>> = Mutex::new(vec![]);
    let problems: Mutex<Vec<String>> = Mutex::new(vec![]);
    let done = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..8 {
            s.spawn(|| loop {
                let Some(p) = queue.lock().unwrap().next() else { break };
                match repo_logo(&p) {
                    Ok(Some((path, bytes))) => found.lock().unwrap().push((p.slug.clone(), path, bytes)),
                    Ok(None) => {}
                    Err(e) => problems.lock().unwrap().push(format!("{}: {e:#}", p.slug)),
                }
                let n = done.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                if n.is_multiple_of(20) {
                    job_log(format!("… {n} looked at"));
                }
            });
        }
    });
    r.problems.extend(problems.into_inner().unwrap());
    let mut found = found.into_inner().unwrap();
    // The same image as the "logo" of several repositories is a template's, not theirs.
    let mut counts: BTreeMap<Vec<u8>, usize> = BTreeMap::new();
    for (_, _, b) in &found {
        *counts.entry(<sha2::Sha256 as sha2::Digest>::digest(b).to_vec()).or_default() += 1;
    }
    found.retain(|(_, _, b)| counts[&<sha2::Sha256 as sha2::Digest>::digest(b).to_vec()] == 1);
    let dir = logo_dir(site);
    std::fs::create_dir_all(&dir)?;
    let mut saved: Vec<(String, String, String)> = vec![];
    for (slug, path, bytes) in found {
        match codecat::normalise_logo(&bytes, SITE_PX) {
            Ok((out, ext)) => {
                let file = format!("{slug}.{ext}");
                remove_logo_files(&dir, &slug)?;
                std::fs::write(dir.join(&file), out)?;
                saved.push((slug, file, path));
            }
            Err(e) => r.problems.push(format!("{slug}: logo {path}: {e:#}")),
        }
    }
    modify(site, |c| {
        for (slug, file, path) in &saved {
            if let Some(p) = c.get_mut(slug) {
                p.logo = file.clone();
                p.logo_source = if path.starts_with("readme:") || path.starts_with("https://") { "readme".into() } else { "repo".into() };
                p.github.logo_path = path.trim_start_matches("readme:").to_string();
            }
        }
        Ok(())
    })?;
    r.logos_found = saved.into_iter().map(|(s, _, _)| s).collect();
    log(format!("{} logos found", r.logos_found.len()));
    Ok(())
}

fn remove_logo_files(dir: &Path, slug: &str) -> Result<()> {
    for ext in codecat::LOGO_EXTS {
        let f = dir.join(format!("{slug}.{ext}"));
        if f.is_file() {
            std::fs::remove_file(f)?;
        }
    }
    Ok(())
}

/// The project's logo: (where it was found, bytes). README images first (`readme:<path>` or a
/// URL), then a logo file in the repository.
fn repo_logo(p: &Project) -> Result<Option<(String, Vec<u8>)>> {
    let branch = if p.github.default_branch.is_empty() { "HEAD" } else { &p.github.default_branch };
    if let Some(readme) = gh_get_opt(&format!("repos/{}/readme", p.repo))? {
        let text = readme["content"].as_str().map(|s| s.replace('\n', "")).and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok()).map(|b| String::from_utf8_lossy(&b).to_string()).unwrap_or_default();
        let dir = readme["path"].as_str().and_then(|p| p.rsplit_once('/').map(|(d, _)| d.to_string())).unwrap_or_default();
        for cand in readme_logo_candidates(&text, &dir, &p.repo) {
            let got = if cand.starts_with("https://") { fetch_url(&cand) } else { gh_file(&p.repo, &cand, branch) };
            if let Ok(bytes) = got {
                if codecat::normalise_logo(&bytes, 16).is_ok() {
                    return Ok(Some((if cand.starts_with("https://") { cand } else { format!("readme:{cand}") }, bytes)));
                }
            }
        }
    }
    if let Some(tree) = gh_get_opt(&format!("repos/{}/git/trees/{}?recursive=1", p.repo, urlencoding::encode(branch)))? {
        let paths: Vec<String> = tree["tree"].as_array().map(|a| a.iter().filter(|e| e["type"] == "blob").filter_map(|e| e["path"].as_str().map(str::to_string)).collect()).unwrap_or_default();
        if let Some(path) = tree_logo(&paths) {
            let bytes = gh_file(&p.repo, &path, branch)?;
            return Ok(Some((path, bytes)));
        }
    }
    Ok(None)
}

fn fetch_url(url: &str) -> Result<Vec<u8>> {
    let r = reqwest::blocking::Client::builder().user_agent("gitwebsite/0.1").timeout(std::time::Duration::from_secs(30)).build()?.get(url).send()?;
    if !r.status().is_success() {
        bail!("{url}: HTTP {}", r.status());
    }
    if r.content_length().is_some_and(|n| n > 8 << 20) {
        bail!("{url}: too big");
    }
    let b = r.bytes()?;
    if b.len() > 8 << 20 {
        bail!("{url}: too big");
    }
    Ok(b.to_vec())
}

fn is_image_path(p: &str) -> bool {
    let l = p.to_ascii_lowercase();
    let l = l.split(['?', '#']).next().unwrap_or("");
    codecat::LOGO_EXTS.iter().any(|e| l.ends_with(&format!(".{e}")))
}

/// Logo candidates in a README, best first: images whose path or alt text says "logo"
/// (or "icon"/"banner"), then the first image in the README's opening lines — never badges.
/// Relative paths are resolved against the README's folder; links to this repository's files on
/// GitHub become paths; other https URLs are kept as they are.
pub fn readme_logo_candidates(readme: &str, readme_dir: &str, repo: &str) -> Vec<String> {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r#"(?i)!\[([^\]]*)\]\(\s*<?([^)\s>]+)>?(?:\s+"[^"]*")?\s*\)|<img\b([^>]*)>"#).unwrap());
    static SRC: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let src_re = SRC.get_or_init(|| regex::Regex::new(r#"(?i)\bsrc\s*=\s*["']([^"']+)["']"#).unwrap());
    static ALT: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let alt_re = ALT.get_or_init(|| regex::Regex::new(r#"(?i)\balt\s*=\s*["']([^"']*)["']"#).unwrap());
    let badge = |s: &str| {
        let l = s.to_ascii_lowercase();
        ["shields.io", "badge", "/workflows/", "travis-ci", "codecov", "circleci", "badgen", "img.shields", "githubusercontent.com/u/", "avatars."].iter().any(|b| l.contains(b))
    };
    // Where the opening ends: the first line of prose (not a heading, image, link or html).
    let mut opening_end = readme.len();
    let mut pos = 0;
    for line in readme.split_inclusive('\n') {
        let t = line.trim();
        let prose = !t.is_empty() && !t.starts_with('#') && !t.starts_with('<') && !t.starts_with("![") && !t.starts_with("[!") && !t.starts_with('[') && !t.starts_with("---") && !t.starts_with("===");
        if prose {
            opening_end = pos;
            break;
        }
        pos += line.len();
    }
    let mut named = vec![];
    let mut opening = vec![];
    for c in re.captures_iter(readme) {
        let at = c.get(0).unwrap().start();
        let (alt, src) = match (c.get(2), c.get(3)) {
            (Some(src), _) => (c.get(1).map(|m| m.as_str()).unwrap_or("").to_string(), src.as_str().to_string()),
            (None, Some(attrs)) => match src_re.captures(attrs.as_str()) {
                Some(s) => (alt_re.captures(attrs.as_str()).map(|a| a[1].to_string()).unwrap_or_default(), s[1].to_string()),
                None => continue,
            },
            _ => continue,
        };
        if badge(&src) {
            continue;
        }
        let Some(resolved) = resolve(&src, readme_dir, repo) else { continue };
        if !resolved.starts_with("https://") && !is_image_path(&resolved) {
            continue;
        }
        let file = src.rsplit('/').next().unwrap_or("").to_ascii_lowercase();
        let says_logo = ["logo", "icon", "banner", "mark"].iter().any(|w| file.contains(w)) || alt.to_ascii_lowercase().contains("logo");
        if says_logo {
            named.push(resolved);
        } else if at < opening_end {
            opening.push(resolved);
        }
    }
    let mut out = vec![];
    for x in named.into_iter().chain(opening) {
        if !out.contains(&x) {
            out.push(x);
        }
    }
    out
}

/// A README image reference as a repository path or an https URL.
fn resolve(src: &str, readme_dir: &str, repo: &str) -> Option<String> {
    let src = src.trim();
    let lower = src.to_ascii_lowercase();
    for prefix in [format!("https://github.com/{}/blob/", repo.to_ascii_lowercase()), format!("https://github.com/{}/raw/", repo.to_ascii_lowercase()), format!("https://raw.githubusercontent.com/{}/", repo.to_ascii_lowercase())] {
        if lower.starts_with(&prefix) {
            // <branch>/<path>
            let rest = &src[prefix.len()..];
            let rest = rest.strip_prefix("refs/heads/").unwrap_or(rest);
            let path = rest.split_once('/')?.1.split(['?', '#']).next()?;
            return Some(urlencoding::decode(path).ok()?.into_owned());
        }
    }
    if lower.starts_with("https://") {
        return Some(src.to_string());
    }
    if lower.starts_with("http:") || lower.starts_with("data:") || lower.starts_with("//") || src.contains("://") {
        return None;
    }
    let path = src.split(['?', '#']).next()?;
    let joined = if let Some(abs) = path.strip_prefix('/') { abs.to_string() } else if readme_dir.is_empty() { path.to_string() } else { format!("{readme_dir}/{path}") };
    let mut parts: Vec<&str> = vec![];
    for seg in joined.split('/') {
        match seg {
            "" | "." => {}
            ".." => { parts.pop()?; }
            s => parts.push(s),
        }
    }
    (!parts.is_empty()).then(|| urlencoding::decode(&parts.join("/")).map(|s| s.into_owned()).unwrap_or_default())
}

/// A logo file in a repository's file list: `logo.*` (or `<anything>-logo.*`) at most three
/// folders deep, outside dependencies and build output; the shallowest wins.
pub fn tree_logo(paths: &[String]) -> Option<String> {
    let skip = ["node_modules/", "vendor/", "third_party/", "dist/", "build/", "target/", ".venv/", "site-packages/", "test", "example", "fixtures/", "pods/"];
    let mut best: Option<(usize, usize, &String)> = None;
    for p in paths {
        let l = p.to_ascii_lowercase();
        if skip.iter().any(|s| l.contains(s)) || l.split('/').count() > 4 || !is_image_path(&l) {
            continue;
        }
        let file = l.rsplit('/').next().unwrap_or("");
        let stem = file.rsplit_once('.').map(|(s, _)| s).unwrap_or(file);
        // Project-template logos (create-react-app and friends), not the project's own.
        if l == "src/logo.svg" || l.ends_with("/src/logo.svg") || stem.starts_with("logo192") || stem.starts_with("logo512") || stem == "logo-small" {
            continue;
        }
        let rank = if stem == "logo" { 0 } else if stem.starts_with("logo") || stem.ends_with("logo") || stem.ends_with("-logo") { 1 } else { continue };
        let key = (rank, l.split('/').count(), p);
        if best.is_none_or(|b| (key.0, key.1) < (b.0, b.1)) {
            best = Some(key);
        }
    }
    best.map(|(_, _, p)| p.clone())
}

// ------------------------------------------------------------------ logos: prompts, import, PRs

pub fn logo_prompts(site: &Site, size: usize) -> Result<Value> {
    let c = load(site)?;
    let batches = codecat::logo_batches(&c, size);
    let n = batches.len();
    Ok(json!({
        "size": size, "count": batches.iter().map(|b| b.len()).sum::<usize>(), "batches": batches.iter().enumerate().map(|(i, b)| json!({
            "number": i + 1, "items": b, "text": codecat::batch_text(b, i + 1, n),
        })).collect::<Vec<_>>(),
        "style": c.style(),
    }))
}

/// Import a rendered logo for a project (`slug`, or matched from `name`): scaled into the site,
/// the full-size image kept for the repository PR.
pub fn import_logo(site: &Site, slug: Option<&str>, name: &str, bytes: &[u8]) -> Result<Value> {
    let c = load(site)?;
    let fam = match slug.filter(|s| !s.is_empty()) {
        Some(s) => s.strip_prefix("family-").map(|id| c.family(id).with_context(|| format!("no family {id}"))).transpose()?,
        None => codecat::match_family_file(&c, name),
    };
    if let Some(f) = fam {
        return import_family_logo(site, &f.id.clone(), name, bytes);
    }
    let p = match slug.filter(|s| !s.is_empty()) {
        Some(s) => c.get(s).with_context(|| format!("no project {s}"))?,
        None => codecat::match_logo_file(&c, name).with_context(|| format!("{name}: no project matches this file name (save it as <slug>.png)"))?,
    };
    let slug = p.slug.clone();
    let (small, ext) = codecat::normalise_logo(bytes, SITE_PX)?;
    let (big, _) = codecat::normalise_logo(bytes, REPO_PX)?;
    let dir = logo_dir(site);
    std::fs::create_dir_all(&dir)?;
    remove_logo_files(&dir, &slug)?;
    std::fs::write(dir.join(format!("{slug}.{ext}")), small)?;
    let src = source_dir(site);
    std::fs::create_dir_all(&src)?;
    remove_logo_files(&src, &slug)?;
    std::fs::write(src.join(format!("{slug}.{ext}")), big)?;
    modify(site, |c| {
        let p = c.get_mut(&slug).unwrap();
        p.logo = format!("{slug}.{ext}");
        p.logo_source = "imported".into();
        p.logo_pr.clear();
        Ok(())
    })?;
    Ok(json!({ "slug": slug, "file": format!("{slug}.{ext}"), "from": name }))
}

/// A family's mark: saved as `family-<id>.<ext>` and shown by every member without its own logo.
fn import_family_logo(site: &Site, id: &str, name: &str, bytes: &[u8]) -> Result<Value> {
    let stem = format!("family-{id}");
    let (small, ext) = codecat::normalise_logo(bytes, SITE_PX)?;
    let (big, _) = codecat::normalise_logo(bytes, REPO_PX)?;
    for (dir, data) in [(logo_dir(site), small), (source_dir(site), big)] {
        std::fs::create_dir_all(&dir)?;
        remove_logo_files(&dir, &stem)?;
        std::fs::write(dir.join(format!("{stem}.{ext}")), data)?;
    }
    let members = modify(site, |c| {
        c.families.iter_mut().find(|f| f.id == id).context("family gone")?.logo = format!("{stem}.{ext}");
        // A new mark: members' earlier pull requests carried the old one.
        let mut n = 0;
        for p in c.projects.iter_mut().filter(|p| p.family == id && p.logo.is_empty()) {
            p.logo_pr.clear();
            n += 1;
        }
        Ok(n)
    })?;
    Ok(json!({ "slug": stem, "family": id, "members": members, "file": format!("{stem}.{ext}"), "from": name }))
}

/// The pickup folder for rendered logos (code.yaml `logo_inbox`, default ~/Pictures/gitwebsite-logos).
pub fn inbox(site: &Site) -> Result<PathBuf> {
    let c = load(site)?;
    Ok(gw_core::config::expand_home(if c.logo_inbox.trim().is_empty() { codecat::DEFAULT_INBOX } else { c.logo_inbox.trim() }))
}

const INBOX_README: &str = "Rendered logos for the gitwebsite code catalog.\n\n\
prompts/batch-NN.txt  paste one into ChatGPT; it names the file for each image\n\
prompts/batch-NN.json the same batch as data (the order, for images saved without their names)\n\n\
Save the images here (or ChatGPT's zip of a batch). Then import: the app's Code > Logos page\n\
(Import from the pickup folder), `gw code import-logos`, or ask Claude. Images named as asked are\n\
matched by name; others by the order they were saved, with --batch N. Imported files move to imported/.\n";

/// Write every batch of prompts into the pickup folder's `prompts/` (replacing earlier ones): a
/// `.txt` to paste into ChatGPT and a `.json` with the order and file names.
pub fn write_prompt_files(site: &Site, size: usize) -> Result<Value> {
    let c = load(site)?;
    let dir = inbox(site)?;
    let pdir = dir.join("prompts");
    std::fs::create_dir_all(&pdir)?;
    std::fs::write(dir.join("README.txt"), INBOX_README)?;
    for e in std::fs::read_dir(&pdir)?.flatten() {
        let n = e.file_name().to_string_lossy().to_string();
        if n.starts_with("batch-") && (n.ends_with(".txt") || n.ends_with(".json")) {
            std::fs::remove_file(e.path())?;
        }
    }
    let batches = codecat::logo_batches(&c, size);
    let mut files = vec![];
    for (i, b) in batches.iter().enumerate() {
        let n = i + 1;
        std::fs::write(pdir.join(format!("batch-{n:02}.txt")), codecat::batch_text(b, n, batches.len()))?;
        std::fs::write(pdir.join(format!("batch-{n:02}.json")), serde_json::to_string_pretty(&json!({ "batch": n, "of": batches.len(), "written": today(), "items": b }))?)?;
        files.push(format!("batch-{n:02}.txt"));
    }
    Ok(json!({ "inbox": dir.display().to_string(), "prompts": pdir.display().to_string(), "batches": batches.len(), "logos": batches.iter().map(|b| b.len()).sum::<usize>(), "files": files }))
}

/// Images in a folder (top level), and the images inside any zip files there (unpacked with
/// ditto into `.unzipped/`). Returns (image, the file to move away when done).
fn collect_images(dir: &Path) -> Result<Vec<(PathBuf, PathBuf)>> {
    let mut out = vec![];
    for e in std::fs::read_dir(dir)?.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if !p.is_file() || name.starts_with('.') {
            continue;
        }
        if is_image_path(&name) {
            out.push((p.clone(), p));
        } else if name.to_ascii_lowercase().ends_with(".zip") {
            let to = dir.join(".unzipped").join(name.trim_end_matches(".zip"));
            let _ = std::fs::remove_dir_all(&to);
            std::fs::create_dir_all(&to)?;
            let ok = Command::new("/usr/bin/ditto").args(["-x", "-k"]).arg(&p).arg(&to).status().map(|s| s.success()).unwrap_or(false);
            if !ok {
                continue;
            }
            for f in walkdir::WalkDir::new(&to).into_iter().flatten().filter(|f| f.file_type().is_file()) {
                let n = f.file_name().to_string_lossy().to_string();
                if is_image_path(&n) && !n.starts_with('.') && !f.path().components().any(|c| c.as_os_str() == "__MACOSX") {
                    out.push((f.path().to_path_buf(), p.clone()));
                }
            }
        }
    }
    Ok(out)
}

fn mtime(p: &Path) -> std::time::SystemTime {
    std::fs::metadata(p).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH)
}

/// Import rendered logos from files and folders — by default the pickup folder. Images named as
/// the prompts asked are matched by name. With `batch`, images whose names match nothing are
/// matched to that batch's logos still missing, in the order they were saved (oldest first) —
/// only when the counts agree. Imported files (and zips) in the pickup folder move to `imported/`.
pub fn import_logos(site: &Site, paths: &[PathBuf], batch: Option<usize>) -> Result<Value> {
    let inbox_dir = inbox(site)?;
    let paths: Vec<PathBuf> = if paths.is_empty() { vec![inbox_dir.clone()] } else { paths.iter().map(|p| gw_core::config::expand_home(&p.to_string_lossy())).collect() };
    let mut files: Vec<(PathBuf, PathBuf)> = vec![];
    for p in &paths {
        if p.is_dir() {
            files.extend(collect_images(p)?);
        } else if p.is_file() {
            files.push((p.clone(), p.clone()));
        } else {
            bail!("{} is not there", p.display());
        }
    }
    files.sort_by_key(|(f, _)| (mtime(f), f.clone()));
    let (mut imported, mut unmatched) = (vec![], vec![]);
    let mut done_sources: BTreeSet<PathBuf> = BTreeSet::new();
    let mut failed_sources: BTreeSet<PathBuf> = BTreeSet::new();
    let mut by_order: Vec<(PathBuf, PathBuf)> = vec![];
    let name_of = |f: &Path| f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    for (f, src) in files {
        let name = name_of(&f);
        let c = load(site)?;
        if codecat::match_family_file(&c, &name).is_none() && codecat::match_logo_file(&c, &name).is_none() {
            by_order.push((f, src));
            continue;
        }
        match std::fs::read(&f).map_err(anyhow::Error::from).and_then(|b| import_logo(site, None, &name, &b)) {
            Ok(mut v) => {
                v["by"] = json!("name");
                imported.push(v);
                done_sources.insert(src);
            }
            Err(e) => {
                unmatched.push(json!({ "file": f.display().to_string(), "error": format!("{e:#}") }));
                failed_sources.insert(src);
            }
        }
    }
    let mut note = String::new();
    if !by_order.is_empty() {
        match batch {
            None => {
                for (f, src) in by_order.drain(..) {
                    unmatched.push(json!({ "file": f.display().to_string(), "error": "the name matches no project (pass the batch number to match by order)" }));
                    failed_sources.insert(src);
                }
            }
            Some(n) => {
                let man = inbox_dir.join("prompts").join(format!("batch-{n:02}.json"));
                let m: Value = serde_json::from_str(&std::fs::read_to_string(&man).with_context(|| format!("no {} — write the prompt files first", man.display()))?)?;
                let items: Vec<codecat::PromptItem> = serde_json::from_value(m["items"].clone())?;
                let c = load(site)?;
                let open: Vec<&codecat::PromptItem> = items
                    .iter()
                    .filter(|i| match i.slug.strip_prefix("family-") {
                        Some(id) => c.family(id).is_some_and(|f| f.logo.is_empty()),
                        None => c.get(&i.slug).is_some_and(|p| p.logo.is_empty()),
                    })
                    .collect();
                if open.len() != by_order.len() {
                    note = format!("batch {n} still needs {} logos but {} images have names that match nothing — nothing was matched by order; rename them (or remove extras) and try again", open.len(), by_order.len());
                    for (f, src) in by_order.drain(..) {
                        unmatched.push(json!({ "file": f.display().to_string(), "error": "not matched" }));
                        failed_sources.insert(src);
                    }
                } else {
                    for ((f, src), it) in by_order.drain(..).zip(open) {
                        match std::fs::read(&f).map_err(anyhow::Error::from).and_then(|b| import_logo(site, Some(&it.slug), &name_of(&f), &b)) {
                            Ok(mut v) => {
                                v["by"] = json!("order");
                                v["for"] = json!(it.title);
                                imported.push(v);
                                done_sources.insert(src);
                            }
                            Err(e) => {
                                unmatched.push(json!({ "file": f.display().to_string(), "error": format!("{e:#}") }));
                                failed_sources.insert(src);
                            }
                        }
                    }
                }
            }
        }
    }
    // Move what was imported out of the pickup folder (a zip only when all of it went in).
    let done = inbox_dir.join("imported");
    for src in done_sources.difference(&failed_sources) {
        if src.parent() == Some(inbox_dir.as_path()) {
            std::fs::create_dir_all(&done)?;
            let mut to = done.join(name_of(src));
            if to.exists() {
                to = done.join(format!("{}-{}", chrono::Local::now().format("%Y%m%d%H%M%S"), name_of(src)));
            }
            std::fs::rename(src, &to)?;
        }
    }
    let _ = std::fs::remove_dir_all(inbox_dir.join(".unzipped"));
    Ok(json!({ "inbox": inbox_dir.display().to_string(), "imported": imported, "unmatched": unmatched, "note": note }))
}

fn relative(from_dir: &str, to: &str) -> String {
    let ups = if from_dir.is_empty() { 0 } else { from_dir.split('/').count() };
    format!("{}{to}", "../".repeat(ups))
}

/// Open a pull request on the project's repository that adds its imported logo (as
/// `assets/logo.<ext>`) and shows it at the top of the README.
pub fn logo_pr(site: &Site, slug: &str, merge: bool, log: &dyn Fn(String)) -> Result<Value> {
    let c = load(site)?;
    let p = c.get(slug).with_context(|| format!("no project {slug}"))?.clone();
    let (file, from_family) = c.logo_of(&p).map(|(f, fam)| (f.to_string(), fam)).with_context(|| format!("{slug} has no logo yet"))?;
    if !from_family && p.logo_source != "imported" {
        bail!("{slug}'s logo came from its repository ({}); there is nothing to add", p.github.logo_path);
    }
    if !p.logo_pr.is_empty() {
        bail!("{slug} already has a logo PR: {}", p.logo_pr);
    }
    let ext = file.rsplit_once('.').map(|(_, e)| e).unwrap_or("png");
    let src = source_dir(site).join(&file);
    let bytes = std::fs::read(&src).or_else(|_| std::fs::read(logo_dir(site).join(&file))).context("reading the logo")?;
    let repo = &p.repo;
    let meta = gh_json(&["api", &format!("repos/{repo}")])?;
    let base = meta["default_branch"].as_str().context("no default branch")?.to_string();
    if meta["permissions"]["push"].as_bool() == Some(false) {
        bail!("you can't push to {repo}");
    }
    let sha = gh_json(&["api", &format!("repos/{repo}/git/ref/heads/{}", enc_path(&base))])?["object"]["sha"].as_str().context("no commit on the default branch")?.to_string();
    let mut head = PR_BRANCH.to_string();
    let mut n = 2;
    while gh_get_opt(&format!("repos/{repo}/git/ref/heads/{}", enc_path(&head)))?.is_some() {
        head = format!("{PR_BRANCH}-{n}");
        n += 1;
    }
    log(format!("{repo}: branch {head} from {base}"));
    gh_send("POST", &format!("repos/{repo}/git/refs"), &json!({ "ref": format!("refs/heads/{head}"), "sha": sha }))?;
    // A family mark never replaces a logo the repository already has elsewhere.
    let logo_path = if from_family { format!("assets/logo.{ext}") } else { codecat::repo_logo_path(&p, ext) };
    let existing = gh_get_opt(&format!("repos/{repo}/contents/{}?ref={}", enc_path(&logo_path), urlencoding::encode(&head)))?;
    let mut body = json!({ "message": format!("Add the project logo ({logo_path})"), "content": base64::engine::general_purpose::STANDARD.encode(&bytes), "branch": head });
    if let Some(sha) = existing.as_ref().and_then(|e| e["sha"].as_str()) {
        body["sha"] = json!(sha);
    }
    gh_send("PUT", &format!("repos/{repo}/contents/{}", enc_path(&logo_path)), &body)?;
    log(format!("{repo}: added {logo_path}"));
    let title = p.display_title();
    let (readme_path, readme_sha, readme) = match gh_get_opt(&format!("repos/{repo}/readme?ref={}", urlencoding::encode(&head)))? {
        Some(r) => {
            let text = r["content"].as_str().map(|s| s.replace('\n', "")).and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok()).map(|b| String::from_utf8_lossy(&b).to_string()).unwrap_or_default();
            (r["path"].as_str().unwrap_or("README.md").to_string(), r["sha"].as_str().map(str::to_string), text)
        }
        None => ("README.md".to_string(), None, format!("# {title}\n\n{}\n", p.display_summary())),
    };
    let dir = readme_path.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
    let lower = readme_path.to_ascii_lowercase();
    let is_adoc = lower.ends_with(".adoc") || lower.ends_with(".asciidoc");
    if is_adoc || lower.ends_with(".md") || !readme_path.contains('.') {
        let img = relative(&dir, &logo_path);
        let alt = format!("{title} logo");
        let updated = if is_adoc { codecat::adoc_readme_with_logo(&readme, &img, &alt) } else { codecat::readme_with_logo(&readme, &img, &alt) };
        if updated != readme {
            let mut body = json!({ "message": "Show the project logo at the top of the README", "content": base64::engine::general_purpose::STANDARD.encode(updated.as_bytes()), "branch": head });
            if let Some(s) = readme_sha {
                body["sha"] = json!(s);
            }
            gh_send("PUT", &format!("repos/{repo}/contents/{}", enc_path(&readme_path)), &body)?;
            log(format!("{repo}: logo added to {readme_path}"));
        }
    } else {
        log(format!("{repo}: {readme_path} is neither Markdown nor AsciiDoc; only the logo file is added"));
    }
    let pr = gh_send("POST", &format!("repos/{repo}/pulls"), &json!({
        "title": "Add a project logo",
        "head": head, "base": base,
        "body": format!("Adds a logo for {title} (`{logo_path}`) and shows it at the top of the README.\n\nThe same logo is used for this project in the code catalog on {}{}{}/.\n\n🤖 Opened by gitwebsite", site.config.site.base_url.trim_end_matches('/'), codecat::URL, p.slug),
    }))?;
    let url = pr["html_url"].as_str().context("GitHub did not return the pull request")?.to_string();
    log(format!("{repo}: {url}"));
    // Merging (squash) is optional; a PR that can't be merged stays open and is still recorded.
    let merged = if merge {
        let n = pr["number"].as_u64().context("GitHub did not return the PR number")?;
        match gh_send("PUT", &format!("repos/{repo}/pulls/{n}/merge"), &json!({ "merge_method": "squash", "commit_title": format!("Add a project logo (#{n})") })) {
            Ok(_) => {
                log(format!("{repo}: merged"));
                if let Err(e) = gh_raw(&["api", "-X", "DELETE", &format!("repos/{repo}/git/refs/heads/{}", enc_path(&head))], None) {
                    log(format!("{repo}: couldn't delete {head}: {e:#}"));
                }
                true
            }
            Err(e) => {
                log(format!("{repo}: not merged: {e:#}"));
                false
            }
        }
    } else {
        false
    };
    modify(site, |c| {
        if let Some(x) = c.get_mut(slug) {
            x.logo_pr = url.clone();
            x.github.logo_path = logo_path.clone();
        }
        Ok(())
    })?;
    Ok(json!({ "slug": slug, "pr": url, "merged": merged, "branch": head, "logo_path": logo_path, "readme": readme_path }))
}

/// Projects whose imported logo has no PR yet.
pub fn awaiting_pr(site: &Site) -> Result<Vec<String>> {
    Ok(pr_candidates(&load(site)?))
}

pub fn logo_prs(site: &Site, slugs: &[String], merge: bool, log: &dyn Fn(String)) -> Result<Value> {
    let (mut ok, mut failed) = (vec![], vec![]);
    for (i, s) in slugs.iter().enumerate() {
        log(format!("[{}/{}] {s}", i + 1, slugs.len()));
        match logo_pr(site, s, merge, log) {
            Ok(v) => ok.push(v),
            Err(e) => {
                log(format!("{s}: {e:#}"));
                failed.push(json!({ "slug": s, "error": format!("{e:#}") }));
            }
        }
    }
    Ok(json!({ "opened": ok, "failed": failed }))
}

// ------------------------------------------------------------------ Claude

/// Ask Claude (the `claude` CLI, no tools) and parse the JSON object in its answer.
fn ask_claude(site: &Site, prompt: &str) -> Result<Value> {
    use std::io::Write;
    let claude = crate::enhance::claude_bin().context("can't find the claude CLI; install Claude Code or set GW_CLAUDE_BIN")?;
    let dir = site.cache_dir();
    std::fs::create_dir_all(&dir)?;
    let mut child = Command::new(claude)
        .args(["-p", "--output-format", "json", "--tools", "", "--no-session-persistence"])
        .current_dir(&dir)
        .env("GW_ACTOR", "claude")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("starting claude")?;
    child.stdin.take().context("claude stdin")?.write_all(prompt.as_bytes())?;
    let out = child.wait_with_output()?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let v: Value = serde_json::from_str(&text).with_context(|| format!("claude: {} {}", text.chars().take(300).collect::<String>(), String::from_utf8_lossy(&out.stderr).chars().take(300).collect::<String>()))?;
    if v["is_error"].as_bool() == Some(true) || !out.status.success() {
        bail!("claude: {}", v["result"].as_str().unwrap_or("failed"));
    }
    let answer = v["result"].as_str().unwrap_or("");
    parse_json_object(answer).context("Claude's answer held no JSON object")
}

/// The first JSON object in a text (Claude may wrap it in a code fence).
pub fn parse_json_object(text: &str) -> Option<Value> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    serde_json::from_str(&text[start..=end]).ok()
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { s.chars().take(n).collect::<String>() + " …(truncated)" }
}

const VOICE: &str = "Write for the website of the person who built these projects: plain, specific and concrete, \
first person where it reads naturally (\"I built this to…\"), no marketing language, no hype words (powerful, seamless, \
revolutionary, cutting-edge), no emoji. Say what it does, who it is for, and what is interesting about how it works. \
Base everything on the README and facts given; never invent features, numbers or users.";

/// The context Claude needs for one project: its fields, facts, README, the categories and the
/// other projects (for related ones).
pub fn context(site: &Site, slug: &str) -> Result<Value> {
    let c = load(site)?;
    let p = c.get(slug).with_context(|| format!("no project {slug}"))?;
    let readme = match gh_get_opt(&format!("repos/{}/readme", p.repo)) {
        Ok(Some(r)) => r["content"].as_str().map(|s| s.replace('\n', "")).and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok()).map(|b| String::from_utf8_lossy(&b).to_string()).unwrap_or_default(),
        _ => String::new(),
    };
    let others: Vec<String> = c.projects.iter().filter(|q| q.listed() && q.slug != p.slug).map(|q| format!("{} — {}: {}", q.slug, q.display_title(), clip(&q.display_summary(), 110))).collect();
    Ok(json!({
        "project": p, "display_title": p.display_title(), "display_summary": p.display_summary(),
        "readme": clip(&readme, 16000),
        "categories": c.categories, "other_projects": others,
        "voice": VOICE, "logo_style": c.style(),
    }))
}

/// Claude's proposal for one project (nothing is saved).
pub fn enhance(site: &Site, slug: &str, log: &dyn Fn(String)) -> Result<Value> {
    log(format!("Reading {slug}'s README…"));
    let ctx = context(site, slug)?;
    let p = &ctx["project"];
    let prompt = format!(
        "You are improving one entry in a catalog of git projects on a personal website.\n\n{VOICE}\n\n\
Project: {title} (GitHub {repo})\nGitHub description: {gdesc}\nLanguage: {lang}\nTopics: {topics}\n\n\
The entry now:\ntitle: {title}\nsummary: {summary}\ndescription:\n{desc}\ncategory: {cat}\ntags: {tags}\nrelated: {related}\n\n\
Categories (id: title):\n{cats}\n\nOther projects in the catalog (slug — title: summary):\n{others}\n\n\
README:\n<<<\n{readme}\n>>>\n\n\
Reply with ONLY a JSON object, no prose before or after:\n\
{{\"title\": \"display name, short, properly cased\",\n\
 \"summary\": \"one sentence, at most 140 characters, for the project's tile\",\n\
 \"description\": \"2 to 4 short paragraphs separated by blank lines for the project's page: what it is, why it exists, how it works or what is notable, how to try it. You may use `code`, **bold**, [text](https://…) links and lines starting with '- ' for a short list.\",\n\
 \"tags\": [\"3 to 8 lower-case tags, hyphenated words\"],\n\
 \"category\": \"the best category id from the list (or empty if none fits)\",\n\
 \"related\": [\"0 to 5 slugs from the other projects that are genuinely related\"],\n\
 \"logo_concept\": \"one or two sentences describing a simple symbol for this project's logo — an object or metaphor, no text\"}}",
        title = ctx["display_title"].as_str().unwrap_or(""),
        repo = p["repo"].as_str().unwrap_or(""),
        gdesc = p["github"]["description"].as_str().unwrap_or(""),
        lang = p["github"]["language"].as_str().unwrap_or(""),
        topics = p["github"]["topics"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default(),
        summary = ctx["display_summary"].as_str().unwrap_or(""),
        desc = p["description"].as_str().unwrap_or(""),
        cat = p["category"].as_str().unwrap_or(""),
        tags = p["tags"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default(),
        related = p["related"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default(),
        cats = ctx["categories"].as_array().map(|a| a.iter().map(|c| format!("{}: {}", c["id"].as_str().unwrap_or(""), c["title"].as_str().unwrap_or(""))).collect::<Vec<_>>().join("\n")).unwrap_or_default(),
        others = ctx["other_projects"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default(),
        readme = ctx["readme"].as_str().filter(|r| !r.is_empty()).unwrap_or("(no README)"),
    );
    log("Asking Claude…".into());
    let v = ask_claude(site, &prompt)?;
    let c = load(site)?;
    let slugs: BTreeSet<&str> = c.projects.iter().map(|p| p.slug.as_str()).collect();
    let cats: BTreeSet<&str> = c.categories.iter().map(|x| x.id.as_str()).collect();
    let s = |k: &str| v[k].as_str().unwrap_or("").trim().to_string();
    let list = |k: &str| v[k].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect::<Vec<_>>()).unwrap_or_default();
    let title = s("title");
    let concept = s("logo_concept");
    let logo_prompt = if concept.is_empty() {
        String::new()
    } else {
        format!("Design a logo for \"{}\" — {}. Concept: {} {}", if title.is_empty() { ctx["display_title"].as_str().unwrap_or("") } else { &title }, s("summary").trim_end_matches('.'), concept, c.style())
    };
    let proposal = json!({
        "slug": slug,
        "title": title, "summary": s("summary"), "description": s("description"),
        "tags": clean_tags(&list("tags")),
        "category": Some(s("category")).filter(|x| cats.contains(x.as_str())).unwrap_or_default(),
        "related": list("related").into_iter().filter(|r| slugs.contains(r.as_str()) && r != slug).take(5).collect::<Vec<_>>(),
        "logo_prompt": logo_prompt,
    });
    log("Proposal ready".into());
    Ok(json!({ "proposal": proposal, "current": show(site, slug)? }))
}

/// Claude's category scheme and tags for every listed project (nothing is saved).
pub fn suggest(site: &Site, log: &dyn Fn(String)) -> Result<Value> {
    let c = load(site)?;
    let listed: Vec<&Project> = c.projects.iter().filter(|p| p.listed()).collect();
    if listed.is_empty() {
        bail!("no projects yet: run a sync first");
    }
    let lines: Vec<String> = listed
        .iter()
        .map(|p| format!("{} | {} | {} | {} | {}", p.slug, p.display_title(), clip(&p.display_summary(), 160), p.github.language, p.github.topics.join(",")))
        .collect();
    let cats = c.categories.iter().map(|x| format!("{}: {} — {}", x.id, x.title, x.description)).collect::<Vec<_>>().join("\n");
    let prompt = format!(
        "You are organising a catalog of {n} git projects on a personal website into categories, like the sections of a tools page.\n\n\
Projects (slug | title | summary | language | topics):\n{list}\n\n\
Existing categories (keep and reuse those that fit; you may rename or add):\n{cats}\n\n\
Choose 6 to 14 categories that a visitor would use to browse — by what the projects are for (for example: AI and agents, \
Retro computing, Developer tools, Search, Data and streaming, Games, Hardware and electronics, Web apps, Learning), not by \
programming language. Every project goes in exactly one category. Also give each project 2 to 6 lower-case tags (hyphenated words) \
that help filtering across categories (technologies, platforms, themes).\n\n\
Reply with ONLY a JSON object, no prose before or after:\n\
{{\"categories\": [{{\"id\": \"short-kebab-id\", \"title\": \"Title\", \"description\": \"one sentence\"}}],\n \"projects\": {{\"<slug>\": {{\"category\": \"<id>\", \"tags\": [\"…\"]}}}}}}",
        n = listed.len(),
        list = lines.join("\n"),
        cats = if cats.is_empty() { "(none yet)".into() } else { cats },
    );
    log(format!("Asking Claude to organise {} projects… (this takes a minute or two)", listed.len()));
    let v = ask_claude(site, &prompt)?;
    let categories: Vec<Category> = v["categories"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| {
            let id = codecat::slug_for(x["id"].as_str()?);
            Some(Category { id, title: x["title"].as_str()?.trim().into(), description: x["description"].as_str().unwrap_or("").trim().into() })
        }).collect())
        .unwrap_or_default();
    if categories.is_empty() {
        bail!("Claude proposed no categories");
    }
    let ids: BTreeSet<&str> = categories.iter().map(|c| c.id.as_str()).collect();
    let mut projects: BTreeMap<String, Value> = BTreeMap::new();
    if let Some(m) = v["projects"].as_object() {
        for (slug, x) in m {
            if c.get(slug).is_none() {
                continue;
            }
            let cat = x["category"].as_str().map(codecat::slug_for).filter(|k| ids.contains(k.as_str())).unwrap_or_default();
            let tags = clean_tags(&x["tags"].as_array().map(|a| a.iter().filter_map(|t| t.as_str().map(str::to_string)).collect::<Vec<_>>()).unwrap_or_default());
            projects.insert(slug.clone(), json!({ "category": cat, "tags": tags }));
        }
    }
    log(format!("{} categories for {} projects", categories.len(), projects.len()));
    Ok(json!({ "categories": categories, "projects": projects }))
}

/// Apply a category scheme (from [`suggest`], perhaps edited): the categories replace the list;
/// each project's category is set (only where it has none, unless `overwrite`); tags are added
/// (or replace the project's, with `overwrite`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Scheme {
    pub categories: Vec<Category>,
    pub projects: BTreeMap<String, SchemeEntry>,
    pub overwrite: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SchemeEntry {
    pub category: String,
    pub tags: Vec<String>,
}

pub fn apply_scheme(site: &Site, s: &Scheme) -> Result<Value> {
    let (mut cats_set, mut tags_set) = (0, 0);
    modify(site, |c| {
        if !s.categories.is_empty() {
            c.categories = s.categories.iter().map(|x| Category { id: codecat::slug_for(&x.id), title: x.title.trim().into(), description: x.description.trim().into() }).collect();
        }
        let ids: BTreeSet<String> = c.categories.iter().map(|x| x.id.clone()).collect();
        for p in c.projects.iter_mut() {
            if !p.category.is_empty() && !ids.contains(&p.category) {
                p.category.clear();
            }
            let Some(e) = s.projects.get(&p.slug) else { continue };
            if ids.contains(&e.category) && (s.overwrite || p.category.is_empty()) && p.category != e.category {
                p.category = e.category.clone();
                cats_set += 1;
            }
            let new = clean_tags(&e.tags);
            if s.overwrite {
                if p.tags != new && !new.is_empty() {
                    p.tags = new;
                    tags_set += 1;
                }
            } else {
                let before = p.tags.len();
                let mut all = p.tags.clone();
                all.extend(new);
                p.tags = clean_tags(&all);
                if p.tags.len() != before {
                    tags_set += 1;
                }
            }
        }
        Ok(())
    })?;
    let mut v = state(site)?;
    v["applied"] = json!({ "categories_set": cats_set, "tags_changed": tags_set });
    Ok(v)
}

// ------------------------------------------------------------------ background jobs for the app

pub fn start_sync(site: &Site, logos: bool, refresh_logos: bool) -> Result<Value> {
    start_job(site, "sync", "GitHub", move |s, log| Ok(serde_json::to_value(sync(s, logos, refresh_logos, log)?)?))
}
pub fn start_enhance(site: &Site, slug: &str) -> Result<Value> {
    load(site)?.get(slug).with_context(|| format!("no project {slug}"))?;
    let slug = slug.to_string();
    start_job(site, "enhance", &slug.clone(), move |s, log| enhance(s, &slug, log))
}
pub fn start_suggest(site: &Site) -> Result<Value> {
    start_job(site, "suggest", "categories", move |s, log| suggest(s, log))
}
pub fn start_logo_prs(site: &Site, slugs: Vec<String>, merge: bool) -> Result<Value> {
    if slugs.is_empty() {
        bail!("no logos waiting for a pull request");
    }
    start_job(site, "logo-pr", &format!("{} repositories", slugs.len()), move |s, log| logo_prs(s, &slugs, merge, log))
}

// ------------------------------------------------------------------ CLI

#[derive(clap::Subcommand, Debug)]
pub enum CodeCmd {
    /// The catalog: every project with its category, tags, logo and whether it is listed.
    List {
        /// Only projects without a logo (listed ones).
        #[arg(long)]
        need_logo: bool,
    },
    /// One project, with the logo prompt it would get.
    Show { slug: String },
    /// Read your repositories from GitHub (gh) into code.yaml: new public repos are added (forks
    /// and archived ones hidden), facts refreshed, and logos found in READMEs copied into the site.
    Sync {
        /// Skip looking for logos.
        #[arg(long)]
        no_logos: bool,
        /// Look again for logos of projects that already have one (except imported ones).
        #[arg(long)]
        refresh_logos: bool,
    },
    /// Change a project: gw code set amiga-mcp --summary "…" --tags amiga,mcp --related amiga-games
    Set {
        slug: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        summary: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        category: Option<String>,
        /// Comma separated (replaces the tags).
        #[arg(long)]
        tags: Option<String>,
        /// Comma separated slugs (replaces the related projects).
        #[arg(long)]
        related: Option<String>,
        #[arg(long, conflicts_with = "show")]
        hide: bool,
        #[arg(long)]
        show: bool,
        #[arg(long)]
        featured: Option<bool>,
        #[arg(long)]
        logo_prompt: Option<String>,
        /// The post that writes it up (content id), if not found by its :repo:.
        #[arg(long)]
        post: Option<String>,
    },
    /// Add or change a category (or --remove it).
    Category {
        id: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        remove: bool,
    },
    /// A logo family — projects sharing one mark: gw code family hitorro --title HiTorro
    /// --concept "a stylised red bull" --category hitorro --category hitorro-mesh
    Family {
        id: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        description: Option<String>,
        /// The mark's idea, for the prompt.
        #[arg(long)]
        concept: Option<String>,
        /// Add these projects (comma separated slugs).
        #[arg(long)]
        add: Option<String>,
        /// Add every project in this category (repeatable).
        #[arg(long)]
        category: Vec<String>,
        /// Take these projects out (comma separated).
        #[arg(long)]
        drop: Option<String>,
        /// Delete the family (its members keep their projects, lose the mark).
        #[arg(long)]
        remove: bool,
    },
    /// Ask Claude for a better title, summary, description, tags, category, related projects and
    /// logo prompt for a project, from its README. Shows the proposal; --apply saves it.
    Enhance {
        slug: String,
        #[arg(long)]
        apply: bool,
    },
    /// Ask Claude for categories and tags for the whole catalog. Shows them; --apply saves them
    /// (setting categories only where a project has none, unless --overwrite).
    Suggest {
        #[arg(long)]
        apply: bool,
        #[arg(long)]
        overwrite: bool,
    },
    /// Image prompts for the logos still missing, in batches to paste into ChatGPT.
    LogoPrompts {
        /// Logos per batch.
        #[arg(long, default_value_t = 15)]
        size: usize,
        /// Write every batch into the pickup folder's prompts/ (batch-NN.txt and .json).
        #[arg(long)]
        write: bool,
        /// Only this batch (1-based).
        #[arg(long)]
        batch: Option<usize>,
        /// Print JSON instead of the text to paste.
        #[arg(long)]
        json: bool,
    },
    /// Import rendered logos: from the pickup folder (default), or image files, folders or zips.
    /// Named <slug>.png / family-<id>.png they match by name; --batch N matches the rest by the
    /// order they were saved against that batch's prompt file.
    ImportLogos {
        paths: Vec<PathBuf>,
        #[arg(long)]
        batch: Option<usize>,
    },
    /// Open a pull request on each repository adding its imported logo (assets/logo.png) to the
    /// top of its README. Give slugs, or --all for every imported logo without a PR.
    LogoPr {
        slugs: Vec<String>,
        #[arg(long)]
        all: bool,
        /// Also merge each pull request (squash) and delete its branch.
        #[arg(long)]
        merge: bool,
        /// Required: you have checked the logos and want the pull requests opened.
        #[arg(long)]
        yes: bool,
    },
}

pub fn run_cli(site: &Site, c: CodeCmd) -> Result<Value> {
    let print = |l: String| eprintln!("{l}");
    match c {
        CodeCmd::List { need_logo } => {
            let mut v = state(site)?;
            if need_logo {
                if let Some(a) = v["projects"].as_array_mut() {
                    a.retain(|p| p["listed"] == true && p["logo"].as_str().unwrap_or("").is_empty());
                }
            }
            let rows: Vec<Value> = v["projects"].as_array().cloned().unwrap_or_default().into_iter().map(|p| json!({
                "slug": p["slug"], "repo": p["repo"], "title": p["display_title"], "category": p["category"], "tags": p["tags"],
                "listed": p["listed"], "logo": p["logo"], "summary": p["display_summary"],
            })).collect();
            Ok(json!({ "counts": v["counts"], "categories": v["categories"], "projects": rows }))
        }
        CodeCmd::Show { slug } => show(site, &slug),
        CodeCmd::Sync { no_logos, refresh_logos } => Ok(serde_json::to_value(sync(site, !no_logos, refresh_logos, &print)?)?),
        CodeCmd::Set { slug, title, summary, description, category, tags, related, hide, show: shown, featured, logo_prompt, post } => {
            let split = |s: Option<String>| s.map(|s| s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect::<Vec<_>>());
            update(site, &slug, &Patch {
                title, summary, description, category, tags: split(tags), related: split(related),
                hidden: if hide { Some(true) } else if shown { Some(false) } else { None },
                featured, logo_prompt, post, ..Default::default()
            })
        }
        CodeCmd::Category { id, title, description, remove } => category(site, &id, title.as_deref(), description.as_deref(), remove),
        CodeCmd::Family { id, title, description, concept, add, category, drop, remove } => {
            let split = |s: Option<String>| s.map(|s| s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect::<Vec<_>>()).unwrap_or_default();
            let v = family(site, &FamilyChange { id, title, description, concept, logo_prompt: None, add: split(add), categories: category, drop: split(drop), remove })?;
            Ok(json!({ "added": v["added"], "families": v["families"].as_array().map(|a| a.iter().map(|f| json!({ "id": f["id"], "title": f["title"], "members": f["members"].as_array().map(|m| m.len()), "logo": f["logo"], "prompt": f["prompt"] })).collect::<Vec<_>>()) }))
        }
        CodeCmd::Enhance { slug, apply } => {
            let v = enhance(site, &slug, &print)?;
            if !apply {
                return Ok(v);
            }
            let p = &v["proposal"];
            let strs = |k: &str| p[k].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect::<Vec<_>>());
            let opt = |k: &str| p[k].as_str().filter(|s| !s.is_empty()).map(str::to_string);
            update(site, &slug, &Patch {
                title: opt("title"), summary: opt("summary"), description: opt("description"), category: opt("category"),
                tags: strs("tags"), related: strs("related"), logo_prompt: opt("logo_prompt"), enhanced: Some(true), ..Default::default()
            })
        }
        CodeCmd::Suggest { apply, overwrite } => {
            let v = suggest(site, &print)?;
            if !apply {
                return Ok(v);
            }
            let mut s: Scheme = serde_json::from_value(v)?;
            s.overwrite = overwrite;
            let r = apply_scheme(site, &s)?;
            Ok(json!({ "applied": r["applied"], "categories": r["categories"] }))
        }
        CodeCmd::LogoPrompts { size, batch, json: as_json, write } => {
            if write {
                return write_prompt_files(site, size);
            }
            let v = logo_prompts(site, size)?;
            let batches: Vec<Value> = v["batches"].as_array().cloned().unwrap_or_default().into_iter().filter(|b| batch.is_none_or(|n| b["number"] == n)).collect();
            if as_json {
                return Ok(json!({ "count": v["count"], "batches": batches }));
            }
            if batches.is_empty() {
                println!("Every listed project has a logo.");
            }
            for b in &batches {
                println!("{}\n{}", b["text"].as_str().unwrap_or(""), "-".repeat(72));
            }
            Ok(Value::Null)
        }
        CodeCmd::ImportLogos { paths, batch } => import_logos(site, &paths, batch),
        CodeCmd::LogoPr { slugs, all, merge, yes } => {
            let slugs = if all { awaiting_pr(site)? } else { slugs };
            if slugs.is_empty() {
                bail!("name projects, or --all");
            }
            if !yes {
                bail!("this opens a pull request on {} repositor{} ({}); check the logos, then run again with --yes", slugs.len(), if slugs.len() == 1 { "y" } else { "ies" }, slugs.join(", "));
            }
            logo_prs(site, &slugs, merge, &print)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readme_candidates() {
        let r = "<p align=\"center\"><img src=\"docs/img/banner.png\" width=200></p>\n\n# Thing [![CI](https://github.com/me/x/actions/workflows/ci.yml/badge.svg)](x)\n\n![shot](images/screen.png)\n\nSome prose here.\n\n![Logo](./assets/Logo%20Mark.svg)\n![later](later.png)\n";
        let c = readme_logo_candidates(r, "", "me/x");
        assert_eq!(c, ["docs/img/banner.png", "assets/Logo Mark.svg", "images/screen.png"], "named first, then the opening's images; badges and later images left out");
        let c = readme_logo_candidates("![x](../art/logo.png)\n", "docs", "me/x");
        assert_eq!(c, ["art/logo.png"]);
        let c = readme_logo_candidates("![](https://raw.githubusercontent.com/me/x/main/a/logo.png)\n![](https://github.com/me/x/blob/main/b/icon.png?raw=true)\n![](https://example.org/logo.png)\n", "", "me/x");
        assert_eq!(c, ["a/logo.png", "b/icon.png", "https://example.org/logo.png"]);
        assert!(readme_logo_candidates("![](../../../../etc/logo.png)\n", "", "me/x").is_empty(), "no escaping the repository");
        assert!(readme_logo_candidates("![](http://insecure/logo.png)\n![](data:image/png;base64,xx)\n![](logo.txt)\n", "", "me/x").is_empty());
        assert!(readme_logo_candidates("Prose first.\n\n![a](a.png)\n", "", "me/x").is_empty(), "images after the opening only count if they say logo");
    }

    #[test]
    fn tree_logos() {
        let paths = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(tree_logo(&paths(&["src/main.rs", "docs/brand/app-logo.png", "assets/logo.svg", "node_modules/x/logo.png"])), Some("assets/logo.svg".into()));
        assert_eq!(tree_logo(&paths(&["a/b/c/d/logo.png", "images/project-logo.png"])), Some("images/project-logo.png".into()));
        assert_eq!(tree_logo(&paths(&["tests/fixtures/logo.png", "catalogue.png", "src/logo.rs"])), None);
        assert_eq!(tree_logo(&paths(&["src/logo.svg", "frontend/src/logo.svg", "public/logo192.png", "public/logo512.png"])), None, "template logos");
    }

    #[test]
    fn json_from_answers_and_paths() {
        assert_eq!(parse_json_object("Here:\n```json\n{\"a\": {\"b\": 1}}\n```\n").unwrap()["a"]["b"], 1);
        assert!(parse_json_object("no json").is_none());
        assert_eq!(relative("", "assets/logo.png"), "assets/logo.png");
        assert_eq!(relative("docs", "assets/logo.png"), "../assets/logo.png");
        assert_eq!(clean_tags(&["  Amiga ".into(), "#mcp".into(), "amiga".into(), "Model Context".into(), "".into()]), ["amiga", "mcp", "model-context"]);
    }

    #[test]
    fn facts_from_gh() {
        let v: Value = serde_json::from_str(r#"{"name":"x","nameWithOwner":"me/x","description":" D ","isPrivate":false,"isFork":true,"isArchived":false,"primaryLanguage":{"name":"Rust"},"repositoryTopics":[{"name":"cli"}],"pushedAt":"2026-10-01T10:00:00Z","createdAt":"2025-01-02T00:00:00Z","stargazerCount":3,"homepageUrl":"","licenseInfo":{"spdxId":"MIT","name":"MIT License"},"defaultBranchRef":{"name":"main"}}"#).unwrap();
        let f = facts(&v);
        assert_eq!((f.description.as_str(), f.language.as_str(), f.pushed.as_str(), f.license.as_str(), f.fork, f.stars), ("D", "Rust", "2026-10-01", "MIT", true, 3));
        assert_eq!(f.topics, ["cli"]);
    }

    fn site_with(text: &str) -> Site {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("gw-code-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("content")).unwrap();
        std::fs::write(root.join("site.yaml"), "site:\n  title: T\n  base_url: https://example.org\n  author: A\n").unwrap();
        std::fs::write(root.join(codecat::FILE), text).unwrap();
        Site::open(Some(&root)).unwrap()
    }

    #[test]
    fn editing_and_schemes() {
        let site = site_with("categories:\n  - { id: tools, title: Tools }\nprojects:\n  - { slug: a, repo: me/a }\n  - { slug: b, repo: me/b, category: tools, tags: [old] }\n");
        let v = update(&site, "a", &Patch { summary: Some(" Hi ".into()), tags: Some(vec!["X".into(), "x".into()]), related: Some(vec!["b".into(), "a".into()]), ..Default::default() }).unwrap();
        assert_eq!((v["summary"].as_str(), v["tags"][0].as_str(), v["related"].as_array().unwrap().len()), (Some("Hi"), Some("x"), 1));
        assert!(update(&site, "a", &Patch { related: Some(vec!["nope".into()]), ..Default::default() }).is_err());
        assert!(update(&site, "a", &Patch { category: Some("nope".into()), ..Default::default() }).is_err());
        assert_eq!(show(&site, "b").unwrap()["related_back"][0], "a");
        let scheme: Scheme = serde_json::from_value(json!({
            "categories": [{ "id": "AI Things", "title": "AI" }, { "id": "tools", "title": "Tools" }],
            "projects": { "a": { "category": "ai-things", "tags": ["llm"] }, "b": { "category": "ai-things", "tags": ["cli"] }, "zz": { "category": "tools" } }
        })).unwrap();
        let r = apply_scheme(&site, &scheme).unwrap();
        let c = load(&site).unwrap();
        assert_eq!(c.get("a").unwrap().category, "ai-things");
        assert_eq!(c.get("b").unwrap().category, "tools", "an existing category is kept without overwrite");
        assert_eq!(c.get("b").unwrap().tags, ["old", "cli"], "tags are added");
        assert_eq!(r["applied"]["categories_set"], 1);
        category(&site, "tools", None, None, true).unwrap();
        assert_eq!(load(&site).unwrap().get("b").unwrap().category, "", "removing a category clears it from projects");
    }

    #[test]
    fn families_get_one_mark() {
        let site = site_with("categories:\n  - { id: fw, title: Framework }\nprojects:\n  - { slug: fw-core, repo: me/fw-core, category: fw }\n  - { slug: fw-ui, repo: me/fw-ui, category: fw }\n  - { slug: other, repo: me/other }\n");
        let v = family(&site, &FamilyChange { id: "FW".into(), title: Some("The Framework".into()), concept: Some("a stylised red bull".into()), categories: vec!["fw".into()], ..Default::default() }).unwrap();
        assert_eq!(v["added"], 2);
        let p = logo_prompts(&site, 10).unwrap();
        assert_eq!(p["count"], 2, "the family mark and the other project");
        assert_eq!(p["batches"][0]["items"][0]["file"], "family-fw.png");
        assert!(p["batches"][0]["items"][0]["prompt"].as_str().unwrap().contains("a stylised red bull"));
        let img = image::RgbaImage::from_pixel(600, 600, image::Rgba([200, 0, 0, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let r = import_logo(&site, None, "Family_FW (1).png", png.get_ref()).unwrap();
        assert_eq!((r["family"].as_str(), r["members"].as_u64()), (Some("fw"), Some(2)));
        let st = state(&site).unwrap();
        let core = st["projects"].as_array().unwrap().iter().find(|p| p["slug"] == "fw-core").unwrap();
        assert!(core["logo_url"].as_str().unwrap().contains("family-fw.png") && core["logo_from_family"] == true);
        assert_eq!(awaiting_pr(&site).unwrap(), ["fw-core", "fw-ui"]);
        assert_eq!(logo_prompts(&site, 10).unwrap()["count"], 1);
        family(&site, &FamilyChange { id: "fw".into(), remove: true, ..Default::default() }).unwrap();
        assert!(load(&site).unwrap().projects.iter().all(|p| p.family.is_empty()));
    }

    fn png_bytes(rgb: [u8; 3]) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(64, 64, image::Rgba([rgb[0], rgb[1], rgb[2], 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img).write_to(&mut png, image::ImageFormat::Png).unwrap();
        png.into_inner()
    }

    #[test]
    fn pickup_folder_by_name_order_and_zip() {
        let site = site_with("projects:\n  - { slug: a, repo: me/a }\n  - { slug: b, repo: me/b }\n  - { slug: c, repo: me/c }\n  - { slug: d, repo: me/d }\n");
        let inbox_dir = site.root.join("inbox");
        modify(&site, |c| { c.logo_inbox = inbox_dir.display().to_string(); Ok(()) }).unwrap();
        let w = write_prompt_files(&site, 3).unwrap();
        assert_eq!((w["batches"].as_u64(), w["logos"].as_u64()), (Some(2), Some(4)));
        let t = std::fs::read_to_string(inbox_dir.join("prompts/batch-01.txt")).unwrap();
        assert!(t.contains("1. FILE NAME: a.png") && t.contains("3. FILE NAME: c.png") && t.contains("logos-batch-01.zip"));
        assert!(inbox_dir.join("prompts/batch-01.json").is_file() && inbox_dir.join("README.txt").is_file());
        // Batch 1: one saved under its name, two as ChatGPT names them (saved in order).
        std::fs::write(inbox_dir.join("b.png"), png_bytes([0, 0, 255])).unwrap();
        std::fs::write(inbox_dir.join("ChatGPT Image 10_01.png"), png_bytes([255, 0, 0])).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(inbox_dir.join("ChatGPT Image 09_59.png"), png_bytes([0, 255, 0])).unwrap();
        let r = import_logos(&site, &[], None).unwrap();
        assert_eq!(r["imported"].as_array().unwrap().len(), 1, "without a batch only names match");
        assert_eq!(r["unmatched"].as_array().unwrap().len(), 2);
        assert!(inbox_dir.join("imported/b.png").is_file() && !inbox_dir.join("b.png").exists());
        let r = import_logos(&site, &[], Some(1)).unwrap();
        let got: Vec<(String, String)> = r["imported"].as_array().unwrap().iter().map(|v| (v["slug"].as_str().unwrap().into(), v["from"].as_str().unwrap().into())).collect();
        assert_eq!(got, [("a".to_string(), "ChatGPT Image 10_01.png".to_string()), ("c".to_string(), "ChatGPT Image 09_59.png".to_string())], "by the order saved, skipping b");
        assert!(std::fs::read_dir(&inbox_dir).unwrap().flatten().all(|e| e.path().is_dir() || e.file_name() == "README.txt"));
        // Counts that disagree match nothing.
        std::fs::write(inbox_dir.join("x1.png"), png_bytes([1, 1, 1])).unwrap();
        std::fs::write(inbox_dir.join("x2.png"), png_bytes([2, 2, 2])).unwrap();
        let r = import_logos(&site, &[], Some(2)).unwrap();
        assert!(r["imported"].as_array().unwrap().is_empty() && r["note"].as_str().unwrap().contains("still needs 1"));
        std::fs::remove_file(inbox_dir.join("x2.png")).unwrap();
        std::fs::remove_file(inbox_dir.join("x1.png")).unwrap();
        // ChatGPT's zip of a batch.
        if std::path::Path::new("/usr/bin/zip").exists() {
            let z = site.root.join("z");
            std::fs::create_dir_all(&z).unwrap();
            std::fs::write(z.join("d.png"), png_bytes([9, 9, 9])).unwrap();
            assert!(Command::new("/usr/bin/zip").current_dir(&z).args(["-q", "../inbox/logos-batch-02.zip", "d.png"]).status().unwrap().success());
            let r = import_logos(&site, &[], None).unwrap();
            assert_eq!(r["imported"][0]["slug"], "d");
            assert!(inbox_dir.join("imported/logos-batch-02.zip").is_file() && !inbox_dir.join(".unzipped").exists());
        }
    }

    #[test]
    fn importing_logos() {
        let site = site_with("projects:\n  - { slug: amiga-mcp, repo: me/amiga_mcp }\n");
        let img = image::RgbaImage::from_pixel(1024, 1024, image::Rgba([10, 120, 200, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let dir = site.root.join("in");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Amiga_MCP (1).png"), png.get_ref()).unwrap();
        std::fs::write(dir.join("other.png"), png.get_ref()).unwrap();
        let r = import_logos(&site, &[dir], None).unwrap();
        assert_eq!(r["imported"].as_array().unwrap().len(), 1);
        assert_eq!(r["unmatched"].as_array().unwrap().len(), 1);
        let p = load(&site).unwrap().get("amiga-mcp").unwrap().clone();
        assert_eq!((p.logo.as_str(), p.logo_source.as_str()), ("amiga-mcp.png", "imported"));
        let small = image::open(site.static_dir().join("code-logos/amiga-mcp.png")).unwrap();
        let big = image::open(site.cache_dir().join("code-logos/amiga-mcp.png")).unwrap();
        assert_eq!((small.width(), big.width()), (SITE_PX, REPO_PX));
        assert_eq!(awaiting_pr(&site).unwrap(), ["amiga-mcp"]);
        let v = logo_prompts(&site, 10).unwrap();
        assert_eq!(v["count"], 0, "it has a logo now");
    }
}
