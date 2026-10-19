//! Operations shared by every front end. The CLI, REST API, MCP server, and desktop app all
//! call these and get JSON back, so behaviour cannot drift between them.

use anyhow::{bail, Context, Result};
use gw_core::build::{build, BuildOptions};
use gw_core::content::{Item, Kind};
use gw_core::ideas::STATUSES;
use gw_core::media::Media;
use gw_core::publish;
use gw_core::registry::Entry;
use gw_core::scaffold::{self, NewItem};
use gw_core::Site;
use serde::Deserialize;
use serde_json::{json, Value};

pub fn item_json(it: &Item) -> Value {
    json!({
        "id": it.id, "kind": it.kind.as_str(), "title": it.title, "url": it.url, "slug": it.slug,
        "date": it.date.map(|d| d.to_string()), "areas": it.areas, "tags": it.tags,
        "project": it.project, "summary": it.summary, "draft": it.draft, "unlisted": it.unlisted,
        "featured": it.featured, "pinned": it.pinned, "repo": it.repo, "app_url": it.app_url, "source": it.source,
        "path": it.path.display().to_string(),
    })
}

#[derive(Debug, Default, Deserialize)]
pub struct ListArgs {
    pub kind: Option<String>,
    pub area: Option<String>,
    pub tag: Option<String>,
    #[serde(default)]
    pub drafts_only: bool,
}

pub fn list(site: &Site, a: &ListArgs) -> Result<Value> {
    let kind = a.kind.as_deref().map(|k| Kind::parse(k).with_context(|| format!("unknown kind {k}"))).transpose()?;
    let area = a.area.as_deref().map(|x| site.config.resolve_area(x).with_context(|| format!("unknown area {x}"))).transpose()?;
    let mut items: Vec<Item> = site
        .items()?
        .into_iter()
        .filter(|i| kind.map(|k| i.kind == k).unwrap_or(true))
        .filter(|i| !a.drafts_only || i.draft)
        .filter(|i| a.tag.as_ref().map(|t| i.tags.contains(t)).unwrap_or(true))
        .filter(|i| {
            area.as_ref()
                .map(|p| i.areas.iter().filter_map(|x| site.config.resolve_area(x)).any(|q| &q == p || q.starts_with(&format!("{p}/"))))
                .unwrap_or(true)
        })
        .collect();
    items.sort_by(|a, b| b.date.cmp(&a.date).then(a.id.cmp(&b.id)));
    Ok(json!(items.iter().map(item_json).collect::<Vec<_>>()))
}

pub fn show(site: &Site, id: &str) -> Result<Value> {
    let it = site.item(id)?;
    let mut v = item_json(&it);
    v["source_text"] = json!(std::fs::read_to_string(&it.path)?);
    v["hero_url"] = json!(gw_core::build::hero_url(&it, &site.media()?));
    v["hero"] = json!(it.hero);
    Ok(v)
}

#[derive(Debug, Default, Deserialize)]
pub struct NewArgs {
    pub kind: String,
    pub title: String,
    #[serde(default)]
    pub areas: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub project: Option<String>,
    #[serde(default)]
    pub summary: String,
    pub repo: Option<String>,
    pub url: Option<String>,
    /// Full AsciiDoc body (after the header). Optional.
    pub body: Option<String>,
    #[serde(default)]
    pub bundle: bool,
}

pub fn new_item(site: &Site, a: &NewArgs) -> Result<Value> {
    let kind = Kind::parse(&a.kind).with_context(|| format!("unknown kind {}", a.kind))?;
    let path = scaffold::create(
        site,
        &NewItem {
            kind,
            title: &a.title,
            areas: a.areas.clone(),
            tags: a.tags.clone(),
            project: a.project.as_deref(),
            summary: &a.summary,
            repo: a.repo.as_deref(),
            url: a.url.as_deref(),
            body: a.body.as_deref(),
            bundle: a.bundle,
        },
    )?;
    let rel = path.strip_prefix(site.content_dir())?.to_path_buf();
    let it = gw_core::content::load_item(&path, &rel)?;
    Ok(json!({ "created": path.display().to_string(), "item": item_json(&it) }))
}

/// Replace an item's whole source (header and body). Validates before writing.
pub fn write_source(site: &Site, id: &str, text: &str) -> Result<Value> {
    let it = site.item(id)?;
    let (title, _) = gw_core::content::parse_header(text);
    if title.is_none() {
        bail!("source must start with a `= Title` line");
    }
    let backup = std::fs::read_to_string(&it.path)?;
    std::fs::write(&it.path, text)?;
    let rel = it.path.strip_prefix(site.content_dir()).map(|p| p.to_path_buf()).unwrap_or_default();
    match gw_core::content::load_item(&it.path, &rel) {
        Ok(n) => Ok(json!({ "saved": it.path.display().to_string(), "item": item_json(&n) })),
        Err(e) => {
            std::fs::write(&it.path, backup)?;
            Err(e.context("not saved"))
        }
    }
}

pub fn set_status(site: &Site, id: &str, status: &str) -> Result<Value> {
    let it = site.item(id)?;
    scaffold::set_status(&it.path, status)?;
    Ok(json!({ "id": it.id, "status": status }))
}

pub fn areas(site: &Site) -> Result<Value> {
    Ok(json!(site
        .config
        .flat_areas()
        .into_iter()
        .map(|(p, a)| json!({"path": p, "id": a.id, "title": a.title, "description": a.description}))
        .collect::<Vec<_>>()))
}

pub fn check(site: &Site) -> Result<Value> {
    let problems = site.check()?;
    Ok(json!({ "ok": problems.is_empty(), "problems": problems }))
}

pub fn do_build(site: &Site, drafts: bool) -> Result<Value> {
    let r = build(site, &BuildOptions { drafts, out_dir: None })?;
    Ok(json!({
        "out_dir": r.out_dir.display().to_string(), "pages": r.pages, "items": r.items,
        "files": r.files.len(), "drafts_included": r.drafts_included, "warnings": r.warnings,
    }))
}

/// Build (without drafts) and show what publishing would change. Never writes the Pages repo.
pub fn publish_plan(site: &Site) -> Result<Value> {
    let r = build(site, &BuildOptions::default())?;
    let p = publish::plan(site, &r.out_dir)?;
    Ok(json!({
        "pages_repo": p.pages_repo.display().to_string(),
        "added": p.added, "updated": p.updated, "removed": p.removed, "unchanged": p.unchanged,
        "warnings": r.warnings,
    }))
}

/// Build without drafts, copy into the Pages repo, commit, and push. Outward-facing: callers
/// must have the user's confirmation.
pub fn publish_site(site: &Site, message: Option<&str>, push: bool) -> Result<Value> {
    let r = build(site, &BuildOptions::default())?;
    let msg = message.map(|s| s.to_string()).unwrap_or_else(|| {
        format!("gitwebsite: publish {}", chrono::Local::now().format("%Y-%m-%d %H:%M"))
    });
    let p = publish::publish(site, &r.out_dir, &msg, push)?;
    Ok(json!({
        "commit": p.commit, "pushed": p.pushed,
        "added": p.plan.added.len(), "updated": p.plan.updated.len(), "removed": p.plan.removed.len(),
        "url": site.config.site.base_url,
    }))
}

// ------------------------------------------------------------------ ideas

pub fn ideas_list(site: &Site, status: Option<&str>) -> Result<Value> {
    let ideas = site.ideas()?;
    Ok(json!(ideas.ideas.iter().filter(|i| status.map(|s| i.status == s).unwrap_or(true)).collect::<Vec<_>>()))
}

#[derive(Debug, Default, Deserialize)]
pub struct IdeaArgs {
    pub title: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub areas: Vec<String>,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub source: String,
}

pub fn idea_add(site: &Site, a: &IdeaArgs) -> Result<Value> {
    let mut ideas = site.ideas()?;
    let v = json!(ideas.add(&a.title, &a.notes, a.areas.clone(), &a.kind, &a.source));
    ideas.save(&site.ideas_path())?;
    Ok(v)
}

#[derive(Debug, Default, Deserialize)]
pub struct IdeaUpdate {
    pub status: Option<String>,
    pub title: Option<String>,
    pub notes: Option<String>,
    pub content: Option<String>,
    pub areas: Option<Vec<String>>,
}

pub fn idea_update(site: &Site, id: u32, u: &IdeaUpdate) -> Result<Value> {
    let mut ideas = site.ideas()?;
    if let Some(s) = &u.status {
        if !STATUSES.contains(&s.as_str()) {
            bail!("status must be one of {}", STATUSES.join(", "));
        }
        ideas.set_status(id, s)?;
    }
    let i = ideas.get_mut(id)?;
    if let Some(t) = &u.title { i.title = t.clone(); }
    if let Some(n) = &u.notes { i.notes = n.clone(); }
    if let Some(c) = &u.content { i.content = c.clone(); }
    if let Some(a) = &u.areas { i.areas = a.clone(); }
    i.updated = chrono::Local::now().date_naive();
    let v = json!(i);
    ideas.save(&site.ideas_path())?;
    Ok(v)
}

// ------------------------------------------------------------------ registry

pub fn registry_list(site: &Site, kind: Option<&str>) -> Result<Value> {
    let r = site.registry()?;
    Ok(json!(r.entries.iter().filter(|e| kind.map(|k| e.kind == k).unwrap_or(true)).collect::<Vec<_>>()))
}

pub fn registry_upsert(site: &Site, e: Entry) -> Result<Value> {
    let mut r = site.registry()?;
    let v = json!(e);
    r.upsert(e);
    r.save(&site.registry_path())?;
    Ok(v)
}

pub fn registry_remove(site: &Site, id: &str) -> Result<Value> {
    let mut r = site.registry()?;
    r.remove(id)?;
    r.save(&site.registry_path())?;
    Ok(json!({ "removed": id }))
}

// ------------------------------------------------------------------ media

pub fn media_list(site: &Site) -> Result<Value> {
    Ok(json!(site.media()?.media))
}

/// Which content uses each media key: `{ key: [{ id, title, url, draft }] }`. A page uses a key
/// when its source mentions `media:<key>` (image/video macros, :hero:).
pub fn media_usage(site: &Site) -> Result<Value> {
    let keys: Vec<String> = site.media()?.media.keys().cloned().collect();
    let mut out = serde_json::Map::new();
    for it in site.items()? {
        let Ok(text) = std::fs::read_to_string(&it.path) else { continue };
        for k in &keys {
            let pat = format!("media:{k}");
            let used = text.match_indices(&pat).any(|(i, _)| {
                let next = text[i + pat.len()..].chars().next();
                !matches!(next, Some(c) if c.is_ascii_alphanumeric() || "-_.".contains(c))
            });
            if used {
                let e = out.entry(k.clone()).or_insert_with(|| json!([]));
                e.as_array_mut().unwrap().push(json!({ "id": it.id, "title": it.title, "url": it.url, "draft": it.draft }));
            }
        }
    }
    Ok(Value::Object(out))
}

pub fn media_add(site: &Site, key: &str, m: Media) -> Result<Value> {
    let mut lib = site.media()?;
    if !key.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) {
        bail!("media keys may use letters, digits, '-', '_', '.'");
    }
    lib.media.insert(key.to_string(), m.clone());
    lib.save(&site.media_path())?;
    Ok(json!({ "key": key, "media": m, "use": format!("image::media:{key}[]") }))
}
