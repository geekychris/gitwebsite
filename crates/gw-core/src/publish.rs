//! Publish `dist/` into the GitHub Pages checkout.
//!
//! gitwebsite owns exactly the files listed in its manifest (`.gitwebsite-manifest.json`).
//! A publish copies the new build in, deletes files it owned last time that are no longer
//! built, and never touches anything else — other projects' directories (books, résumé)
//! keep living beside the site. Paths listed under `publish.preserve` are refused outright.

use crate::build::MANIFEST;
use crate::config::expand_home;
use crate::site::Site;
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Serialize, Default)]
pub struct Plan {
    pub pages_repo: PathBuf,
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub removed: Vec<String>,
    pub unchanged: usize,
}

#[derive(Debug, Serialize)]
pub struct Published {
    pub plan: Plan,
    pub commit: Option<String>,
    pub pushed: bool,
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let o = Command::new("git").current_dir(dir).args(args).output().context("running git")?;
    if !o.status.success() {
        bail!("git {}: {}", args.join(" "), String::from_utf8_lossy(&o.stderr));
    }
    Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
}

pub fn pages_repo(site: &Site) -> Result<PathBuf> {
    let p = &site.config.publish;
    if p.pages_repo.is_empty() {
        bail!("publish.pages_repo is not set in site.yaml");
    }
    let dir = expand_home(&p.pages_repo);
    if !dir.join(".git").exists() {
        if p.remote.is_empty() {
            bail!("{} is not a git checkout and publish.remote is not set", dir.display());
        }
        std::fs::create_dir_all(dir.parent().unwrap())?;
        let url = format!("https://github.com/{}.git", p.remote);
        let st = Command::new("git").args(["clone", "--depth", "1", &url]).arg(&dir).status()?;
        if !st.success() {
            bail!("cloning {url} failed");
        }
    }
    Ok(dir)
}

fn read_manifest(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join(MANIFEST))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn preserved(site: &Site, rel: &str) -> bool {
    site.config.publish.preserve.iter().any(|p| {
        let p = p.trim_end_matches('/');
        rel == p || rel.starts_with(&format!("{p}/"))
    })
}

/// What a publish would change, without changing anything.
pub fn plan(site: &Site, dist: &Path) -> Result<Plan> {
    let repo = pages_repo(site)?;
    let new: Vec<String> = serde_json::from_str(
        &std::fs::read_to_string(dist.join(MANIFEST)).context("no build manifest: run a build first")?,
    )?;
    let old: BTreeSet<String> = read_manifest(&repo).into_iter().collect();
    let newset: BTreeSet<&String> = new.iter().collect();
    let mut plan = Plan { pages_repo: repo.clone(), ..Default::default() };
    for f in &new {
        if preserved(site, f) {
            bail!("the build produces {f}, which is a preserved path in the Pages repo");
        }
        let dst = repo.join(f);
        if !dst.exists() {
            plan.added.push(f.clone());
        } else if std::fs::read(&dst)? != std::fs::read(dist.join(f))? {
            plan.updated.push(f.clone());
        } else {
            plan.unchanged += 1;
        }
    }
    for f in &old {
        if !newset.contains(f) && !preserved(site, f) && repo.join(f).exists() {
            plan.removed.push(f.clone());
        }
    }
    Ok(plan)
}

/// Copy the build into the Pages checkout, commit, and (if `push`) push.
pub fn publish(site: &Site, dist: &Path, message: &str, push: bool) -> Result<Published> {
    let repo = pages_repo(site)?;
    let branch = if site.config.publish.branch.is_empty() { "main".to_string() } else { site.config.publish.branch.clone() };
    if git(&repo, &["status", "--porcelain"])?.lines().any(|l| !l.is_empty()) {
        bail!("{} has uncommitted changes; commit or discard them first", repo.display());
    }
    let _ = git(&repo, &["pull", "--rebase", "-q", "origin", &branch]);
    let plan = plan(site, dist)?;
    for f in plan.added.iter().chain(plan.updated.iter()) {
        let dst = repo.join(f);
        std::fs::create_dir_all(dst.parent().unwrap())?;
        std::fs::copy(dist.join(f), &dst)?;
    }
    for f in &plan.removed {
        let _ = std::fs::remove_file(repo.join(f));
    }
    std::fs::copy(dist.join(MANIFEST), repo.join(MANIFEST))?;
    git(&repo, &["add", "-A"])?;
    if git(&repo, &["status", "--porcelain"])?.is_empty() {
        return Ok(Published { plan, commit: None, pushed: false });
    }
    git(&repo, &["commit", "-q", "-m", message])?;
    let sha = git(&repo, &["rev-parse", "--short", "HEAD"])?;
    if push {
        git(&repo, &["push", "-q", "origin", &branch])?;
    }
    Ok(Published { plan, commit: Some(sha), pushed: push })
}
