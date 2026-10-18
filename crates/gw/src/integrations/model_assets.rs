//! `gw media add-model <project>/<slug>`: web assets for a Meshy model, made into the site.
//!
//! From the models repository's local-only full-resolution textured `<slug>.glb` (~850k
//! triangles, ~40–60 MB) Blender makes, in `static/models/<key>/`:
//!
//! - `<key>.glb` — decimated to `--polycount` (150k) triangles, textures at most 2048 px as
//!   JPEG, Draco-compressed (model-viewer fetches the Draco decoder itself), scaled to the
//!   printed size so AR shows it life size. A few MB.
//! - `<key>.usdz` — the same decimated mesh for AR Quick Look on iOS (checked with `usdcat`).
//! - `poster.webp` — the model's `images/hero.webp` (or its first image).
//! - the print file with `--download stl` (the committed `<slug>-print.stl` or plate STL) or
//!   `--download 3mf`; `--download-polycount N` decimates an STL for download.
//!
//! and registers `media:<key>` (kind `model`). Nothing in the models repository changes.

use super::meshy;
use anyhow::{bail, Context, Result};
use gw_core::media::Media;
use gw_core::Site;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

const SCRIPT: &str = include_str!("model_blender.py");
/// Print files above this are slow to download; warn and suggest --download-polycount.
pub const DOWNLOAD_WARN_BYTES: u64 = 25 * 1024 * 1024;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AddModel {
    /// `<project>/<slug>` in the models repository.
    pub model: String,
    /// Media key (default: the slug).
    pub key: Option<String>,
    /// stl | 3mf | none
    pub download: String,
    /// Triangles in the web model.
    pub polycount: u64,
    /// Decimate a downloaded STL to this many triangles.
    pub download_polycount: Option<u64>,
    /// Longest texture edge in the web model.
    pub max_texture: u32,
    pub draco: bool,
    /// Make no USDZ (no AR Quick Look on iOS).
    pub no_usdz: bool,
    /// Replace an existing key that holds a different model.
    pub force: bool,
}

impl Default for AddModel {
    fn default() -> Self {
        AddModel { model: String::new(), key: None, download: "none".into(), polycount: 150_000, download_polycount: None, max_texture: 2048, draco: true, no_usdz: false, force: false }
    }
}

/// Blender: `$GW_BLENDER`, the macOS app, or `blender` on the PATH.
pub fn blender() -> Result<PathBuf> {
    if let Some(b) = std::env::var_os("GW_BLENDER").filter(|b| !b.is_empty()) {
        return Ok(PathBuf::from(b));
    }
    let mac = PathBuf::from("/Applications/Blender.app/Contents/MacOS/Blender");
    if mac.is_file() {
        return Ok(mac);
    }
    if let Some(p) = std::env::var_os("PATH").and_then(|path| std::env::split_paths(&path).map(|d| d.join("blender")).find(|p| p.is_file())) {
        return Ok(p);
    }
    bail!("Blender is needed to make web models: install it (blender.org) or set GW_BLENDER")
}

fn run_blender(args: &Value) -> Result<Value> {
    let script = std::env::temp_dir().join(format!("gw-model-blender-{}.py", std::process::id()));
    std::fs::write(&script, SCRIPT)?;
    let out = Command::new(blender()?)
        .args(["-b", "--factory-startup", "-noaudio", "-P"])
        .arg(&script)
        .arg("--")
        .arg(args.to_string())
        .output()
        .context("running Blender")?;
    let _ = std::fs::remove_file(&script);
    let text = String::from_utf8_lossy(&out.stdout);
    if let Some(line) = text.lines().rev().find_map(|l| l.strip_prefix("GWRESULT ")) {
        return Ok(serde_json::from_str(line)?);
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let tail: Vec<&str> = text.lines().chain(err.lines()).filter(|l| l.contains("rror")).take(12).collect();
    bail!("Blender did not finish ({}):\n{}", out.status, tail.join("\n"))
}

fn size(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

/// `key` as a folder name under static/models: a media key that is not `.`/`..` or hidden.
pub fn valid_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 80 && !key.starts_with('.') && key.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
}

pub fn add_model(site: &Site, o: &AddModel) -> Result<Value> {
    let ctx = meshy::Ctx::load()?;
    let repo = ctx.repo.clone().context("no Meshy models repository: `gw meshy config --repo <path>`")?;
    add_model_from(site, &repo, o)
}

pub fn add_model_from(site: &Site, repo: &Path, o: &AddModel) -> Result<Value> {
    let (project, slug) = o.model.trim_matches('/').split_once('/').context("give the model as <project>/<slug> (see `gw meshy models`)")?;
    if !meshy::safe_segment(project) || !meshy::safe_segment(slug) {
        bail!("bad project or slug");
    }
    let key = o.key.clone().unwrap_or_else(|| slug.to_string());
    if !valid_key(&key) {
        bail!("media keys may use letters, digits, '-', '_', '.' (and may not start with '.')");
    }
    if !["stl", "3mf", "none"].contains(&o.download.as_str()) {
        bail!("--download is stl, 3mf or none");
    }
    if o.polycount < 1000 {
        bail!("--polycount must be at least 1000");
    }
    let dir = repo.join(project).join(slug);
    let meta: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("meshy.json")).with_context(|| format!("no model {project}/{slug}"))?)?;
    let glb = dir.join(format!("{slug}.glb"));
    if !glb.is_file() {
        bail!("{project}/{slug} has no {slug}.glb (it is local-only: sync the models repository on this Mac)");
    }
    let id = format!("{project}/{slug}");
    let mut lib = site.media()?;
    if let Some(old) = lib.resolve(&key) {
        if old.ids.get("model").map(String::as_str) != Some(id.as_str()) && !o.force {
            bail!("media:{key} already exists ({}); choose another --key or pass --force", if old.kind == "model" { old.ids.get("model").cloned().unwrap_or_default() } else { old.kind.clone() });
        }
    }
    let out_dir = site.static_dir().join("models").join(&key);
    std::fs::create_dir_all(&out_dir)?;
    let url = |f: &str| format!("/models/{key}/{f}");
    let mut warnings: Vec<String> = vec![];

    // Web GLB + USDZ.
    let print = &meta["print"];
    let dims: Vec<f64> = print["dimensions_mm"].as_array().into_iter().flatten().filter_map(|x| x.as_f64()).collect();
    let size_mm = print["size_mm"].as_f64().or_else(|| dims.iter().cloned().reduce(f64::max)).unwrap_or(0.0);
    let web_glb = out_dir.join(format!("{key}.glb"));
    let usdz = out_dir.join(format!("{key}.usdz"));
    let tmp = std::env::temp_dir().join(format!("gw-model-tex-{}-{key}", std::process::id()));
    let r = run_blender(&json!({
        "mode": "web", "src": glb, "glb": web_glb, "usdz": if o.no_usdz { Value::Null } else { json!(usdz) },
        "polycount": o.polycount, "max_texture": o.max_texture, "draco": o.draco, "size_m": size_mm / 1000.0,
        "tmp": tmp, "jpeg_quality": 85,
    }));
    let _ = std::fs::remove_dir_all(&tmp);
    let r = r?;
    let mut ios = String::new();
    if !o.no_usdz {
        let ok = Command::new("usdcat").arg("--loadOnly").arg(&usdz).output().map(|o| o.status.success());
        match ok {
            Ok(true) | Err(_) if usdz.is_file() => {
                if ok.is_err() {
                    warnings.push("usdcat not found; the USDZ was not checked".into());
                }
                ios = url(&format!("{key}.usdz"));
            }
            _ => {
                warnings.push("the USDZ did not load in usdcat; left out (no AR Quick Look on iOS)".into());
                let _ = std::fs::remove_file(&usdz);
            }
        }
    } else {
        let _ = std::fs::remove_file(&usdz);
    }

    // Poster.
    let poster_src = ["images/hero.webp", "images/front.webp", "images/preview.png"].iter().map(|p| dir.join(p)).find(|p| p.is_file());
    let poster = match poster_src {
        Some(p) => {
            let name = format!("poster.{}", p.extension().and_then(|e| e.to_str()).unwrap_or("webp"));
            std::fs::copy(&p, out_dir.join(&name))?;
            url(&name)
        }
        None => {
            warnings.push("no images/hero.webp; the model has no poster".into());
            String::new()
        }
    };

    // Download.
    let mut download = String::new();
    let mut download_info = Value::Null;
    if o.download != "none" {
        let mut cands: Vec<String> = print["print_file"].as_str().into_iter().map(str::to_string).collect();
        cands.extend(print["plate_files"].as_array().into_iter().flatten().filter_map(|f| f.as_str().map(str::to_string)));
        cands.push(format!("{slug}.{}", o.download));
        let src = cands.iter().filter(|f| f.to_ascii_lowercase().ends_with(&format!(".{}", o.download))).filter(|f| meshy::safe_segment(f)).map(|f| dir.join(f)).find(|p| p.is_file());
        let Some(src) = src else { bail!("{id} has no {} print file", o.download.to_uppercase()) };
        let name = src.file_name().unwrap().to_string_lossy().to_string();
        let dst = out_dir.join(&name);
        match o.download_polycount {
            Some(n) if o.download == "stl" => {
                let d = run_blender(&json!({ "mode": "stl", "src": src, "out": dst, "polycount": n }))?;
                if d["non_manifold_edges"].as_u64().unwrap_or(0) > d["non_manifold_before"].as_u64().unwrap_or(0) {
                    warnings.push(format!("the decimated STL has {} non-manifold edges (the original had {}); check it in a slicer", d["non_manifold_edges"], d["non_manifold_before"]));
                }
                download_info = json!({ "decimated": d });
            }
            Some(_) => bail!("--download-polycount works with --download stl"),
            None => {
                std::fs::copy(&src, &dst)?;
            }
        }
        let b = size(&dst);
        if b > DOWNLOAD_WARN_BYTES {
            warnings.push(format!("{name} is {} — large for readers to download; try --download-polycount 400000", gw_core::model::human_bytes(b)));
        }
        download = url(&name);
        if let Value::Object(m) = &mut download_info { m.insert("bytes".into(), json!(b)); } else { download_info = json!({ "bytes": b }); }
        download_info["file"] = json!(download);
    }
    // Remove files an earlier run left behind (another download choice).
    let keep: Vec<String> = [&web_glb, &usdz].iter().map(|p| p.file_name().unwrap().to_string_lossy().to_string())
        .chain([&poster, &download].iter().filter(|u| !u.is_empty()).map(|u| u.rsplit('/').next().unwrap().to_string()))
        .collect();
    for e in std::fs::read_dir(&out_dir)?.flatten() {
        let n = e.file_name().to_string_lossy().to_string();
        if !keep.contains(&n) && e.path().is_file() {
            let _ = std::fs::remove_file(e.path());
        }
    }

    let mut ids = BTreeMap::new();
    ids.insert("model".to_string(), id.clone());
    if let Some(t) = meta["meshy"]["id"].as_str() {
        ids.insert("meshy_id".to_string(), t.to_string());
    }
    if let Some(t) = r["triangles_before"].as_u64() {
        ids.insert("source_triangles".to_string(), t.to_string());
    }
    let name = meta["name"].as_str().unwrap_or(slug).trim_end_matches(" 3D").to_string();
    let parts = print["parts"].as_u64().unwrap_or(1);
    let m = Media {
        url: url(&format!("{key}.glb")),
        kind: "model".into(),
        provider: "local".into(),
        thumb: poster.clone(),
        alt: meta["summary"].as_str().map(|s| format!("3-D model: {s}")).unwrap_or_default(),
        caption: name,
        ids,
        ios,
        poster,
        download,
        download_bytes: download_info["bytes"].as_u64().unwrap_or(0),
        bytes: size(&web_glb),
        ios_bytes: if o.no_usdz { 0 } else { size(&usdz) },
        triangles: r["triangles"].as_u64().unwrap_or(0),
        dimensions_mm: dims,
        parts: if parts > 1 { parts } else { 0 },
    };
    lib = site.media()?;
    lib.media.insert(key.clone(), m.clone());
    lib.save(&site.media_path())?;
    Ok(json!({
        "key": key, "media": m, "dir": out_dir.display().to_string(),
        "files": {
            "glb": { "path": m.url, "bytes": m.bytes, "triangles": m.triangles, "textures": r["textures"] },
            "usdz": { "path": m.ios, "bytes": m.ios_bytes },
            "poster": { "path": m.poster, "bytes": size(&site.static_dir().join(m.poster.trim_start_matches('/'))) },
            "download": download_info,
        },
        "warnings": warnings,
        "use": format!("model::media:{key}[]"),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_options_are_checked_before_any_work() {
        assert!(valid_key("wizard-lemming") && valid_key("a.b_c"));
        assert!(!valid_key("") && !valid_key("..") && !valid_key(".hidden") && !valid_key("a/b") && !valid_key("a b"));
        let root = std::env::temp_dir().join(format!("gw-addmodel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("site/content")).unwrap();
        std::fs::write(root.join("site/site.yaml"), "site:\n  title: T\n  base_url: https://example.org\n  author: A\n").unwrap();
        let site = Site::open(Some(&root.join("site"))).unwrap();
        let repo = root.join("repo");
        std::fs::create_dir_all(repo.join("p/m")).unwrap();
        std::fs::write(repo.join("p/m/meshy.json"), "{\"name\": \"M\", \"print\": {}}").unwrap();
        let err = |o: AddModel| add_model_from(&site, &repo, &o).unwrap_err().to_string();
        assert!(err(AddModel { model: "../etc/m".into(), ..Default::default() }).contains("bad project"));
        assert!(err(AddModel { model: "p/..".into(), ..Default::default() }).contains("bad project"));
        assert!(err(AddModel { model: "nope".into(), ..Default::default() }).contains("<project>/<slug>"));
        assert!(err(AddModel { model: "p/m".into(), key: Some("../x".into()), ..Default::default() }).contains("media keys"));
        assert!(err(AddModel { model: "p/m".into(), download: "zip".into(), ..Default::default() }).contains("--download"));
        assert!(err(AddModel { model: "p/q".into(), ..Default::default() }).contains("no model p/q"));
        assert!(err(AddModel { model: "p/m".into(), ..Default::default() }).contains("local-only"));
        // An existing key holding something else is not overwritten.
        std::fs::write(repo.join("p/m/m.glb"), b"glTF").unwrap();
        std::fs::write(root.join("site/media.yaml"), "media:\n  m:\n    url: /a.jpg\n").unwrap();
        assert!(err(AddModel { model: "p/m".into(), ..Default::default() }).contains("already exists"));
        assert!(!root.join("site/static/models").exists(), "nothing written");
        std::fs::remove_dir_all(&root).ok();
    }
}
