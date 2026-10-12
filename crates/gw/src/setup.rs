//! Setting up: creating a site (`gw init`), and the user's list of sites and settings
//! (`gw config`). Shared by the CLI and the app's setup screen.

use anyhow::{bail, Context, Result};
use gw_core::init::{init as scaffold, pages_url, InitOptions};
use gw_core::userconfig::{config_path, norm, UserConfig};
use gw_core::Site;
use serde_json::{json, Value};
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct InitArgs {
    pub dir: PathBuf,
    pub title: Option<String>,
    pub author: Option<String>,
    pub base_url: Option<String>,
    pub tagline: String,
    pub description: String,
    pub pages_repo: Option<String>,
    pub branch: String,
    pub pages_checkout: Option<String>,
    pub no_theme_copy: bool,
    pub no_starter: bool,
    pub git: bool,
    pub create_pages_repo: bool,
    pub no_current: bool,
}

fn ask(label: &str) -> Result<String> {
    if !std::io::stdin().is_terminal() {
        bail!("{label} is required (pass it as a flag)");
    }
    eprint!("{label}: ");
    std::io::stderr().flush().ok();
    let mut s = String::new();
    std::io::stdin().lock().read_line(&mut s)?;
    Ok(s.trim().to_string())
}

fn run(dir: &Path, prog: &str, args: &[&str]) -> Result<()> {
    let o = Command::new(prog).args(args).current_dir(dir).output().with_context(|| format!("running {prog}"))?;
    if !o.status.success() {
        bail!("{prog} {} failed: {}", args.join(" "), String::from_utf8_lossy(&o.stderr).trim());
    }
    Ok(())
}

/// `gw init`: scaffold, optionally `git init` and create the Pages repository, and register
/// the site in the user config.
pub fn init(a: InitArgs) -> Result<Value> {
    let title = match a.title { Some(t) => t, None => ask("Site title")? };
    let author = match a.author { Some(t) => t, None => ask("Author name")? };
    let pages_remote = a.pages_repo.unwrap_or_default();
    let base_url = match a.base_url {
        Some(u) => u,
        None => match pages_url(&pages_remote) {
            Some(u) => u,
            None => ask("Base URL (e.g. https://octocat.github.io)")?,
        },
    };
    let o = InitOptions {
        title,
        author,
        base_url,
        tagline: a.tagline,
        description: a.description,
        pages_remote: pages_remote.clone(),
        branch: a.branch,
        pages_repo: a.pages_checkout.unwrap_or_default(),
        copy_theme: !a.no_theme_copy,
        no_starter: a.no_starter,
    };
    create(&a.dir, &o, !a.no_current, a.git, a.create_pages_repo)
}

/// Scaffold a site, then the optional git steps, then add it to the user config.
pub fn create(dir: &Path, o: &InitOptions, make_current: bool, git: bool, create_pages_repo: bool) -> Result<Value> {
    let r = scaffold(dir, o)?;
    let mut notes = vec![];
    if git {
        run(dir, "git", &["init", "-q"])?;
        run(dir, "git", &["add", "-A"])?;
        run(dir, "git", &["commit", "-q", "-m", "New site (gw init)"])?;
        notes.push("made the folder a git repository with a first commit".to_string());
    }
    if create_pages_repo {
        if o.pages_remote.is_empty() {
            bail!("--create-pages-repo needs --pages-repo owner/name");
        }
        run(dir, "gh", &["repo", "create", &o.pages_remote, "--public", "--description", &format!("{} — built with gitwebsite", o.title)])?;
        notes.push(format!("created github.com/{} (turn on GitHub Pages for its main branch in the repository settings)", o.pages_remote));
    }
    let mut uc = UserConfig::load()?;
    let entry = uc.add_site(&dir.to_string_lossy(), Some(&o.title), make_current)?;
    uc.save()?;
    Ok(json!({
        "created": r.root.display().to_string(),
        "files": r.files.len(),
        "site": entry.path,
        "current": uc.current,
        "notes": notes,
        "next": ["gw serve --open   (preview and the management app)", "gw new article \"My first post\"", "gw publish"],
    }))
}

pub fn show() -> Result<Value> {
    let uc = UserConfig::load()?;
    let sites: Vec<Value> = uc
        .sites
        .iter()
        .map(|s| json!({ "name": s.name, "path": s.path, "current": uc.current.as_deref() == Some(s.path.as_str()), "exists": Path::new(&s.path).join("site.yaml").exists() }))
        .collect();
    Ok(json!({
        "config": config_path().display().to_string(),
        "current": uc.current,
        "sites": sites,
        "repo_roots": uc.repo_roots().iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        "claude_bin": uc.claude_bin,
        "gw_bin": uc.gw_bin,
    }))
}

pub fn add_site(path: &Path, name: Option<&str>, current: bool) -> Result<Value> {
    let mut uc = UserConfig::load()?;
    let e = uc.add_site(&path.to_string_lossy(), name, current)?;
    uc.save()?;
    Ok(json!({ "added": e.path, "name": e.name, "current": uc.current }))
}

pub fn use_site(which: &str) -> Result<Value> {
    let mut uc = UserConfig::load()?;
    let e = match uc.find(which) {
        Some(_) => uc.use_site(which)?,
        // A path not yet in the list: add it.
        None if Path::new(&norm(which)).join("site.yaml").exists() => uc.add_site(which, None, true)?,
        None => uc.use_site(which)?,
    };
    uc.save()?;
    Site::open(Some(Path::new(&e.path)))?;
    Ok(json!({ "current": e.path, "name": e.name }))
}

pub fn remove_site(which: &str) -> Result<Value> {
    let mut uc = UserConfig::load()?;
    let e = uc.remove_site(which)?;
    uc.save()?;
    Ok(json!({ "removed": e.path, "current": uc.current }))
}

pub fn set(key: &str, value: &str) -> Result<Value> {
    let mut uc = UserConfig::load()?;
    let v = value.trim();
    let opt = || (!v.is_empty()).then(|| v.to_string());
    match key.replace('_', "-").as_str() {
        "repo-roots" => uc.repo_roots = v.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect(),
        "claude-bin" => uc.claude_bin = opt(),
        "gw-bin" => uc.gw_bin = opt(),
        k => bail!("unknown setting {k}: use repo-roots, claude-bin, or gw-bin"),
    }
    uc.save()?;
    show()
}
