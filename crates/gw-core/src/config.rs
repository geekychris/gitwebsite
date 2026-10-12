//! `site.yaml`: identity, navigation, the information architecture (areas), build and publish
//! settings, and external content sources.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteConfig {
    pub site: Identity,
    #[serde(default)]
    pub nav: Vec<NavItem>,
    /// The ontology. Areas form a tree; an item can sit in several areas at once.
    #[serde(default)]
    pub areas: Vec<Area>,
    #[serde(default)]
    pub build: BuildConfig,
    #[serde(default)]
    pub publish: PublishConfig,
    /// AsciiDoc kept in other git repositories, pulled into the site at build time.
    #[serde(default)]
    pub sources: Vec<Source>,
    #[serde(default)]
    pub tools: ToolsConfig,
    /// The /articles/ listing.
    #[serde(default)]
    pub articles: ListingConfig,
    /// The /projects/ listing (grouped by area unless `group_by_area: false`).
    #[serde(default)]
    pub projects: ListingConfig,
}

/// How a listing page reads and is arranged.
///
/// ```yaml
/// projects:
///   intro: Things I have built.
///   group_by_area: true          # sections by each item's first area's top-level area
///   group_labels: { work: Software }
///   other_label: Other           # items with no area
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ListingConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intro: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_by_area: Option<bool>,
    /// Section headings by top-level area id; default: the area's title.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub group_labels: std::collections::BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub other_label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolsConfig {
    /// Category ids and titles, in display order. Tools name theirs with `:category:`.
    #[serde(default)]
    pub categories: Vec<ToolCategory>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCategory {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub title: String,
    #[serde(default)]
    pub tagline: String,
    pub base_url: String,
    pub author: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub links: Vec<NavItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NavItem {
    pub label: String,
    pub href: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Area {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub children: Vec<Area>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildConfig {
    #[serde(default = "default_out")]
    pub out_dir: String,
    #[serde(default = "default_asciidoctor")]
    pub ruby: String,
    /// Folders under static/ that only draft (preview) builds copy; published builds leave them out.
    #[serde(default)]
    pub preview_only: Vec<String>,
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self { out_dir: default_out(), ruby: default_asciidoctor(), preview_only: Vec::new() }
    }
}

fn default_out() -> String { "dist".into() }
fn default_asciidoctor() -> String { "ruby".into() }

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PublishConfig {
    /// Local checkout of the GitHub Pages repository.
    #[serde(default)]
    pub pages_repo: String,
    /// `owner/name`, used to clone the checkout if it is missing.
    #[serde(default)]
    pub remote: String,
    #[serde(default)]
    pub branch: String,
    /// Top-level paths in the Pages repo that gitwebsite must never touch.
    #[serde(default)]
    pub preserve: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    /// Local path to a git repository (`~` allowed).
    pub repo: String,
    /// Glob of AsciiDoc files inside it, relative to the repo root.
    pub include: String,
    #[serde(default)]
    pub label: String,
}

impl SiteConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        serde_yaml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self> {
        serde_yaml::from_str(text).context("parsing site.yaml")
    }

    /// Every area as `(path, area)`, where path is `parent/child`.
    pub fn flat_areas(&self) -> Vec<(String, Area)> {
        fn walk(prefix: &str, list: &[Area], out: &mut Vec<(String, Area)>) {
            for a in list {
                let p = if prefix.is_empty() { a.id.clone() } else { format!("{prefix}/{}", a.id) };
                out.push((p.clone(), a.clone()));
                walk(&p, &a.children, out);
            }
        }
        let mut out = vec![];
        walk("", &self.areas, &mut out);
        out
    }

    /// Resolve an area reference written in content: a full path (`making/woodworking`) or a
    /// unique leaf id (`woodworking`).
    pub fn resolve_area(&self, r: &str) -> Option<String> {
        let all = self.flat_areas();
        if all.iter().any(|(p, _)| p == r) {
            return Some(r.to_string());
        }
        let hits: Vec<_> = all.iter().filter(|(_, a)| a.id == r).collect();
        if hits.len() == 1 { Some(hits[0].0.clone()) } else { None }
    }
}

/// Set `publish.<key>` in the text of a site.yaml, keeping its comments and layout.
pub fn set_publish_key(text: &str, key: &str, value: &str) -> Result<String> {
    let quoted = serde_json::to_string(value)?;
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    match lines.iter().position(|l| l.trim_end() == "publish:") {
        Some(start) => {
            let mut end = start + 1;
            while end < lines.len() && (lines[end].starts_with(' ') || lines[end].trim().is_empty() || lines[end].starts_with('#')) {
                end += 1;
            }
            let prefix = format!("  {key}:");
            match (start + 1..end).find(|&i| lines[i].starts_with(&prefix)) {
                Some(i) => lines[i] = format!("{prefix} {quoted}"),
                None => lines.insert(start + 1, format!("{prefix} {quoted}")),
            }
        }
        None => {
            lines.push(String::new());
            lines.push("publish:".into());
            lines.push(format!("  {key}: {quoted}"));
        }
    }
    let mut out = lines.join("\n");
    if text.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

pub fn expand_home(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(h) = std::env::var_os("HOME") {
            return PathBuf::from(h).join(rest);
        }
    }
    PathBuf::from(p)
}
