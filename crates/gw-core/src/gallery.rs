//! The public 3-D models gallery: `/models/` and `/models/<project>/<slug>/`.
//!
//! Which models are shown is the site's own choice, kept in `models.yaml` beside `site.yaml`:
//!
//! ```yaml
//! title: 3-D models
//! intro: Figures and props I have designed with Meshy.ai and printed.
//! projects:                      # optional: nicer chip titles / descriptions per project
//!   ai-force: { title: AI Force }
//! models:
//!   - key: lemmings/wizard-lemming   # <project>/<slug> in the Meshy models repository
//!     viewer: true                   # show the interactive 3-D viewer (needs a media entry
//!                                    #   of kind model with ids.model = the key)
//!     featured: true                 # first in the grid
//!     order: 10                      # sort before entries without an order
//!     name: Wizard lemming           # optional overrides of the repository's name,
//!     summary: …                     #   summary and description
//!     description: …
//! ```
//!
//! The pictures come from the models repository, but the build never reads it: `gw models
//! sync-site` (and the app's "Show on website" toggles) copy each selected model's renders
//! (`hero`, `front`, `side`, `top`, `plate`, `thumb` — WebP, ~1200 px) and a trimmed
//! `meta.json` into `static/model-gallery/<project>/<slug>/`, and remove the folders of models
//! no longer selected. So the site repository alone builds the gallery, reproducibly.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The selection file, beside site.yaml.
pub const FILE: &str = "models.yaml";
/// Under `static/`: the copied renders and meta.json, served at `/model-gallery/...`.
pub const STATIC_DIR: &str = "model-gallery";
/// The renders copied for each model, in display order (`thumb` is for the grid only).
pub const VIEWS: &[&str] = &["hero", "front", "side", "top", "plate", "thumb"];

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Gallery {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub intro: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub projects: BTreeMap<String, ProjectInfo>,
    #[serde(default)]
    pub models: Vec<Pick>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ProjectInfo {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

/// One selected model.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Pick {
    /// `<project>/<slug>`.
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<i64>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub featured: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub viewer: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// A folder name from the models repository: letters, digits, `-`, `_`, `.`; not hidden.
pub fn safe_segment(s: &str) -> bool {
    !s.is_empty() && s.len() <= 120 && !s.starts_with('.') && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// `<project>/<slug>` split and checked (no traversal, no extra segments).
pub fn parse_key(key: &str) -> Result<(&str, &str)> {
    let Some((p, s)) = key.split_once('/') else { bail!("model key {key:?}: use <project>/<slug>") };
    if !safe_segment(p) || !safe_segment(s) {
        bail!("model key {key:?}: <project>/<slug> may use letters, digits, '-', '_', '.' (and may not start with '.')");
    }
    Ok((p, s))
}

impl Gallery {
    pub fn path(site_root: &Path) -> PathBuf {
        site_root.join(FILE)
    }

    /// The site's models.yaml, or `None` when it has none.
    pub fn load(site_root: &Path) -> Result<Option<Gallery>> {
        let p = Self::path(site_root);
        if !p.exists() {
            return Ok(None);
        }
        let t = std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
        Ok(Some(Self::parse(&t).with_context(|| format!("parsing {}", p.display()))?))
    }

    pub fn parse(text: &str) -> Result<Gallery> {
        if text.trim().is_empty() {
            return Ok(Gallery::default());
        }
        let g: Gallery = serde_yaml::from_str(text)?;
        let mut seen = std::collections::BTreeSet::new();
        for m in &g.models {
            parse_key(&m.key)?;
            if !seen.insert(m.key.as_str()) {
                bail!("{} is listed twice", m.key);
            }
        }
        Ok(g)
    }

    pub fn to_yaml(&self) -> Result<String> {
        Ok(format!(
            "# The public 3-D models gallery (/models/): which models from the Meshy models repository are\n\
             # shown. Edit by hand or with the gitwebsite app's Models page; then `gw models sync-site`\n\
             # copies their pictures into static/model-gallery/. Keys are <project>/<slug>; optional:\n\
             # viewer (interactive 3-D, needs a model media entry), featured, order, name, summary, description.\n{}",
            serde_yaml::to_string(self)?
        ))
    }

    pub fn save(&self, site_root: &Path) -> Result<()> {
        let p = Self::path(site_root);
        let tmp = p.with_extension("yaml.tmp");
        std::fs::write(&tmp, self.to_yaml()?)?;
        std::fs::rename(&tmp, &p)?;
        Ok(())
    }

    pub fn get(&self, key: &str) -> Option<&Pick> {
        self.models.iter().find(|m| m.key == key)
    }

    /// Switch a model on (kept where it is, or added after the last of its project) or off.
    /// Returns whether anything changed.
    pub fn set_on(&mut self, key: &str, on: bool) -> Result<bool> {
        let (project, _) = parse_key(key)?;
        let at = self.models.iter().position(|m| m.key == key);
        match (on, at) {
            (true, Some(_)) | (false, None) => Ok(false),
            (false, Some(i)) => {
                self.models.remove(i);
                Ok(true)
            }
            (true, None) => {
                let pick = Pick { key: key.to_string(), ..Default::default() };
                match self.models.iter().rposition(|m| m.key.split('/').next() == Some(project)) {
                    Some(i) => self.models.insert(i + 1, pick),
                    None => self.models.push(pick),
                }
                Ok(true)
            }
        }
    }

    /// Turn the interactive viewer on or off; on also switches the model on.
    pub fn set_viewer(&mut self, key: &str, viewer: bool) -> Result<bool> {
        if viewer {
            self.set_on(key, true)?;
        }
        Ok(match self.models.iter_mut().find(|m| m.key == key) {
            Some(m) if m.viewer != viewer => {
                m.viewer = viewer;
                true
            }
            _ => false,
        })
    }

    /// Display order: featured first, then by `order`, then as listed.
    pub fn ordered(&self) -> Vec<&Pick> {
        let mut v: Vec<(usize, &Pick)> = self.models.iter().enumerate().collect();
        v.sort_by_key(|(i, m)| (!m.featured, m.order.unwrap_or(i64::MAX), *i));
        v.into_iter().map(|(_, m)| m).collect()
    }
}

// ------------------------------------------------------------------ meta.json

/// What the site keeps of a model's meshy.json (no Meshy task ids, prompts or file hashes).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Meta {
    pub project: String,
    pub slug: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub project_description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triangles: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dimensions_mm: Vec<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parts: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plates: Option<u64>,
    #[serde(default)]
    pub multi_part: bool,
    #[serde(default)]
    pub textured: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub orientation: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub created: String,
    /// The renders present, by view name (see [`VIEWS`]), each `<view>.webp` beside meta.json.
    #[serde(default)]
    pub images: Vec<String>,
}

impl Meta {
    pub fn from_meshy(m: &Value, project: &str, slug: &str, project_description: &str, images: Vec<String>) -> Meta {
        let s = |v: &Value| v.as_str().unwrap_or("").trim().to_string();
        let print = &m["print"];
        Meta {
            project: project.into(),
            slug: slug.into(),
            name: s(&m["name"]),
            summary: s(&m["summary"]),
            description: s(&m["description"]),
            project_description: project_description.into(),
            triangles: m["mesh"]["triangles"].as_u64(),
            dimensions_mm: print["dimensions_mm"].as_array().map(|a| a.iter().filter_map(|x| x.as_f64()).collect()).unwrap_or_default(),
            parts: print["parts"].as_u64(),
            plates: print["plates"].as_u64(),
            multi_part: print["kind"] == "multi",
            textured: m["textured"].as_bool().unwrap_or(false),
            orientation: s(&print["orientation"]),
            created: s(&m["meshy"]["created"]).chars().take(10).collect(),
            images,
        }
    }
}

/// A repository name made presentable: `gino-lemming-3d` → `Gino Lemming`, `Bill the Lemming
/// 3D` → `Bill the Lemming`.
pub fn display_name(name: &str, slug: &str) -> String {
    let n = if name.trim().is_empty() { slug } else { name.trim() };
    let slugish = !n.contains(' ') && n.contains('-');
    let words: Vec<String> = if slugish { n.split('-').map(str::to_string).collect() } else { n.split_whitespace().map(str::to_string).collect() };
    let mut words: Vec<String> = words.into_iter().filter(|w| !w.is_empty()).collect();
    if words.len() > 1 && words.last().is_some_and(|w| w.eq_ignore_ascii_case("3d")) {
        words.pop();
    }
    if slugish {
        words = words.iter().map(|w| capitalise(w)).collect();
    }
    words.join(" ")
}

fn capitalise(w: &str) -> String {
    let mut c = w.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// `ai-force` → `Ai force`; models.yaml `projects:` gives better titles.
pub fn project_title(id: &str) -> String {
    capitalise(&id.replace(['-', '_'], " "))
}

// ------------------------------------------------------------------ sync

#[derive(Debug, Default, Serialize)]
pub struct SyncReport {
    /// Models whose files were written or changed.
    pub updated: Vec<String>,
    pub unchanged: usize,
    /// Model folders removed (no longer selected).
    pub removed: Vec<String>,
    /// Bytes now in static/model-gallery/.
    pub bytes: u64,
    pub files: usize,
    pub problems: Vec<String>,
}

fn write_if_changed(p: &Path, bytes: &[u8]) -> Result<bool> {
    if std::fs::read(p).ok().as_deref() == Some(bytes) {
        return Ok(false);
    }
    std::fs::write(p, bytes)?;
    Ok(true)
}

/// Bring `static/model-gallery/` in line with the selection, from the models repository `repo`
/// (`<project>/<slug>/meshy.json`, `images/*.webp`, `projects.yaml`). Only WebP renders named in
/// [`VIEWS`] and a trimmed meta.json are written; a model missing from the repository is a
/// problem (its old folder, if any, is kept).
pub fn sync(static_dir: &Path, repo: &Path, g: &Gallery) -> Result<SyncReport> {
    let mut r = SyncReport::default();
    let root = static_dir.join(STATIC_DIR);
    let repo_c = repo.canonicalize().with_context(|| format!("the models repository {} is not there", repo.display()))?;
    let descr: serde_yaml::Value = std::fs::read_to_string(repo.join("projects.yaml")).ok().and_then(|t| serde_yaml::from_str(&t).ok()).unwrap_or_default();
    let mut keep: std::collections::BTreeSet<(String, String)> = Default::default();
    for pick in &g.models {
        let (project, slug) = parse_key(&pick.key)?;
        keep.insert((project.into(), slug.into()));
        let src = repo.join(project).join(slug);
        let Ok(text) = std::fs::read_to_string(src.join("meshy.json")) else {
            r.problems.push(format!("{}: not in the models repository", pick.key));
            continue;
        };
        let meshy: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                r.problems.push(format!("{}: meshy.json: {e}", pick.key));
                continue;
            }
        };
        let dest = root.join(project).join(slug);
        std::fs::create_dir_all(&dest)?;
        let mut changed = false;
        let mut images = vec![];
        let mut wanted = vec!["meta.json".to_string()];
        for v in VIEWS {
            let f = src.join("images").join(format!("{v}.webp"));
            // Follow links only within the repository.
            let Some(fc) = f.canonicalize().ok().filter(|c| c.starts_with(&repo_c) && c.is_file()) else { continue };
            let bytes = std::fs::read(&fc)?;
            changed |= write_if_changed(&dest.join(format!("{v}.webp")), &bytes)?;
            wanted.push(format!("{v}.webp"));
            images.push(v.to_string());
        }
        if !images.iter().any(|i| i == "hero") {
            r.problems.push(format!("{}: no images/hero.webp in the repository", pick.key));
        }
        let pd = descr["projects"][project].as_str().unwrap_or("");
        let meta = Meta::from_meshy(&meshy, project, slug, pd, images);
        changed |= write_if_changed(&dest.join("meta.json"), format!("{}\n", serde_json::to_string_pretty(&meta)?).as_bytes())?;
        for e in std::fs::read_dir(&dest)?.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if !wanted.contains(&n) {
                if e.path().is_dir() { std::fs::remove_dir_all(e.path())?; } else { std::fs::remove_file(e.path())?; }
                changed = true;
            }
        }
        if changed { r.updated.push(pick.key.clone()); } else { r.unchanged += 1; }
    }
    // Folders of models no longer selected (only those holding a meta.json: nothing else is ours).
    if root.is_dir() {
        for p in std::fs::read_dir(&root)?.flatten().filter(|e| e.path().is_dir()) {
            let project = p.file_name().to_string_lossy().to_string();
            for m in std::fs::read_dir(p.path())?.flatten().filter(|e| e.path().is_dir()) {
                let slug = m.file_name().to_string_lossy().to_string();
                if !keep.contains(&(project.clone(), slug.clone())) && m.path().join("meta.json").is_file() {
                    std::fs::remove_dir_all(m.path())?;
                    r.removed.push(format!("{project}/{slug}"));
                }
            }
            if std::fs::read_dir(p.path())?.next().is_none() {
                std::fs::remove_dir(p.path())?;
            }
        }
        for e in walkdir::WalkDir::new(&root).into_iter().flatten().filter(|e| e.file_type().is_file()) {
            r.files += 1;
            r.bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    Ok(r)
}

/// Read a synced model's meta.json from the site.
pub fn read_meta(static_dir: &Path, key: &str) -> Result<Meta> {
    let (p, s) = parse_key(key)?;
    let f = static_dir.join(STATIC_DIR).join(p).join(s).join("meta.json");
    let t = std::fs::read_to_string(&f).with_context(|| format!("{key}: no {STATIC_DIR}/{p}/{s}/meta.json — run `gw models sync-site`"))?;
    serde_json::from_str(&t).with_context(|| format!("parsing {}", f.display()))
}

/// The media key of the site's 3-D web model for `key` (a `kind: model` entry whose
/// `ids.model` is the key), if there is one.
pub fn media_key_for(lib: &crate::media::Library, key: &str) -> Option<String> {
    lib.media.iter().find(|(_, m)| m.kind == "model" && m.ids.get("model").map(String::as_str) == Some(key)).map(|(k, _)| k.clone())
}

/// Problems with the gallery, for `gw check`.
pub fn check(site_root: &Path, static_dir: &Path, lib: &crate::media::Library) -> Vec<String> {
    let g = match Gallery::load(site_root) {
        Ok(Some(g)) => g,
        Ok(None) => return vec![],
        Err(e) => return vec![format!("{FILE}: {e:#}")],
    };
    let mut out = vec![];
    for m in &g.models {
        match read_meta(static_dir, &m.key) {
            Ok(meta) if !meta.images.iter().any(|i| i == "hero") => out.push(format!("{FILE}: {}: no hero image in {STATIC_DIR}/", m.key)),
            Ok(_) => {}
            Err(e) => out.push(format!("{FILE}: {e:#}")),
        }
        if m.viewer && media_key_for(lib, &m.key).is_none() {
            out.push(format!("{FILE}: {}: viewer is on but no 3-D web model is in the media library (gw media add-model {} --download none)", m.key, m.key));
        }
    }
    out
}

// ------------------------------------------------------------------ pages

/// A model as the gallery templates see it.
#[derive(Debug, Clone, Serialize)]
pub struct Card {
    pub key: String,
    pub project: String,
    pub project_title: String,
    pub slug: String,
    pub name: String,
    pub summary: String,
    pub description_html: String,
    /// `/models/<project>/<slug>/`
    pub url: String,
    pub thumb: String,
    pub hero: String,
    /// Absolute URL of the hero render (Open Graph).
    pub og_image: String,
    /// The angle views (hero, front, side, top, plate) with labels.
    pub images: Vec<ImageView>,
    /// `64 × 55 × 100 mm`
    pub dims: String,
    pub parts: u64,
    pub plates: u64,
    pub multi_part: bool,
    pub triangles_text: String,
    pub textured: bool,
    pub orientation: String,
    pub created_human: String,
    pub featured: bool,
    /// The `<model-viewer>` figure, when the viewer is on and the site has the web model.
    pub viewer_html: String,
    pub media_key: String,
    pub prev: Option<NavLink>,
    pub next: Option<NavLink>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImageView {
    pub name: String,
    pub label: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct NavLink {
    pub url: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectChip {
    pub id: String,
    pub title: String,
    pub description: String,
    pub count: usize,
}

/// Everything the gallery pages need.
#[derive(Debug, Clone, Serialize)]
pub struct Pages {
    pub title: String,
    pub intro: String,
    pub cards: Vec<Card>,
    pub projects: Vec<ProjectChip>,
}

pub const DEFAULT_TITLE: &str = "3-D models";
pub const DEFAULT_INTRO: &str = "Figures and objects I have made as 3-D models and printed. Open one to see it from every side.";

fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn paragraphs(text: &str) -> String {
    text.split("\n\n").map(str::trim).filter(|p| !p.is_empty()).map(|p| format!("<p>{}</p>", crate::model::esc(p))).collect::<Vec<_>>().join("\n")
}

fn human_date(iso: &str) -> String {
    chrono::NaiveDate::parse_from_str(iso, "%Y-%m-%d").map(|d| d.format("%-d %B %Y").to_string()).unwrap_or_default()
}

/// Build the gallery's cards from models.yaml and the synced `static/model-gallery/`. Models
/// that were not synced are left out with a warning.
pub fn pages(g: &Gallery, static_dir: &Path, lib: &crate::media::Library, base_url: &str, warnings: &mut Vec<String>) -> Pages {
    let base = base_url.trim_end_matches('/');
    let mut cards: Vec<Card> = vec![];
    for pick in g.ordered() {
        let meta = match read_meta(static_dir, &pick.key) {
            Ok(m) => m,
            Err(e) => {
                warnings.push(format!("models gallery: {e:#}"));
                continue;
            }
        };
        let (project, slug) = (meta.project.clone(), meta.slug.clone());
        if format!("{project}/{slug}") != pick.key {
            warnings.push(format!("models gallery: {}: meta.json is for {project}/{slug}", pick.key));
            continue;
        }
        let dir = format!("/{STATIC_DIR}/{project}/{slug}/");
        let has = |v: &str| meta.images.iter().any(|i| i == v);
        if !has("hero") {
            warnings.push(format!("models gallery: {}: no hero image", pick.key));
            continue;
        }
        let name = pick.name.clone().unwrap_or_else(|| display_name(&meta.name, &slug));
        let images: Vec<ImageView> = VIEWS
            .iter()
            .filter(|v| **v != "thumb" && has(v))
            .map(|v| ImageView { name: v.to_string(), label: capitalise(v), url: format!("{dir}{v}.webp") })
            .collect();
        let pinfo = g.projects.get(&project);
        let mut viewer_html = String::new();
        let mut media_key = String::new();
        if pick.viewer {
            match media_key_for(lib, &pick.key) {
                Some(k) => match crate::model::viewer_html(lib, &k, &name, Some(static_dir)) {
                    Ok(h) => {
                        viewer_html = h;
                        media_key = k;
                    }
                    Err(e) => warnings.push(format!("models gallery: {}: {e}", pick.key)),
                },
                None => warnings.push(format!("models gallery: {}: viewer is on but the site has no 3-D web model for it (gw media add-model {} --download none)", pick.key, pick.key)),
            }
        }
        let dims = if meta.dimensions_mm.len() == 3 {
            format!("{} mm", meta.dimensions_mm.iter().map(|x| format!("{x:.0}")).collect::<Vec<_>>().join(" × "))
        } else {
            String::new()
        };
        cards.push(Card {
            key: pick.key.clone(),
            project_title: pinfo.map(|p| p.title.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| project_title(&project)),
            url: format!("/models/{project}/{slug}/"),
            thumb: format!("{dir}{}.webp", if has("thumb") { "thumb" } else { "hero" }),
            hero: format!("{dir}hero.webp"),
            og_image: format!("{base}{dir}hero.webp"),
            images,
            summary: pick.summary.clone().unwrap_or(meta.summary.clone()),
            description_html: paragraphs(pick.description.as_deref().unwrap_or(&meta.description)),
            dims,
            parts: meta.parts.unwrap_or(1),
            plates: meta.plates.unwrap_or(0),
            multi_part: meta.multi_part,
            triangles_text: meta.triangles.map(thousands).unwrap_or_default(),
            textured: meta.textured,
            orientation: meta.orientation.clone(),
            created_human: human_date(&meta.created),
            featured: pick.featured,
            viewer_html,
            media_key,
            prev: None,
            next: None,
            name,
            project,
            slug,
        });
    }
    let links: Vec<NavLink> = cards.iter().map(|c| NavLink { url: c.url.clone(), name: c.name.clone() }).collect();
    for (i, c) in cards.iter_mut().enumerate() {
        c.prev = i.checked_sub(1).map(|j| links[j].clone());
        c.next = links.get(i + 1).cloned();
    }
    // Project chips in the order projects first appear in the grid.
    let mut projects: Vec<ProjectChip> = vec![];
    for c in &cards {
        match projects.iter_mut().find(|p| p.id == c.project) {
            Some(p) => p.count += 1,
            None => {
                let description = g.projects.get(&c.project).map(|p| p.description.clone()).filter(|d| !d.is_empty())
                    .or_else(|| read_meta(static_dir, &c.key).ok().map(|m| m.project_description))
                    .unwrap_or_default();
                projects.push(ProjectChip { id: c.project.clone(), title: c.project_title.clone(), description, count: 1 });
            }
        }
    }
    Pages {
        title: if g.title.is_empty() { DEFAULT_TITLE.into() } else { g.title.clone() },
        intro: if g.intro.is_empty() { DEFAULT_INTRO.into() } else { g.intro.clone() },
        cards,
        projects,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!("gw-gallery-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn parses_and_round_trips() {
        let text = "title: 3-D models\nintro: Things.\nprojects:\n  ai-force: { title: AI Force }\nmodels:\n  - key: lemmings/wizard-lemming\n    viewer: true\n    featured: true\n  - key: ai-force/ai-force-teen\n    order: 3\n    name: Teen\n";
        let g = Gallery::parse(text).unwrap();
        assert_eq!(g.models.len(), 2);
        assert!(g.models[0].viewer && g.models[0].featured);
        assert_eq!(g.models[1].order, Some(3));
        assert_eq!(g.projects["ai-force"].title, "AI Force");
        let again = Gallery::parse(&g.to_yaml().unwrap()).unwrap();
        assert_eq!(g, again);
        // Defaults are left out of the file.
        let y = g.to_yaml().unwrap();
        assert!(!y.contains("viewer: false") && !y.contains("featured: false") && !y.contains("order: null"));
        assert_eq!(Gallery::parse("").unwrap(), Gallery::default());
        assert!(Gallery::load(&tmp("none")).unwrap().is_none());
    }

    #[test]
    fn rejects_bad_keys_and_duplicates() {
        for bad in ["../etc/passwd", "lemmings", "a/b/c", "/abs/x", "a/..", ".git/x", "a/.hidden", "a\\b/c", "a/b c", ""] {
            assert!(parse_key(bad).is_err(), "{bad}");
            assert!(Gallery::parse(&format!("models:\n  - key: {bad:?}\n")).is_err(), "{bad}");
        }
        assert!(Gallery::parse("models:\n  - key: a/b\n  - key: a/b\n").unwrap_err().to_string().contains("twice"));
        assert!(Gallery::default().set_on("../x", true).is_err());
    }

    #[test]
    fn toggles_keep_projects_together_and_order() {
        let mut g = Gallery::parse("models:\n  - key: a/one\n  - key: b/one\n").unwrap();
        assert!(g.set_on("a/two", true).unwrap());
        assert!(!g.set_on("a/two", true).unwrap());
        assert_eq!(g.models.iter().map(|m| m.key.as_str()).collect::<Vec<_>>(), ["a/one", "a/two", "b/one"]);
        assert!(g.set_viewer("c/new", true).unwrap() || g.get("c/new").unwrap().viewer);
        assert!(g.get("c/new").unwrap().viewer);
        assert!(g.set_on("a/one", false).unwrap());
        assert!(!g.set_on("a/one", false).unwrap());
        g.models[1].featured = true; // b/one
        g.models[0].order = Some(1); // a/two
        let o: Vec<&str> = g.ordered().iter().map(|m| m.key.as_str()).collect();
        assert_eq!(o, ["b/one", "a/two", "c/new"]);
    }

    #[test]
    fn display_names() {
        assert_eq!(display_name("gino-lemming-3d", "gino-lemming"), "Gino Lemming");
        assert_eq!(display_name("Bill the Lemming 3D", "x"), "Bill the Lemming");
        assert_eq!(display_name("AI Force Teen", "x"), "AI Force Teen");
        assert_eq!(display_name("", "rj-lemming-figure"), "Rj Lemming Figure");
        assert_eq!(project_title("ai-force"), "Ai force");
    }

    fn fake_repo() -> PathBuf {
        let repo = tmp("repo");
        std::fs::write(repo.join("projects.yaml"), "projects:\n  lemmings: Lemmings-style figures\n").unwrap();
        for slug in ["wiz", "gino"] {
            let d = repo.join("lemmings").join(slug);
            std::fs::create_dir_all(d.join("images")).unwrap();
            std::fs::write(
                d.join("meshy.json"),
                format!(r#"{{"name":"{slug}-3d","slug":"{slug}","project":"lemmings","summary":"S {slug}","description":"D {slug}","meshy":{{"id":"secret-task","object_prompt":"secret","created":"2026-08-09T17:47:07+00:00"}},"print":{{"dimensions_mm":[64.0,55.0,100.0],"parts":2,"kind":"multi","plates":1,"orientation":"upright"}},"mesh":{{"triangles":421642}},"textured":true}}"#),
            )
            .unwrap();
            for v in ["hero", "front", "side", "top", "thumb"] {
                std::fs::write(d.join("images").join(format!("{v}.webp")), format!("{slug}-{v}")).unwrap();
            }
            std::fs::write(d.join("images/preview.png"), "big png").unwrap();
            std::fs::write(d.join(format!("{slug}-print.stl")), "solid").unwrap();
        }
        repo
    }

    #[test]
    fn sync_copies_selected_and_removes_the_rest() {
        let repo = fake_repo();
        let st = tmp("static");
        let mut g = Gallery::parse("models:\n  - key: lemmings/wiz\n  - key: lemmings/gino\n  - key: lemmings/missing\n").unwrap();
        let r = sync(&st, &repo, &g).unwrap();
        assert_eq!(r.updated, ["lemmings/wiz", "lemmings/gino"]);
        assert_eq!(r.problems.len(), 1, "{:?}", r.problems);
        let d = st.join("model-gallery/lemmings/wiz");
        let mut names: Vec<String> = std::fs::read_dir(&d).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        names.sort();
        assert_eq!(names, ["front.webp", "hero.webp", "meta.json", "side.webp", "thumb.webp", "top.webp"]);
        let meta_text = std::fs::read_to_string(d.join("meta.json")).unwrap();
        assert!(!meta_text.contains("secret"), "meshy ids/prompts stay out of the site");
        let meta = read_meta(&st, "lemmings/wiz").unwrap();
        assert_eq!((meta.parts, meta.multi_part, meta.triangles), (Some(2), true, Some(421642)));
        assert_eq!(meta.project_description, "Lemmings-style figures");
        assert_eq!(meta.created, "2026-08-09");
        assert_eq!(meta.images, ["hero", "front", "side", "top", "thumb"]);
        // Again: nothing changes.
        let r = sync(&st, &repo, &g).unwrap();
        assert!(r.updated.is_empty() && r.unchanged == 2);
        // Switched off → removed; an unrelated file in the folder is left alone.
        std::fs::write(st.join("model-gallery/README.txt"), "x").unwrap();
        g.set_on("lemmings/gino", false).unwrap();
        let r = sync(&st, &repo, &g).unwrap();
        assert_eq!(r.removed, ["lemmings/gino"]);
        assert!(!st.join("model-gallery/lemmings/gino").exists());
        assert!(st.join("model-gallery/README.txt").exists());
        // Nothing selected → the project folder goes too.
        let r = sync(&st, &repo, &Gallery::default()).unwrap();
        assert_eq!(r.removed, ["lemmings/wiz"]);
        assert!(!st.join("model-gallery/lemmings").exists());
    }

    #[cfg(unix)]
    #[test]
    fn sync_does_not_follow_links_out_of_the_repository() {
        let repo = fake_repo();
        let outside = tmp("outside");
        std::fs::write(outside.join("secret.webp"), "secret").unwrap();
        let img = repo.join("lemmings/wiz/images/front.webp");
        std::fs::remove_file(&img).unwrap();
        std::os::unix::fs::symlink(outside.join("secret.webp"), &img).unwrap();
        let st = tmp("static-link");
        sync(&st, &repo, &Gallery::parse("models:\n  - key: lemmings/wiz\n").unwrap()).unwrap();
        assert!(!st.join("model-gallery/lemmings/wiz/front.webp").exists());
        assert!(st.join("model-gallery/lemmings/wiz/hero.webp").exists());
    }

    #[test]
    fn build_writes_the_gallery() {
        let repo = fake_repo();
        let root = tmp("site");
        std::fs::create_dir_all(root.join("content/pages")).unwrap();
        std::fs::write(root.join("content/pages/about.adoc"), "= About\n:type: page\n\nHi.\n").unwrap();
        std::fs::write(root.join("site.yaml"), "site:\n  title: T\n  base_url: https://example.org\n  author: A\n").unwrap();
        std::fs::write(
            root.join("media.yaml"),
            "media:\n  wiz:\n    url: /models/wiz/wiz.glb\n    kind: model\n    provider: local\n    poster: /models/wiz/poster.webp\n    ids:\n      model: lemmings/wiz\n",
        )
        .unwrap();
        std::fs::write(
            root.join(FILE),
            "title: Models\nintro: My figures.\nprojects:\n  lemmings: { title: Lemmings }\nmodels:\n  - key: lemmings/gino\n  - key: lemmings/wiz\n    viewer: true\n    featured: true\n",
        )
        .unwrap();
        let site = crate::Site::open(Some(&root)).unwrap();
        let g = Gallery::load(&root).unwrap().unwrap();
        sync(&site.static_dir(), &repo, &g).unwrap();
        assert!(site.check().unwrap().is_empty(), "{:?}", site.check().unwrap());
        let out = root.join("dist");
        let r = crate::build::build(&site, &crate::build::BuildOptions { drafts: false, out_dir: Some(out.clone()) }).unwrap();
        let read = |p: &str| std::fs::read_to_string(out.join(p)).unwrap_or_else(|_| panic!("{p} missing"));
        let index = read("models/index.html");
        assert!(index.contains("My figures."));
        // Featured first; cards link to the model pages and use the small thumbnails.
        let (w, gi) = (index.find("/models/lemmings/wiz/").unwrap(), index.find("/models/lemmings/gino/").unwrap());
        assert!(w < gi);
        assert!(index.contains("/model-gallery/lemmings/gino/thumb.webp") && !index.contains("hero.webp\" alt=\"\" width=\"240\""));
        assert!(index.contains("loading=\"lazy\"") && index.contains("/css/models.css") && index.contains("/js/models.js"));
        assert!(!index.contains("<model-viewer"), "the viewer loads only with a model's detail");
        let wiz = read("models/lemmings/wiz/index.html");
        assert!(wiz.contains("<model-viewer") && wiz.contains("model-viewer.min.js"));
        assert!(wiz.contains("og:image\" content=\"https://example.org/model-gallery/lemmings/wiz/hero.webp"));
        assert!(wiz.contains("data-model-detail") && wiz.contains("/model-gallery/lemmings/wiz/front.webp"));
        assert!(wiz.contains("Wiz") && wiz.contains("421,642") && wiz.contains("64 × 55 × 100 mm") && wiz.contains("2 parts"));
        assert!(!wiz.contains("Download"), "no download unless the media entry has one");
        let gino = read("models/lemmings/gino/index.html");
        assert!(!gino.contains("<model-viewer") && !gino.contains("model-viewer.min.js"));
        assert!(gino.contains("rel=\"prev\" href=\"/models/lemmings/wiz/\""));
        assert!(out.join("model-gallery/lemmings/wiz/hero.webp").is_file() && out.join("model-gallery/lemmings/gino/side.webp").is_file());
        let sm = read("sitemap.xml");
        assert!(sm.contains("https://example.org/models/</loc>") && sm.contains("/models/lemmings/wiz/</loc>") && sm.contains("/models/lemmings/gino/</loc>"));
        assert!(r.files.iter().any(|f| f == "models/lemmings/gino/index.html"));

        // An unsynced model and a viewer without a web model are reported by check (and left out).
        std::fs::write(root.join(FILE), "models:\n  - key: lemmings/gino\n    viewer: true\n  - key: lemmings/other\n").unwrap();
        let problems = site.check().unwrap();
        assert!(problems.iter().any(|p| p.contains("lemmings/other") && p.contains("sync-site")), "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("lemmings/gino") && p.contains("add-model")), "{problems:?}");
        crate::build::build(&site, &crate::build::BuildOptions { drafts: false, out_dir: Some(out.clone()) }).unwrap();
        assert!(!out.join("models/lemmings/other").exists() && out.join("models/lemmings/gino/index.html").is_file());
        // No models.yaml → no gallery.
        std::fs::remove_file(root.join(FILE)).unwrap();
        crate::build::build(&site, &crate::build::BuildOptions { drafts: false, out_dir: Some(out.clone()) }).unwrap();
        assert!(!out.join("models/index.html").exists());
        assert!(!read("sitemap.xml").contains("/models/"));
    }
}
