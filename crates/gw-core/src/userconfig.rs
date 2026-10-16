//! The user's own settings, shared by the CLI, the MCP server, and the desktop app:
//! which sites they have (one is current), where programs live, and where to look for
//! local checkouts of the repositories their posts are about.
//!
//! ```toml
//! # ~/.config/gitwebsite/config.toml
//! current = "/Users/me/sites/my-site"
//! repo_roots = ["~/code", "~/src"]
//! claude_bin = "/opt/homebrew/bin/claude"   # optional
//!
//! [[sites]]
//! name = "My site"
//! path = "/Users/me/sites/my-site"
//! ```
//!
//! The file lives in `$GW_CONFIG_DIR`, else `$XDG_CONFIG_HOME/gitwebsite`, else
//! `~/.config/gitwebsite` (`%APPDATA%\gitwebsite` on Windows).

use crate::config::{expand_home, SiteConfig};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct UserConfig {
    /// Path of the current site (one of `sites`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
    /// Directories searched for local checkouts of a post's repository (`~` allowed).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repo_roots: Vec<String>,
    /// The `claude` CLI, when it is not somewhere usual.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_bin: Option<String>,
    /// The `gw` binary, when it is not somewhere usual.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gw_bin: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sites: Vec<SiteEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SiteEntry {
    pub name: String,
    pub path: String,
}

/// Default places to look for checkouts, used when `repo_roots` is empty (only those that exist).
pub const DEFAULT_REPO_ROOTS: &[&str] = &["~/code", "~/src", "~/projects", "~/dev", "~/git", "~/Developer"];

pub fn config_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("GW_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    default_config_dir()
}

/// The config directory when `GW_CONFIG_DIR` is not set.
pub fn default_config_dir() -> PathBuf {
    if cfg!(windows) {
        if let Some(a) = std::env::var_os("APPDATA") {
            return PathBuf::from(a).join("gitwebsite");
        }
    }
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(x).join("gitwebsite");
    }
    expand_home("~/.config/gitwebsite")
}

/// Is the config directory in use the user's real one (not a `GW_CONFIG_DIR` scratch/test dir)?
/// Things that live outside the directory (the OS keychain) are only touched for the real one.
pub fn is_default_config_dir() -> bool {
    let (a, b) = (config_dir(), default_config_dir());
    a == b || (a.canonicalize().ok().is_some() && a.canonicalize().ok() == b.canonicalize().ok())
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

/// Normalise a site path for storage and comparison: `~` expanded, absolute, no trailing slash.
pub fn norm(p: &str) -> String {
    let pb = expand_home(p.trim());
    let pb = if pb.is_absolute() { pb } else { std::env::current_dir().map(|c| c.join(&pb)).unwrap_or(pb) };
    let pb = std::fs::canonicalize(&pb).unwrap_or(pb);
    pb.to_string_lossy().trim_end_matches('/').to_string()
}

impl UserConfig {
    /// The config file, or an empty config if there is none yet.
    pub fn load() -> Result<Self> {
        let p = config_path();
        if !p.exists() {
            return Ok(Self::default());
        }
        let t = std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
        toml::from_str(&t).with_context(|| format!("parsing {}", p.display()))
    }

    pub fn save(&self) -> Result<()> {
        let p = config_path();
        std::fs::create_dir_all(p.parent().unwrap())?;
        let body = format!("# gitwebsite settings — see `gw config --help`.\n{}", toml::to_string_pretty(self)?);
        let tmp = p.with_extension("toml.tmp");
        std::fs::write(&tmp, body)?;
        std::fs::rename(&tmp, &p)?;
        Ok(())
    }

    /// The current site's directory, if one is set and still has a site.yaml.
    pub fn current_site(&self) -> Option<PathBuf> {
        let p = PathBuf::from(self.current.as_ref()?);
        p.join("site.yaml").exists().then_some(p)
    }

    /// Find a site by name or path.
    pub fn find(&self, which: &str) -> Option<&SiteEntry> {
        let n = norm(which);
        self.sites.iter().find(|s| s.path == n || s.name == which)
    }

    /// Add a site (a directory holding site.yaml); returns its entry. Re-adding updates the name.
    pub fn add_site(&mut self, path: &str, name: Option<&str>, make_current: bool) -> Result<SiteEntry> {
        let p = norm(path);
        let cfg_file = Path::new(&p).join("site.yaml");
        if !cfg_file.exists() {
            bail!("{p} has no site.yaml — run `gw init {p}` to create a site there");
        }
        let title = SiteConfig::load(&cfg_file).map(|c| c.site.title).unwrap_or_default();
        let name = name.map(str::to_string).filter(|n| !n.trim().is_empty()).unwrap_or(if title.is_empty() { p.clone() } else { title });
        let e = SiteEntry { name, path: p.clone() };
        match self.sites.iter_mut().find(|s| s.path == p) {
            Some(s) => *s = e.clone(),
            None => self.sites.push(e.clone()),
        }
        if make_current || self.current.is_none() {
            self.current = Some(p);
        }
        Ok(e)
    }

    pub fn use_site(&mut self, which: &str) -> Result<SiteEntry> {
        let e = self.find(which).cloned().with_context(|| format!("no site {which:?} in {}; add it with `gw config add-site <path>`", config_path().display()))?;
        self.current = Some(e.path.clone());
        Ok(e)
    }

    pub fn remove_site(&mut self, which: &str) -> Result<SiteEntry> {
        let e = self.find(which).cloned().with_context(|| format!("no site {which:?}"))?;
        self.sites.retain(|s| s.path != e.path);
        if self.current.as_deref() == Some(e.path.as_str()) {
            self.current = self.sites.first().map(|s| s.path.clone());
        }
        Ok(e)
    }

    /// Where to look for checkouts: `GW_REPO_ROOTS` (`:`-separated), else `repo_roots`, else the
    /// defaults that exist.
    pub fn repo_roots(&self) -> Vec<PathBuf> {
        if let Ok(v) = std::env::var("GW_REPO_ROOTS") {
            if !v.trim().is_empty() {
                return v.split(':').filter(|s| !s.is_empty()).map(expand_home).collect();
            }
        }
        if !self.repo_roots.is_empty() {
            return self.repo_roots.iter().map(|r| expand_home(r)).collect();
        }
        DEFAULT_REPO_ROOTS.iter().map(|r| expand_home(r)).filter(|p| p.is_dir()).collect()
    }
}

/// Where a command should find its site when none was given: `GW_SITE`, then `site.yaml` here
/// or in a parent directory (a `site/site.yaml` counts, as in a gitwebsite checkout), then the
/// current site in the user config.
pub fn resolve_site() -> Result<PathBuf> {
    if let Some(env) = std::env::var_os("GW_SITE").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(env));
    }
    if let Ok(mut d) = std::env::current_dir() {
        loop {
            if d.join("site.yaml").exists() {
                return Ok(d);
            }
            if d.join("site/site.yaml").exists() {
                return Ok(d.join("site"));
            }
            if !d.pop() {
                break;
            }
        }
    }
    let uc = UserConfig::load()?;
    if let Some(p) = uc.current_site() {
        return Ok(p);
    }
    bail!(
        "no site found: pass --site <dir>, set GW_SITE, run gw inside a site, or create one with `gw init <dir>` \
         (an existing site can be added with `gw config add-site <dir> --current`)"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_use_remove() {
        let d = std::env::temp_dir().join(format!("gw-uc-{}", std::process::id()));
        let a = d.join("a");
        let b = d.join("b");
        for s in [&a, &b] {
            std::fs::create_dir_all(s).unwrap();
            std::fs::write(s.join("site.yaml"), "site: {title: T, base_url: 'https://x.test', author: A}\n").unwrap();
        }
        let mut c = UserConfig::default();
        c.add_site(a.to_str().unwrap(), None, false).unwrap();
        assert_eq!(c.current.as_deref(), Some(norm(a.to_str().unwrap()).as_str()));
        c.add_site(b.to_str().unwrap(), Some("bee"), false).unwrap();
        assert_eq!(c.sites.len(), 2);
        assert!(c.current.as_deref().unwrap().ends_with("/a"));
        c.use_site("bee").unwrap();
        assert!(c.current.as_deref().unwrap().ends_with("/b"));
        c.remove_site("bee").unwrap();
        assert!(c.current.as_deref().unwrap().ends_with("/a"));
        assert!(c.add_site(d.join("nope").to_str().unwrap(), None, false).is_err());
        let round: UserConfig = toml::from_str(&toml::to_string_pretty(&c).unwrap()).unwrap();
        assert_eq!(round, c);
        let _ = std::fs::remove_dir_all(&d);
    }
}
