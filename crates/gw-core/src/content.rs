//! Content items: AsciiDoc files with a small set of header attributes.
//!
//! ```asciidoc
//! = Title
//! :type: article            // page | article | project | update | app | playground
//! :date: 2026-10-09
//! :areas: making/woodworking, 3d-printing
//! :tags: jig, walnut
//! :project: workbench       // for updates: the project slug they belong to
//! :summary: One line for cards and feeds.
//! :hero: media:workbench-front   // or images/x.jpg, or https://...
//! :status: draft            // draft | published
//! :visibility: public       // public | unlisted
//! :repo: https://github.com/octocat/thing
//! :url: https://app.example  // apps: where the live app runs
//! :embed: true              // apps: embed the live app in an iframe
//! :source: wordpress-123    // registry id this content came from
//! :featured: true
//! :pinned: 1               // home page: pinned at the top (optional number orders the pins)
//! ```
//!
//! Where an item lives decides its kind and URL when `:type:` is absent:
//! `content/pages/about.adoc` → `/about/`, `content/articles/x.adoc` → `/articles/x/`,
//! `content/projects/p/index.adoc` → `/projects/p/`,
//! `content/projects/p/updates/u.adoc` → `/projects/p/u/`,
//! `content/lab/apps/a.adoc` → `/lab/apps/a/`, `content/lab/playground/x.adoc` → `/lab/playground/x/`,
//! `content/tools/t.adoc` → `/tools/t/` (`:tool:` names the script, `:category:` groups it).
//! A directory holding `index.adoc` is a bundle: its other files are copied beside the page.

use anyhow::{bail, Context, Result};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Page,
    Article,
    Project,
    Update,
    App,
    Playground,
    /// An interactive client-side tool (calculator, encoder, DNS lookup...) under /tools/.
    Tool,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "page" => Kind::Page,
            "article" | "post" => Kind::Article,
            "project" => Kind::Project,
            "update" => Kind::Update,
            "app" => Kind::App,
            "playground" | "code" => Kind::Playground,
            "tool" => Kind::Tool,
            _ => return None,
        })
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            Kind::Page => "page",
            Kind::Article => "article",
            Kind::Project => "project",
            Kind::Update => "update",
            Kind::App => "app",
            Kind::Playground => "playground",
            Kind::Tool => "tool",
        }
    }
    /// Posts are the dated stream: articles and project updates.
    pub fn is_post(&self) -> bool {
        matches!(self, Kind::Article | Kind::Update)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    /// Stable identifier: the path relative to the content root, without `.adoc`
    /// (or the bundle directory for `index.adoc`). External items are prefixed `ext:`.
    pub id: String,
    pub kind: Kind,
    pub slug: String,
    pub title: String,
    pub url: String,
    pub date: Option<NaiveDate>,
    pub updated: Option<NaiveDate>,
    pub areas: Vec<String>,
    pub tags: Vec<String>,
    pub project: Option<String>,
    pub summary: String,
    pub hero: Option<String>,
    pub draft: bool,
    pub unlisted: bool,
    pub featured: bool,
    /// Pinned to the top of the home page (`:pinned:`).
    pub pinned: bool,
    /// `:pinned: <n>` orders the pins, lowest first; pins without a number follow, newest first.
    pub pin_order: Option<i64>,
    pub repo: Option<String>,
    pub app_url: Option<String>,
    pub embed: bool,
    pub source: Option<String>,
    /// Every header attribute, for templates that want something not modelled above.
    pub attrs: BTreeMap<String, String>,
    pub path: PathBuf,
    /// Directory whose non-AsciiDoc files are copied beside the rendered page (bundles only).
    pub bundle_dir: Option<PathBuf>,
}

impl Item {
    pub fn listed(&self) -> bool {
        !self.draft && !self.unlisted
    }
}

/// Parse the document header: `= Title` then `:name: value` lines until the first blank line.
pub fn parse_header(text: &str) -> (Option<String>, BTreeMap<String, String>) {
    let mut title = None;
    let mut attrs = BTreeMap::new();
    let mut started = false;
    for line in text.lines() {
        let l = line.trim_end();
        if !started {
            if l.is_empty() || l.starts_with("//") {
                continue;
            }
            started = true;
            if let Some(t) = l.strip_prefix("= ") {
                title = Some(t.trim().to_string());
                continue;
            }
        }
        if l.is_empty() {
            break;
        }
        if let Some(rest) = l.strip_prefix(':') {
            if let Some((k, v)) = rest.split_once(':') {
                attrs.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
    (title, attrs)
}

fn list(v: Option<&String>) -> Vec<String> {
    v.map(|s| s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect())
        .unwrap_or_default()
}

fn truthy(v: Option<&String>) -> bool {
    matches!(v.map(|s| s.trim().to_ascii_lowercase()).as_deref(), Some("true" | "yes" | "1" | ""))
        && v.is_some()
}

/// `:pinned: 2` → `Some(2)`; `:pinned:` or `:pinned: true` → `None` (pinned, unordered).
fn pin_order(v: Option<&String>) -> Option<i64> {
    v.and_then(|s| s.trim().parse::<i64>().ok()).filter(|n| *n > 0)
}

fn date(v: Option<&String>) -> Result<Option<NaiveDate>> {
    match v {
        None => Ok(None),
        Some(s) if s.trim().is_empty() => Ok(None),
        Some(s) => Ok(Some(
            NaiveDate::parse_from_str(s.trim().get(..10).unwrap_or(s), "%Y-%m-%d")
                .with_context(|| format!("bad date {s:?} (use YYYY-MM-DD)"))?,
        )),
    }
}

pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in s.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    out.trim_end_matches('-').to_string()
}

/// Load one item. `rel` is the path relative to the content root (used for kind, id, URL).
pub fn load_item(path: &Path, rel: &Path) -> Result<Item> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let (title, attrs) = parse_header(&text);
    let title = title.with_context(|| format!("{}: missing `= Title` line", path.display()))?;

    let comps: Vec<String> = rel.with_extension("").iter().map(|c| c.to_string_lossy().to_string()).collect();
    let is_index = comps.last().map(|c| c == "index").unwrap_or(false);
    let stem_comps: Vec<String> = if is_index { comps[..comps.len() - 1].to_vec() } else { comps.clone() };
    let id = stem_comps.join("/");
    let file_slug = stem_comps.last().cloned().unwrap_or_default();
    let slug = attrs.get("slug").cloned().unwrap_or_else(|| slugify(&file_slug));

    let inferred = match stem_comps.first().map(|s| s.as_str()) {
        Some("pages") => Kind::Page,
        Some("projects") if stem_comps.len() >= 4 && stem_comps[2] == "updates" => Kind::Update,
        Some("projects") => Kind::Project,
        Some("lab") if stem_comps.get(1).map(|s| s.as_str()) == Some("apps") => Kind::App,
        Some("lab") if stem_comps.get(1).map(|s| s.as_str()) == Some("playground") => Kind::Playground,
        Some("tools") => Kind::Tool,
        _ => Kind::Article,
    };
    let kind = match attrs.get("type") {
        Some(t) => Kind::parse(t).with_context(|| format!("{}: unknown :type: {t}", path.display()))?,
        None => inferred,
    };
    let project = attrs.get("project").cloned().or_else(|| {
        (kind == Kind::Update && stem_comps.len() >= 2).then(|| stem_comps[1].clone())
    });
    let url = match kind {
        Kind::Page if slug == "home" => "/".to_string(),
        Kind::Page => format!("/{slug}/"),
        Kind::Article => format!("/articles/{slug}/"),
        Kind::Project => format!("/projects/{slug}/"),
        Kind::Update => format!("/projects/{}/{slug}/", project.clone().unwrap_or_default()),
        Kind::App => format!("/lab/apps/{slug}/"),
        Kind::Playground => format!("/lab/playground/{slug}/"),
        Kind::Tool => format!("/tools/{slug}/"),
    };
    let status = attrs.get("status").map(|s| s.to_ascii_lowercase()).unwrap_or_else(|| "published".into());
    if !matches!(status.as_str(), "draft" | "published") {
        bail!("{}: :status: must be draft or published", path.display());
    }
    let visibility = attrs.get("visibility").map(|s| s.to_ascii_lowercase()).unwrap_or_else(|| "public".into());
    Ok(Item {
        id,
        kind,
        slug,
        title,
        url,
        date: date(attrs.get("date"))?,
        updated: date(attrs.get("updated"))?,
        areas: list(attrs.get("areas")),
        tags: list(attrs.get("tags")),
        project,
        summary: attrs.get("summary").cloned().unwrap_or_default(),
        hero: attrs.get("hero").cloned().filter(|s| !s.is_empty()),
        draft: status == "draft",
        unlisted: visibility == "unlisted",
        featured: truthy(attrs.get("featured")),
        pinned: truthy(attrs.get("pinned")) || pin_order(attrs.get("pinned")).is_some(),
        pin_order: pin_order(attrs.get("pinned")),
        repo: attrs.get("repo").cloned().filter(|s| !s.is_empty()),
        app_url: attrs.get("url").cloned().filter(|s| !s.is_empty()),
        embed: truthy(attrs.get("embed")),
        source: attrs.get("source").cloned().filter(|s| !s.is_empty()),
        attrs,
        path: path.to_path_buf(),
        bundle_dir: is_index.then(|| path.parent().unwrap().to_path_buf()),
    })
}

/// Every `.adoc` under the content root. Files whose name starts with `_` are partials.
pub fn scan(root: &Path) -> Result<Vec<Item>> {
    let mut items = vec![];
    for e in WalkDir::new(root).sort_by_file_name() {
        let e = e?;
        let p = e.path();
        if p.extension().map(|x| x == "adoc").unwrap_or(false)
            && !p.file_name().unwrap().to_string_lossy().starts_with('_')
        {
            let rel = p.strip_prefix(root)?;
            items.push(load_item(p, rel)?);
        }
    }
    Ok(items)
}
