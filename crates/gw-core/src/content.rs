//! Content items: AsciiDoc files with a small set of header attributes.
//!
//! ```asciidoc
//! = Title
//! :type: article            // page | article | project | update | app | playground | tool | exploration
//! :date: 2026-10-09
//! :areas: making/woodworking, 3d-printing
//! :tags: jig, walnut
//! :project: workbench       // for updates: the project slug they belong to
//! :summary: One line for cards and feeds.
//! :hero: media:workbench-front   // or images/x.jpg, or https://...
//! :hero-crop: 10, 20, 80, 45      // the part of the hero cards show (left, top, width, height in %)
//! :status: draft            // draft | published
//! :visibility: public       // public | unlisted
//! :repo: https://github.com/octocat/thing
//! :url: https://app.example  // apps: where the live app runs
//! :embed: true              // apps: embed the live app in an iframe
//! :source: wordpress-123    // registry id this content came from
//! :featured: true
//! :pinned: 1               // home page: pinned at the top (optional number orders the pins)
//! :works-with: graph: plots the fitted curve; units: converts the inputs   // tools only
//! :lab: true                // tools: listed on the Lab page too
//! :series: Consistency      // explorations: prev/next within the series (`:series-order: 2` orders it)
//! :redirect-from: /old/path/, /older/   // old addresses that send readers here
//! ```
//!
//! Where an item lives decides its kind and URL when `:type:` is absent:
//! `content/pages/about.adoc` → `/about/`, `content/articles/x.adoc` → `/articles/x/`,
//! `content/projects/p/index.adoc` → `/projects/p/`,
//! `content/projects/p/updates/u.adoc` → `/projects/p/u/`,
//! `content/lab/apps/a.adoc` → `/lab/apps/a/`, `content/lab/playground/x.adoc` → `/lab/playground/x/`
//! (`content/lab/sketches/` and `content/lab/rosetta/` are playgrounds too, at `/lab/sketches/x/` and
//! `/lab/rosetta/x/`), `content/lab/explorations/e.adoc` → `/lab/explorations/e/`,
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
    /// A Lab narrative that learns by running things: prose around live tool views (`tool::`)
    /// and playgrounds, under /lab/explorations/.
    Exploration,
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
            "exploration" => Kind::Exploration,
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
            Kind::Exploration => "exploration",
        }
    }
    /// Posts are the dated stream: articles and project updates.
    pub fn is_post(&self) -> bool {
        matches!(self, Kind::Article | Kind::Update)
    }
    /// Every kind, in the order pickers show them.
    pub const ALL: [Kind; 8] = [Kind::Article, Kind::Project, Kind::Update, Kind::Page, Kind::App, Kind::Playground, Kind::Tool, Kind::Exploration];
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
    /// `:hero-crop: left, top, width, height` — the part of the hero image (percentages of its
    /// width and height) that cards show, chosen in the app's Tile image panel.
    pub fn hero_crop(&self) -> Option<[f64; 4]> {
        parse_crop(self.attrs.get("hero-crop")?)
    }
    /// `:works-with:` — other tools this one is used together with (see [`parse_works_with`]).
    pub fn works_with(&self) -> Vec<Result<WorksWith, String>> {
        self.attrs.get("works-with").map(|s| parse_works_with(s)).unwrap_or_default()
    }
    /// `:redirect-from: /old/path/, articles/older` — earlier addresses of this page, each as a
    /// site path with a leading `/` (and a trailing `/` unless it names a file, e.g. `x.html`).
    pub fn redirect_from(&self) -> Vec<String> {
        list(self.attrs.get("redirect-from")).iter().map(|p| redirect_path(p)).filter(|p| p != "/").collect()
    }
    /// `:lab: true` on a tool page: listed on the Lab page as well as under /tools/.
    pub fn in_lab(&self) -> bool {
        truthy(self.attrs.get("lab"))
    }
}

/// `old/path` or `https://site/old/path` → `/old/path/`; a last segment with a dot is a file
/// (`/old.html`) and gets no trailing slash. Queries and fragments are dropped.
pub fn redirect_path(p: &str) -> String {
    let p = p.trim();
    let p = p.split_once("://").map(|(_, rest)| rest.find('/').map(|i| &rest[i..]).unwrap_or("/")).unwrap_or(p);
    let p = p.split(['?', '#']).next().unwrap_or("");
    let segs: Vec<&str> = p.split('/').filter(|s| !s.is_empty() && *s != "." && *s != "..").collect();
    let mut out = format!("/{}", segs.join("/"));
    if !segs.is_empty() && !segs.last().unwrap().contains('.') {
        out.push('/');
    }
    out
}

/// One `:works-with:` entry on a tool page: another tool (by slug) and why they go together.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorksWith {
    pub slug: String,
    pub reason: String,
}

/// `:works-with: graph: plots the fitted curve; units: converts the inputs` → one entry per
/// `;`-separated part, the slug before its first `:` and the reason (plain text) after it; a
/// malformed part (no `:`, an empty or invalid slug, no reason) is an `Err` describing it. Empty
/// parts (a trailing `;`) are ignored.
pub fn parse_works_with(s: &str) -> Vec<Result<WorksWith, String>> {
    s.split(';')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| {
            let Some((slug, reason)) = p.split_once(':') else {
                return Err(format!("{p:?} wants `<tool slug>: <reason>`"));
            };
            let (slug, reason) = (slug.trim(), reason.trim());
            if slug.is_empty() || !slug.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_') {
                return Err(format!("{p:?}: {slug:?} is not a tool slug (lowercase letters, digits, -)"));
            }
            if reason.is_empty() {
                return Err(format!("{p:?}: say why, after the `:`"));
            }
            Ok(WorksWith { slug: slug.to_string(), reason: reason.to_string() })
        })
        .collect()
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

/// `12.5, 8, 70, 39.4` (or with `%`) → left, top, width and height within the image, in percent.
pub fn parse_crop(s: &str) -> Option<[f64; 4]> {
    let v: Vec<f64> = s.split([',', ' ']).filter(|x| !x.is_empty()).map(|x| x.trim_end_matches('%').parse().ok()).collect::<Option<_>>()?;
    let [x, y, w, h] = <[f64; 4]>::try_from(v).ok()?;
    let ok = x >= 0.0 && y >= 0.0 && w > 0.5 && h > 0.5 && x + w <= 100.01 && y + h <= 100.01;
    ok.then_some([x, y, w, h])
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
        Some("lab") if matches!(stem_comps.get(1).map(|s| s.as_str()), Some("playground" | "sketches" | "rosetta")) => Kind::Playground,
        Some("lab") if stem_comps.get(1).map(|s| s.as_str()) == Some("explorations") => Kind::Exploration,
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
        // Sketches and Rosetta pages keep their own folder in the address.
        Kind::Playground => match (stem_comps.first().map(|s| s.as_str()), stem_comps.get(1).map(|s| s.as_str())) {
            (Some("lab"), Some(f @ ("sketches" | "rosetta"))) if stem_comps.len() > 2 => format!("/lab/{f}/{slug}/"),
            _ => format!("/lab/playground/{slug}/"),
        },
        Kind::Tool => format!("/tools/{slug}/"),
        Kind::Exploration => format!("/lab/explorations/{slug}/"),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(slug: &str, reason: &str) -> Result<WorksWith, String> {
        Ok(WorksWith { slug: slug.into(), reason: reason.into() })
    }

    #[test]
    fn works_with_entries_and_separators() {
        assert_eq!(
            parse_works_with("graph: plots the fitted curve; units: converts the inputs"),
            vec![ok("graph", "plots the fitted curve"), ok("units", "converts the inputs")]
        );
        // Whitespace around slugs, reasons and separators; empty parts (a trailing `;`) are ignored.
        assert_eq!(parse_works_with("  graph :  plots it  ;;  units:converts ;  "), vec![ok("graph", "plots it"), ok("units", "converts")]);
        // Only the first `:` separates: the reason may contain more.
        assert_eq!(parse_works_with("clock: shows UTC: the time zone the log uses"), vec![ok("clock", "shows UTC: the time zone the log uses")]);
        assert_eq!(parse_works_with(""), vec![]);
        assert_eq!(parse_works_with("  ;  "), vec![]);
    }

    #[test]
    fn malformed_works_with_entries() {
        let r = parse_works_with("graph; units: ; : no slug; Big Tool: why; kafka-sim: fine");
        assert!(r[0].as_ref().is_err_and(|e| e.contains("wants")), "{r:?}");
        assert!(r[1].as_ref().is_err_and(|e| e.contains("say why")), "{r:?}");
        assert!(r[2].is_err() && r[3].is_err(), "{r:?}");
        assert_eq!(r[4], ok("kafka-sim", "fine"));
    }

    #[test]
    fn works_with_from_the_header() {
        let (_, attrs) = parse_header("= T\n:type: tool\n:works-with: a: one: two; b: three\n\nBody.\n");
        let it = Item { attrs, ..test_item() };
        assert_eq!(it.works_with(), vec![ok("a", "one: two"), ok("b", "three")]);
        assert_eq!(test_item().works_with(), vec![]);
    }

    #[test]
    fn lab_kinds_and_addresses() {
        let dir = std::env::temp_dir().join(format!("gw-lab-kinds-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("x.adoc");
        std::fs::write(&f, "= X\n:redirect-from: old/x, https://example.org/older/page.html?a=1, /\n:lab: true\n\nBody.\n").unwrap();
        let at = |rel: &str| load_item(&f, Path::new(rel)).unwrap();
        let e = at("lab/explorations/raft-by-hand.adoc");
        assert_eq!((e.kind, e.url.as_str()), (Kind::Exploration, "/lab/explorations/raft-by-hand/"));
        assert_eq!(e.redirect_from(), vec!["/old/x/", "/older/page.html"]);
        assert_eq!((at("lab/sketches/waves.adoc").kind, at("lab/sketches/waves.adoc").url.as_str()), (Kind::Playground, "/lab/sketches/waves/"));
        assert_eq!(at("lab/rosetta/fizzbuzz.adoc").url, "/lab/rosetta/fizzbuzz/");
        assert_eq!(at("lab/playground/lua.adoc").url, "/lab/playground/lua/");
        assert!(at("tools/t.adoc").in_lab());
        assert_eq!(Kind::parse("Exploration"), Some(Kind::Exploration));
        assert_eq!(Kind::Exploration.as_str(), "exploration");
        assert!(Kind::ALL.iter().all(|k| Kind::parse(k.as_str()) == Some(*k)));
        assert_eq!(redirect_path("a/./../b/"), "/a/b/");
        std::fs::remove_dir_all(&dir).ok();
    }

    fn test_item() -> Item {
        Item {
            id: "tools/t".into(), kind: Kind::Tool, slug: "t".into(), title: "T".into(), url: "/tools/t/".into(),
            date: None, updated: None, areas: vec![], tags: vec![], project: None, summary: String::new(), hero: None,
            draft: false, unlisted: false, featured: false, pinned: false, pin_order: None, repo: None, app_url: None,
            embed: false, source: None, attrs: BTreeMap::new(), path: PathBuf::new(), bundle_dir: None,
        }
    }
}
