//! The site's public 3-D models gallery, from the gitwebsite side: which Meshy models are on
//! the website (`models.yaml`), switched on and off from the app's Models page or `gw models`,
//! and `gw models sync-site`, which copies the selected models' renders into the site
//! (`static/model-gallery/`). See [`gw_core::gallery`] for the file and the pages.

use super::meshy;
use anyhow::{bail, Context, Result};
use gw_core::gallery::{self, Gallery};
use gw_core::Site;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// One read-modify-write of models.yaml at a time (the app can send toggles quickly).
static LOCK: Mutex<()> = Mutex::new(());

/// One change: a model (`key`) or every model of a `project`, switched `on`/off and/or its
/// interactive 3-D `viewer` on/off.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Change {
    pub key: Option<String>,
    pub project: Option<String>,
    pub on: Option<bool>,
    pub viewer: Option<bool>,
    pub featured: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Changes {
    pub changes: Vec<Change>,
}

fn repo() -> Result<PathBuf> {
    meshy::Ctx::load()?.repo.context("no Meshy models repository: `gw meshy config --repo <path>`")
}

/// `<project>/<slug>` keys of the models in the repository (folders holding a meshy.json).
pub fn repo_keys(repo: &Path, project: Option<&str>) -> Vec<String> {
    let mut out = vec![];
    let Ok(rd) = std::fs::read_dir(repo) else { return out };
    for p in rd.flatten().filter(|e| e.path().is_dir()) {
        let pn = p.file_name().to_string_lossy().to_string();
        if !gallery::safe_segment(&pn) || pn == "tools" || project.is_some_and(|x| x != pn) {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(p.path()) else { continue };
        for m in rd.flatten() {
            let sn = m.file_name().to_string_lossy().to_string();
            if gallery::safe_segment(&sn) && m.path().join("meshy.json").is_file() {
                out.push(format!("{pn}/{sn}"));
            }
        }
    }
    out.sort();
    out
}

/// The key, checked: well formed, and a model in the repository.
fn existing(repo: &Path, key: &str) -> Result<String> {
    let (p, s) = gallery::parse_key(key)?;
    if !repo.join(p).join(s).join("meshy.json").is_file() {
        bail!("no model {key} in the models repository");
    }
    Ok(format!("{p}/{s}"))
}

/// The selection as the app shows it.
pub fn state(site: &Site) -> Result<Value> {
    let g = Gallery::load(&site.root)?;
    let exists = g.is_some();
    let g = g.unwrap_or_default();
    let lib = site.media()?;
    let st = site.static_dir();
    let models: Vec<Value> = g
        .models
        .iter()
        .map(|m| {
            let mk = gallery::media_key_for(&lib, &m.key);
            json!({
                "key": m.key, "viewer": m.viewer, "featured": m.featured, "order": m.order,
                "synced": gallery::read_meta(&st, &m.key).is_ok(),
                "media_key": mk,
            })
        })
        .collect();
    // 3-D web models the site has for any Meshy model (on the gallery or not).
    let web_models: serde_json::Map<String, Value> = lib
        .media
        .iter()
        .filter(|(_, m)| m.kind == "model")
        .filter_map(|(k, m)| m.ids.get("model").map(|id| (id.clone(), json!(k))))
        .collect();
    let dir = st.join(gallery::STATIC_DIR);
    let bytes: u64 = walk_bytes(&dir);
    let home = gallery_home(site)?;
    Ok(json!({
        "file": Gallery::path(&site.root).display().to_string(), "exists": exists,
        "title": g.title, "intro": g.intro, "count": g.models.len(), "models": models,
        "web_models": web_models, "bytes": bytes,
        "url": format!("{}{}", site.config.site.base_url.trim_end_matches('/'), home.as_ref().map(|h| h.url.as_str()).unwrap_or("/models/")),
        "home": home.map(|h| json!({ "id": h.id, "title": h.title, "url": h.url, "draft": h.draft })),
    }))
}

fn walk_bytes(dir: &Path) -> u64 {
    walkdir::WalkDir::new(dir).into_iter().flatten().filter(|e| e.file_type().is_file()).map(|e| e.metadata().map(|m| m.len()).unwrap_or(0)).sum()
}

/// Apply changes to models.yaml (creating it), then sync the pictures into the site.
pub fn apply(site: &Site, repo: &Path, ch: &Changes) -> Result<Value> {
    let _l = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut g = Gallery::load(&site.root)?.unwrap_or_else(|| Gallery { title: gallery::DEFAULT_TITLE.into(), intro: gallery::DEFAULT_INTRO.into(), ..Default::default() });
    let mut changed = 0usize;
    for c in &ch.changes {
        let keys: Vec<String> = match (&c.key, &c.project) {
            (Some(k), None) => {
                // Switching off a model that is gone from the repository is allowed.
                if c.on == Some(false) && c.viewer.is_none() && g.get(k).is_some() {
                    vec![k.clone()]
                } else {
                    vec![existing(repo, k)?]
                }
            }
            (None, Some(p)) => {
                if !gallery::safe_segment(p) {
                    bail!("bad project {p:?}");
                }
                if c.on != Some(false) && c.viewer.is_some() {
                    bail!("the viewer is switched per model, not per project");
                }
                let mut ks = repo_keys(repo, Some(p));
                if c.on == Some(false) {
                    ks.extend(g.models.iter().filter(|m| m.key.split('/').next() == Some(p.as_str())).map(|m| m.key.clone()));
                    ks.sort();
                    ks.dedup();
                }
                if ks.is_empty() {
                    bail!("no models in project {p}");
                }
                ks
            }
            _ => bail!("give either a key or a project"),
        };
        for k in keys {
            if let Some(on) = c.on {
                changed += g.set_on(&k, on)? as usize;
            }
            if let Some(v) = c.viewer {
                changed += g.set_viewer(&k, v)? as usize;
            }
            if let Some(f) = c.featured {
                if let Some(m) = g.models.iter_mut().find(|m| m.key == k) {
                    if m.featured != f {
                        m.featured = f;
                        changed += 1;
                    }
                }
            }
        }
    }
    if changed > 0 || !Gallery::path(&site.root).exists() {
        g.save(&site.root)?;
    }
    let r = gallery::sync(&site.static_dir(), repo, &g)?;
    let mut v = state(site)?;
    v["changed"] = json!(changed);
    v["sync"] = serde_json::to_value(&r)?;
    Ok(v)
}

/// The post that shows the gallery (`models::gallery[]` without a project): published ones first.
pub fn gallery_home(site: &Site) -> Result<Option<gw_core::content::Item>> {
    let mut found: Vec<gw_core::content::Item> = site
        .items()?
        .into_iter()
        .filter(|it| std::fs::read_to_string(&it.path).is_ok_and(|t| gallery::embeds(&t).iter().any(|e| matches!(e, Ok(e) if e.project.is_none()))))
        .collect();
    found.sort_by_key(|it| it.draft);
    Ok(found.into_iter().next())
}

// ------------------------------------------------------------------ models in a post

/// A post's 3-D models (`model::` lines), each with what the site knows of it.
pub fn post_models(site: &Site, id: &str) -> Result<Value> {
    let it = site.item(id)?;
    let text = std::fs::read_to_string(&it.path)?;
    let lib = site.media()?;
    let models: Vec<Value> = gw_core::model::embeds(&text)
        .into_iter()
        .map(|e| {
            let m = e.key.as_deref().and_then(|k| lib.resolve(k));
            let mut v = serde_json::to_value(&e).unwrap_or_default();
            v["found"] = json!(e.key.is_none() || m.is_some_and(|m| m.kind == "model"));
            if let Some(m) = m {
                v["model"] = json!(m.ids.get("model"));
                v["poster"] = json!(if m.poster.is_empty() { &m.thumb } else { &m.poster });
                v["default_caption"] = json!(m.caption);
                v["has_download"] = json!(!m.download.is_empty());
            }
            v
        })
        .collect();
    Ok(json!({ "id": it.id, "title": it.title, "url": it.url, "path": it.path.display().to_string(), "models": models,
        "galleries": gallery::embeds(&text).len() }))
}

/// One change to a post's models (the API body; the CLI and MCP build one).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PostChange {
    /// add | remove | update
    pub action: String,
    /// add: a media key (`wizard-lemming`, `media:wizard-lemming`) or a repository model
    /// (`lemmings/wizard-lemming`; its web model is made first when the site has none).
    /// remove/update: the same, or use `index`.
    pub model: Option<String>,
    /// remove/update: the model's position in the post (0-based, from `post_models`).
    pub index: Option<usize>,
    pub caption: Option<String>,
    /// true/false; for update, null removes `download=`.
    pub download: Option<Value>,
    /// add: after this 1-based line (default: at the end).
    pub after_line: Option<usize>,
}

/// The media key for `model`: a key in the library, or a repository model's web model (made
/// with Blender when missing, with the print STL as its download when `download`).
fn media_key(site: &Site, model: &str, download: bool) -> Result<String> {
    let model = model.trim().trim_start_matches("media:");
    let lib = site.media()?;
    if model.contains('/') {
        let key = existing(&repo()?, model)?;
        if let Some(k) = gallery::media_key_for(&lib, &key) {
            return Ok(k);
        }
        let o = super::model_assets::AddModel { model: key, download: if download { "stl".into() } else { "none".into() }, ..Default::default() };
        let r = super::model_assets::add_model(site, &o)?;
        return r["key"].as_str().map(str::to_string).context("add-model returned no key");
    }
    match lib.resolve(model) {
        Some(m) if m.kind == "model" => Ok(model.to_string()),
        Some(m) => bail!("media:{model} is not a 3-D model (kind: {})", m.kind),
        None => bail!("no media:{model}: give a 3-D model's media key, or <project>/<slug> from the models repository"),
    }
}

/// The post's model at `index`, or the first showing `model` (media key or repository model).
fn find_index(site: &Site, text: &str, ch: &PostChange) -> Result<usize> {
    if let Some(i) = ch.index {
        return Ok(i);
    }
    let m = ch.model.as_deref().context("give index or model")?.trim().trim_start_matches("media:").to_string();
    let lib = site.media()?;
    gw_core::model::embeds(text)
        .into_iter()
        .find(|e| e.key.as_deref() == Some(m.as_str()) || e.target == m || e.key.as_deref().and_then(|k| lib.resolve(k)).and_then(|x| x.ids.get("model")) == Some(&m))
        .map(|e| e.index)
        .with_context(|| format!("the post does not show {m}"))
}

/// Add, remove or change a model in a post (writes the post's file).
pub fn post_change(site: &Site, id: &str, ch: &PostChange) -> Result<Value> {
    let it = site.item(id)?;
    let text = std::fs::read_to_string(&it.path)?;
    let download = match &ch.download {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::Bool(b)) => Some(Some(*b)),
        Some(v) => bail!("download is true, false or null, not {v}"),
    };
    let (new, did) = match ch.action.as_str() {
        "add" => {
            let model = ch.model.as_deref().context("add: give model (a media key or <project>/<slug>)")?;
            let key = media_key(site, model, download == Some(Some(true)))?;
            let line = gw_core::model::macro_line(&key, ch.caption.as_deref().unwrap_or(""), download.flatten());
            let place = ch.after_line.map(gw_core::model::Place::After).unwrap_or(gw_core::model::Place::End);
            (gw_core::model::insert(&text, &line, place), json!({ "added": line }))
        }
        "remove" => {
            let i = find_index(site, &text, ch)?;
            (gw_core::model::remove(&text, i)?, json!({ "removed": i }))
        }
        "update" => {
            let i = find_index(site, &text, ch)?;
            (gw_core::model::update(&text, i, ch.caption.as_deref(), download)?, json!({ "updated": i }))
        }
        a => bail!("action {a:?}: use add, remove or update"),
    };
    crate::ops::write_source(site, &it.id, &new)?;
    let mut v = post_models(site, &it.id)?;
    v["change"] = did;
    Ok(v)
}

/// `gw models sync-site`: copy the selected models' renders into the site, remove the others.
pub fn sync_site(site: &Site, repo: &Path) -> Result<Value> {
    let _l = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let g = Gallery::load(&site.root)?.with_context(|| format!("no {} in the site: switch models on with `gw models on <project>/<slug>` or the app's Models page", gallery::FILE))?;
    Ok(serde_json::to_value(gallery::sync(&site.static_dir(), repo, &g)?)?)
}

// ------------------------------------------------------------------ CLI

#[derive(clap::Subcommand, Debug)]
pub enum ModelsCmd {
    /// The models on the website (models.yaml) and whether their pictures are in the site.
    List,
    /// Put models on the website: <project>/<slug>…, or --project <p> for all of a project, or --all.
    On {
        keys: Vec<String>,
        #[arg(long)]
        project: Option<String>,
        #[arg(long, conflicts_with = "project")]
        all: bool,
        /// Also show the interactive 3-D viewer (needs `gw media add-model <key> --download none`).
        #[arg(long)]
        viewer: bool,
    },
    /// Take models off the website.
    Off {
        keys: Vec<String>,
        #[arg(long)]
        project: Option<String>,
    },
    /// Turn a model's interactive 3-D viewer on or off.
    Viewer {
        key: String,
        #[arg(long, conflicts_with = "off")]
        on: bool,
        #[arg(long)]
        off: bool,
    },
    /// Copy the selected models' renders and a trimmed meta.json from the models repository into
    /// static/model-gallery/ (and remove models no longer selected). The build reads only those.
    SyncSite,
    /// The 3-D models a post shows (its model:: lines).
    In {
        /// The post: id, slug or URL.
        post: String,
    },
    /// Show a model in a post: gw models add lemmings-project wizard-lemming --caption "The wizard".
    /// MODEL is a media key or <project>/<slug> (its web model is made with Blender if needed).
    Add {
        post: String,
        model: String,
        #[arg(long)]
        caption: Option<String>,
        /// Offer readers the print file (--download true|false; default: the media entry's).
        #[arg(long)]
        download: Option<bool>,
        /// Put it after this line of the file (default: at the end).
        #[arg(long)]
        after_line: Option<usize>,
    },
    /// Take a model out of a post: MODEL is a media key, <project>/<slug>, or #n from `gw models in`.
    Remove { post: String, model: String },
}

pub fn run_cli(site: &Site, c: ModelsCmd) -> Result<Value> {
    let change = |key: Option<String>, project: Option<String>, on: Option<bool>, viewer: Option<bool>| Change { key, project, on, viewer, featured: None };
    match c {
        ModelsCmd::List => state(site),
        ModelsCmd::In { post } => post_models(site, &post),
        ModelsCmd::Add { post, model, caption, download, after_line } => {
            post_change(site, &post, &PostChange { action: "add".into(), model: Some(model), caption, download: download.map(Value::Bool), after_line, ..Default::default() })
        }
        ModelsCmd::Remove { post, model } => {
            let ch = match model.strip_prefix('#').and_then(|n| n.parse::<usize>().ok()) {
                Some(n) if n >= 1 => PostChange { action: "remove".into(), index: Some(n - 1), ..Default::default() },
                _ => PostChange { action: "remove".into(), model: Some(model), ..Default::default() },
            };
            post_change(site, &post, &ch)
        }
        ModelsCmd::SyncSite => sync_site(site, &repo()?),
        ModelsCmd::On { keys, project, all, viewer } => {
            let repo = repo()?;
            let mut changes: Vec<Change> = keys.into_iter().map(|k| change(Some(k), None, Some(true), viewer.then_some(true))).collect();
            if let Some(p) = project {
                changes.push(change(None, Some(p), Some(true), None));
            }
            if all {
                changes.extend(repo_keys(&repo, None).into_iter().map(|k| change(Some(k), None, Some(true), None)));
            }
            if changes.is_empty() {
                bail!("name models (<project>/<slug>), --project <p> or --all");
            }
            apply(site, &repo, &Changes { changes })
        }
        ModelsCmd::Off { keys, project } => {
            let mut changes: Vec<Change> = keys.into_iter().map(|k| change(Some(k), None, Some(false), None)).collect();
            if let Some(p) = project {
                changes.push(change(None, Some(p), Some(false), None));
            }
            if changes.is_empty() {
                bail!("name models (<project>/<slug>) or --project <p>");
            }
            apply(site, &repo()?, &Changes { changes })
        }
        ModelsCmd::Viewer { key, on, off } => {
            if on == off {
                bail!("--on or --off");
            }
            apply(site, &repo()?, &Changes { changes: vec![change(Some(key), None, None, Some(on))] })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn fixture(name: &str) -> (Site, PathBuf) {
        let base = std::env::temp_dir().join(format!("gw-site-models-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("site");
        std::fs::create_dir_all(root.join("content/pages")).unwrap();
        std::fs::write(root.join("site.yaml"), "site:\n  title: T\n  base_url: https://example.org\n  author: A\n").unwrap();
        let repo = base.join("repo");
        for (p, s) in [("lemmings", "wiz"), ("lemmings", "gino"), ("ai-force", "teen")] {
            let d = repo.join(p).join(s);
            std::fs::create_dir_all(d.join("images")).unwrap();
            std::fs::write(d.join("meshy.json"), format!(r#"{{"name":"{s}","slug":"{s}","project":"{p}","print":{{"dimensions_mm":[1.0,2.0,3.0]}}}}"#)).unwrap();
            for v in ["hero", "thumb"] {
                std::fs::write(d.join("images").join(format!("{v}.webp")), v).unwrap();
            }
        }
        (Site::open(Some(&root)).unwrap(), repo)
    }

    fn key(k: &str, on: bool) -> Change {
        Change { key: Some(k.into()), on: Some(on), ..Default::default() }
    }

    #[test]
    fn add_update_remove_models_in_a_post() {
        let (site, _) = fixture("post");
        std::fs::write(site.root.join("media.yaml"), "media:\n  wiz:\n    url: /models/wiz/wiz.glb\n    kind: model\n    provider: local\n    caption: The wizard\n    ids:\n      model: lemmings/wiz\n  photo:\n    url: /p.jpg\n").unwrap();
        std::fs::create_dir_all(site.root.join("content/articles")).unwrap();
        std::fs::write(site.root.join("content/articles/figs.adoc"), "= Figures\n:type: article\n:date: 2026-10-01\n\nSome text.\n").unwrap();
        let ch = |action: &str, model: Option<&str>| PostChange { action: action.into(), model: model.map(str::to_string), ..Default::default() };
        let v = post_change(&site, "figs", &PostChange { caption: Some("Him, again".into()), download: Some(json!(false)), ..ch("add", Some("media:wiz")) }).unwrap();
        assert_eq!(v["change"]["added"], "model::media:wiz[\"Him, again\", download=false]");
        assert_eq!(v["models"][0]["model"], "lemmings/wiz");
        assert_eq!(v["models"][0]["found"], true);
        let text = std::fs::read_to_string(site.root.join("content/articles/figs.adoc")).unwrap();
        assert!(text.ends_with("Some text.\n\nmodel::media:wiz[\"Him, again\", download=false]\n"), "{text}");
        // Update by repository key; download=null drops the attribute.
        let v = post_change(&site, "figs", &PostChange { caption: Some("Wiz".into()), download: Some(Value::Null), ..ch("update", Some("lemmings/wiz")) }).unwrap();
        assert_eq!(v["models"][0]["attrs"], "Wiz");
        assert!(post_change(&site, "figs", &ch("add", Some("photo"))).unwrap_err().to_string().contains("not a 3-D model"));
        assert!(post_change(&site, "figs", &ch("add", Some("nope"))).is_err());
        assert!(post_change(&site, "figs", &ch("remove", Some("other"))).is_err());
        let v = post_change(&site, "figs", &PostChange { index: Some(0), ..ch("remove", None) }).unwrap();
        assert_eq!(v["models"].as_array().unwrap().len(), 0);
        assert_eq!(std::fs::read_to_string(site.root.join("content/articles/figs.adoc")).unwrap(), "= Figures\n:type: article\n:date: 2026-10-01\n\nSome text.\n");
        // The gallery's home post.
        assert!(gallery_home(&site).unwrap().is_none());
        std::fs::write(site.root.join("content/articles/all.adoc"), "= All my models\n:type: article\n:date: 2026-10-01\n:status: published\n\nmodels::gallery[]\n").unwrap();
        assert_eq!(gallery_home(&site).unwrap().unwrap().title, "All my models");
        let st = state(&site).unwrap();
        assert_eq!(st["home"]["url"], "/articles/all/");
        assert_eq!(st["url"], "https://example.org/articles/all/");
    }

    #[test]
    fn toggles_write_models_yaml_and_sync() {
        let (site, repo) = fixture("toggle");
        let v = apply(&site, &repo, &Changes { changes: vec![key("lemmings/wiz", true)] }).unwrap();
        assert_eq!(v["count"], 1);
        assert!(site.root.join("static/model-gallery/lemmings/wiz/hero.webp").is_file());
        let g = Gallery::load(&site.root).unwrap().unwrap();
        assert_eq!(g.models[0].key, "lemmings/wiz");
        // A project at once, then off again.
        let v = apply(&site, &repo, &Changes { changes: vec![Change { project: Some("lemmings".into()), on: Some(true), ..Default::default() }] }).unwrap();
        assert_eq!(v["count"], 2);
        let v = apply(&site, &repo, &Changes { changes: vec![Change { project: Some("lemmings".into()), on: Some(false), ..Default::default() }] }).unwrap();
        assert_eq!(v["count"], 0);
        assert!(!site.root.join("static/model-gallery/lemmings").exists());
        // Viewer on switches the model on.
        let v = apply(&site, &repo, &Changes { changes: vec![Change { key: Some("ai-force/teen".into()), viewer: Some(true), ..Default::default() }] }).unwrap();
        assert_eq!(v["models"][0]["viewer"], true);
        assert_eq!(v["models"][0]["synced"], true);
        assert!(v["models"][0]["media_key"].is_null());
    }

    #[test]
    fn keys_must_exist_and_stay_inside() {
        let (site, repo) = fixture("safety");
        for bad in ["../repo/lemmings", "lemmings/../../etc", "lemmings/nope", "lemmings", "/etc/passwd", "lemmings/.git", "lemmings/wiz/images"] {
            let e = apply(&site, &repo, &Changes { changes: vec![key(bad, true)] });
            assert!(e.is_err(), "{bad} was accepted");
        }
        for bad in ["..", ".git", "a/b"] {
            assert!(apply(&site, &repo, &Changes { changes: vec![Change { project: Some(bad.into()), on: Some(true), ..Default::default() }] }).is_err(), "{bad}");
        }
        assert!(apply(&site, &repo, &Changes { changes: vec![Change { on: Some(true), ..Default::default() }] }).is_err());
        assert!(!site.root.join("static/model-gallery").exists() || walk_bytes(&site.root.join("static/model-gallery")) == 0);
    }
}
