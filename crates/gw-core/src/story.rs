//! Project stories: a workspace for turning photos and a few notes into a post.
//!
//! A story lives in `site/stories/<slug>/` — outside `content/`, so it is never built or
//! published:
//!
//! ```text
//! stories/amiga-40th-exhibit/
//!   story.yaml      title, target (project | article | update), areas, tags, notes, status,
//!                   images [{id, source, caption, notes, taken, width, height, include, ...}]
//!   images/         originals dropped in from disk (git-ignored: they can be large)
//!   interview.md    the interview transcript, appended question by question
//!   outline.md      the agreed outline
//!   changes.jsonl   what changed, when, and from where (CLI, app, MCP) — the change feed
//!   .rev            the current revision number (cheap to poll)
//!   .lock           held while a writer re-reads, changes, and saves the story
//! ```
//!
//! Images are *candidates* (offered: a folder scan, a SmugMug album, pasted photos), *selected*
//! (going into the story — each needs the author's comment), or *rejected*. Every image has a
//! stable number (`#7`) so the author can pick from a contact sheet in the terminal.
//!
//! Image sources are `file:images/<name>` (a local original), `path:/abs/file.jpg` (a candidate
//! still on disk where it was found — copied into `images/` when selected), `smugmug:<ImageKey>`,
//! `media:<key>` (already in the media library), or `url:<https://…>`. A draft refers to
//! images as `image::story:<id>[Alt]` (and `:hero: story:<id>`); [`draft`] resolves those to a
//! web-sized copy in the content bundle (resized to at most 2000 px, re-encoded so EXIF —
//! GPS included — is dropped) or to a `media:<key>` for SmugMug-hosted photos.

use crate::config::expand_home;
use crate::content::{self, slugify, Kind};
use crate::media::Media;
use crate::site::Site;
use anyhow::{bail, Context, Result};
use chrono::{Local, NaiveDateTime};
use image::{DynamicImage, ImageDecoder, ImageReader};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const TARGETS: &[&str] = &["project", "article", "update"];
pub const STATUSES: &[&str] = &["collecting", "interviewing", "drafting", "drafted", "released"];
/// What an image is to the story.
pub const STATES: &[&str] = &["candidate", "selected", "rejected"];
/// Longest edge of images copied into a post.
pub const WEB_MAX: u32 = 2000;
/// Longest edge of review thumbnails (what Claude looks at during the interview).
pub const THUMB_MAX: u32 = 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Story {
    pub title: String,
    pub slug: String,
    /// project | article | update
    pub target: String,
    /// For updates: the project they belong to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default)]
    pub areas: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// The author's own notes, in their words.
    #[serde(default)]
    pub notes: String,
    pub status: String,
    pub created: NaiveDateTime,
    pub updated: NaiveDateTime,
    #[serde(default)]
    pub images: Vec<StoryImage>,
    /// The content file generated from this story, relative to the site root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<String>,
    /// Files the last draft wrote into its bundle (relative to the site root), so a re-draft
    /// can remove the ones it no longer uses.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub draft_files: Vec<String>,
    /// "The story so far": the author's general account, in their words (distinct from notes).
    #[serde(default)]
    pub narrative: String,
    /// Bumped whenever the narrative changes, so concurrent editors can detect a conflict.
    #[serde(default)]
    pub narrative_rev: u64,
    /// Revision of the whole story: bumped by every change (see [`Story::modify`]).
    #[serde(default)]
    pub rev: u64,
    /// Questions Claude has asked and is waiting on; answered from the CLI or the app.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending: Vec<PendingQuestion>,
    /// How many questions have been asked (pending ids come from this).
    #[serde(default)]
    pub asked: u32,
    /// An *enhancement*: this story reworks an existing content item (its id) instead of
    /// making a new post. Drafts become proposals the author accepts (see [`crate::enhance`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enhances: Option<String>,
    /// The code repository behind the item, when known (enhancements).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<crate::enhance::RepoRef>,
    /// The author's answers to the standard questions (enhancements): question id → answer.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub key_answers: BTreeMap<String, String>,
    /// A revised post waiting for the author (enhancements): `proposed.adoc` in the story.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal: Option<crate::enhance::Proposal>,
    /// The last Claude session started from the app for this story.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude: Option<crate::enhance::ClaudeRun>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PendingQuestion {
    pub id: u32,
    pub q: String,
    pub asked: NaiveDateTime,
}

/// One entry of the change feed (`changes.jsonl`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Change {
    pub rev: u64,
    pub at: NaiveDateTime,
    /// Who made it: `app`, `cli`, `mcp`, … (see `GW_ACTOR`).
    pub by: String,
    /// candidates | select | unselect | reject | comment | caption | images | order | narrative |
    /// notes | ask | answer | qa | outline | draft | settings | created
    pub kind: String,
    /// One line for people: "selected #3, #7".
    pub what: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ids: Vec<String>,
}

/// What a change did, returned by the closure given to [`Story::modify`].
#[derive(Debug, Clone, Default)]
pub struct ChangeNote {
    pub kind: String,
    pub what: String,
    pub ids: Vec<String>,
}

impl ChangeNote {
    pub fn new(kind: &str, what: impl Into<String>) -> Self {
        ChangeNote { kind: kind.into(), what: what.into(), ids: vec![] }
    }
    pub fn ids(mut self, ids: Vec<String>) -> Self {
        self.ids = ids;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StoryImage {
    pub id: String,
    /// file:images/<name> | smugmug:<ImageKey> | media:<key> | url:<https://…>
    pub source: String,
    #[serde(default)]
    pub caption: String,
    /// What the author says about this image ("what's happening here / why it matters").
    #[serde(default)]
    pub notes: String,
    /// When the photo was taken (EXIF DateTimeOriginal, or SmugMug's date), if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub taken: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(default = "yes")]
    pub include: bool,
    /// 1-based position (kept equal to the list order).
    #[serde(default)]
    pub order: u32,
    /// The original file name, for display.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub filename: String,
    /// Hosted images (SmugMug, URLs): where they are served from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<Remote>,
    /// candidate | selected | rejected. Stories made before candidates existed hold only
    /// selected images, so that is the default.
    #[serde(default = "selected_state")]
    pub state: String,
    /// Stable number within the story (`#7`), for picking from a contact sheet.
    #[serde(default)]
    pub number: u32,
    /// Where a candidate was found (an absolute path, or the SmugMug page it came from).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub origin: String,
}

impl StoryImage {
    pub fn is_selected(&self) -> bool {
        self.state == "selected"
    }
    /// Selected but still waiting for the author's comment.
    pub fn needs_comment(&self) -> bool {
        self.is_selected() && self.notes.trim().is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Remote {
    /// The URL readers get (large size).
    pub url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thumb: String,
    /// The image's own page (SmugMug gallery page).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub web: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub album_key: String,
    /// image | video
    #[serde(default = "image_kind")]
    pub kind: String,
}

fn yes() -> bool {
    true
}
fn image_kind() -> String {
    "image".into()
}
fn selected_state() -> String {
    "selected".into()
}

const HEADER: &str = "# A project story in progress: photos, notes, and the interview behind a post.\n# Managed by `gw story ...` and the Stories section of the app. Not part of the built site.\n";

// ------------------------------------------------------------------ locations

pub fn stories_dir(site: &Site) -> PathBuf {
    site.root.join("stories")
}

pub fn story_dir(site: &Site, slug: &str) -> PathBuf {
    stories_dir(site).join(slug)
}

/// Review thumbnails live outside the repository: `~/.gitwebsite/cache/stories/<slug>/`.
pub fn cache_dir(slug: &str) -> PathBuf {
    std::env::var_os("GW_STORY_CACHE")
        .map(PathBuf::from)
        .unwrap_or_else(|| expand_home("~/.gitwebsite/cache/stories"))
        .join(slug)
}

pub fn thumb_path(slug: &str, id: &str) -> PathBuf {
    cache_dir(slug).join(format!("{id}.jpg"))
}

pub(crate) fn valid_slug(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

// ------------------------------------------------------------------ the story

impl Story {
    pub fn dir(&self, site: &Site) -> PathBuf {
        story_dir(site, &self.slug)
    }

    pub fn load(site: &Site, slug: &str) -> Result<Story> {
        if !valid_slug(slug) {
            bail!("no story {slug:?}");
        }
        let p = story_dir(site, slug).join("story.yaml");
        if !p.exists() {
            bail!("no story {slug} (stories/{slug}/story.yaml); see `gw story list`");
        }
        let mut s: Story = serde_yaml::from_str(&std::fs::read_to_string(&p)?).with_context(|| format!("parsing {}", p.display()))?;
        s.slug = slug.to_string();
        s.number_images();
        Ok(s)
    }

    /// Give every image a stable number (older stories have none).
    fn number_images(&mut self) {
        let mut next = self.images.iter().map(|i| i.number).max().unwrap_or(0);
        for im in self.images.iter_mut() {
            if im.number == 0 {
                next += 1;
                im.number = next;
            }
            if !STATES.contains(&im.state.as_str()) {
                im.state = "selected".into();
            }
        }
    }

    pub fn next_number(&self) -> u32 {
        self.images.iter().map(|i| i.number).max().unwrap_or(0) + 1
    }

    /// Write story.yaml atomically (a temporary file renamed into place), so a reader never sees
    /// half a file. Use [`Story::modify`] for changes that others may be making at the same time.
    pub fn save(&mut self, site: &Site) -> Result<()> {
        self.updated = Local::now().naive_local().with_nanosecond_zero();
        for (i, im) in self.images.iter_mut().enumerate() {
            im.order = i as u32 + 1;
        }
        let d = self.dir(site);
        std::fs::create_dir_all(&d)?;
        let body = serde_yaml::to_string(self)?;
        atomic_write(&d.join("story.yaml"), format!("{HEADER}{body}").as_bytes())?;
        atomic_write(&d.join(".rev"), self.rev.to_string().as_bytes())
    }

    /// Change a story safely while the CLI, the app, and Claude may all be editing it: take the
    /// story's lock, re-read it from disk, apply `f`, bump the revision, save atomically, and
    /// record the change in the feed. `by` says where the change came from.
    pub fn modify<T>(site: &Site, slug: &str, by: &str, f: impl FnOnce(&mut Story) -> Result<(T, ChangeNote)>) -> Result<(T, Story)> {
        let _lock = lock(site, slug)?;
        let mut s = Story::load(site, slug)?;
        let (out, note) = f(&mut s)?;
        s.rev += 1;
        s.save(site)?;
        s.record(site, by, &note)?;
        Ok((out, s))
    }

    fn record(&self, site: &Site, by: &str, note: &ChangeNote) -> Result<()> {
        use std::io::Write;
        let c = Change { rev: self.rev, at: Local::now().naive_local().with_nanosecond_zero(), by: by.into(), kind: note.kind.clone(), what: note.what.clone(), ids: note.ids.clone() };
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(self.dir(site).join("changes.jsonl"))?;
        writeln!(f, "{}", serde_json::to_string(&c)?)?;
        Ok(())
    }

    /// Changes after revision `since`, oldest first.
    pub fn changes_since(site: &Site, slug: &str, since: u64) -> Result<Vec<Change>> {
        let p = story_dir(site, slug).join("changes.jsonl");
        let text = std::fs::read_to_string(&p).unwrap_or_default();
        Ok(text.lines().filter_map(|l| serde_json::from_str::<Change>(l).ok()).filter(|c| c.rev > since).collect())
    }

    /// The story's revision, read cheaply (for polling).
    pub fn current_rev(site: &Site, slug: &str) -> Option<u64> {
        std::fs::read_to_string(story_dir(site, slug).join(".rev")).ok()?.trim().parse().ok()
    }

    pub fn list(site: &Site) -> Result<Vec<Story>> {
        let d = stories_dir(site);
        let mut out = vec![];
        if d.exists() {
            for e in std::fs::read_dir(&d)? {
                let e = e?;
                let slug = e.file_name().to_string_lossy().to_string();
                if e.path().join("story.yaml").exists() {
                    out.push(Story::load(site, &slug)?);
                }
            }
        }
        out.sort_by(|a, b| b.updated.cmp(&a.updated));
        Ok(out)
    }

    pub fn create(site: &Site, n: &NewStory) -> Result<Story> {
        let title = n.title.trim();
        if title.is_empty() {
            bail!("a story needs a title");
        }
        let target = n.target.as_deref().unwrap_or("project");
        check_target(site, target, n.project.as_deref())?;
        let areas = check_areas(site, &n.areas)?;
        let base = n.slug.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| slugify(title));
        if !valid_slug(&base) {
            bail!("slug {base:?} must use lowercase letters, digits, and '-'");
        }
        let mut slug = base.clone();
        let mut i = 2;
        while story_dir(site, &slug).exists() {
            slug = format!("{base}-{i}");
            i += 1;
        }
        let now = Local::now().naive_local().with_nanosecond_zero();
        let mut s = Story {
            title: title.into(),
            slug,
            target: target.into(),
            project: n.project.clone().filter(|p| !p.is_empty()),
            areas,
            tags: n.tags.iter().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect(),
            notes: n.notes.clone().unwrap_or_default(),
            status: "collecting".into(),
            created: now,
            updated: now,
            images: vec![],
            draft: None,
            draft_files: vec![],
            narrative: String::new(),
            narrative_rev: 0,
            rev: 1,
            pending: vec![],
            asked: 0,
            enhances: None,
            repo: None,
            key_answers: BTreeMap::new(),
            proposal: None,
            claude: None,
        };
        std::fs::create_dir_all(s.dir(site).join("images"))?;
        s.save(site)?;
        Ok(s)
    }

    /// Record the story's creation in its change feed.
    pub fn record_created(&self, site: &Site, by: &str) -> Result<()> {
        self.record(site, by, &ChangeNote::new("created", format!("started “{}”", self.title)))
    }

    /// Change the story's settings. `None` leaves a field as it is.
    pub fn update(&mut self, site: &Site, u: &StoryUpdate) -> Result<()> {
        if let Some(t) = &u.title {
            if t.trim().is_empty() {
                bail!("a story needs a title");
            }
            self.title = t.trim().into();
        }
        let target = u.target.clone().unwrap_or_else(|| self.target.clone());
        let project = match &u.project {
            Some(p) if p.is_empty() => None,
            Some(p) => Some(p.clone()),
            None => self.project.clone(),
        };
        check_target(site, &target, project.as_deref())?;
        self.target = target;
        self.project = project;
        if let Some(a) = &u.areas {
            self.areas = check_areas(site, a)?;
        }
        if let Some(t) = &u.tags {
            self.tags = t.iter().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect();
        }
        if let Some(n) = &u.notes {
            self.notes = n.clone();
        }
        if let Some(s) = &u.status {
            if !STATUSES.contains(&s.as_str()) {
                bail!("status must be one of {}", STATUSES.join(", "));
            }
            self.status = s.clone();
        }
        Ok(())
    }

    pub fn next_id(&self) -> String {
        let n = self.images.iter().filter_map(|i| i.id.strip_prefix("img")?.parse::<u32>().ok()).max().unwrap_or(0);
        format!("img{}", n + 1)
    }

    pub fn image(&self, id: &str) -> Result<&StoryImage> {
        self.images.iter().find(|i| i.id == id).with_context(|| format!("no image {id} in story {}", self.slug))
    }

    pub fn image_mut(&mut self, id: &str) -> Result<&mut StoryImage> {
        let slug = self.slug.clone();
        self.images.iter_mut().find(|i| i.id == id).with_context(|| format!("no image {id} in story {slug}"))
    }

    /// The SmugMug image already in this story, if any.
    pub fn find_source(&self, source: &str) -> Option<&StoryImage> {
        self.images.iter().find(|i| i.source == source)
    }

    /// Copy a photo from disk into the story (`images/`), reading its size and date. HEIC is
    /// converted to JPEG with macOS `sips`.
    pub fn add_file(&mut self, site: &Site, src: &Path, caption: &str, notes: &str) -> Result<StoryImage> {
        if !src.is_file() {
            bail!("{} is not a file", src.display());
        }
        let name = src.file_name().context("no file name")?.to_string_lossy().to_string();
        let bytes = std::fs::read(src).with_context(|| format!("reading {}", src.display()))?;
        self.add_bytes(site, &name, &bytes, caption, notes)
    }

    /// Add an image from its bytes (an upload from the app).
    pub fn add_bytes(&mut self, site: &Site, name: &str, bytes: &[u8], caption: &str, notes: &str) -> Result<StoryImage> {
        let dir = self.dir(site).join("images");
        std::fs::create_dir_all(&dir)?;
        let stem = slugify(Path::new(name).file_stem().map(|s| s.to_string_lossy().to_string()).as_deref().unwrap_or("image"));
        let stem = if stem.is_empty() { "image".to_string() } else { stem };
        let ext = Path::new(name).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let is_heic = matches!(ext.as_str(), "heic" | "heif") || is_heic_bytes(bytes);
        let (ext, data) = if is_heic {
            ("jpg".to_string(), heic_to_jpeg(bytes)?)
        } else {
            let fmt = image::guess_format(bytes).map_err(|_| anyhow::anyhow!("{name}: not an image this tool can read (JPEG, PNG, GIF, WebP, or HEIC)"))?;
            let ext = fmt.extensions_str().first().copied().unwrap_or("img").to_string();
            (if ext == "jpeg" { "jpg".into() } else { ext }, bytes.to_vec())
        };
        let mut file = format!("{stem}.{ext}");
        let mut i = 2;
        while dir.join(&file).exists() {
            file = format!("{stem}-{i}.{ext}");
            i += 1;
        }
        let path = dir.join(&file);
        std::fs::write(&path, &data)?;
        let info = match read_info(&path) {
            Ok(i) => i,
            Err(e) => {
                let _ = std::fs::remove_file(&path);
                return Err(e.context(format!("{name}: could not read the image")));
            }
        };
        let im = StoryImage {
            id: self.next_id(),
            source: format!("file:images/{file}"),
            caption: caption.into(),
            notes: notes.into(),
            taken: info.taken,
            width: Some(info.width),
            height: Some(info.height),
            include: true,
            order: 0,
            filename: name.into(),
            remote: None,
            state: "selected".into(),
            number: self.next_number(),
            origin: String::new(),
        };
        self.images.push(im.clone());
        Ok(im)
    }

    /// Copy a photo into `images/` and return the stored name (HEIC becomes JPEG).
    fn import_original(&self, site: &Site, name: &str, bytes: &[u8]) -> Result<String> {
        let dir = self.dir(site).join("images");
        std::fs::create_dir_all(&dir)?;
        let stem = slugify(Path::new(name).file_stem().map(|s| s.to_string_lossy().to_string()).as_deref().unwrap_or("image"));
        let stem = if stem.is_empty() { "image".to_string() } else { stem };
        let ext = Path::new(name).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let (ext, data) = if matches!(ext.as_str(), "heic" | "heif") || is_heic_bytes(bytes) {
            ("jpg".to_string(), heic_to_jpeg(bytes)?)
        } else {
            let fmt = image::guess_format(bytes).map_err(|_| anyhow::anyhow!("{name}: not an image this tool can read (JPEG, PNG, GIF, WebP, TIFF, or HEIC)"))?;
            let ext = fmt.extensions_str().first().copied().unwrap_or("img").to_string();
            (if ext == "jpeg" { "jpg".into() } else { ext }, bytes.to_vec())
        };
        let mut file = format!("{stem}.{ext}");
        let mut i = 2;
        while dir.join(&file).exists() {
            file = format!("{stem}-{i}.{ext}");
            i += 1;
        }
        std::fs::write(dir.join(&file), &data)?;
        Ok(file)
    }

    /// Add a hosted image (SmugMug, media library, or URL) whose details are already known.
    /// It is selected unless `im.state` says otherwise.
    pub fn add_remote(&mut self, mut im: StoryImage) -> StoryImage {
        im.id = self.next_id();
        if im.number == 0 {
            im.number = self.next_number();
        }
        if !STATES.contains(&im.state.as_str()) {
            im.state = "selected".into();
        }
        self.images.push(im.clone());
        im
    }

    /// Offer a photo from disk as a candidate, referenced where it is (nothing is copied until
    /// it is selected). Returns `None` if it is already in the story.
    pub fn add_path_candidate(&mut self, path: &Path, taken: Option<String>) -> Option<StoryImage> {
        let source = format!("path:{}", path.display());
        if self.find_source(&source).is_some() || self.images.iter().any(|i| i.origin == path.display().to_string()) {
            return None;
        }
        let im = StoryImage {
            id: self.next_id(),
            source,
            taken,
            include: true,
            filename: path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            state: "candidate".into(),
            number: self.next_number(),
            origin: path.display().to_string(),
            ..Default::default()
        };
        self.images.push(im.clone());
        Some(im)
    }

    /// Resolve `#7`, `7`, or `img3` to an image id.
    pub fn resolve(&self, r: &str) -> Result<String> {
        let r = r.trim();
        let n = r.trim_start_matches('#');
        if let Ok(num) = n.parse::<u32>() {
            return self.images.iter().find(|i| i.number == num).map(|i| i.id.clone()).with_context(|| format!("no image #{num} in story {}", self.slug));
        }
        Ok(self.image(r)?.id.clone())
    }

    /// Resolve a list like `3,7-9,12` (or `img3 img4`, or `all`) to image ids, in the order given.
    pub fn resolve_many(&self, refs: &str) -> Result<Vec<String>> {
        let mut out: Vec<String> = vec![];
        for part in refs.split([',', ' ']).map(|p| p.trim()).filter(|p| !p.is_empty()) {
            let ids: Vec<String> = if part == "all" {
                self.images.iter().map(|i| i.id.clone()).collect()
            } else if let Some((a, b)) = part.trim_start_matches('#').split_once('-').filter(|(a, b)| a.parse::<u32>().is_ok() && b.trim_start_matches('#').parse::<u32>().is_ok()) {
                let (a, b): (u32, u32) = (a.parse()?, b.trim_start_matches('#').parse()?);
                let (lo, hi) = (a.min(b), a.max(b));
                if hi - lo > 10_000 {
                    bail!("range {part} is too large");
                }
                (lo..=hi).map(|n| self.resolve(&n.to_string())).collect::<Result<_>>()?
            } else {
                vec![self.resolve(part)?]
            };
            for id in ids {
                if !out.contains(&id) {
                    out.push(id);
                }
            }
        }
        if out.is_empty() {
            bail!("no images given (use numbers like 3,7-9,12 or ids like img3)");
        }
        Ok(out)
    }

    /// Select images for the story. A candidate still on disk is copied into `images/` now.
    /// With a comment, every image gets it; without one, images without a comment are left
    /// waiting for one (see [`StoryImage::needs_comment`]).
    pub fn select(&mut self, site: &Site, ids: &[String], comment: Option<&str>) -> Result<()> {
        let comment = comment.map(|c| c.trim()).filter(|c| !c.is_empty());
        for id in ids {
            let im = self.image(id)?.clone();
            if let Some(p) = im.source.strip_prefix("path:") {
                let src = PathBuf::from(p);
                let bytes = std::fs::read(&src).with_context(|| format!("#{}: reading {}", im.number, src.display()))?;
                let file = self.import_original(site, &im.filename, &bytes)?;
                let dst = self.dir(site).join("images").join(&file);
                let info = read_info(&dst).ok();
                let m = self.image_mut(id)?;
                m.source = format!("file:images/{file}");
                if let Some(info) = info {
                    m.width = Some(info.width);
                    m.height = Some(info.height);
                    if m.taken.is_none() {
                        m.taken = info.taken;
                    }
                }
            }
            let m = self.image_mut(id)?;
            m.state = "selected".into();
            m.include = true;
            if let Some(c) = comment {
                m.notes = c.to_string();
            }
        }
        Ok(())
    }

    /// Set an image's state back to candidate, or to rejected.
    pub fn set_state(&mut self, ids: &[String], state: &str) -> Result<()> {
        if !STATES.contains(&state) {
            bail!("state must be one of {}", STATES.join(", "));
        }
        for id in ids {
            self.image_mut(id)?.state = state.into();
        }
        Ok(())
    }

    /// Selected images still waiting for a comment.
    pub fn needing_comments(&self) -> Vec<&StoryImage> {
        self.images.iter().filter(|i| i.needs_comment()).collect()
    }

    /// Set the narrative. `base` is the narrative revision the editor started from; if someone
    /// else changed it since, this fails with a `conflict:` error instead of overwriting.
    pub fn set_narrative(&mut self, text: &str, append: bool, base: Option<u64>) -> Result<()> {
        if let Some(b) = base {
            if b != self.narrative_rev {
                bail!("conflict: the story text changed since you started editing (yours is based on revision {b}, it is now {})", self.narrative_rev);
            }
        }
        if append {
            let t = text.trim();
            if t.is_empty() {
                return Ok(());
            }
            self.narrative = if self.narrative.trim().is_empty() { t.to_string() } else { format!("{}\n\n{}", self.narrative.trim_end(), t) };
        } else {
            self.narrative = text.to_string();
        }
        self.narrative_rev += 1;
        Ok(())
    }

    /// Record a question Claude is waiting on; returns its id.
    pub fn ask(&mut self, q: &str) -> Result<u32> {
        let q = q.trim();
        if q.is_empty() {
            bail!("the question is empty");
        }
        // Asking again (a retry, a resumed session) reuses the question already waiting.
        if let Some(p) = self.pending.iter().find(|p| p.q == q) {
            return Ok(p.id);
        }
        self.asked = self.asked.max(self.pending.iter().map(|p| p.id).max().unwrap_or(0)) + 1;
        self.pending.push(PendingQuestion { id: self.asked, q: q.into(), asked: Local::now().naive_local().with_nanosecond_zero() });
        if self.status == "collecting" {
            self.status = "interviewing".into();
        }
        Ok(self.asked)
    }

    /// Answer a pending question (by id, or the oldest one) and record it in the transcript.
    pub fn answer(&mut self, site: &Site, id: Option<u32>, a: &str) -> Result<(PendingQuestion, usize)> {
        let pos = match id {
            Some(i) => self.pending.iter().position(|p| p.id == i).with_context(|| format!("no pending question {i}"))?,
            None if !self.pending.is_empty() => 0,
            None => bail!("there is no question waiting for an answer"),
        };
        let p = self.pending[pos].clone();
        let n = self.add_qa(site, &p.q, a)?;
        self.pending.retain(|x| x.id != p.id);
        Ok((p, n))
    }

    pub fn remove_image(&mut self, site: &Site, id: &str) -> Result<()> {
        let im = self.image(id)?.clone();
        self.images.retain(|i| i.id != id);
        if let Some(rel) = im.source.strip_prefix("file:") {
            let p = self.dir(site).join(rel);
            if p.starts_with(self.dir(site)) {
                let _ = std::fs::remove_file(p);
            }
        }
        let _ = std::fs::remove_file(thumb_path(&self.slug, id));
        Ok(())
    }

    /// Put images in the given order; ids not listed keep their relative order at the end.
    pub fn reorder(&mut self, ids: &[String]) -> Result<()> {
        for id in ids {
            self.image(id)?;
        }
        let mut out = vec![];
        for id in ids {
            if let Some(p) = self.images.iter().position(|i| &i.id == id) {
                out.push(self.images.remove(p));
            }
        }
        out.append(&mut self.images);
        self.images = out;
        Ok(())
    }

    pub fn interview_path(&self, site: &Site) -> PathBuf {
        self.dir(site).join("interview.md")
    }
    pub fn outline_path(&self, site: &Site) -> PathBuf {
        self.dir(site).join("outline.md")
    }

    /// Append one question and answer to the interview transcript.
    pub fn add_qa(&mut self, site: &Site, q: &str, a: &str) -> Result<usize> {
        if q.trim().is_empty() || a.trim().is_empty() {
            bail!("both the question and the answer are needed");
        }
        let p = self.interview_path(site);
        let mut text = std::fs::read_to_string(&p).unwrap_or_else(|_| format!("# Interview: {}\n", self.title));
        let n = text.matches("\n## Q").count() + 1;
        text = text.trim_end().to_string();
        text += &format!("\n\n## Q{n}. {}\n\n{}\n", q.trim(), a.trim());
        std::fs::write(&p, text)?;
        self.pending.retain(|p| p.q.trim() != q.trim());
        if self.status == "collecting" {
            self.status = "interviewing".into();
        }
        Ok(n)
    }

    pub fn interview(&self, site: &Site) -> String {
        std::fs::read_to_string(self.interview_path(site)).unwrap_or_default()
    }
    pub fn outline(&self, site: &Site) -> String {
        std::fs::read_to_string(self.outline_path(site)).unwrap_or_default()
    }

    pub fn set_outline(&mut self, site: &Site, text: &str) -> Result<()> {
        std::fs::write(self.outline_path(site), text.trim_end().to_string() + "\n")?;
        if matches!(self.status.as_str(), "collecting" | "interviewing") {
            self.status = "drafting".into();
        }
        Ok(())
    }

    /// Where a local original lives on disk: in the story (`file:`) or where it was found (`path:`).
    pub fn local_path(&self, site: &Site, im: &StoryImage) -> Option<PathBuf> {
        if let Some(p) = im.source.strip_prefix("path:") {
            return Some(PathBuf::from(p));
        }
        im.source.strip_prefix("file:").map(|rel| self.dir(site).join(rel))
    }

    /// The file to look at when reviewing an image: the cached thumbnail if there is one, else
    /// the local original.
    pub fn review_path(&self, site: &Site, im: &StoryImage) -> Option<PathBuf> {
        let t = thumb_path(&self.slug, &im.id);
        if t.exists() {
            return Some(t);
        }
        self.local_path(site, im).filter(|p| p.exists())
    }

    /// Make the review thumbnail for a local original (hosted images are fetched by the caller).
    pub fn thumb_local(&self, site: &Site, im: &StoryImage) -> Result<Option<PathBuf>> {
        let Some(src) = self.local_path(site, im) else { return Ok(None) };
        let dst = thumb_path(&self.slug, &im.id);
        if dst.exists() && modified(&dst) >= modified(&src) {
            return Ok(Some(dst));
        }
        std::fs::create_dir_all(dst.parent().unwrap())?;
        write_jpeg(&load_any(&src)?, THUMB_MAX, &dst, 82)?;
        Ok(Some(dst))
    }
}

pub(crate) trait NoNanos {
    fn with_nanosecond_zero(self) -> Self;
}
impl NoNanos for NaiveDateTime {
    fn with_nanosecond_zero(self) -> Self {
        use chrono::Timelike;
        self.with_nanosecond(0).unwrap_or(self)
    }
}

fn modified(p: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Write a file by renaming a temporary sibling into place.
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_file_name(format!(".{}.tmp-{}-{}", path.file_name().unwrap().to_string_lossy(), std::process::id(),
        Local::now().timestamp_nanos_opt().unwrap_or(0)));
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))
}

/// The story's write lock (an advisory file lock, released when dropped). Every process —
/// the CLI, the app's server, the MCP server — takes it before a read-modify-write.
pub fn lock(site: &Site, slug: &str) -> Result<std::fs::File> {
    if !valid_slug(slug) || !story_dir(site, slug).join("story.yaml").exists() {
        bail!("no story {slug}; see `gw story list`");
    }
    let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(story_dir(site, slug).join(".lock"))?;
    f.lock().context("locking the story")?;
    Ok(f)
}

fn check_target(site: &Site, target: &str, project: Option<&str>) -> Result<()> {
    if !TARGETS.contains(&target) {
        bail!("target must be one of {}", TARGETS.join(", "));
    }
    if target == "update" {
        let p = project.filter(|p| !p.is_empty()).context("an update needs --project <slug>")?;
        if !site.content_dir().join("projects").join(p).join("index.adoc").exists() {
            bail!("no project {p} (content/projects/{p}/)");
        }
    }
    Ok(())
}

fn check_areas(site: &Site, areas: &[String]) -> Result<Vec<String>> {
    let mut out = vec![];
    for a in areas.iter().map(|a| a.trim()).filter(|a| !a.is_empty()) {
        if site.config.resolve_area(a).is_none() {
            bail!("unknown area {a:?}; see `gw areas`");
        }
        out.push(a.to_string());
    }
    Ok(out)
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct NewStory {
    pub title: String,
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub areas: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct StoryUpdate {
    pub title: Option<String>,
    pub target: Option<String>,
    pub project: Option<String>,
    pub areas: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,
    pub notes: Option<String>,
    pub status: Option<String>,
}

// ------------------------------------------------------------------ images

pub struct ImageInfo {
    pub width: u32,
    pub height: u32,
    /// EXIF DateTimeOriginal as `YYYY-MM-DD HH:MM:SS`.
    pub taken: Option<String>,
}

/// Size (after EXIF orientation) and capture date of an image file.
pub fn read_info(path: &Path) -> Result<ImageInfo> {
    let img = load_oriented(path)?;
    Ok(ImageInfo { width: img.width(), height: img.height(), taken: exif_taken(path) })
}

fn exif_taken(path: &Path) -> Option<String> {
    let f = std::fs::File::open(path).ok()?;
    let ex = exif::Reader::new().read_from_container(&mut std::io::BufReader::new(f)).ok()?;
    let field = ex.get_field(exif::Tag::DateTimeOriginal, exif::In::PRIMARY)
        .or_else(|| ex.get_field(exif::Tag::DateTime, exif::In::PRIMARY))?;
    let s = field.display_value().to_string();
    let s = s.trim().trim_matches('"').to_string();
    // "2026:10:05 14:03:22" → "2026-10-05 14:03:22"
    let fixed = if s.len() >= 10 && s.as_bytes()[4] == b':' && s.as_bytes()[7] == b':' {
        format!("{}-{}-{}", &s[0..4], &s[5..7], &s[8..])
    } else {
        s
    };
    (!fixed.is_empty()).then_some(fixed)
}

/// Decode an image and apply its EXIF orientation, so portrait photos stay upright once the
/// metadata is gone.
pub fn load_oriented(path: &Path) -> Result<DynamicImage> {
    let mut dec = ImageReader::open(path)?.with_guessed_format()?.into_decoder()
        .with_context(|| format!("{}: unsupported or damaged image", path.display()))?;
    let orientation = dec.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(dec)?;
    img.apply_orientation(orientation);
    Ok(img)
}

/// Decode any photo this tool accepts: [`load_oriented`], with HEIC/HEIF converted by `sips`.
pub fn load_any(path: &Path) -> Result<DynamicImage> {
    match load_oriented(path) {
        Ok(i) => Ok(i),
        Err(e) => {
            let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            let bytes = std::fs::read(path)?;
            if matches!(ext.as_str(), "heic" | "heif") || is_heic_bytes(&bytes) {
                decode_oriented_bytes(&heic_to_jpeg(&bytes)?)
            } else {
                Err(e)
            }
        }
    }
}

fn decode_oriented_bytes(bytes: &[u8]) -> Result<DynamicImage> {
    let mut dec = ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?.into_decoder()?;
    let orientation = dec.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(dec)?;
    img.apply_orientation(orientation);
    Ok(img)
}

fn fit(img: &DynamicImage, max: u32) -> DynamicImage {
    if img.width().max(img.height()) > max {
        img.resize(max, max, image::imageops::FilterType::Lanczos3)
    } else {
        img.clone()
    }
}

fn write_jpeg(img: &DynamicImage, max: u32, dst: &Path, quality: u8) -> Result<()> {
    let small = fit(img, max).to_rgb8();
    let mut out = std::io::BufWriter::new(std::fs::File::create(dst)?);
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality).encode_image(&small)?;
    Ok(())
}

/// Make the web copy of a local original in `dir`: at most [`WEB_MAX`] px, re-encoded (so no
/// EXIF, no GPS), PNG when the original has transparency or is a PNG, JPEG otherwise. Animated
/// GIFs are copied as they are (GIF carries no EXIF). Returns the file name written.
pub fn web_copy(src: &Path, dir: &Path, stem: &str) -> Result<String> {
    std::fs::create_dir_all(dir)?;
    let bytes = std::fs::read(src)?;
    let fmt = image::guess_format(&bytes).ok();
    if fmt == Some(image::ImageFormat::Gif) {
        let name = format!("{stem}.gif");
        std::fs::write(dir.join(&name), &bytes)?;
        return Ok(name);
    }
    let img = load_oriented(src)?;
    if fmt == Some(image::ImageFormat::Png) || img.color().has_alpha() {
        let name = format!("{stem}.png");
        fit(&img, WEB_MAX).save_with_format(dir.join(&name), image::ImageFormat::Png)?;
        Ok(name)
    } else {
        let name = format!("{stem}.jpg");
        write_jpeg(&img, WEB_MAX, &dir.join(&name), 85)?;
        Ok(name)
    }
}

/// Write a review thumbnail for an image held in memory (fetched from SmugMug or a URL).
pub fn write_thumb_bytes(bytes: &[u8], dst: &Path) -> Result<(u32, u32)> {
    let img = image::load_from_memory(bytes).context("not an image")?;
    std::fs::create_dir_all(dst.parent().unwrap())?;
    write_jpeg(&img, THUMB_MAX, dst, 82)?;
    Ok((img.width(), img.height()))
}

fn is_heic_bytes(b: &[u8]) -> bool {
    b.len() > 12 && &b[4..8] == b"ftyp" && matches!(&b[8..12], b"heic" | b"heix" | b"mif1" | b"msf1" | b"heim" | b"heis" | b"hevc")
}

/// Convert HEIC/HEIF (iPhone photos) to JPEG with macOS `sips`; elsewhere, explain.
fn heic_to_jpeg(bytes: &[u8]) -> Result<Vec<u8>> {
    let tmp = std::env::temp_dir().join(format!("gw-heic-{}-{}", std::process::id(), Local::now().timestamp_nanos_opt().unwrap_or(0)));
    std::fs::create_dir_all(&tmp)?;
    let (src, dst) = (tmp.join("in.heic"), tmp.join("out.jpg"));
    std::fs::write(&src, bytes)?;
    let ok = std::process::Command::new("sips")
        .args(["-s", "format", "jpeg", "-s", "formatOptions", "92"])
        .arg(&src)
        .arg("--out")
        .arg(&dst)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let out = if ok { std::fs::read(&dst).ok() } else { None };
    let _ = std::fs::remove_dir_all(&tmp);
    out.context("HEIC photos need converting to JPEG first (on macOS this uses `sips`, which failed or is missing): export the photo as JPEG and add that")
}

/// A short word for where an image comes from: file, path (on disk where found), smugmug, media, url.
pub fn source_kind(src: &str) -> &str {
    match src.split(':').next().unwrap_or("") {
        "path" => "on disk",
        "file" => "in story",
        k => k,
    }
}

// ------------------------------------------------------------------ finding photos on disk

pub const PHOTO_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp", "heic", "heif", "tif", "tiff", "gif"];
pub const VIDEO_EXTS: &[&str] = &["mov", "mp4", "m4v", "avi", "mkv", "3gp", "mts", "webm"];

#[derive(Debug, Default)]
pub struct Found {
    /// Photos with their capture date (EXIF, else the file's modification time), oldest first.
    pub photos: Vec<(PathBuf, Option<String>)>,
    /// Videos seen (reported, not added: stories are photos).
    pub videos: Vec<PathBuf>,
    /// Photos left out by `since` or `limit`.
    pub filtered: usize,
}

/// Look for photos in a folder (hidden and system files are skipped).
pub fn scan_dir(dir: &Path, recursive: bool, since: Option<chrono::NaiveDate>, limit: Option<usize>) -> Result<Found> {
    if !dir.is_dir() {
        bail!("{} is not a folder", dir.display());
    }
    let mut found = Found::default();
    let walker = walkdir::WalkDir::new(dir).max_depth(if recursive { usize::MAX } else { 1 }).sort_by_file_name();
    let mut photos = vec![];
    for e in walker.into_iter().filter_entry(|e| e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.')) {
        let Ok(e) = e else { continue };
        if !e.file_type().is_file() {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name.starts_with("._") || name.eq_ignore_ascii_case("Thumbs.db") {
            continue;
        }
        let ext = e.path().extension().map(|x| x.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        if VIDEO_EXTS.contains(&ext.as_str()) {
            found.videos.push(e.path().to_path_buf());
        } else if PHOTO_EXTS.contains(&ext.as_str()) {
            let taken = exif_taken(e.path()).or_else(|| {
                let m = modified(e.path())?;
                Some(chrono::DateTime::<Local>::from(m).naive_local().format("%Y-%m-%d %H:%M:%S").to_string())
            });
            photos.push((e.path().to_path_buf(), taken));
        }
    }
    photos.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    let total = photos.len();
    if let Some(d) = since {
        let d = d.format("%Y-%m-%d").to_string();
        photos.retain(|(_, t)| t.as_deref().map(|t| t[..10.min(t.len())] >= *d).unwrap_or(true));
    }
    if let Some(n) = limit {
        photos.truncate(n);
    }
    found.filtered = total - photos.len();
    found.photos = photos;
    Ok(found)
}

// ------------------------------------------------------------------ contact sheets

/// One numbered tile on a contact sheet.
#[derive(Debug, Clone, Default)]
pub struct SheetTile {
    pub number: u32,
    /// First line under the number (file name).
    pub label: String,
    /// Second line (date, or what's wrong).
    pub sub: String,
    pub state: String,
    pub needs_comment: bool,
    /// The thumbnail to draw (None: a blank tile saying so).
    pub thumb: Option<PathBuf>,
}

const SHEET_FONT: &[u8] = include_bytes!("../assets/fonts/DejaVuSansMono-Bold.ttf");
const CELL_W: u32 = 260;
const CELL_H: u32 = 280;
const PIC_W: u32 = 244;
const PIC_H: u32 = 183;
const HEAD_H: u32 = 52;

fn fill(img: &mut image::RgbImage, x: u32, y: u32, w: u32, h: u32, c: [u8; 3]) {
    for yy in y..(y + h).min(img.height()) {
        for xx in x..(x + w).min(img.width()) {
            img.put_pixel(xx, yy, image::Rgb(c));
        }
    }
}

/// Draw text with the bundled DejaVu Sans Mono Bold; returns the width drawn.
fn text(img: &mut image::RgbImage, x: f32, baseline: f32, size: f32, c: [u8; 3], s: &str) -> f32 {
    use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
    let font = FontRef::try_from_slice(SHEET_FONT).expect("bundled font");
    let sf = font.as_scaled(PxScale::from(size));
    let mut pen = x;
    for ch in s.chars() {
        let g = sf.scaled_glyph(ch);
        let adv = sf.h_advance(g.id);
        let g = ab_glyph::Glyph { position: ab_glyph::point(pen, baseline), ..g };
        if let Some(o) = font.outline_glyph(g) {
            let b = o.px_bounds();
            o.draw(|gx, gy, cov| {
                let (px, py) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                if px >= 0 && py >= 0 && (px as u32) < img.width() && (py as u32) < img.height() {
                    let p = img.get_pixel_mut(px as u32, py as u32);
                    for k in 0..3 {
                        p.0[k] = (p.0[k] as f32 * (1.0 - cov) + c[k] as f32 * cov).round() as u8;
                    }
                }
            });
        }
        pen += adv;
    }
    pen - x
}

/// Shorten to at most `n` characters, keeping the end (file names differ at the end).
fn clip(s: &str, n: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= n {
        s.to_string()
    } else {
        let keep = n.saturating_sub(1);
        format!("…{}", chars[chars.len() - keep..].iter().collect::<String>())
    }
}

/// Render numbered tiles as one JPEG: a grid with the number big under each photo, so the
/// author can say "3, 7-9" in the terminal. Selected photos get a green frame, ones still
/// needing a comment an orange one, rejected ones are faded.
pub fn render_contact_sheet(tiles: &[SheetTile], cols: u32, title: &str, dst: &Path) -> Result<()> {
    let cols = cols.max(1).min(tiles.len().max(1) as u32);
    let rows = (tiles.len() as u32).div_ceil(cols).max(1);
    let (w, h) = (cols * CELL_W + 12, HEAD_H + rows * CELL_H + 8);
    let mut img = image::RgbImage::from_pixel(w, h, image::Rgb([244, 241, 236]));
    text(&mut img, 16.0, 34.0, 22.0, [40, 36, 32], &clip(title, (w as usize - 32) / 13));
    for (i, t) in tiles.iter().enumerate() {
        let (cx, cy) = (8 + (i as u32 % cols) * CELL_W, HEAD_H + (i as u32 / cols) * CELL_H);
        let frame = match (t.state.as_str(), t.needs_comment) {
            ("selected", true) => Some([224, 138, 30]),
            ("selected", false) => Some([46, 157, 87]),
            ("rejected", _) => Some([192, 57, 43]),
            _ => None,
        };
        fill(&mut img, cx, cy, CELL_W - 8, CELL_H - 8, frame.unwrap_or([255, 255, 255]));
        let pad = if frame.is_some() { 4 } else { 0 };
        fill(&mut img, cx + pad, cy + pad, CELL_W - 8 - 2 * pad, CELL_H - 8 - 2 * pad, [255, 255, 255]);
        let (px, py) = (cx + 4 + (CELL_W - 8 - 8 - PIC_W) / 2, cy + 8);
        fill(&mut img, px, py, PIC_W, PIC_H, [232, 228, 222]);
        let pic = t.thumb.as_deref().and_then(|p| image::open(p).ok());
        match pic {
            Some(p) => {
                let small = p.resize(PIC_W, PIC_H, image::imageops::FilterType::Triangle).to_rgb8();
                let (ox, oy) = (px + (PIC_W - small.width()) / 2, py + (PIC_H - small.height()) / 2);
                for (x, y, pxl) in small.enumerate_pixels() {
                    let mut c = pxl.0;
                    if t.state == "rejected" {
                        for k in &mut c {
                            *k = (*k as u16 * 2 / 5 + 255 * 3 / 5) as u8;
                        }
                    }
                    img.put_pixel(ox + x, oy + y, image::Rgb(c));
                }
            }
            None => {
                text(&mut img, px as f32 + 60.0, py as f32 + 96.0, 15.0, [120, 110, 100], "no preview");
            }
        }
        let tx = cx as f32 + 12.0;
        let nw = text(&mut img, tx, (py + PIC_H) as f32 + 30.0, 26.0, [20, 20, 20], &format!("#{}", t.number));
        let tag = match (t.state.as_str(), t.needs_comment) {
            ("selected", true) => ("SELECTED, needs comment", [196, 110, 10]),
            ("selected", false) => ("SELECTED", [36, 130, 70]),
            ("rejected", _) => ("rejected", [170, 50, 40]),
            _ => ("", [0, 0, 0]),
        };
        if !tag.0.is_empty() {
            text(&mut img, tx + nw + 10.0, (py + PIC_H) as f32 + 27.0, 13.0, tag.1, tag.0);
        }
        text(&mut img, tx, (py + PIC_H) as f32 + 50.0, 13.0, [60, 56, 52], &clip(&t.label, 28));
        text(&mut img, tx, (py + PIC_H) as f32 + 67.0, 13.0, [120, 112, 104], &clip(&t.sub, 28));
    }
    if let Some(d) = dst.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut out = std::io::BufWriter::new(std::fs::File::create(dst)?);
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85).encode_image(&img)?;
    Ok(())
}

// ------------------------------------------------------------------ drafting

pub(crate) fn story_ref_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // image::story:img3[…], image:story:img3[…] (inline), video::story:…, and :hero: story:img3
    RE.get_or_init(|| Regex::new(r"(?m)(image::?|video::|^:hero:[ \t]*)story:([A-Za-z0-9_\-]+)").unwrap())
}

/// The ids a draft refers to with `story:<id>`, in order of first use.
pub fn referenced_ids(text: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for c in story_ref_re().captures_iter(text) {
        let id = c[2].to_string();
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

#[derive(Debug, Serialize)]
pub struct DraftResult {
    /// Content file, relative to the site root.
    pub path: String,
    pub id: String,
    pub url: String,
    pub kind: String,
    pub status: String,
    /// Image files written into the bundle (relative to the site root).
    pub files: Vec<String>,
    /// Media-library keys registered for hosted images.
    pub media_keys: Vec<String>,
    pub warnings: Vec<String>,
}

/// Where the story's post lives: a bundle so its images sit beside the text.
fn draft_path(site: &Site, s: &Story, title: &str) -> Result<PathBuf> {
    if let Some(d) = &s.draft {
        let p = site.root.join(d);
        if p.exists() {
            return Ok(p);
        }
    }
    let slug = slugify(title);
    if slug.is_empty() {
        bail!("the draft's title has no usable characters for a slug");
    }
    let c = site.content_dir();
    let p = match s.target.as_str() {
        "project" => c.join("projects").join(&slug).join("index.adoc"),
        "article" => c.join("articles").join(&slug).join("index.adoc"),
        "update" => c.join("projects").join(s.project.as_deref().unwrap_or_default()).join("updates").join(&slug).join("index.adoc"),
        t => bail!("unknown target {t}"),
    };
    let flat = p.parent().unwrap().with_extension("adoc");
    if p.exists() || flat.exists() {
        bail!("{} already exists and was not made from this story; give the draft a different title", p.display());
    }
    Ok(p)
}

/// Make sure the header carries what the site needs: type, date, status, project, areas, tags.
fn complete_header(s: &Story, text: &str, existing_published: bool) -> Result<String> {
    let (title, attrs) = content::parse_header(text);
    if title.is_none() {
        bail!("the draft must start with a `= Title` line");
    }
    let mut add = vec![];
    if !attrs.contains_key("type") {
        add.push(format!(":type: {}", s.target));
    }
    if !attrs.contains_key("date") {
        add.push(format!(":date: {}", Local::now().date_naive()));
    }
    if !attrs.contains_key("status") {
        add.push(format!(":status: {}", if existing_published { "published" } else { "draft" }));
    }
    if s.target == "update" && !attrs.contains_key("project") {
        if let Some(p) = &s.project {
            add.push(format!(":project: {p}"));
        }
    }
    if !attrs.contains_key("areas") && !s.areas.is_empty() {
        add.push(format!(":areas: {}", s.areas.join(", ")));
    }
    if !attrs.contains_key("tags") && !s.tags.is_empty() {
        add.push(format!(":tags: {}", s.tags.join(", ")));
    }
    if add.is_empty() {
        return Ok(text.to_string());
    }
    // Insert right after the title line (before the first blank line).
    let mut out = vec![];
    let mut done = false;
    for line in text.lines() {
        out.push(line.to_string());
        if !done && line.trim_start().starts_with("= ") {
            out.extend(add.iter().cloned());
            done = true;
        }
    }
    Ok(out.join("\n") + "\n")
}

/// Write the story's post from AsciiDoc that refers to images as `story:<id>`. Local photos are
/// copied into the bundle at web size without metadata; SmugMug photos are registered in the
/// media library as `<story>-<id>`; library and URL images are referenced directly.
pub fn draft(site: &Site, s: &mut Story, text: &str) -> Result<DraftResult> {
    let (title, _) = content::parse_header(text);
    let title = title.context("the draft must start with a `= Title` line")?;
    let path = draft_path(site, s, &title)?;
    let dir = path.parent().unwrap().to_path_buf();
    let rel_dir = dir.strip_prefix(&site.root).unwrap_or(&dir).to_path_buf();
    let existing_published = path.exists()
        && content::parse_header(&std::fs::read_to_string(&path).unwrap_or_default()).1.get("status").map(|v| v == "published").unwrap_or(false);

    let mut warnings = vec![];
    let ids = referenced_ids(text);
    let mut lib = site.media()?;
    let mut lib_changed = false;
    let mut resolved: BTreeMap<String, String> = BTreeMap::new();
    let mut files = vec![];
    let mut media_keys = vec![];
    for id in &ids {
        let im = s.image(id).with_context(|| format!("the draft uses story:{id}, which is not an image in this story"))?.clone();
        if !im.include {
            warnings.push(format!("{id} is marked as left out but the draft uses it"));
        }
        if !im.is_selected() {
            warnings.push(format!("{id} (#{}) is a {} — select it (with a comment) before using it", im.number, im.state));
        } else if im.needs_comment() {
            warnings.push(format!("{id} (#{}) has no comment from the author yet", im.number));
        }
        let local = s.local_path(site, &im);
        let target = if let Some(src) = local {
            if !src.exists() {
                bail!("{id}: the original {} is missing", src.display());
            }
            let name = web_copy(&src, &dir, &format!("story-{id}"))?;
            files.push(rel_dir.join(&name).to_string_lossy().to_string());
            name
        } else if let Some(ik) = im.source.strip_prefix("smugmug:") {
            let r = im.remote.clone().with_context(|| format!("{id}: SmugMug image {ik} has no URL recorded; re-add it"))?;
            let key = format!("{}-{id}", s.slug);
            let mut ids_map = BTreeMap::new();
            ids_map.insert("image_key".to_string(), ik.to_string());
            if !r.web.is_empty() {
                ids_map.insert("web_uri".to_string(), r.web.clone());
            }
            if !r.album_key.is_empty() {
                ids_map.insert("album_key".to_string(), r.album_key.clone());
            }
            ids_map.insert("story".to_string(), s.slug.clone());
            let m = Media { url: r.url.clone(), kind: r.kind.clone(), provider: "smugmug".into(), thumb: r.thumb.clone(), alt: String::new(), caption: im.caption.clone(), ids: ids_map };
            if lib.media.get(&key).map(|old| old.url != m.url || old.caption != m.caption).unwrap_or(true) {
                lib.media.insert(key.clone(), m);
                lib_changed = true;
            }
            media_keys.push(key.clone());
            format!("media:{key}")
        } else if let Some(k) = im.source.strip_prefix("media:") {
            if lib.resolve(k).is_none() {
                bail!("{id}: media key {k} is not in the library");
            }
            format!("media:{k}")
        } else if let Some(u) = im.source.strip_prefix("url:") {
            u.to_string()
        } else {
            bail!("{id}: unknown source {}", im.source);
        };
        resolved.insert(id.clone(), target);
    }
    let body = story_ref_re()
        .replace_all(text, |c: &regex::Captures| format!("{}{}", &c[1], resolved.get(&c[2]).cloned().unwrap_or_default()))
        .into_owned();
    let body = complete_header(s, &body, existing_published)?;

    std::fs::create_dir_all(&dir)?;
    let backup = std::fs::read_to_string(&path).ok();
    std::fs::write(&path, &body)?;
    let rel = path.strip_prefix(site.content_dir()).unwrap_or(&path).to_path_buf();
    let item = match content::load_item(&path, &rel) {
        Ok(it) => it,
        Err(e) => {
            match backup {
                Some(b) => std::fs::write(&path, b)?,
                None => {
                    let _ = std::fs::remove_file(&path);
                }
            }
            return Err(e.context("draft not saved"));
        }
    };
    for a in &item.areas {
        if site.config.resolve_area(a).is_none() {
            warnings.push(format!("unknown area {a:?}; see `gw areas`"));
        }
    }
    if item.kind != Kind::Project && item.kind != Kind::Article && item.kind != Kind::Update {
        warnings.push(format!("the draft's :type: is {}, not {}", item.kind.as_str(), s.target));
    }
    if lib_changed {
        lib.save(&site.media_path())?;
    }
    // Remove bundle images an earlier draft wrote that this one no longer uses.
    for old in &s.draft_files {
        if !files.contains(old) {
            let p = site.root.join(old);
            if p.starts_with(&dir) {
                let _ = std::fs::remove_file(p);
            }
        }
    }
    let path_rel = path.strip_prefix(&site.root).unwrap_or(&path).to_string_lossy().to_string();
    s.draft = Some(path_rel.clone());
    s.draft_files = files.clone();
    if s.status != "released" {
        s.status = "drafted".into();
    }
    if existing_published {
        warnings.push("this post was already released; it stays published, so the changes go live at the next publish".into());
    }
    Ok(DraftResult {
        path: path_rel,
        id: item.id,
        url: item.url,
        kind: item.kind.as_str().into(),
        status: if item.draft { "draft".into() } else { "published".into() },
        files,
        media_keys,
        warnings,
    })
}

// ------------------------------------------------------------------ SmugMug references

/// What a SmugMug reference points at, before asking the API.
#[derive(Debug, PartialEq, Eq)]
pub enum SmugRef {
    /// An image: `smugmug:<ImageKey>`, a gallery URL ending `/i-<ImageKey>`, or `/api/v2/image/<key>-0`.
    Image(String),
    /// An album: `album:<AlbumKey>` or `/api/v2/album/<key>`.
    Album(String),
    /// Any other SmugMug page (album, folder, or custom-domain URL) — resolved with the API's
    /// web-URI lookup.
    Page(String),
}

/// Recognise SmugMug references. `None` for things that are not SmugMug (files, other URLs).
pub fn parse_smugmug(s: &str) -> Option<SmugRef> {
    let s = s.trim();
    if let Some(k) = s.strip_prefix("smugmug:") {
        let k = k.trim();
        if let Some(a) = k.strip_prefix("album:") {
            return Some(SmugRef::Album(a.to_string()));
        }
        return parse_smugmug(k).or_else(|| Some(SmugRef::Image(k.to_string())));
    }
    if let Some(a) = s.strip_prefix("album:") {
        return Some(SmugRef::Album(a.trim().to_string()));
    }
    if let Some(rest) = s.strip_prefix("/api/v2/image/") {
        return Some(SmugRef::Image(rest.split(['-', '!', '?']).next().unwrap_or("").to_string()));
    }
    if let Some(rest) = s.strip_prefix("/api/v2/album/") {
        return Some(SmugRef::Album(rest.split(['!', '?', '/']).next().unwrap_or("").to_string()));
    }
    if s.starts_with("http://") || s.starts_with("https://") {
        let lower = s.to_ascii_lowercase();
        // /i-KEY anywhere in the path (gallery lightbox URLs: …/n-xyz/i-AbC12dE/A)
        let path = s.split(['?', '#']).next().unwrap_or(s);
        if let Some(i) = path.rfind("/i-") {
            let key: String = path[i + 3..].chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
            if !key.is_empty() {
                return Some(SmugRef::Image(key));
            }
        }
        if lower.contains("smugmug.com") || lower.contains("smugmug") {
            return Some(SmugRef::Page(s.to_string()));
        }
        return None;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_site() -> (Site, PathBuf) {
        // Unique per test even when tests start in the same microsecond; one shared review cache
        // (tests run in parallel and the cache location is process-wide).
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        static CACHE: OnceLock<()> = OnceLock::new();
        CACHE.get_or_init(|| std::env::set_var("GW_STORY_CACHE", std::env::temp_dir().join(format!("gw-story-test-cache-{}", std::process::id()))));
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("gw-story-test-{}-{n}-{}", std::process::id(), Local::now().timestamp_nanos_opt().unwrap()));
        std::fs::create_dir_all(root.join("content/projects/bench")).unwrap();
        std::fs::write(root.join("content/projects/bench/index.adoc"), "= Bench\n:type: project\n\nBody.\n").unwrap();
        std::fs::write(
            root.join("site.yaml"),
            "site:\n  title: T\n  base_url: https://example.org\n  author: A\nareas:\n  - id: retro\n    title: Retro\n    children:\n      - id: amiga\n        title: Amiga\n",
        )
        .unwrap();
        (Site::open(Some(&root)).unwrap(), root)
    }

    /// A JPEG with an EXIF block carrying a date, an orientation, and GPS coordinates.
    pub fn jpeg_with_gps(w: u32, h: u32, orientation: u16) -> Vec<u8> {
        let img = image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x * 255 / w) as u8, (y * 255 / h) as u8, 90]));
        let mut jpg = vec![];
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 90).encode_image(&img).unwrap();
        // TIFF (big-endian): IFD0 {Orientation, ExifIFD, GPSIFD}, Exif IFD {DateTimeOriginal}, GPS IFD {LatRef, Lat}
        let mut t: Vec<u8> = vec![];
        let be16 = |v: u16| v.to_be_bytes().to_vec();
        let be32 = |v: u32| v.to_be_bytes().to_vec();
        t.extend(b"MM\0\x2a");
        t.extend(be32(8));
        // IFD0 at 8: 3 entries → 2 + 36 + 4 = 42 bytes → ends at 50
        let exif_ifd = 50u32;
        // Exif IFD: 1 entry → 2 + 12 + 4 = 18 → ends at 68; date string (20 bytes) at 68..88
        let date_off = 68u32;
        let gps_ifd = 88u32;
        // GPS IFD: 2 entries → 2 + 24 + 4 = 30 → ends at 118; latitude rationals (24 bytes) at 118
        let lat_off = 118u32;
        t.extend(be16(3));
        t.extend(be16(0x0112)); t.extend(be16(3)); t.extend(be32(1)); t.extend(be16(orientation)); t.extend(be16(0));
        t.extend(be16(0x8769)); t.extend(be16(4)); t.extend(be32(1)); t.extend(be32(exif_ifd));
        t.extend(be16(0x8825)); t.extend(be16(4)); t.extend(be32(1)); t.extend(be32(gps_ifd));
        t.extend(be32(0));
        t.extend(be16(1));
        t.extend(be16(0x9003)); t.extend(be16(2)); t.extend(be32(20)); t.extend(be32(date_off));
        t.extend(be32(0));
        t.extend(b"2026:10:05 14:03:22\0");
        t.extend(be16(2));
        t.extend(be16(0x0001)); t.extend(be16(2)); t.extend(be32(2)); t.extend(b"N\0\0\0");
        t.extend(be16(0x0002)); t.extend(be16(5)); t.extend(be32(3)); t.extend(be32(lat_off));
        t.extend(be32(0));
        for (n, d) in [(37u32, 1u32), (25, 1), (1234, 100)] {
            t.extend(be32(n));
            t.extend(be32(d));
        }
        assert_eq!(t.len(), 118 + 24);
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend(&t);
        let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
        out.extend(((app1.len() + 2) as u16).to_be_bytes());
        out.extend(&app1);
        out.extend(&jpg[2..]);
        out
    }

    fn has_gps(path: &Path) -> bool {
        let f = std::fs::File::open(path).unwrap();
        match exif::Reader::new().read_from_container(&mut std::io::BufReader::new(f)) {
            Ok(ex) => ex.get_field(exif::Tag::GPSLatitude, exif::In::PRIMARY).is_some(),
            Err(_) => false,
        }
    }

    #[test]
    fn yaml_round_trip_and_ids() {
        let (site, root) = test_site();
        let mut s = Story::create(&site, &NewStory { title: "Creating the Amiga 40th exhibit".into(), areas: vec!["amiga".into()], tags: vec!["exhibit".into()], ..Default::default() }).unwrap();
        assert_eq!(s.slug, "creating-the-amiga-40th-exhibit");
        assert_eq!(s.target, "project");
        s.add_remote(StoryImage { source: "smugmug:AbC12dE".into(), caption: "Table of A500s".into(), notes: "Borrowed from the club".into(), include: true, remote: Some(Remote { url: "https://photos.smugmug.com/x-X3.jpg".into(), thumb: "https://photos.smugmug.com/x-Th.jpg".into(), web: "https://me.smugmug.com/i-AbC12dE".into(), album_key: "Zz".into(), kind: "image".into() }), ..Default::default() });
        s.add_remote(StoryImage { source: "url:https://example.org/a.jpg".into(), include: false, ..Default::default() });
        s.save(&site).unwrap();
        let back = Story::load(&site, &s.slug).unwrap();
        assert_eq!(back.images.len(), 2);
        assert_eq!(back.images[0].id, "img1");
        assert_eq!(back.images[1].id, "img2");
        assert_eq!(back.images[0].remote, s.images[0].remote);
        assert_eq!(back.images[0].notes, "Borrowed from the club");
        assert!(!back.images[1].include);
        assert_eq!(back.areas, vec!["amiga"]);
        // a second story with the same title gets its own slug
        let s2 = Story::create(&site, &NewStory { title: "Creating the Amiga 40th exhibit".into(), ..Default::default() }).unwrap();
        assert_eq!(s2.slug, "creating-the-amiga-40th-exhibit-2");
        assert!(Story::create(&site, &NewStory { title: "x".into(), areas: vec!["nope".into()], ..Default::default() }).is_err());
        assert!(Story::create(&site, &NewStory { title: "x".into(), target: Some("update".into()), project: Some("nope".into()), ..Default::default() }).is_err());
        assert!(Story::create(&site, &NewStory { title: "x".into(), target: Some("update".into()), project: Some("bench".into()), ..Default::default() }).is_ok());
        assert_eq!(Story::list(&site).unwrap().len(), 3);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reorder_qa_outline() {
        let (site, root) = test_site();
        let mut s = Story::create(&site, &NewStory { title: "Bench build".into(), ..Default::default() }).unwrap();
        for u in ["a", "b", "c"] {
            s.add_remote(StoryImage { source: format!("url:https://example.org/{u}.jpg"), include: true, ..Default::default() });
        }
        s.reorder(&["img3".into(), "img1".into()]).unwrap();
        s.save(&site).unwrap();
        let ids: Vec<_> = s.images.iter().map(|i| (i.id.as_str(), i.order)).collect();
        assert_eq!(ids, vec![("img3", 1), ("img1", 2), ("img2", 3)]);
        assert!(s.reorder(&["img9".into()]).is_err());
        assert_eq!(s.add_qa(&site, "Why?", "Because.").unwrap(), 1);
        assert_eq!(s.add_qa(&site, "When?", "Last spring.").unwrap(), 2);
        assert!(s.interview(&site).contains("## Q2. When?\n\nLast spring."));
        assert_eq!(s.status, "interviewing");
        s.set_outline(&site, "1. Intro").unwrap();
        assert_eq!(s.status, "drafting");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn local_images_lose_gps_keep_orientation_and_shrink() {
        let (site, root) = test_site();
        let mut s = Story::create(&site, &NewStory { title: "Exhibit".into(), ..Default::default() }).unwrap();
        // 3000x1000 landscape pixels, EXIF orientation 6 (rotate 90° clockwise to view) → portrait
        let jpg = jpeg_with_gps(3000, 1000, 6);
        let src = root.join("IMG_0001.JPG");
        std::fs::write(&src, &jpg).unwrap();
        assert!(has_gps(&src), "fixture should carry GPS");
        let im = s.add_file(&site, &src, "The front table", "Set up at 7am").unwrap();
        assert_eq!(im.id, "img1");
        assert_eq!((im.width, im.height), (Some(1000), Some(3000)), "orientation applied");
        assert_eq!(im.taken.as_deref(), Some("2026-10-05 14:03:22"));
        assert!(im.source.starts_with("file:images/img-0001"));
        // review thumbnail
        let t = s.thumb_local(&site, &im).unwrap().unwrap();
        let ti = image::open(&t).unwrap();
        assert_eq!(ti.height(), THUMB_MAX);
        assert!(!has_gps(&t));
        // draft: web copy is ≤2000 px, portrait, and carries no EXIF at all
        let text = "= The Amiga exhibit\n:summary: How it came together.\n\n.The front table\nimage::story:img1[A table of computers]\n";
        let r = draft(&site, &mut s, text).unwrap();
        assert_eq!(r.path, "content/projects/the-amiga-exhibit/index.adoc");
        assert_eq!(r.url, "/projects/the-amiga-exhibit/");
        assert_eq!(r.files, vec!["content/projects/the-amiga-exhibit/story-img1.jpg".to_string()]);
        let out = root.join(&r.files[0]);
        let oi = image::open(&out).unwrap();
        assert_eq!((oi.width(), oi.height()), (667, 2000));
        assert!(!has_gps(&out));
        assert!(exif::Reader::new().read_from_container(&mut std::io::BufReader::new(std::fs::File::open(&out).unwrap())).is_err(), "no EXIF block at all");
        let written = std::fs::read_to_string(root.join(&r.path)).unwrap();
        assert!(written.contains("image::story-img1.jpg[A table of computers]"));
        assert!(written.contains(":type: project"));
        assert!(written.contains(":status: draft"));
        assert_eq!(s.status, "drafted");
        assert_eq!(s.draft.as_deref(), Some("content/projects/the-amiga-exhibit/index.adoc"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn draft_resolves_every_source_and_rejects_unknown_ids() {
        let (site, root) = test_site();
        std::fs::write(root.join("media.yaml"), "media:\n  bench-front:\n    url: https://example.org/b.jpg\n").unwrap();
        let mut s = Story::create(&site, &NewStory { title: "Exhibit".into(), target: Some("article".into()), areas: vec!["retro/amiga".into()], ..Default::default() }).unwrap();
        s.add_remote(StoryImage { source: "smugmug:AbC12dE".into(), caption: "Crowd".into(), include: true, remote: Some(Remote { url: "https://photos.smugmug.com/AbC-X3.jpg".into(), thumb: "https://photos.smugmug.com/AbC-Th.jpg".into(), web: "https://me.smugmug.com/i-AbC12dE".into(), album_key: "Zz".into(), kind: "image".into() }), ..Default::default() });
        s.add_remote(StoryImage { source: "media:bench-front".into(), include: true, ..Default::default() });
        s.add_remote(StoryImage { source: "url:https://example.org/poster.png".into(), include: true, ..Default::default() });
        let bad = draft(&site, &mut s, "= Exhibit day\n\nimage::story:img9[x]\n");
        assert!(bad.unwrap_err().to_string().contains("story:img9"));
        let text = "= Exhibit day\n:hero: story:img1\n\nimage::story:img1[Crowd]\nimage::story:img2[Bench]\nSee image:story:img3[poster,120] inline.\nA story: about stories.\n";
        let r = draft(&site, &mut s, text).unwrap();
        assert_eq!(r.path, "content/articles/exhibit-day/index.adoc");
        assert_eq!(r.media_keys, vec!["exhibit-img1".to_string()]);
        let w = std::fs::read_to_string(root.join(&r.path)).unwrap();
        assert!(w.contains(":hero: media:exhibit-img1"));
        assert!(w.contains("image::media:exhibit-img1[Crowd]"));
        assert!(w.contains("image::media:bench-front[Bench]"));
        assert!(w.contains("image:https://example.org/poster.png[poster,120]"));
        assert!(w.contains("A story: about stories."), "prose is untouched");
        assert!(w.contains(":areas: retro/amiga"));
        assert!(w.contains(":type: article"));
        let lib = site.media().unwrap();
        let m = lib.resolve("exhibit-img1").unwrap();
        assert_eq!(m.provider, "smugmug");
        assert_eq!(m.ids.get("image_key").map(|s| s.as_str()), Some("AbC12dE"));
        // re-drafting keeps the same file even if the title changes
        let r2 = draft(&site, &mut s, "= Exhibit day, revisited\n\nimage::story:img2[Bench]\n").unwrap();
        assert_eq!(r2.path, r.path);
        assert!(site.check().unwrap().is_empty(), "{:?}", site.check().unwrap());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn heic_detection() {
        let mut b = vec![0, 0, 0, 0x18];
        b.extend(b"ftypheic");
        b.extend([0u8; 16]);
        assert!(is_heic_bytes(&b));
        assert!(!is_heic_bytes(b"\xFF\xD8\xFF\xE0 not heic at all"));
    }

    #[test]
    fn smugmug_references() {
        use SmugRef::*;
        assert_eq!(parse_smugmug("smugmug:AbC12dE"), Some(Image("AbC12dE".into())));
        assert_eq!(parse_smugmug("https://someone.smugmug.com/Retro/Amiga-40/n-Ab12Cd/i-XyZ98wV/A"), Some(Image("XyZ98wV".into())));
        assert_eq!(parse_smugmug("https://someone.smugmug.com/Retro/Amiga-40/n-Ab12Cd/i-XyZ98wV"), Some(Image("XyZ98wV".into())));
        assert_eq!(parse_smugmug("https://photos.example.com/Events/i-Q1w2E3r?lb=1"), Some(Image("Q1w2E3r".into())));
        assert_eq!(parse_smugmug("/api/v2/image/AbC12dE-0"), Some(Image("AbC12dE".into())));
        assert_eq!(parse_smugmug("album:Zbcd1"), Some(Album("Zbcd1".into())));
        assert_eq!(parse_smugmug("smugmug:album:Zbcd1"), Some(Album("Zbcd1".into())));
        assert_eq!(parse_smugmug("/api/v2/album/Zbcd1"), Some(Album("Zbcd1".into())));
        assert_eq!(parse_smugmug("https://someone.smugmug.com/Retro/Amiga-40/n-Ab12Cd"), Some(Page("https://someone.smugmug.com/Retro/Amiga-40/n-Ab12Cd".into())));
        assert_eq!(parse_smugmug("https://someone.smugmug.com/Retro"), Some(Page("https://someone.smugmug.com/Retro".into())));
        assert_eq!(parse_smugmug("https://example.org/photo.jpg"), None);
        assert_eq!(parse_smugmug("/Users/me/photo.jpg"), None);
    }

    /// A plain JPEG (no EXIF) of the given size.
    fn plain_jpeg(w: u32, h: u32, shade: u8) -> Vec<u8> {
        let img = image::RgbImage::from_fn(w, h, |x, _| image::Rgb([shade, (x * 255 / w) as u8, 128]));
        let mut out = vec![];
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85).encode_image(&img).unwrap();
        out
    }

    #[test]
    fn old_story_yaml_migrates_to_selected_and_numbered() {
        let (site, root) = test_site();
        let d = root.join("stories/old");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("story.yaml"), "title: Old\nslug: old\ntarget: project\nstatus: collecting\ncreated: 2026-10-01T10:00:00\nupdated: 2026-10-01T10:00:00\nimages:\n- id: img1\n  source: url:https://example.org/a.jpg\n  notes: First\n- id: img2\n  source: url:https://example.org/b.jpg\n").unwrap();
        let s = Story::load(&site, "old").unwrap();
        assert_eq!(s.images.iter().map(|i| (i.number, i.state.as_str())).collect::<Vec<_>>(), vec![(1, "selected"), (2, "selected")]);
        assert!(s.images[1].needs_comment() && !s.images[0].needs_comment());
        assert_eq!((s.rev, s.narrative.as_str()), (0, ""));
        // the first locked change writes it back in the new shape and starts the feed
        let (_, s) = Story::modify(&site, "old", "test", |s| Ok(((), ChangeNote::new("notes", "touched")))).unwrap();
        assert_eq!(s.rev, 1);
        let text = std::fs::read_to_string(d.join("story.yaml")).unwrap();
        assert!(text.contains("state: selected") && text.contains("number: 2"), "{text}");
        assert_eq!(Story::changes_since(&site, "old", 0).unwrap().len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn number_lists_resolve() {
        let (site, root) = test_site();
        let mut s = Story::create(&site, &NewStory { title: "Nums".into(), ..Default::default() }).unwrap();
        for i in 0..12 {
            s.add_remote(StoryImage { source: format!("url:https://example.org/{i}.jpg"), state: "candidate".into(), ..Default::default() });
        }
        let n = |ids: Vec<String>| ids.iter().map(|id| s.image(id).unwrap().number).collect::<Vec<_>>();
        assert_eq!(n(s.resolve_many("3,7-9,12").unwrap()), vec![3, 7, 8, 9, 12]);
        assert_eq!(n(s.resolve_many("#2 #4-#5, 2").unwrap()), vec![2, 4, 5], "duplicates dropped, # optional");
        assert_eq!(n(s.resolve_many("9-7").unwrap()), vec![7, 8, 9], "reversed range");
        assert_eq!(n(s.resolve_many("img1, img2").unwrap()), vec![1, 2]);
        assert_eq!(s.resolve_many("all").unwrap().len(), 12);
        assert!(s.resolve_many("13").unwrap_err().to_string().contains("no image #13"));
        assert!(s.resolve_many(" , ").is_err());
        assert!(s.resolve_many("1-99999").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn candidates_from_a_folder_select_with_comments() {
        let (site, root) = test_site();
        let dir = root.join("photos");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("b.jpg"), plain_jpeg(400, 300, 10)).unwrap();
        std::fs::write(dir.join("a.jpg"), jpeg_with_gps(500, 300, 1)).unwrap();
        std::fs::write(dir.join("sub/c.png"), {
            let mut v = vec![];
            image::DynamicImage::ImageRgb8(image::RgbImage::new(30, 20)).write_to(&mut std::io::Cursor::new(&mut v), image::ImageFormat::Png).unwrap();
            v
        }).unwrap();
        std::fs::write(dir.join("clip.mov"), b"not really").unwrap();
        std::fs::write(dir.join(".hidden.jpg"), plain_jpeg(10, 10, 0)).unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        let f = scan_dir(&dir, false, None, None).unwrap();
        assert_eq!(f.photos.len(), 2, "not recursive, hidden and non-photos skipped");
        assert_eq!(f.videos.len(), 1);
        let f = scan_dir(&dir, true, None, None).unwrap();
        assert_eq!(f.photos.len(), 3);
        assert_eq!(scan_dir(&dir, true, None, Some(1)).unwrap().photos.len(), 1);
        assert_eq!(scan_dir(&dir, true, chrono::NaiveDate::from_ymd_opt(2999, 1, 1), None).unwrap().photos.len(), 0);

        let mut s = Story::create(&site, &NewStory { title: "Folder".into(), ..Default::default() }).unwrap();
        for (p, t) in f.photos.clone() {
            assert!(s.add_path_candidate(&p, t).is_some());
        }
        assert!(s.add_path_candidate(&f.photos[0].0, None).is_none(), "no duplicates");
        assert_eq!(s.images.iter().map(|i| i.state.as_str()).collect::<Vec<_>>(), vec!["candidate"; 3]);
        // the EXIF-dated photo sorts first (2026-10-05) — numbers follow the scan order
        assert_eq!(s.images[0].filename, "a.jpg");
        let ids = s.resolve_many("1,2").unwrap();
        s.select(&site, &ids[..1], Some("Front table, 7am")).unwrap();
        s.select(&site, &ids[1..], None).unwrap();
        assert_eq!(s.needing_comments().iter().map(|i| i.number).collect::<Vec<_>>(), vec![2]);
        let a = s.image(&ids[0]).unwrap().clone();
        assert!(a.source.starts_with("file:images/a"), "copied into the story on selection: {}", a.source);
        assert_eq!(a.origin, dir.join("a.jpg").display().to_string());
        assert!(dir.join("a.jpg").exists(), "the original is untouched");
        assert_eq!((a.width, a.height), (Some(500), Some(300)));
        // a candidate's thumbnail is made from where it lies
        let c = s.images[2].clone();
        assert!(c.source.starts_with("path:"));
        assert!(s.thumb_local(&site, &c).unwrap().unwrap().exists());
        s.set_state(&ids[1..], "rejected").unwrap();
        assert!(s.needing_comments().is_empty());
        assert!(s.set_state(&ids, "maybe").is_err());
        s.save(&site).unwrap();
        // drafting with a candidate warns; the selected one is copied without GPS
        let r = draft(&site, &mut s, &format!("= Folder story\n\nimage::story:{}[Table]\nimage::story:{}[Png]\n", ids[0], c.id)).unwrap();
        assert!(r.warnings.iter().any(|w| w.contains("is a candidate")), "{:?}", r.warnings);
        assert!(!has_gps(&root.join(&r.files[0])));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_writers_lose_nothing() {
        let (site, root) = test_site();
        Story::create(&site, &NewStory { title: "Busy".into(), ..Default::default() }).unwrap();
        let root2 = root.clone();
        let writers: Vec<_> = (0..2).map(|w| {
            let root = root2.clone();
            std::thread::spawn(move || {
                let site = Site::open(Some(&root)).unwrap();
                for i in 0..40 {
                    Story::modify(&site, "busy", if w == 0 { "app" } else { "cli" }, |s| {
                        s.add_remote(StoryImage { source: format!("url:https://example.org/{w}-{i}.jpg"), ..Default::default() });
                        Ok(((), ChangeNote::new("images", format!("{w}-{i}"))))
                    }).unwrap();
                }
            })
        }).collect();
        for t in writers {
            t.join().unwrap();
        }
        let s = Story::load(&site, "busy").unwrap();
        assert_eq!(s.images.len(), 80, "every writer's change survived");
        let mut nums: Vec<u32> = s.images.iter().map(|i| i.number).collect();
        nums.sort();
        nums.dedup();
        assert_eq!(nums.len(), 80, "numbers are unique");
        assert_eq!(s.rev, 81);
        let ch = Story::changes_since(&site, "busy", 0).unwrap();
        assert_eq!(ch.len(), 80);
        assert_eq!(ch.iter().map(|c| c.rev).collect::<Vec<_>>(), (2..=81).collect::<Vec<_>>(), "feed in revision order");
        assert_eq!(ch.iter().filter(|c| c.by == "app").count(), 40);
        assert_eq!(Story::changes_since(&site, "busy", 79).unwrap().len(), 2);
        assert_eq!(Story::current_rev(&site, "busy"), Some(81));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn narrative_conflicts_and_pending_questions() {
        let (site, root) = test_site();
        let mut s = Story::create(&site, &NewStory { title: "Talk".into(), ..Default::default() }).unwrap();
        s.set_narrative("It started with a borrowed hall.", true, None).unwrap();
        s.set_narrative("Then the machines arrived.", true, Some(1)).unwrap();
        assert_eq!(s.narrative, "It started with a borrowed hall.\n\nThen the machines arrived.");
        let e = s.set_narrative("Mine", false, Some(1)).unwrap_err().to_string();
        assert!(e.starts_with("conflict:"), "{e}");
        assert_eq!(s.narrative_rev, 2);
        let q1 = s.ask("How many machines?").unwrap();
        let q2 = s.ask("Who helped?").unwrap();
        assert_eq!((q1, q2), (1, 2));
        assert_eq!(s.status, "interviewing");
        let (p, n) = s.answer(&site, Some(2), "The club, mostly.").unwrap();
        assert_eq!((p.q.as_str(), n), ("Who helped?", 1));
        // answering with the same question text through add_qa clears it too
        s.add_qa(&site, "How many machines?", "Twenty-two.").unwrap();
        assert!(s.pending.is_empty());
        assert!(s.answer(&site, None, "x").is_err());
        assert_eq!(s.ask("Next?").unwrap(), 3, "ids keep counting");
        assert_eq!(s.ask("Next?").unwrap(), 3, "the same question waiting is reused");
        assert_eq!(s.pending.len(), 1);
        assert!(s.interview(&site).contains("## Q2. How many machines?\n\nTwenty-two."));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn contact_sheet_renders_a_numbered_grid() {
        let (_site, root) = test_site();
        let t = root.join("t.jpg");
        std::fs::write(&t, plain_jpeg(800, 600, 200)).unwrap();
        let tiles: Vec<SheetTile> = (1..=8).map(|n| SheetTile {
            number: n, label: format!("IMG_{n:04}.JPG"), sub: "2026-10-05 14:03".into(),
            state: ["candidate", "selected", "rejected"][n as usize % 3].into(), needs_comment: n == 2,
            thumb: if n == 5 { None } else { Some(t.clone()) },
        }).collect();
        let out = root.join("sheet.jpg");
        render_contact_sheet(&tiles, 6, "Test — candidates", &out).unwrap();
        let img = image::open(&out).unwrap();
        assert_eq!((img.width(), img.height()), (6 * CELL_W + 12, HEAD_H + 2 * CELL_H + 8));
        assert_eq!(clip("abcdefghij", 5), "…ghij");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn referenced_ids_in_order() {
        assert_eq!(referenced_ids(":hero: story:img2\n\nimage::story:img1[]\nimage::story:img2[]\nstory:img7 in prose\n"), vec!["img2", "img1"]);
    }
}
