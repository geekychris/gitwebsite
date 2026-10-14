//! Project stories: operations shared by the CLI (`gw story …`), the REST API
//! (`/api/stories`), the MCP server (`story_*` tools), and the app's Stories section.
//!
//! The story workspace itself (files, images, drafting) is [`gw_core::story`]; this layer adds
//! what needs the network — SmugMug lookups and fetching review thumbnails — and JSON output.

use crate::integrations::smugmug::{self, Auth};
use anyhow::{bail, Context, Result};
use gw_core::story::{self, parse_smugmug, ChangeNote, NewStory, Remote, SheetTile, SmugRef, Story, StoryImage, StoryUpdate};
use gw_core::Site;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Where the app serves the draft preview.
pub const PREVIEW_BASE: &str = "http://127.0.0.1:8790/preview";
/// The app's story page (the gitwebsite app, or any browser while it runs).
pub const APP_BASE: &str = "http://127.0.0.1:8790/app/#story/";

static ACTOR: OnceLock<String> = OnceLock::new();

/// Say who this process's changes come from in the change feed: `cli`, `app` (the server), or
/// `mcp`. `GW_ACTOR` overrides it (e.g. `GW_ACTOR=claude`).
pub fn set_actor(a: &str) {
    let _ = ACTOR.set(std::env::var("GW_ACTOR").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| a.to_string()));
}

pub fn actor() -> String {
    ACTOR.get().cloned().unwrap_or_else(|| std::env::var("GW_ACTOR").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| "cli".into()))
}

/// Change a story under its lock and record the change (see [`Story::modify`]).
fn modify<T>(site: &Site, slug: &str, f: impl FnOnce(&mut Story) -> Result<(T, ChangeNote)>) -> Result<(T, Story)> {
    Story::modify(site, slug, &actor(), f)
}

/// "#3, #7-9" for people.
fn numbers(s: &Story, ids: &[String]) -> String {
    let mut ns: Vec<u32> = ids.iter().filter_map(|id| s.image(id).ok().map(|i| i.number)).collect();
    ns.sort();
    let mut out: Vec<String> = vec![];
    let mut i = 0;
    while i < ns.len() {
        let mut j = i;
        while j + 1 < ns.len() && ns[j + 1] == ns[j] + 1 {
            j += 1;
        }
        out.push(if j > i + 1 { format!("#{}-{}", ns[i], ns[j]) } else if j == i + 1 { format!("#{}, #{}", ns[i], ns[j]) } else { format!("#{}", ns[i]) });
        i = j + 1;
    }
    out.join(", ")
}

/// Absolute path for display (Claude opens these with its Read tool).
fn abs(p: &Path) -> String {
    std::fs::canonicalize(p).unwrap_or_else(|_| std::env::current_dir().map(|d| d.join(p)).unwrap_or_else(|_| p.to_path_buf())).display().to_string()
}

fn image_json(site: &Site, s: &Story, im: &StoryImage) -> Value {
    let mut v = json!(im);
    v["needs_comment"] = json!(im.needs_comment());
    v["review_path"] = json!(s.review_path(site, im).map(|p| abs(&p)));
    v["original_path"] = json!(s.local_path(site, im).map(|p| abs(&p)));
    // What the app shows: the hosted thumbnail, or the API's thumbnail endpoint.
    v["thumb_url"] = json!(match &im.remote {
        Some(r) if !r.thumb.is_empty() => r.thumb.clone(),
        _ => format!("/api/stories/{}/images/{}/thumb", s.slug, im.id),
    });
    v
}

pub fn story_json(site: &Site, s: &Story) -> Value {
    let mut v = json!(s);
    v["dir"] = json!(abs(&s.dir(site)));
    v["images"] = json!(s.images.iter().map(|im| image_json(site, s, im)).collect::<Vec<_>>());
    v["interview"] = json!(s.interview(site));
    v["outline"] = json!(s.outline(site));
    v["cache_dir"] = json!(story::cache_dir(&s.slug).display().to_string());
    if let Some(d) = &s.draft {
        let p = site.root.join(d);
        if let Ok(rel) = p.strip_prefix(site.content_dir()) {
            if let Ok(it) = gw_core::content::load_item(&p, rel) {
                v["draft_item"] = json!({
                    "id": it.id, "url": it.url, "title": it.title, "draft": it.draft,
                    "preview_url": format!("{PREVIEW_BASE}{}", it.url),
                });
            }
        }
    }
    v["interview_command"] = json!(format!("/project-story {}", s.slug));
    v["app_url"] = json!(format!("{APP_BASE}{}", s.slug));
    let count = |st: &str| s.images.iter().filter(|i| i.state == st).count();
    v["counts"] = json!({ "candidates": count("candidate"), "selected": count("selected"), "rejected": count("rejected"),
        "needs_comment": s.needing_comments().len() });
    v["needs_comment"] = json!(s.needing_comments().iter().map(|i| json!({ "number": i.number, "id": i.id, "filename": i.filename })).collect::<Vec<_>>());
    v["selection_complete"] = json!(count("selected") > 0 && s.needing_comments().is_empty());
    crate::enhance::extend_json(site, s, &mut v);
    v
}

pub fn list(site: &Site) -> Result<Value> {
    Ok(json!(Story::list(site)?
        .iter()
        .map(|s| json!({
            "slug": s.slug, "title": s.title, "status": s.status, "target": s.target,
            "images": s.images.iter().filter(|i| i.is_selected()).count(),
            "candidates": s.images.iter().filter(|i| i.state == "candidate").count(),
            "pending": s.pending.len(), "rev": s.rev,
            "updated": s.updated.to_string(), "draft": s.draft, "enhances": s.enhances, "has_proposal": s.proposal.is_some(),
            "thumbs": s.images.iter().filter(|i| i.include && i.is_selected()).take(6).map(|im| image_json(site, s, im)["thumb_url"].clone()).collect::<Vec<_>>(),
        }))
        .collect::<Vec<_>>()))
}

pub fn show(site: &Site, slug: &str) -> Result<Value> {
    Ok(story_json(site, &Story::load(site, slug)?))
}

/// The story, or `{unchanged: true}` if it is still at revision `since` (cheap polling).
pub fn show_since(site: &Site, slug: &str, since: Option<u64>) -> Result<Value> {
    if let (Some(since), Some(rev)) = (since, Story::current_rev(site, slug)) {
        if rev == since {
            return Ok(json!({ "unchanged": true, "rev": rev }));
        }
    }
    show(site, slug)
}

pub fn create(site: &Site, n: &NewStory) -> Result<Value> {
    let s = Story::create(site, n)?;
    s.record_created(site, &actor())?;
    Ok(story_json(site, &s))
}

pub fn update(site: &Site, slug: &str, u: &StoryUpdate) -> Result<Value> {
    let (_, s) = modify(site, slug, |s| {
        s.update(site, u)?;
        Ok(((), ChangeNote::new("settings", "changed the story's settings")))
    })?;
    Ok(story_json(site, &s))
}

/// Delete a story workspace (its originals and transcript). Drafted posts are left alone.
pub fn delete(site: &Site, slug: &str) -> Result<Value> {
    let s = Story::load(site, slug)?;
    std::fs::remove_dir_all(s.dir(site))?;
    let _ = std::fs::remove_dir_all(story::cache_dir(slug));
    Ok(json!({ "deleted": slug, "draft_kept": s.draft }))
}

// ------------------------------------------------------------------ adding images

#[derive(Debug, Default, Deserialize)]
pub struct AddArgs {
    /// Files, http(s) URLs, SmugMug page URLs / `smugmug:<ImageKey>` / `album:<AlbumKey>`,
    /// or `media:<key>`.
    pub sources: Vec<String>,
    #[serde(default)]
    pub caption: Option<String>,
    /// What's happening here / why it matters. Required for SmugMug images unless they are
    /// added as candidates.
    #[serde(default)]
    pub comment: Option<String>,
    /// Offer these as candidates to choose from, rather than selecting them now.
    #[serde(default)]
    pub candidate: bool,
}

fn http() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder().user_agent("gitwebsite/0.1").timeout(std::time::Duration::from_secs(120)).build()?)
}

/// Download a hosted image's thumbnail into the review cache (best effort).
fn fetch_thumb(slug: &str, id: &str, url: &str) -> Result<std::path::PathBuf> {
    let dst = story::thumb_path(slug, id);
    if dst.exists() {
        return Ok(dst);
    }
    let r = http()?.get(url).send()?;
    if !r.status().is_success() {
        bail!("{url}: HTTP {}", r.status());
    }
    let bytes = r.bytes()?;
    story::write_thumb_bytes(&bytes, &dst)?;
    Ok(dst)
}

/// A SmugMug image as a story image (looks up its served URLs, size, and caption).
fn smugmug_image(auth: &Auth, key: &str, album_key: Option<&str>, caption: Option<&str>, comment: &str) -> Result<StoryImage> {
    let (m, img) = smugmug::media_and_image(auth, key, album_key, None, None)?;
    let s = |k: &str| img.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let n = |k: &str| img.get(k).and_then(|x| x.as_u64()).map(|x| x as u32);
    let album = album_key.map(|a| a.to_string()).or_else(|| {
        img.pointer("/Uris/Album/Uri").and_then(|x| x.as_str()).and_then(|u| u.rsplit('/').next()).map(|s| s.to_string())
    }).unwrap_or_default();
    Ok(StoryImage {
        source: format!("smugmug:{key}"),
        caption: caption.map(|c| c.to_string()).filter(|c| !c.is_empty()).unwrap_or_else(|| s("Caption")),
        notes: comment.to_string(),
        taken: img.get("DateTimeOriginal").or_else(|| img.get("Date")).and_then(|x| x.as_str()).map(|d| d.replace('T', " ").chars().take(19).collect()),
        width: n("OriginalWidth"),
        height: n("OriginalHeight"),
        include: true,
        filename: s("FileName"),
        remote: Some(Remote {
            url: m.url.clone(),
            thumb: if m.thumb.is_empty() { smugmug::sized(&s("ThumbnailUrl"), "M") } else { m.thumb.clone() },
            web: m.ids.get("web_uri").cloned().unwrap_or_default(),
            album_key: album,
            kind: m.kind.clone(),
        }),
        ..Default::default()
    })
}

/// Look up several SmugMug images at once (a few requests each, so in parallel).
fn smugmug_images(auth: &Auth, picks: &[(String, Option<String>, Option<String>, String)]) -> Vec<Result<StoryImage>> {
    let mut out: Vec<Option<Result<StoryImage>>> = (0..picks.len()).map(|_| None).collect();
    for chunk in (0..picks.len()).collect::<Vec<_>>().chunks(6) {
        std::thread::scope(|sc| {
            let hs: Vec<_> = chunk
                .iter()
                .map(|&i| {
                    let (k, a, c, n) = &picks[i];
                    (i, sc.spawn(move || smugmug_image(auth, k, a.as_deref(), c.as_deref(), n)))
                })
                .collect();
            for (i, h) in hs {
                out[i] = Some(h.join().unwrap_or_else(|_| Err(anyhow::anyhow!("lookup failed"))));
            }
        });
    }
    out.into_iter().map(|r| r.unwrap()).collect()
}

/// Add images from files, URLs, SmugMug, or the media library. SmugMug images need a comment
/// unless they are added as candidates (to choose from later).
pub fn add(site: &Site, slug: &str, a: &AddArgs) -> Result<Value> {
    let snapshot = Story::load(site, slug)?;
    let caption = a.caption.clone().unwrap_or_default();
    let comment = a.comment.clone().unwrap_or_default();
    let state = if a.candidate { "candidate" } else { "selected" };
    let mut skipped = vec![];
    let mut auth: Option<Auth> = None;
    // (ImageKey, AlbumKey, caption, comment, origin page)
    let mut smug: Vec<(String, Option<String>, Option<String>, String, String)> = vec![];
    let mut hosted: Vec<StoryImage> = vec![];
    let mut files: Vec<PathBuf> = vec![];
    for src in &a.sources {
        let src = src.trim();
        if src.is_empty() {
            continue;
        }
        if let Some(r) = parse_smugmug(src) {
            if comment.trim().is_empty() && !a.candidate {
                bail!("{src}: SmugMug images need a comment (--comment \"what's happening here / why it matters\"), or add them as candidates to choose from (--candidate)");
            }
            if auth.is_none() {
                auth = Some(Auth::load()?);
            }
            let au = auth.as_ref().unwrap();
            match r {
                SmugRef::Image(k) => smug.push((k, None, a.caption.clone(), comment.clone(), src.to_string())),
                other => {
                    let target = match other { SmugRef::Album(k) => format!("album:{k}"), SmugRef::Page(u) => u, SmugRef::Image(_) => unreachable!() };
                    let mut start = 1;
                    loop {
                        let page = smugmug::browse(au, &target, start, 100)?;
                        if page["kind"] == "folder" {
                            bail!("{src} is a folder; add one of its albums (gw story smugmug-browse {src})");
                        }
                        for im in page["images"].as_array().cloned().unwrap_or_default() {
                            let k = im["image_key"].as_str().unwrap_or("").to_string();
                            let ak = im["album_key"].as_str().map(|s| s.to_string()).filter(|s| !s.is_empty());
                            if !k.is_empty() {
                                smug.push((k, ak, a.caption.clone(), comment.clone(), src.to_string()));
                            }
                        }
                        match page["next_start"].as_u64() { Some(n) => start = n as usize, None => break }
                    }
                }
            }
        } else if let Some(k) = src.strip_prefix("media:") {
            let lib = site.media()?;
            let m = lib.resolve(k).with_context(|| format!("media key {k} is not in the library"))?;
            hosted.push(StoryImage {
                source: src.to_string(),
                caption: if caption.is_empty() { m.caption.clone() } else { caption.clone() },
                notes: comment.clone(),
                include: true,
                state: state.into(),
                remote: Some(Remote { url: m.url.clone(), thumb: if m.thumb.is_empty() { m.url.clone() } else { m.thumb.clone() }, kind: m.kind.clone(), ..Default::default() }),
                ..Default::default()
            });
        } else if src.starts_with("http://") || src.starts_with("https://") {
            hosted.push(StoryImage {
                source: format!("url:{src}"),
                caption: caption.clone(),
                notes: comment.clone(),
                include: true,
                state: state.into(),
                filename: src.rsplit('/').next().unwrap_or("").split('?').next().unwrap_or("").to_string(),
                remote: Some(Remote { url: src.to_string(), thumb: src.to_string(), kind: "image".into(), ..Default::default() }),
                ..Default::default()
            });
        } else {
            let p = gw_core::config::expand_home(src);
            if p.is_dir() {
                bail!("{src} is a folder; use `gw story add-dir {slug} {src}` to offer its photos as candidates");
            }
            if !p.is_file() {
                bail!("{src}: no such file");
            }
            files.push(std::fs::canonicalize(&p).unwrap_or(p));
        }
    }
    // SmugMug lookups happen before taking the story's lock (they are network calls).
    if !smug.is_empty() {
        let au = auth.as_ref().unwrap();
        let mut seen = std::collections::HashSet::new();
        let fresh: Vec<_> = smug.into_iter().filter(|(k, ..)| {
            let dup = snapshot.find_source(&format!("smugmug:{k}")).is_some() || !seen.insert(k.clone());
            if dup {
                skipped.push(json!({ "source": format!("smugmug:{k}"), "reason": "already in the story" }));
            }
            !dup
        }).collect();
        let picks: Vec<_> = fresh.iter().map(|(k, ak, c, n, _)| (k.clone(), ak.clone(), c.clone(), n.clone())).collect();
        for (r, (k, _, _, _, origin)) in smugmug_images(au, &picks).into_iter().zip(&fresh) {
            match r {
                Ok(mut im) => {
                    im.state = state.into();
                    im.origin = origin.clone();
                    hosted.push(im);
                }
                Err(e) => skipped.push(json!({ "source": format!("smugmug:{k}"), "reason": format!("{e:#}") })),
            }
        }
    }
    let ((added, more_skipped), s) = modify(site, slug, |s| {
        let mut added = vec![];
        let mut sk = vec![];
        for im in hosted {
            if s.find_source(&im.source).is_some() {
                sk.push(json!({ "source": im.source, "reason": "already in the story" }));
            } else {
                added.push(s.add_remote(im));
            }
        }
        for p in &files {
            if a.candidate {
                let taken = story::read_info(p).ok().and_then(|i| i.taken);
                match s.add_path_candidate(p, taken) {
                    Some(im) => added.push(im),
                    None => sk.push(json!({ "source": p.display().to_string(), "reason": "already in the story" })),
                }
            } else {
                added.push(s.add_file(site, p, &caption, &comment)?);
            }
        }
        let ids: Vec<String> = added.iter().map(|i: &StoryImage| i.id.clone()).collect();
        let what = format!("added {} {} {}", ids.len(), if a.candidate { "candidate" } else { "photo" } .to_string() + if ids.len() == 1 { "" } else { "s" }, numbers(s, &ids));
        Ok(((added, sk), ChangeNote::new(if a.candidate { "candidates" } else { "images" }, what).ids(ids)))
    })?;
    skipped.extend(more_skipped);
    // Review thumbnails (best effort; `review-images` / contact sheets retry).
    for im in &added {
        match &im.remote {
            Some(r) => {
                let _ = fetch_thumb(&s.slug, &im.id, &r.thumb);
            }
            None => {
                let _ = s.thumb_local(site, im);
            }
        }
    }
    Ok(json!({
        "added": added.iter().map(|im| image_json(site, &s, im)).collect::<Vec<_>>(),
        "skipped": skipped,
        "story": s.slug,
        "rev": s.rev,
    }))
}

#[derive(Debug, Default, Deserialize)]
pub struct AddDirArgs {
    pub dirs: Vec<String>,
    #[serde(default)]
    pub recursive: bool,
    /// Only photos taken on or after this date (YYYY-MM-DD).
    #[serde(default)]
    pub since: Option<String>,
    /// At most this many photos per folder (oldest first).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Offer the photos in folders as candidates. They stay where they are until selected; videos
/// are reported but not added.
pub fn add_dir(site: &Site, slug: &str, a: &AddDirArgs) -> Result<Value> {
    Story::load(site, slug)?;
    if a.dirs.is_empty() {
        bail!("give at least one folder");
    }
    let since = match a.since.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(d) => Some(chrono::NaiveDate::parse_from_str(d.trim(), "%Y-%m-%d").with_context(|| format!("--since {d}: use YYYY-MM-DD"))?),
        None => None,
    };
    let mut found = vec![];
    let mut videos = vec![];
    let mut filtered = 0;
    let mut folders = vec![];
    for d in &a.dirs {
        let p = gw_core::config::expand_home(d.trim());
        let p = std::fs::canonicalize(&p).with_context(|| format!("{d}: no such folder"))?;
        let f = story::scan_dir(&p, a.recursive, since, a.limit)?;
        folders.push(json!({ "folder": p.display().to_string(), "photos": f.photos.len(), "videos": f.videos.len(), "filtered": f.filtered }));
        filtered += f.filtered;
        videos.extend(f.videos.iter().map(|v| v.display().to_string()));
        found.extend(f.photos);
    }
    let ((added, already), s) = modify(site, slug, |s| {
        let mut added = vec![];
        let mut already = 0;
        for (p, taken) in found {
            match s.add_path_candidate(&p, taken) {
                Some(im) => added.push(im),
                None => already += 1,
            }
        }
        let ids: Vec<String> = added.iter().map(|i: &StoryImage| i.id.clone()).collect();
        let what = format!("found {} candidate photo{} in {} {}", ids.len(), if ids.len() == 1 { "" } else { "s" }, a.dirs.join(", "), numbers(s, &ids));
        Ok(((added, already), ChangeNote::new("candidates", what).ids(ids)))
    })?;
    let nums: Vec<u32> = added.iter().map(|i| i.number).collect();
    Ok(json!({
        "story": s.slug, "rev": s.rev,
        "added": added.len(),
        "numbers": numbers(&s, &added.iter().map(|i| i.id.clone()).collect::<Vec<_>>()),
        "first": nums.first(), "last": nums.last(),
        "already_in_story": already,
        "filtered_out": filtered,
        "videos_not_added": videos,
        "folders": folders,
        "next": format!("gw story contact-sheet {} --candidates", s.slug),
    }))
}

#[derive(Debug, Default, Deserialize)]
pub struct AddPhotosArgs {
    /// An album id from `photos_albums`; without one, the photos selected in Photos now.
    #[serde(default)]
    pub album: Option<String>,
    /// Add as candidates to choose from (the default for an album) rather than as chosen photos
    /// (the default for a selection).
    #[serde(default)]
    pub candidate: Option<bool>,
    /// At most this many photos from an album (default 300).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Bring photos in from Apple Photos (macOS): the photos selected in Photos, or an album.
/// Photos exports the current versions (edits applied) as JPEG. A selection is added as chosen
/// photos (each still needs a comment); an album's photos are offered as candidates, kept in
/// `~/.gitwebsite/cache/stories/<slug>/photos/<album>/` until chosen. Videos are skipped.
pub fn add_photos(site: &Site, slug: &str, a: &AddPhotosArgs) -> Result<Value> {
    use crate::integrations::photos::{self, Pick};
    Story::load(site, slug)?;
    let album = a.album.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let candidate = a.candidate.unwrap_or(album.is_some());
    let tmp = std::env::temp_dir().join(format!("gw-photos-{}-{}", std::process::id(), chrono::Local::now().timestamp_nanos_opt().unwrap_or(0)));
    let pick = match album {
        Some(id) => Pick::Album { id, limit: a.limit.unwrap_or(300) },
        None => Pick::Selection,
    };
    let exported = photos::export(&pick, &tmp);
    let result = (|| -> Result<Value> {
        let exported = exported?;
        let found = story::scan_dir(&tmp, false, None, None)?;
        let videos = found.videos.len();
        // Candidates must stay on disk until chosen: keep them in a folder per album (or per
        // import of a selection), and reuse a file of the same name so a re-import doesn't repeat.
        let keep = story::cache_dir(slug).join("photos").join(match album {
            Some(id) => id.split('/').next().unwrap_or(id).to_string(),
            None => format!("selection-{}", chrono::Local::now().format("%Y%m%d-%H%M%S")),
        });
        let mut items = vec![];
        for (p, taken) in found.photos {
            if candidate {
                std::fs::create_dir_all(&keep)?;
                let dest = keep.join(p.file_name().context("no file name")?);
                if !dest.exists() {
                    std::fs::rename(&p, &dest).or_else(|_| std::fs::copy(&p, &dest).map(|_| ()))?;
                }
                items.push((dest, taken));
            } else {
                items.push((p, taken));
            }
        }
        let ((added, already), s) = modify(site, slug, |s| {
            let mut added = vec![];
            let mut already = 0;
            for (p, taken) in &items {
                if candidate {
                    match s.add_path_candidate(p, taken.clone()) {
                        Some(im) => added.push(im),
                        None => already += 1,
                    }
                } else {
                    // The same Photos picture imported again: same file name and capture time.
                    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    if s.images.iter().any(|i| i.filename == name && i.taken.is_some() && &i.taken == taken) {
                        already += 1;
                        continue;
                    }
                    let mut im = s.add_file(site, p, "", "")?;
                    if im.taken.is_none() && taken.is_some() {
                        im.taken = taken.clone();
                        if let Ok(x) = s.image_mut(&im.id) { x.taken = taken.clone(); }
                    }
                    added.push(im);
                }
            }
            let ids: Vec<String> = added.iter().map(|i: &StoryImage| i.id.clone()).collect();
            let from = if album.is_some() { "an Apple Photos album" } else { "the Apple Photos selection" };
            let what = format!("added {} {} from {from} {}", ids.len(), if candidate { "candidate" } else { "photo" }.to_string() + if ids.len() == 1 { "" } else { "s" }, numbers(s, &ids));
            Ok(((added, already), ChangeNote::new(if candidate { "candidates" } else { "images" }, what).ids(ids)))
        })?;
        for im in &added {
            let _ = s.thumb_local(site, im);
        }
        Ok(json!({
            "story": s.slug, "rev": s.rev,
            "exported": exported,
            "added": added.len(),
            "as": if candidate { "candidates" } else { "selected" },
            "numbers": numbers(&s, &added.iter().map(|i| i.id.clone()).collect::<Vec<_>>()),
            "already_in_story": already,
            "videos_skipped": videos,
            "next": if candidate { format!("gw story contact-sheet {} --candidates", s.slug) } else { format!("Each chosen photo needs a comment: gw story comment {} <number> \"…\"", s.slug) },
        }))
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

/// The albums in Apple Photos (macOS), for choosing one to import.
pub fn photos_albums() -> Result<Value> {
    Ok(json!({ "albums": crate::integrations::photos::albums()? }))
}

/// Add one uploaded file (from the app).
pub fn upload(site: &Site, slug: &str, name: &str, bytes: &[u8], caption: &str, comment: &str) -> Result<Value> {
    let (im, s) = modify(site, slug, |s| {
        let im = s.add_bytes(site, name, bytes, caption, comment)?;
        let what = format!("added #{} ({name})", im.number);
        let id = im.id.clone();
        Ok((im, ChangeNote::new("images", what).ids(vec![id])))
    })?;
    let _ = s.thumb_local(site, &im);
    Ok(image_json(site, &s, &im))
}

#[derive(Debug, Default, Deserialize)]
pub struct ImageUpdate {
    pub caption: Option<String>,
    pub notes: Option<String>,
    pub include: Option<bool>,
}

/// Change one image's caption, comment, or include flag. `id` may be `img3` or a number (`#3`).
pub fn image_update(site: &Site, slug: &str, id: &str, u: &ImageUpdate) -> Result<Value> {
    let (id, s) = modify(site, slug, |s| {
        let id = s.resolve(id)?;
        let im = s.image_mut(&id)?;
        let mut what = vec![];
        if let Some(c) = &u.caption {
            im.caption = c.clone();
            what.push("caption");
        }
        if let Some(n) = &u.notes {
            if n.trim().is_empty() && !im.notes.trim().is_empty() && (im.is_selected() || im.source.starts_with("smugmug:")) {
                bail!("selected photos keep a comment (SmugMug ones too); change it, or unselect or remove the photo instead");
            }
            im.notes = n.clone();
            what.push("comment");
        }
        if let Some(i) = u.include {
            im.include = i;
            what.push(if i { "included" } else { "left out" });
        }
        let kind = if what == ["caption"] { "caption" } else if what.contains(&"comment") { "comment" } else { "images" };
        let note = ChangeNote::new(kind, format!("#{}: {}", im.number, what.join(", "))).ids(vec![id.clone()]);
        Ok((id, note))
    })?;
    let im = s.image(&id)?.clone();
    Ok(image_json(site, &s, &im))
}

pub fn image_remove(site: &Site, slug: &str, id: &str) -> Result<Value> {
    let (id, _) = modify(site, slug, |s| {
        let id = s.resolve(id)?;
        let n = s.image(&id)?.number;
        s.remove_image(site, &id)?;
        Ok((id.clone(), ChangeNote::new("images", format!("removed #{n}")).ids(vec![id])))
    })?;
    Ok(json!({ "removed": id }))
}

pub fn reorder(site: &Site, slug: &str, ids: &[String]) -> Result<Value> {
    let (_, s) = modify(site, slug, |s| {
        let ids: Vec<String> = ids.iter().map(|i| s.resolve(i)).collect::<Result<_>>()?;
        s.reorder(&ids)?;
        Ok(((), ChangeNote::new("order", "reordered the photos")))
    })?;
    Ok(json!({ "order": s.images.iter().map(|i| i.id.clone()).collect::<Vec<_>>() }))
}

// ------------------------------------------------------------------ choosing photos

/// Select photos (`3,7-9,12`, `img3`, or `all`). A comment, if given, goes on every one;
/// without it, photos lacking a comment are listed as needing one.
pub fn select(site: &Site, slug: &str, refs: &str, comment: Option<&str>) -> Result<Value> {
    let (ids, s) = modify(site, slug, |s| {
        let ids = s.resolve_many(refs)?;
        s.select(site, &ids, comment)?;
        let what = format!("selected {}", numbers(s, &ids));
        Ok((ids.clone(), ChangeNote::new("select", what).ids(ids)))
    })?;
    let need: Vec<u32> = s.needing_comments().iter().map(|i| i.number).collect();
    Ok(json!({
        "selected": ids.iter().filter_map(|id| s.image(id).ok()).map(|i| json!({ "number": i.number, "id": i.id, "filename": i.filename, "comment": i.notes })).collect::<Vec<_>>(),
        "needs_comment": need,
        "selection_complete": need.is_empty(),
        "hint": if need.is_empty() { String::new() } else { format!("Each selected photo needs a comment: gw story comment {} <number> \"what's happening, why it matters\"", s.slug) },
        "rev": s.rev,
    }))
}

/// Put photos back among the candidates, or reject them.
pub fn set_state(site: &Site, slug: &str, refs: &str, state: &str) -> Result<Value> {
    let (ids, s) = modify(site, slug, |s| {
        let ids = s.resolve_many(refs)?;
        s.set_state(&ids, state)?;
        let (kind, verb) = if state == "rejected" { ("reject", "rejected") } else { ("unselect", "moved back to candidates") };
        let what = format!("{verb} {}", numbers(s, &ids));
        Ok((ids.clone(), ChangeNote::new(kind, what).ids(ids)))
    })?;
    Ok(json!({ "state": state, "numbers": numbers(&s, &ids), "rev": s.rev }))
}

/// Set a photo's comment (required for selected photos).
pub fn comment(site: &Site, slug: &str, r: &str, text: &str) -> Result<Value> {
    if text.trim().is_empty() {
        bail!("the comment is empty");
    }
    image_update(site, slug, r, &ImageUpdate { notes: Some(text.trim().to_string()), ..Default::default() })
}

/// Render a numbered contact sheet (one page) of candidates, selected photos, or all.
pub fn contact_sheet(site: &Site, slug: &str, which: &str, page: usize, per_page: usize) -> Result<Value> {
    let s = Story::load(site, slug)?;
    let per_page = per_page.clamp(4, 60);
    let list: Vec<&StoryImage> = s.images.iter().filter(|i| match which {
        "candidates" | "candidate" => i.state == "candidate",
        "selected" => i.is_selected(),
        "rejected" => i.state == "rejected",
        "all" => true,
        w => w.is_empty() && i.state != "rejected",
    }).collect();
    let mut list = list;
    list.sort_by_key(|i| i.number);
    let total = list.len();
    let pages = total.div_ceil(per_page).max(1);
    let page = page.clamp(1, pages);
    let slice: Vec<&StoryImage> = list.iter().skip((page - 1) * per_page).take(per_page).copied().collect();
    // Thumbnails, a few at a time (local decodes and SmugMug fetches are slow one by one).
    let mut thumbs: Vec<Option<PathBuf>> = vec![None; slice.len()];
    for chunk in (0..slice.len()).collect::<Vec<_>>().chunks(8) {
        std::thread::scope(|sc| {
            let hs: Vec<_> = chunk.iter().map(|&i| {
                let (s, im) = (&s, slice[i]);
                (i, sc.spawn(move || -> Option<PathBuf> {
                    match s.thumb_local(site, im) {
                        Ok(Some(p)) => Some(p),
                        Ok(None) => im.remote.as_ref().and_then(|r| fetch_thumb(&s.slug, &im.id, if r.thumb.is_empty() { &r.url } else { &r.thumb }).ok()),
                        Err(_) => None,
                    }
                }))
            }).collect();
            for (i, h) in hs {
                thumbs[i] = h.join().ok().flatten();
            }
        });
    }
    let tiles: Vec<SheetTile> = slice.iter().zip(&thumbs).map(|(im, t)| SheetTile {
        number: im.number,
        label: if im.filename.is_empty() { im.source.clone() } else { im.filename.clone() },
        sub: im.taken.clone().map(|t| t.chars().take(16).collect()).unwrap_or_else(|| story::source_kind(&im.source).to_string()),
        state: im.state.clone(),
        needs_comment: im.needs_comment(),
        thumb: t.clone(),
    }).collect();
    let label = if which.is_empty() { "photos" } else { which };
    let title = format!("{} — {label}{}  (#numbers: say e.g. \"3, 7-9\")", s.title, if pages > 1 { format!(", page {page}/{pages}") } else { String::new() });
    let path = story::cache_dir(&s.slug).join("sheets").join(format!("{label}-p{page}.jpg"));
    if tiles.is_empty() {
        bail!("no {label} to show in story {}", s.slug);
    }
    story::render_contact_sheet(&tiles, 6, &title, &path)?;
    Ok(json!({
        "story": s.slug, "which": label, "page": page, "pages": pages, "per_page": per_page, "total": total,
        "path": abs(&path),
        "items": slice.iter().zip(&thumbs).map(|(i, t)| json!({ "number": i.number, "id": i.id, "state": i.state, "filename": i.filename,
            "taken": i.taken, "comment": i.notes, "needs_comment": i.needs_comment(), "source": i.source, "thumb": t.as_ref().map(|p| abs(p)) })).collect::<Vec<_>>(),
        "select_with": format!("gw story select {} 3,7-9 --comment \"…\"", s.slug),
    }))
}

// ------------------------------------------------------------------ the story so far, questions, the change feed

pub fn narrative(site: &Site, slug: &str, text: &str, append: bool, base: Option<u64>) -> Result<Value> {
    let (_, s) = modify(site, slug, |s| {
        s.set_narrative(text, append, base)?;
        let what = if append { format!("added to the story text: {}", clip_line(text, 120)) } else { "rewrote the story text".to_string() };
        Ok(((), ChangeNote::new("narrative", what)))
    })?;
    Ok(json!({ "narrative": s.narrative, "narrative_rev": s.narrative_rev, "rev": s.rev }))
}

fn clip_line(s: &str, n: usize) -> String {
    let one = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= n { one } else { one.chars().take(n - 1).collect::<String>() + "…" }
}

/// Record a question Claude is waiting on (answerable from the CLI or the app).
pub fn ask(site: &Site, slug: &str, q: &str) -> Result<Value> {
    let (id, s) = modify(site, slug, |s| {
        let id = s.ask(q)?;
        Ok((id, ChangeNote::new("ask", format!("asked Q{id}: {}", clip_line(q, 160)))))
    })?;
    Ok(json!({ "id": id, "q": q.trim(), "pending": s.pending, "rev": s.rev, "answer_in_app": format!("{APP_BASE}{}", s.slug) }))
}

/// Answer a pending question (the oldest, or `id`) and record it in the transcript.
pub fn answer(site: &Site, slug: &str, id: Option<u32>, a: &str) -> Result<Value> {
    let ((q, n), s) = modify(site, slug, |s| {
        let (p, n) = s.answer(site, id, a)?;
        let what = format!("answered Q{} “{}”: {}", p.id, clip_line(&p.q, 80), clip_line(a, 400));
        Ok(((p, n), ChangeNote::new("answer", what)))
    })?;
    Ok(json!({ "answered": q.id, "q": q.q, "a": a.trim(), "recorded": n, "pending": s.pending, "rev": s.rev }))
}

pub fn pending(site: &Site, slug: &str) -> Result<Value> {
    let s = Story::load(site, slug)?;
    Ok(json!({ "pending": s.pending, "rev": s.rev }))
}

/// What changed after revision `since` (who, what, when).
pub fn changes(site: &Site, slug: &str, since: u64) -> Result<Value> {
    let s = Story::load(site, slug)?;
    Ok(json!({ "rev": s.rev, "since": since, "changes": Story::changes_since(site, slug, since)?, "pending": s.pending }))
}

/// Wait (up to `timeout` seconds) for a change after `since` made somewhere else — e.g. an
/// answer typed in the app. With `answer_only`, wait until no question is pending.
pub fn wait(site: &Site, slug: &str, since: Option<u64>, timeout: u64, answer_only: bool) -> Result<Value> {
    let start = std::time::Instant::now();
    let since = match since { Some(r) => r, None => Story::load(site, slug)?.rev };
    let me = actor();
    loop {
        let ch: Vec<_> = Story::changes_since(site, slug, since)?.into_iter().filter(|c| c.by != me).collect();
        let done = if answer_only { Story::load(site, slug)?.pending.is_empty() || ch.iter().any(|c| c.kind == "answer") } else { !ch.is_empty() };
        if done {
            return Ok(json!({ "changed": true, "changes": Story::changes_since(site, slug, since)?, "rev": Story::load(site, slug)?.rev, "pending": Story::load(site, slug)?.pending }));
        }
        if start.elapsed().as_secs() >= timeout {
            return Ok(json!({ "changed": false, "rev": since, "waited": timeout }));
        }
        std::thread::sleep(std::time::Duration::from_millis(700));
    }
}

/// The thumbnail file for the app (cached; made from the local original if needed).
pub fn thumb_file(site: &Site, slug: &str, id: &str) -> Result<std::path::PathBuf> {
    let s = Story::load(site, slug)?;
    let im = s.image(id)?.clone();
    if let Some(p) = s.thumb_local(site, &im)? {
        return Ok(p);
    }
    let r = im.remote.as_ref().context("no image to show")?;
    fetch_thumb(slug, id, &r.thumb)
}

// ------------------------------------------------------------------ notes, interview, outline, draft

pub fn note(site: &Site, slug: &str, text: &str, replace: bool) -> Result<Value> {
    let (_, s) = modify(site, slug, |s| {
        if replace {
            s.notes = text.to_string();
        } else if !text.trim().is_empty() {
            s.notes = if s.notes.trim().is_empty() { text.trim().to_string() } else { format!("{}\n\n{}", s.notes.trim_end(), text.trim()) };
        }
        Ok(((), ChangeNote::new("notes", if replace { "edited the notes".to_string() } else { format!("added a note: {}", clip_line(text, 120)) })))
    })?;
    Ok(json!({ "notes": s.notes, "rev": s.rev }))
}

/// Record a question and answer (a pending question with the same text is answered by it).
pub fn qa(site: &Site, slug: &str, q: &str, a: &str) -> Result<Value> {
    let (n, s) = modify(site, slug, |s| {
        let n = s.add_qa(site, q, a)?;
        Ok((n, ChangeNote::new("qa", format!("Q{n} “{}”: {}", clip_line(q, 80), clip_line(a, 400)))))
    })?;
    Ok(json!({ "recorded": n, "status": s.status, "transcript": abs(&s.interview_path(site)), "pending": s.pending, "rev": s.rev }))
}

pub fn outline(site: &Site, slug: &str, text: &str) -> Result<Value> {
    let (_, s) = modify(site, slug, |s| {
        s.set_outline(site, text)?;
        Ok(((), ChangeNote::new("outline", "saved the outline")))
    })?;
    Ok(json!({ "saved": abs(&s.outline_path(site)), "status": s.status, "rev": s.rev }))
}

/// Write the post from AsciiDoc using `image::story:<id>[…]`, then rebuild the preview.
pub fn draft(site: &Site, slug: &str, text: &str, preview: bool) -> Result<Value> {
    // An enhancement's draft is a proposal: the post changes only when the author accepts it.
    if Story::load(site, slug)?.is_enhancement() {
        return crate::enhance::propose(site, slug, text);
    }
    let (r, _s) = modify(site, slug, |s| {
        let r = story::draft(site, s, text)?;
        let what = format!("wrote the draft {}", r.url);
        Ok((r, ChangeNote::new("draft", what)))
    })?;
    let mut v = json!(r);
    v["preview_url"] = json!(format!("{PREVIEW_BASE}{}", r.url));
    if preview {
        match crate::server::build_preview(site) {
            Ok(b) => v["preview_built"] = json!(b.pages),
            Err(e) => v["preview_error"] = json!(format!("{e:#}")),
        }
    }
    Ok(v)
}

/// Make sure every image has a review thumbnail on disk and list them, so Claude can look at
/// each photo during the interview.
pub fn review_images(site: &Site, slug: &str) -> Result<Value> {
    let s = Story::load(site, slug)?;
    let mut out = vec![];
    // Selected photos only; candidates are reviewed as contact sheets.
    for im in s.images.iter().filter(|i| i.is_selected()) {
        let path = match s.thumb_local(site, im) {
            Ok(Some(p)) => Ok(p),
            Ok(None) => match &im.remote {
                Some(r) => fetch_thumb(&s.slug, &im.id, if r.thumb.is_empty() { &r.url } else { &r.thumb }),
                None => Err(anyhow::anyhow!("nothing to show")),
            },
            Err(e) => Err(e),
        };
        out.push(json!({
            "id": im.id, "number": im.number, "needs_comment": im.needs_comment(), "include": im.include, "source": im.source, "filename": im.filename,
            "caption": im.caption, "comment": im.notes, "taken": im.taken,
            "path": path.as_ref().ok().map(|p| abs(p)),
            "error": path.as_ref().err().map(|e| format!("{e:#}")),
        }));
    }
    Ok(json!({ "story": s.slug, "title": s.title, "notes": s.notes, "narrative": s.narrative, "images": out,
        "candidates": s.images.iter().filter(|i| i.state == "candidate").count() }))
}

// ------------------------------------------------------------------ SmugMug picker

pub fn smugmug_albums() -> Result<Value> {
    smugmug::albums(&Auth::load()?)
}

pub fn smugmug_browse(target: &str, start: usize, count: usize) -> Result<Value> {
    smugmug::browse(&Auth::load()?, target, start, count)
}

#[derive(Debug, Clone, Deserialize)]
pub struct Pick {
    pub image_key: String,
    #[serde(default)]
    pub album_key: Option<String>,
    #[serde(default)]
    pub caption: Option<String>,
    #[serde(default)]
    pub comment: String,
    /// Take this image out of the story.
    #[serde(default)]
    pub remove: bool,
}

/// Apply the picker's selection: add new images, update comments and captions of ones already
/// in the story, remove deselected ones. Every kept image needs a comment.
pub fn smugmug_pick(site: &Site, slug: &str, picks: &[Pick]) -> Result<Value> {
    let missing: Vec<&str> = picks.iter().filter(|p| !p.remove && p.comment.trim().is_empty()).map(|p| p.image_key.as_str()).collect();
    if !missing.is_empty() {
        bail!("every selected image needs a comment; missing for {}", missing.join(", "));
    }
    // Look up new images first (network), then apply everything under the story's lock.
    let snapshot = Story::load(site, slug)?;
    let new: Vec<_> = picks.iter().filter(|p| !p.remove && snapshot.find_source(&format!("smugmug:{}", p.image_key)).is_none())
        .map(|p| (p.image_key.clone(), p.album_key.clone(), p.caption.clone(), p.comment.trim().to_string())).collect();
    let mut looked_up = vec![];
    let mut failed = vec![];
    if !new.is_empty() {
        let auth = Auth::load()?;
        for (r, (k, ..)) in smugmug_images(&auth, &new).into_iter().zip(&new) {
            match r {
                Ok(im) => looked_up.push(im),
                Err(e) => failed.push(json!({ "image_key": k, "error": format!("{e:#}") })),
            }
        }
    }
    let ((updated, removed, added), s) = modify(site, slug, |s| {
        let (mut updated, mut removed, mut added) = (vec![], vec![], vec![]);
        for p in picks {
            let source = format!("smugmug:{}", p.image_key);
            let existing = s.find_source(&source).map(|i| i.id.clone());
            match (existing, p.remove) {
                (Some(id), true) => {
                    s.remove_image(site, &id)?;
                    removed.push(id);
                }
                (Some(id), false) => {
                    let im = s.image_mut(&id)?;
                    im.notes = p.comment.trim().to_string();
                    im.state = "selected".into();
                    if let Some(c) = &p.caption {
                        im.caption = c.clone();
                    }
                    updated.push(id);
                }
                _ => {}
            }
        }
        for im in looked_up {
            if s.find_source(&im.source).is_none() {
                added.push(s.add_remote(im));
            }
        }
        let ids: Vec<String> = added.iter().map(|i: &StoryImage| i.id.clone()).chain(updated.iter().cloned()).collect();
        let what = format!("SmugMug picker: added {}, updated {}, removed {}", added.len(), updated.len(), removed.len());
        Ok(((updated, removed, added), ChangeNote::new("select", what).ids(ids)))
    })?;
    for im in &added {
        if let Some(r) = &im.remote {
            let _ = fetch_thumb(&s.slug, &im.id, &r.thumb);
        }
    }
    Ok(json!({
        "added": added.iter().map(|i| i.id.clone()).collect::<Vec<_>>(),
        "updated": updated, "removed": removed, "failed": failed,
        "story": story_json(site, &s),
    }))
}

/// Read a draft or outline argument: a file path, `-` for stdin, or the text itself.
pub fn read_text_arg(file: Option<&Path>, text: Option<&str>) -> Result<String> {
    match (file, text) {
        (Some(p), _) if p == Path::new("-") => {
            let mut s = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
            Ok(s)
        }
        (Some(p), _) => std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display())),
        (None, Some(t)) => Ok(t.to_string()),
        (None, None) => bail!("pass --file <path> (or - for stdin) or --text"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site() -> (Site, std::path::PathBuf) {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        static CACHE: OnceLock<()> = OnceLock::new();
        CACHE.get_or_init(|| std::env::set_var("GW_STORY_CACHE", std::env::temp_dir().join(format!("gw-stories-ops-cache-{}", std::process::id()))));
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("gw-stories-ops-{}-{n}-{}", std::process::id(), chrono::Local::now().timestamp_nanos_opt().unwrap()));
        std::fs::create_dir_all(root.join("content")).unwrap();
        std::fs::write(root.join("site.yaml"), "site:\n  title: T\n  base_url: https://example.org\n  author: A\nareas:\n  - id: retro\n    title: Retro\n").unwrap();
        (Site::open(Some(&root)).unwrap(), root)
    }

    /// A story holding one SmugMug image, as the picker would have added it (invented values).
    fn with_smugmug(site: &Site) -> Story {
        let mut s = Story::create(site, &NewStory { title: "Picker".into(), ..Default::default() }).unwrap();
        s.add_remote(StoryImage {
            source: "smugmug:AbC12dE".into(), notes: "First comment".into(), caption: "Old caption".into(), include: true,
            remote: Some(Remote { url: "https://photos.smugmug.com/a-X3.jpg".into(), thumb: "https://photos.smugmug.com/a-M.jpg".into(), ..Default::default() }),
            ..Default::default()
        });
        s.save(site).unwrap();
        s
    }

    #[test]
    fn picks_need_comments() {
        let (site, root) = site();
        with_smugmug(&site);
        let p = |k: &str, c: &str| Pick { image_key: k.into(), album_key: None, caption: None, comment: c.into(), remove: false };
        let e = smugmug_pick(&site, "picker", &[p("AbC12dE", "ok"), p("Zz9", "  ")]).unwrap_err().to_string();
        assert!(e.contains("missing for Zz9"), "{e}");
        // nothing was changed by the refused call
        assert_eq!(Story::load(&site, "picker").unwrap().images[0].notes, "First comment");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn picks_update_and_remove_existing_without_the_network() {
        let (site, root) = site();
        with_smugmug(&site);
        let r = smugmug_pick(&site, "picker", &[Pick { image_key: "AbC12dE".into(), album_key: None, caption: Some("New caption".into()), comment: "Better comment".into(), remove: false }]).unwrap();
        assert_eq!(r["updated"], json!(["img1"]));
        let s = Story::load(&site, "picker").unwrap();
        assert_eq!((s.images[0].notes.as_str(), s.images[0].caption.as_str()), ("Better comment", "New caption"));
        let r = smugmug_pick(&site, "picker", &[Pick { image_key: "AbC12dE".into(), album_key: None, caption: None, comment: String::new(), remove: true }]).unwrap();
        assert_eq!(r["removed"], json!(["img1"]));
        assert!(Story::load(&site, "picker").unwrap().images.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn smugmug_sources_need_a_comment_and_comments_cannot_be_cleared() {
        let (site, root) = site();
        with_smugmug(&site);
        let e = add(&site, "picker", &AddArgs { sources: vec!["https://me.smugmug.com/A/i-Q1w2E3r".into()], ..Default::default() }).unwrap_err().to_string();
        assert!(e.contains("need a comment"), "{e}");
        let e = image_update(&site, "picker", "img1", &ImageUpdate { notes: Some(" ".into()), ..Default::default() }).unwrap_err().to_string();
        assert!(e.contains("keep a comment"), "{e}");
        // URLs and the media library need no comment
        std::fs::write(root.join("media.yaml"), "media:\n  bench:\n    url: https://example.org/b.jpg\n").unwrap();
        let r = add(&site, "picker", &AddArgs { sources: vec!["media:bench".into()], ..Default::default() }).unwrap();
        assert_eq!(r["added"][0]["source"], "media:bench");
        let r = add(&site, "picker", &AddArgs { sources: vec!["media:bench".into()], ..Default::default() }).unwrap();
        assert_eq!(r["skipped"][0]["reason"], "already in the story");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn interview_flow_through_the_operations() {
        let (site, root) = site();
        let dir = root.join("photos");
        std::fs::create_dir_all(&dir).unwrap();
        for (i, name) in ["one.jpg", "two.jpg", "three.jpg"].iter().enumerate() {
            let img = image::RgbImage::from_fn(64, 48, |x, _| image::Rgb([(i * 80) as u8, x as u8 * 3, 90]));
            img.save(dir.join(name)).unwrap();
        }
        create(&site, &NewStory { title: "Interview".into(), ..Default::default() }).unwrap();
        let r = add_dir(&site, "interview", &AddDirArgs { dirs: vec![dir.display().to_string()], ..Default::default() }).unwrap();
        assert_eq!(r["added"], 3);
        assert_eq!(r["numbers"], "#1-3");
        let again = add_dir(&site, "interview", &AddDirArgs { dirs: vec![dir.display().to_string()], ..Default::default() }).unwrap();
        assert_eq!((again["added"].as_u64(), again["already_in_story"].as_u64()), (Some(0), Some(3)));
        // a folder given to `add` points at add-dir
        assert!(add(&site, "interview", &AddArgs { sources: vec![dir.display().to_string()], ..Default::default() }).unwrap_err().to_string().contains("add-dir"));
        let sheet = contact_sheet(&site, "interview", "candidates", 1, 24).unwrap();
        assert_eq!(sheet["items"].as_array().unwrap().len(), 3);
        assert!(std::path::Path::new(sheet["path"].as_str().unwrap()).exists());
        let v = select(&site, "interview", "1-2", None).unwrap();
        assert_eq!(v["needs_comment"], json!([1, 2]));
        assert_eq!(v["selection_complete"], false);
        comment(&site, "interview", "#1", "The first table").unwrap();
        let v = select(&site, "interview", "2", Some("Setting up")).unwrap();
        assert_eq!(v["selection_complete"], true);
        // a selected photo can't lose its comment; reject/unselect work by number
        assert!(comment(&site, "interview", "2", "  ").is_err());
        assert!(image_update(&site, "interview", "#2", &ImageUpdate { notes: Some(String::new()), ..Default::default() }).unwrap_err().to_string().contains("keep a comment"));
        set_state(&site, "interview", "3", "rejected").unwrap();
        let shown = show(&site, "interview").unwrap();
        assert_eq!(shown["counts"], json!({ "candidates": 0, "selected": 2, "rejected": 1, "needs_comment": 0 }));
        assert_eq!(shown["app_url"], "http://127.0.0.1:8790/app/#story/interview");
        // narrative, questions answered from "the app", and the change feed
        let rev = shown["rev"].as_u64().unwrap();
        assert_eq!(show_since(&site, "interview", Some(rev)).unwrap()["unchanged"], true);
        narrative(&site, "interview", "We borrowed a hall.", true, None).unwrap();
        let e = narrative(&site, "interview", "x", false, Some(0)).unwrap_err().to_string();
        assert!(e.starts_with("conflict:"));
        ask(&site, "interview", "When was it?").unwrap();
        assert_eq!(pending(&site, "interview").unwrap()["pending"].as_array().unwrap().len(), 1);
        answer(&site, "interview", None, "Last October.").unwrap();
        let c = changes(&site, "interview", rev).unwrap();
        let kinds: Vec<&str> = c["changes"].as_array().unwrap().iter().map(|x| x["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, vec!["narrative", "ask", "answer"]);
        assert!(c["changes"][2]["what"].as_str().unwrap().contains("Last October."));
        assert!(c["pending"].as_array().unwrap().is_empty());
        // `wait` ignores this process's own changes and returns for someone else's (the app)
        let now = c["rev"].as_u64().unwrap();
        assert_eq!(wait(&site, "interview", Some(rev), 1, false).unwrap()["changed"], json!(false));
        Story::modify(&site, "interview", "app", |s| { s.notes = "typed in the app".into(); Ok(((), ChangeNote::new("notes", "edited the notes"))) }).unwrap();
        let w = wait(&site, "interview", Some(now), 5, false).unwrap();
        assert_eq!(w["changed"], json!(true));
        assert_eq!(w["changes"][0]["by"], "app");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn show_lists_what_claude_needs() {
        let (site, root) = site();
        with_smugmug(&site);
        let v = show(&site, "picker").unwrap();
        assert_eq!(v["interview_command"], "/project-story picker");
        assert_eq!(v["images"][0]["notes"], "First comment");
        assert_eq!(v["images"][0]["thumb_url"], "https://photos.smugmug.com/a-M.jpg");
        assert!(v["dir"].as_str().unwrap().starts_with('/'), "absolute path");
        std::fs::remove_dir_all(root).unwrap();
    }
}
