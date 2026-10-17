//! REST API, management UI, and live preview (`gw serve`).
//!
//! - `/api/*`   JSON endpoints, one per operation in [`crate::ops`]
//! - `/app/*`   the management UI (from `ui/` beside the site when present, else embedded)
//! - `/preview/` and site paths: a draft-inclusive build in `<site>/.gw-cache/preview`
//! - `/`        redirects to `/app/` (or serves the preview home inside the UI's iframe)
//!
//! Binds 127.0.0.1 only. A fresh [`Site`] is opened per request so edits on disk are seen.

use crate::{enhance, ops, server_companion as sc, stories};
use anyhow::Result;
use axum::{
    body::Body,
    extract::{Path as AxPath, Query, State},
    http::{header, HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Redirect, Response},
    routing::{get, patch, post},
    Json, Router,
};
use gw_core::build::{build, BuildOptions, BuildReport};
use gw_core::media::Media;
use gw_core::registry::Entry;
use gw_core::Site;
use serde::Deserialize;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, UNIX_EPOCH};
use tower_http::cors::{AllowOrigin, CorsLayer};

const UI_INDEX: &str = include_str!("../../../ui/index.html");
const UI_JS: &str = include_str!("../../../ui/app.js");
const UI_CSS: &str = include_str!("../../../ui/app.css");

#[derive(Clone)]
struct AppState {
    /// The site being managed; `None` until one is chosen on the setup screen, and it can be
    /// switched (`POST /api/sites/use`).
    root: Arc<RwLock<Option<PathBuf>>>,
}

/// The error text the UI looks for to show its setup screen.
const NO_SITE: &str = "no site configured: open an existing site or create one";

impl AppState {
    fn root(&self) -> Result<PathBuf, ApiError> {
        self.root.read().unwrap().clone().ok_or_else(|| ApiError(StatusCode::CONFLICT, NO_SITE.into()))
    }
    fn set_root(&self, p: PathBuf) {
        *self.root.write().unwrap() = Some(p);
    }
    fn site(&self) -> Result<Site, ApiError> {
        Site::open(Some(&self.root()?)).map_err(ApiError::server)
    }
    fn preview_dir(&self) -> Option<PathBuf> {
        self.root().ok().map(|r| r.join(".gw-cache").join("preview"))
    }
    /// `ui/` beside the site (as in a gitwebsite checkout whose site lives in `site/`), for live
    /// UI edits during development. Otherwise the UI built into the binary is served.
    fn ui_dir(&self) -> Option<PathBuf> {
        let d = self.root().ok()?.parent()?.join("ui");
        d.join("index.html").exists().then_some(d)
    }
}

// ------------------------------------------------------------------ errors

struct ApiError(StatusCode, String);

impl ApiError {
    fn server(e: impl std::fmt::Display) -> Self {
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    }
    fn bad(e: impl std::fmt::Display) -> Self {
        ApiError(StatusCode::BAD_REQUEST, e.to_string())
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        let msg = format!("{e:#}");
        let code = if msg.starts_with("conflict:") {
            StatusCode::CONFLICT
        } else if msg.starts_with("no content item") || msg.starts_with("no idea") || msg.starts_with("no registry entry")
            || msg.starts_with("no story") || (msg.starts_with("no image ") && msg.contains(" in story "))
        {
            StatusCode::NOT_FOUND
        } else {
            StatusCode::BAD_REQUEST
        };
        ApiError(code, msg)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

type ApiResult = Result<Json<Value>, ApiError>;

/// Run a blocking operation against a freshly opened site.
async fn with_site<F>(st: &AppState, f: F) -> ApiResult
where
    F: FnOnce(&Site) -> anyhow::Result<Value> + Send + 'static,
{
    let st = st.clone();
    let v = tokio::task::spawn_blocking(move || -> Result<Value, ApiError> {
        let site = st.site()?;
        Ok(f(&site)?)
    })
    .await
    .map_err(ApiError::server)??;
    Ok(Json(v))
}

// ------------------------------------------------------------------ handlers: items

async fn items_list(State(st): State<AppState>, Query(q): Query<ops::ListArgs>) -> ApiResult {
    with_site(&st, move |s| ops::list(s, &q)).await
}

async fn items_create(State(st): State<AppState>, Json(a): Json<ops::NewArgs>) -> ApiResult {
    with_site(&st, move |s| ops::new_item(s, &a)).await
}

async fn items_get(State(st): State<AppState>, AxPath(id): AxPath<String>) -> ApiResult {
    with_site(&st, move |s| ops::show(s, &id)).await
}

#[derive(Deserialize)]
struct SourceBody {
    text: String,
}

#[derive(Deserialize)]
struct StatusBody {
    status: String,
}

/// PUT /api/items/{id}/source — ids contain slashes, so the suffix is parsed here.
async fn items_put(State(st): State<AppState>, AxPath(rest): AxPath<String>, Json(b): Json<SourceBody>) -> ApiResult {
    let id = rest.strip_suffix("/source").ok_or_else(|| ApiError::bad("use PUT /api/items/{id}/source"))?.to_string();
    with_site(&st, move |s| ops::write_source(s, &id, &b.text)).await
}

/// POST /api/items/{id}/status, or POST /api/items/{id}/enhance (start or reopen an
/// enhancement of the item; returns the story).
async fn items_post(State(st): State<AppState>, AxPath(rest): AxPath<String>, body: Option<Json<Value>>) -> ApiResult {
    if let Some(id) = rest.strip_suffix("/enhance") {
        let id = id.to_string();
        return with_site(&st, move |s| enhance::start(s, &id)).await;
    }
    let id = rest.strip_suffix("/status").ok_or_else(|| ApiError::bad("use POST /api/items/{id}/status or /enhance"))?.to_string();
    let b: StatusBody = serde_json::from_value(body.map(|b| b.0).unwrap_or(Value::Null)).map_err(ApiError::bad)?;
    with_site(&st, move |s| ops::set_status(s, &id, &b.status)).await
}

// ------------------------------------------------------------------ handlers: site, build, publish

async fn site_info(State(st): State<AppState>) -> ApiResult {
    with_site(&st, |s| {
        let c = &s.config;
        Ok(json!({
            "title": c.site.title, "tagline": c.site.tagline, "base_url": c.site.base_url,
            "author": c.site.author, "root": s.root.display().to_string(), "nav": c.nav,
            "areas": c.flat_areas().len(), "publish": c.publish, "sources": c.sources,
            "preview": "/preview/",
        }))
    })
    .await
}

// ------------------------------------------------------------------ handlers: sites (setup, switching)

/// The sites in the user config and which one this server is showing.
async fn sites_list(State(st): State<AppState>) -> ApiResult {
    let showing = st.root().ok().map(|p| p.display().to_string());
    let v = tokio::task::spawn_blocking(crate::setup::show).await.map_err(ApiError::server)??;
    Ok(Json(json!({ "showing": showing, "config": v })))
}

#[derive(Deserialize)]
struct SitePathBody {
    path: String,
    #[serde(default)]
    name: Option<String>,
}

/// Switch this server (and the user config's current site) to another site, then build its preview.
fn switch_to(st: &AppState, path: &Path) -> anyhow::Result<Value> {
    let site = Site::open(Some(path))?;
    let root = site.root.clone();
    st.set_root(root.clone());
    initial_preview(&root);
    Ok(json!({ "showing": root.display().to_string(), "title": site.config.site.title }))
}

/// POST /api/sites/use {path} — a site already in the list (by path or name).
async fn sites_use(State(st): State<AppState>, Json(b): Json<SitePathBody>) -> ApiResult {
    let st2 = st.clone();
    let v = tokio::task::spawn_blocking(move || -> anyhow::Result<Value> {
        let r = crate::setup::use_site(&b.path)?;
        switch_to(&st2, Path::new(r["current"].as_str().unwrap_or_default()))
    })
    .await
    .map_err(ApiError::server)??;
    Ok(Json(v))
}

/// POST /api/sites/open {path, name?} — an existing site folder: add it and switch to it.
async fn sites_open(State(st): State<AppState>, Json(b): Json<SitePathBody>) -> ApiResult {
    let st2 = st.clone();
    let v = tokio::task::spawn_blocking(move || -> anyhow::Result<Value> {
        let path = gw_core::config::expand_home(b.path.trim());
        let r = crate::setup::add_site(&path, b.name.as_deref(), true)?;
        switch_to(&st2, Path::new(r["added"].as_str().unwrap_or_default()))
    })
    .await
    .map_err(ApiError::server)??;
    Ok(Json(v))
}

#[derive(Deserialize)]
struct InitBody {
    path: String,
    #[serde(flatten)]
    options: gw_core::init::InitOptions,
    /// Make the new folder a repository with a first commit.
    #[serde(default)]
    repo: bool,
}

/// POST /api/sites/init {path, title, author, base_url?, tagline?, pages_remote?, branch?, repo?}
/// — create a site (as `gw init`) and switch to it.
async fn sites_init(State(st): State<AppState>, Json(b): Json<InitBody>) -> ApiResult {
    let st2 = st.clone();
    let v = tokio::task::spawn_blocking(move || -> anyhow::Result<Value> {
        let dir = gw_core::config::expand_home(b.path.trim());
        let mut r = crate::setup::create(&dir, &b.options, true, b.repo, false)?;
        r["switched"] = switch_to(&st2, Path::new(r["site"].as_str().unwrap_or_default()))?;
        Ok(r)
    })
    .await
    .map_err(ApiError::server)??;
    Ok(Json(v))
}

#[derive(Deserialize)]
struct PublishTargetBody {
    #[serde(default)]
    remote: Option<String>,
    #[serde(default)]
    branch: Option<String>,
    #[serde(default)]
    pages_repo: Option<String>,
}

/// PATCH /api/site/publish {remote?, branch?, pages_repo?} — where `publish` pushes to.
async fn site_publish(State(st): State<AppState>, Json(b): Json<PublishTargetBody>) -> ApiResult {
    with_site(&st, move |s| {
        let path = s.root.join("site.yaml");
        let text = std::fs::read_to_string(&path)?;
        let mut out = text.clone();
        for (k, v) in [("remote", &b.remote), ("branch", &b.branch), ("pages_repo", &b.pages_repo)] {
            if let Some(v) = v {
                out = gw_core::config::set_publish_key(&out, k, v)?;
            }
        }
        gw_core::config::SiteConfig::parse(&out)?;
        if out != text {
            std::fs::write(&path, out)?;
        }
        let c = gw_core::config::SiteConfig::load(&path)?;
        Ok(json!({ "publish": c.publish }))
    })
    .await
}

async fn areas(State(st): State<AppState>) -> ApiResult {
    with_site(&st, ops::areas).await
}

async fn check(State(st): State<AppState>) -> ApiResult {
    with_site(&st, ops::check).await
}

#[derive(Deserialize, Default)]
struct BuildBody {
    #[serde(default)]
    drafts: bool,
}

async fn do_build(State(st): State<AppState>, body: Option<Json<BuildBody>>) -> ApiResult {
    let drafts = body.map(|b| b.drafts).unwrap_or(false);
    let preview = st.preview_dir().unwrap_or_default();
    with_site(&st, move |s| {
        if drafts {
            // The preview build: drafts included, written where /preview/ serves from.
            let r = build_preview_into(s, preview.clone())?;
            Ok(json!({
                "out_dir": r.out_dir.display().to_string(), "pages": r.pages, "items": r.items,
                "files": r.files.len(), "drafts_included": true, "warnings": r.warnings, "preview": "/preview/",
            }))
        } else {
            ops::do_build(s, false)
        }
    })
    .await
}

async fn publish_plan(State(st): State<AppState>) -> ApiResult {
    with_site(&st, ops::publish_plan).await
}

#[derive(Deserialize)]
struct PublishBody {
    message: Option<String>,
    #[serde(default = "yes")]
    push: bool,
    #[serde(default)]
    confirm: bool,
}

fn yes() -> bool {
    true
}

async fn publish(State(st): State<AppState>, Json(b): Json<PublishBody>) -> ApiResult {
    if !b.confirm {
        return Err(ApiError(
            StatusCode::PRECONDITION_REQUIRED,
            "publishing changes the live site: send {\"confirm\": true} after the user has approved".into(),
        ));
    }
    with_site(&st, move |s| ops::publish_site(s, b.message.as_deref(), b.push)).await
}

// ------------------------------------------------------------------ handlers: ideas, registry, media

#[derive(Deserialize)]
struct IdeasQuery {
    status: Option<String>,
}

async fn ideas_list(State(st): State<AppState>, Query(q): Query<IdeasQuery>) -> ApiResult {
    with_site(&st, move |s| ops::ideas_list(s, q.status.as_deref())).await
}

async fn ideas_add(State(st): State<AppState>, Json(a): Json<ops::IdeaArgs>) -> ApiResult {
    with_site(&st, move |s| ops::idea_add(s, &a)).await
}

async fn ideas_update(State(st): State<AppState>, AxPath(id): AxPath<u32>, Json(u): Json<ops::IdeaUpdate>) -> ApiResult {
    with_site(&st, move |s| ops::idea_update(s, id, &u)).await
}

#[derive(Deserialize)]
struct KindQuery {
    kind: Option<String>,
}

async fn registry_list(State(st): State<AppState>, Query(q): Query<KindQuery>) -> ApiResult {
    with_site(&st, move |s| ops::registry_list(s, q.kind.as_deref())).await
}

async fn registry_put(State(st): State<AppState>, Json(e): Json<Entry>) -> ApiResult {
    with_site(&st, move |s| ops::registry_upsert(s, e)).await
}

async fn registry_delete(State(st): State<AppState>, AxPath(id): AxPath<String>) -> ApiResult {
    with_site(&st, move |s| ops::registry_remove(s, &id)).await
}

async fn media_list(State(st): State<AppState>) -> ApiResult {
    with_site(&st, ops::media_list).await
}

#[derive(Deserialize)]
struct MediaBody {
    key: String,
    #[serde(flatten)]
    media: Media,
}

async fn media_add(State(st): State<AppState>, Json(b): Json<MediaBody>) -> ApiResult {
    with_site(&st, move |s| ops::media_add(s, &b.key, b.media)).await
}

async fn media_usage(State(st): State<AppState>) -> ApiResult {
    with_site(&st, ops::media_usage).await
}

async fn media_refresh(State(st): State<AppState>, AxPath(key): AxPath<String>) -> ApiResult {
    with_site(&st, move |s| crate::integrations::smugmug::run(s, crate::integrations::SmugmugCmd::Refresh { key })).await
}

async fn smugmug_album(State(st): State<AppState>) -> ApiResult {
    with_site(&st, crate::integrations::smugmug::site_album).await
}

// ------------------------------------------------------------------ handlers: stories

async fn stories_list(State(st): State<AppState>) -> ApiResult {
    with_site(&st, stories::list).await
}

async fn stories_create(State(st): State<AppState>, Json(n): Json<gw_core::story::NewStory>) -> ApiResult {
    with_site(&st, move |s| stories::create(s, &n)).await
}

#[derive(Deserialize, Default)]
struct SinceQuery {
    since: Option<u64>,
}

/// GET /api/stories/{slug}?since=<rev> — `{unchanged: true}` while the story is still at `rev`.
async fn story_get(State(st): State<AppState>, AxPath(slug): AxPath<String>, Query(q): Query<SinceQuery>) -> ApiResult {
    with_site(&st, move |s| stories::show_since(s, &slug, q.since)).await
}

async fn story_add_dir(State(st): State<AppState>, AxPath(slug): AxPath<String>, Json(a): Json<stories::AddDirArgs>) -> ApiResult {
    with_site(&st, move |s| stories::add_dir(s, &slug, &a)).await
}
async fn story_add_photos(State(st): State<AppState>, AxPath(slug): AxPath<String>, Json(a): Json<stories::AddPhotosArgs>) -> ApiResult {
    with_site(&st, move |s| stories::add_photos(s, &slug, &a)).await
}
async fn photos_albums(State(st): State<AppState>) -> ApiResult {
    with_site(&st, |_| stories::photos_albums()).await
}

#[derive(Deserialize)]
struct SheetQuery {
    #[serde(default)]
    which: Option<String>,
    page: Option<usize>,
    per_page: Option<usize>,
}

async fn story_sheet(State(st): State<AppState>, AxPath(slug): AxPath<String>, Query(q): Query<SheetQuery>) -> ApiResult {
    with_site(&st, move |s| stories::contact_sheet(s, &slug, q.which.as_deref().unwrap_or("candidates"), q.page.unwrap_or(1), q.per_page.unwrap_or(24))).await
}

/// The contact sheet itself, as a JPEG (rendered on request).
async fn story_sheet_jpg(State(st): State<AppState>, AxPath(slug): AxPath<String>, Query(q): Query<SheetQuery>) -> Response {
    let st2 = st.clone();
    let r = tokio::task::spawn_blocking(move || -> anyhow::Result<PathBuf> {
        let site = st2.site().map_err(|e| anyhow::anyhow!(e.1))?;
        let v = stories::contact_sheet(&site, &slug, q.which.as_deref().unwrap_or("candidates"), q.page.unwrap_or(1), q.per_page.unwrap_or(24))?;
        Ok(PathBuf::from(v["path"].as_str().unwrap_or("")))
    }).await;
    match r {
        Ok(Ok(p)) => file_response(&p).unwrap_or_else(|| StatusCode::NOT_FOUND.into_response()),
        Ok(Err(e)) => ApiError::from(e).into_response(),
        Err(e) => ApiError::server(e).into_response(),
    }
}

#[derive(Deserialize)]
struct RefsBody {
    refs: String,
    #[serde(default)]
    comment: Option<String>,
}

async fn story_select(State(st): State<AppState>, AxPath(slug): AxPath<String>, Json(b): Json<RefsBody>) -> ApiResult {
    with_site(&st, move |s| stories::select(s, &slug, &b.refs, b.comment.as_deref())).await
}

async fn story_unselect(State(st): State<AppState>, AxPath(slug): AxPath<String>, Json(b): Json<RefsBody>) -> ApiResult {
    with_site(&st, move |s| stories::set_state(s, &slug, &b.refs, "candidate")).await
}

async fn story_reject(State(st): State<AppState>, AxPath(slug): AxPath<String>, Json(b): Json<RefsBody>) -> ApiResult {
    with_site(&st, move |s| stories::set_state(s, &slug, &b.refs, "rejected")).await
}

#[derive(Deserialize)]
struct CommentBody {
    #[serde(rename = "ref")]
    r: String,
    text: String,
}

async fn story_comment(State(st): State<AppState>, AxPath(slug): AxPath<String>, Json(b): Json<CommentBody>) -> ApiResult {
    with_site(&st, move |s| stories::comment(s, &slug, &b.r, &b.text)).await
}

#[derive(Deserialize)]
struct NarrativeBody {
    text: String,
    #[serde(default)]
    append: bool,
    /// The narrative revision the editor started from (a mismatch is a 409 conflict).
    #[serde(default)]
    base: Option<u64>,
}

async fn story_narrative(State(st): State<AppState>, AxPath(slug): AxPath<String>, Json(b): Json<NarrativeBody>) -> ApiResult {
    with_site(&st, move |s| stories::narrative(s, &slug, &b.text, b.append, b.base)).await
}

#[derive(Deserialize)]
struct AskBody {
    q: String,
}

async fn story_ask(State(st): State<AppState>, AxPath(slug): AxPath<String>, Json(b): Json<AskBody>) -> ApiResult {
    with_site(&st, move |s| stories::ask(s, &slug, &b.q)).await
}

#[derive(Deserialize)]
struct AnswerBody {
    #[serde(default)]
    id: Option<u32>,
    a: String,
}

async fn story_answer(State(st): State<AppState>, AxPath(slug): AxPath<String>, Json(b): Json<AnswerBody>) -> ApiResult {
    with_site(&st, move |s| {
        let mut v = stories::answer(s, &slug, b.id, &b.a)?;
        // An enhancement whose Claude session stopped while waiting: start it again.
        if let Some(r) = enhance::resume_if_stopped(s, &slug)? {
            v["resumed"] = r;
        }
        Ok(v)
    })
    .await
}

async fn story_pending(State(st): State<AppState>, AxPath(slug): AxPath<String>) -> ApiResult {
    with_site(&st, move |s| stories::pending(s, &slug)).await
}

async fn story_changes(State(st): State<AppState>, AxPath(slug): AxPath<String>, Query(q): Query<SinceQuery>) -> ApiResult {
    with_site(&st, move |s| stories::changes(s, &slug, q.since.unwrap_or(0))).await
}

/// GET /api/stories/{slug}/events — Server-Sent Events: a `change` event (JSON: rev, changes,
/// pending) whenever the story changes, from anywhere (CLI, Claude, another window).
async fn story_events(State(st): State<AppState>, AxPath(slug): AxPath<String>, Query(q): Query<SinceQuery>) -> Response {
    use axum::response::sse::{Event, KeepAlive, Sse};
    struct Poll {
        root: PathBuf,
        slug: String,
        last: Option<u64>,
        started: bool,
    }
    let root = match st.root() {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };
    let init = Poll { root, slug, last: q.since, started: false };
    let stream = futures_util::stream::unfold(init, |mut p| async move {
        loop {
            if p.started {
                tokio::time::sleep(std::time::Duration::from_millis(700)).await;
            }
            p.started = true;
            let (root, slug, last) = (p.root.clone(), p.slug.clone(), p.last);
            let r = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<(u64, Value)>> {
                let site = Site::open(Some(&root))?;
                let rev = match gw_core::story::Story::current_rev(&site, &slug) {
                    Some(r) => r,
                    None => {
                        gw_core::story::Story::load(&site, &slug)?;
                        return Ok(None);
                    }
                };
                match last {
                    Some(l) if l == rev => Ok(None),
                    Some(l) => Ok(Some((rev, stories::changes(&site, &slug, l)?))),
                    None => Ok(Some((rev, json!({ "rev": rev, "changes": [], "hello": true })))),
                }
            }).await;
            match r {
                Ok(Ok(Some((rev, v)))) => {
                    p.last = Some(rev);
                    let ev = Event::default().event("change").data(v.to_string());
                    return Some((Ok::<_, std::convert::Infallible>(ev), p));
                }
                Ok(Ok(None)) => continue,
                // The story was deleted (or the site is unreadable): say so and stop.
                _ => {
                    let ev = Event::default().event("gone").data("{}");
                    return if p.last == Some(u64::MAX) { None } else { p.last = Some(u64::MAX); Some((Ok(ev), p)) };
                }
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}

async fn story_patch(State(st): State<AppState>, AxPath(slug): AxPath<String>, Json(u): Json<gw_core::story::StoryUpdate>) -> ApiResult {
    with_site(&st, move |s| stories::update(s, &slug, &u)).await
}

async fn story_delete(State(st): State<AppState>, AxPath(slug): AxPath<String>) -> ApiResult {
    with_site(&st, move |s| stories::delete(s, &slug)).await
}

async fn story_add(State(st): State<AppState>, AxPath(slug): AxPath<String>, Json(a): Json<stories::AddArgs>) -> ApiResult {
    with_site(&st, move |s| stories::add(s, &slug, &a)).await
}

#[derive(Deserialize)]
struct UploadQuery {
    name: String,
    #[serde(default)]
    caption: String,
    #[serde(default)]
    comment: String,
}

/// POST /api/stories/{slug}/upload?name=IMG_1.JPG — the request body is the file itself.
async fn story_upload(State(st): State<AppState>, AxPath(slug): AxPath<String>, Query(q): Query<UploadQuery>, body: axum::body::Bytes) -> ApiResult {
    with_site(&st, move |s| stories::upload(s, &slug, &q.name, &body, &q.caption, &q.comment)).await
}

async fn story_image_patch(State(st): State<AppState>, AxPath((slug, id)): AxPath<(String, String)>, Json(u): Json<stories::ImageUpdate>) -> ApiResult {
    with_site(&st, move |s| stories::image_update(s, &slug, &id, &u)).await
}

async fn story_image_delete(State(st): State<AppState>, AxPath((slug, id)): AxPath<(String, String)>) -> ApiResult {
    with_site(&st, move |s| stories::image_remove(s, &slug, &id)).await
}

async fn story_image_thumb(State(st): State<AppState>, AxPath((slug, id)): AxPath<(String, String)>) -> Response {
    let st2 = st.clone();
    let r = tokio::task::spawn_blocking(move || -> anyhow::Result<PathBuf> {
        let site = st2.site().map_err(|e| anyhow::anyhow!(e.1))?;
        stories::thumb_file(&site, &slug, &id)
    })
    .await;
    match r {
        Ok(Ok(p)) => file_response(&p).unwrap_or_else(|| StatusCode::NOT_FOUND.into_response()),
        Ok(Err(e)) => ApiError::from(e).into_response(),
        Err(e) => ApiError::server(e).into_response(),
    }
}

#[derive(Deserialize)]
struct OrderBody {
    ids: Vec<String>,
}

async fn story_order(State(st): State<AppState>, AxPath(slug): AxPath<String>, Json(b): Json<OrderBody>) -> ApiResult {
    with_site(&st, move |s| stories::reorder(s, &slug, &b.ids)).await
}

#[derive(Deserialize)]
struct NoteBody {
    text: String,
    #[serde(default)]
    replace: bool,
}

async fn story_notes(State(st): State<AppState>, AxPath(slug): AxPath<String>, Json(b): Json<NoteBody>) -> ApiResult {
    with_site(&st, move |s| stories::note(s, &slug, &b.text, b.replace)).await
}

#[derive(Deserialize)]
struct QaBody {
    q: String,
    a: String,
}

async fn story_qa(State(st): State<AppState>, AxPath(slug): AxPath<String>, Json(b): Json<QaBody>) -> ApiResult {
    with_site(&st, move |s| stories::qa(s, &slug, &b.q, &b.a)).await
}

async fn story_outline(State(st): State<AppState>, AxPath(slug): AxPath<String>, Json(b): Json<SourceBody>) -> ApiResult {
    with_site(&st, move |s| stories::outline(s, &slug, &b.text)).await
}

#[derive(Deserialize)]
struct DraftBody {
    text: String,
    #[serde(default = "yes")]
    preview: bool,
}

async fn story_draft(State(st): State<AppState>, AxPath(slug): AxPath<String>, Json(b): Json<DraftBody>) -> ApiResult {
    with_site(&st, move |s| stories::draft(s, &slug, &b.text, b.preview)).await
}

async fn story_review(State(st): State<AppState>, AxPath(slug): AxPath<String>) -> ApiResult {
    with_site(&st, move |s| stories::review_images(s, &slug)).await
}

#[derive(Deserialize)]
struct PickBody {
    picks: Vec<stories::Pick>,
}

async fn story_pick(State(st): State<AppState>, AxPath(slug): AxPath<String>, Json(b): Json<PickBody>) -> ApiResult {
    with_site(&st, move |s| stories::smugmug_pick(s, &slug, &b.picks)).await
}

// ------------------------------------------------------------------ handlers: enhancements

async fn story_context(State(st): State<AppState>, AxPath(slug): AxPath<String>) -> ApiResult {
    with_site(&st, move |s| enhance::context(s, &slug)).await
}

#[derive(Deserialize)]
struct KeyAnswersBody {
    answers: std::collections::BTreeMap<String, String>,
}

async fn story_key_answers(State(st): State<AppState>, AxPath(slug): AxPath<String>, Json(b): Json<KeyAnswersBody>) -> ApiResult {
    with_site(&st, move |s| enhance::key_answers(s, &slug, &b.answers)).await
}

#[derive(Deserialize)]
struct RepoBody {
    #[serde(default)]
    url: String,
    #[serde(default)]
    path: Option<String>,
}

async fn story_repo(State(st): State<AppState>, AxPath(slug): AxPath<String>, Json(b): Json<RepoBody>) -> ApiResult {
    with_site(&st, move |s| enhance::set_repo(s, &slug, &b.url, b.path.as_deref())).await
}

async fn story_proposal(State(st): State<AppState>, AxPath(slug): AxPath<String>) -> ApiResult {
    with_site(&st, move |s| enhance::proposal(s, &slug)).await
}

async fn story_accept(State(st): State<AppState>, AxPath(slug): AxPath<String>) -> ApiResult {
    with_site(&st, move |s| enhance::accept(s, &slug)).await
}

async fn story_discard(State(st): State<AppState>, AxPath(slug): AxPath<String>) -> ApiResult {
    with_site(&st, move |s| enhance::discard(s, &slug)).await
}

async fn story_request_changes(State(st): State<AppState>, AxPath(slug): AxPath<String>, Json(b): Json<SourceBody>) -> ApiResult {
    with_site(&st, move |s| {
        let mut v = enhance::request_changes(s, &slug, &b.text)?;
        if let Some(r) = enhance::resume_if_stopped(s, &slug)? {
            v["resumed"] = r;
        }
        Ok(v)
    })
    .await
}

#[derive(Deserialize)]
struct LinesQuery {
    #[serde(default)]
    lines: Option<usize>,
}

async fn story_claude_status(State(st): State<AppState>, AxPath(slug): AxPath<String>, Query(q): Query<LinesQuery>) -> ApiResult {
    with_site(&st, move |s| enhance::claude_status(s, &slug, q.lines.unwrap_or(200))).await
}

async fn story_claude_start(State(st): State<AppState>, AxPath(slug): AxPath<String>) -> ApiResult {
    with_site(&st, move |s| enhance::claude_start(s, &slug)).await
}

async fn story_claude_stop(State(st): State<AppState>, AxPath(slug): AxPath<String>) -> ApiResult {
    with_site(&st, move |s| enhance::claude_stop(s, &slug)).await
}

async fn smugmug_albums(State(st): State<AppState>) -> ApiResult {
    with_site(&st, |_| stories::smugmug_albums()).await
}

#[derive(Deserialize)]
struct BrowseQuery {
    target: String,
    #[serde(default)]
    start: Option<usize>,
    #[serde(default)]
    count: Option<usize>,
}

async fn smugmug_browse(State(st): State<AppState>, Query(q): Query<BrowseQuery>) -> ApiResult {
    with_site(&st, move |_| stories::smugmug_browse(&q.target, q.start.unwrap_or(1), q.count.unwrap_or(100))).await
}

// ------------------------------------------------------------------ UI and preview

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "pdf" => "application/pdf",
        "txt" => "text/plain; charset=utf-8",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

fn file_response(path: &Path) -> Option<Response> {
    let bytes = std::fs::read(path).ok()?;
    Some(
        Response::builder()
            .header(header::CONTENT_TYPE, content_type(path))
            .header(header::CACHE_CONTROL, "no-store")
            .body(Body::from(bytes))
            .unwrap(),
    )
}

async fn ui(State(st): State<AppState>, uri: Uri) -> Response {
    let rel = uri.path().strip_prefix("/app").unwrap_or("").trim_start_matches('/');
    let rel = if rel.is_empty() { "index.html" } else { rel };
    if rel.contains("..") {
        return StatusCode::BAD_REQUEST.into_response();
    }
    if let Some(dir) = st.ui_dir() {
        if let Some(r) = file_response(&dir.join(rel)) {
            return r;
        }
    }
    let (body, ct) = match rel {
        "index.html" => (UI_INDEX, "text/html; charset=utf-8"),
        "app.js" => (UI_JS, "text/javascript; charset=utf-8"),
        "app.css" => (UI_CSS, "text/css; charset=utf-8"),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    ([(header::CONTENT_TYPE, ct), (header::CACHE_CONTROL, "no-store")], body).into_response()
}

/// Serve a path from the preview build (site URLs are root-relative, so the preview's own
/// links — /articles/, /css/site.css — arrive here too).
fn preview_file(st: &AppState, path: &str) -> Response {
    let Some(dir) = st.preview_dir() else {
        return (StatusCode::NOT_FOUND, "no site yet: choose or create one in the app").into_response();
    };
    let rel = path.trim_start_matches('/');
    if rel.contains("..") {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let mut p = dir.join(rel);
    if rel.is_empty() || rel.ends_with('/') || p.is_dir() {
        p = p.join("index.html");
    }
    file_response(&p)
        .or_else(|| file_response(&dir.join("404.html")).map(|mut r| {
            *r.status_mut() = StatusCode::NOT_FOUND;
            r
        }))
        .unwrap_or_else(|| (StatusCode::NOT_FOUND, "not built yet: POST /api/build {\"drafts\": true}").into_response())
}

async fn preview(State(st): State<AppState>, uri: Uri) -> Response {
    let p = uri.path().strip_prefix("/preview").unwrap_or("/");
    preview_file(&st, if p.is_empty() { "/" } else { p })
}

async fn root(State(st): State<AppState>, headers: HeaderMap) -> Response {
    // Inside the UI's preview iframe the site's "home" link should stay in the preview.
    let in_frame = headers.get("sec-fetch-dest").map(|v| v == "iframe").unwrap_or(false);
    let from_preview = headers
        .get(header::REFERER)
        .and_then(|r| r.to_str().ok())
        .map(|r| !r.contains("/app/"))
        .unwrap_or(false);
    if in_frame || from_preview {
        preview_file(&st, "/")
    } else {
        Redirect::temporary("/app/").into_response()
    }
}

async fn fallback(State(st): State<AppState>, method: Method, uri: Uri) -> Response {
    if method != Method::GET || uri.path().starts_with("/api/") {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": format!("no route {} {}", method, uri.path()) }))).into_response();
    }
    preview_file(&st, uri.path())
}

// ------------------------------------------------------------------ native hooks

/// Starts macOS dictation in the focused text field. Installed by the desktop app (the server
/// also runs headless as `gw serve`, where there is no window to dictate into).
static DICTATE: std::sync::OnceLock<Box<dyn Fn() + Send + Sync>> = std::sync::OnceLock::new();

/// Called once by the desktop app; the UI's 🎤 buttons appear when it is set.
pub fn set_dictation_hook(f: impl Fn() + Send + Sync + 'static) {
    let _ = DICTATE.set(Box::new(f));
}

async fn dictate_info() -> Json<Value> {
    Json(json!({ "available": DICTATE.get().is_some() }))
}

async fn dictate() -> Response {
    match DICTATE.get() {
        Some(f) => {
            f();
            Json(json!({ "ok": true })).into_response()
        }
        None => (StatusCode::NOT_FOUND, Json(json!({ "error": "dictation is only available in the gitwebsite app" }))).into_response(),
    }
}

// ------------------------------------------------------------------ server

pub fn router(site_root: Option<PathBuf>) -> Router {
    router_for(AppState { root: Arc::new(RwLock::new(site_root)) })
}

fn router_for(st: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin, _| {
            origin.to_str().map(|o| o.starts_with("http://127.0.0.1") || o.starts_with("http://localhost") || o.starts_with("tauri://")).unwrap_or(false)
        }))
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::PATCH, Method::DELETE])
        .allow_headers([header::CONTENT_TYPE]);
    Router::new()
        .route("/api/site", get(site_info))
        .route("/api/site/publish", patch(site_publish))
        .route("/api/sites", get(sites_list))
        .route("/api/sites/use", post(sites_use))
        .route("/api/sites/open", post(sites_open))
        .route("/api/sites/init", post(sites_init))
        .route("/api/items", get(items_list).post(items_create))
        .route("/api/items/{*id}", get(items_get).put(items_put).post(items_post))
        .route("/api/areas", get(areas))
        .route("/api/check", get(check))
        .route("/api/dictate", get(dictate_info).post(dictate))
        .route("/api/build", post(do_build))
        .route("/api/publish/plan", get(publish_plan))
        .route("/api/publish", post(publish))
        .route("/api/ideas", get(ideas_list).post(ideas_add))
        .route("/api/ideas/{id}", patch(ideas_update))
        .route("/api/registry", get(registry_list).post(registry_put))
        .route("/api/registry/{id}", axum::routing::delete(registry_delete))
        .route("/api/media", get(media_list).post(media_add))
        .route("/api/media/usage", get(media_usage))
        .route("/api/media/{key}/refresh", post(media_refresh))
        .route("/api/smugmug/album", get(smugmug_album))
        .route("/api/stories", get(stories_list).post(stories_create))
        .route("/api/stories/{slug}", get(story_get).patch(story_patch).delete(story_delete))
        .route("/api/stories/{slug}/images", post(story_add))
        .route("/api/stories/{slug}/upload", post(story_upload).layer(axum::extract::DefaultBodyLimit::max(256 * 1024 * 1024)))
        .route("/api/stories/{slug}/images/{id}", patch(story_image_patch).delete(story_image_delete))
        .route("/api/stories/{slug}/images/{id}/thumb", get(story_image_thumb))
        .route("/api/stories/{slug}/order", post(story_order))
        .route("/api/stories/{slug}/notes", post(story_notes))
        .route("/api/stories/{slug}/qa", post(story_qa))
        .route("/api/stories/{slug}/outline", axum::routing::put(story_outline))
        .route("/api/stories/{slug}/draft", post(story_draft))
        .route("/api/stories/{slug}/review-images", post(story_review))
        .route("/api/stories/{slug}/smugmug-pick", post(story_pick))
        .route("/api/stories/{slug}/add-dir", post(story_add_dir))
        .route("/api/stories/{slug}/add-photos", post(story_add_photos))
        .route("/api/stories/{slug}/contact-sheet", get(story_sheet))
        .route("/api/stories/{slug}/contact-sheet.jpg", get(story_sheet_jpg))
        .route("/api/stories/{slug}/select", post(story_select))
        .route("/api/stories/{slug}/unselect", post(story_unselect))
        .route("/api/stories/{slug}/reject", post(story_reject))
        .route("/api/stories/{slug}/comment", post(story_comment))
        .route("/api/stories/{slug}/narrative", axum::routing::put(story_narrative))
        .route("/api/stories/{slug}/ask", post(story_ask))
        .route("/api/stories/{slug}/answer", post(story_answer))
        .route("/api/stories/{slug}/pending", get(story_pending))
        .route("/api/stories/{slug}/changes", get(story_changes))
        .route("/api/stories/{slug}/events", get(story_events))
        .route("/api/stories/{slug}/context", get(story_context))
        .route("/api/stories/{slug}/key-answers", axum::routing::put(story_key_answers))
        .route("/api/stories/{slug}/repo", axum::routing::put(story_repo))
        .route("/api/stories/{slug}/proposal", get(story_proposal))
        .route("/api/stories/{slug}/accept", post(story_accept))
        .route("/api/stories/{slug}/discard", post(story_discard))
        .route("/api/stories/{slug}/request-changes", post(story_request_changes))
        .route("/api/stories/{slug}/claude", get(story_claude_status).post(story_claude_start).delete(story_claude_stop))
        .route("/api/smugmug/albums", get(smugmug_albums))
        .route("/api/photos/albums", get(photos_albums))
        .route("/api/smugmug/browse", get(smugmug_browse))
        .route("/api/tools", get(|State(st): State<AppState>, q: Query<sc::ToolsQuery>| async move { sc::tools_list(st.root().ok(), sc::tools_query(q)).await }))
        .route("/api/tools/{slug}", get(|State(st): State<AppState>, AxPath(slug): AxPath<String>| async move { sc::tool_info(st.root().ok(), slug).await }))
        .route("/api/view", get(sc::view_list).post(|State(st): State<AppState>, h: HeaderMap, Json(r): Json<crate::companion::view::ViewRequest>| async move { sc::view_open(st.root().ok(), h, r).await }))
        .route("/api/view/{id}", axum::routing::delete(sc::view_close))
        .route("/api/view/{id}/screenshot", post(sc::view_screenshot))
        .route("/api/companion", get(sc::status))
        .route("/api/companion/settings", axum::routing::put(sc::settings_put))
        .route("/api/companion/audit", get(sc::audit))
        .route("/api/companion/pair/{which}", post(sc::pair_decide))
        .route("/api/companion/approvals/{which}", post(sc::approval_decide))
        .route("/api/companion/tokens/{id}", axum::routing::delete(sc::token_revoke))
        .route("/api/companion/secrets/{name}", axum::routing::put(sc::secret_put).delete(sc::secret_delete))
        .route("/app", get(|| async { Redirect::permanent("/app/") }))
        .route("/app/", get(ui))
        .route("/app/{*path}", get(ui))
        .route("/preview", get(|| async { Redirect::permanent("/preview/") }))
        .route("/preview/", get(preview))
        .route("/preview/{*path}", get(preview))
        .route("/", get(root))
        .fallback(fallback)
        .layer(cors)
        .with_state(st)
}

/// Fingerprint of the site's sources when the preview was last built (`None` before the first build).
/// Every preview build goes through [`build_preview`], so one at a time writes the folder.
static PREVIEW: Mutex<Option<u64>> = Mutex::new(None);

/// Build the draft-inclusive preview in `<site>/.gw-cache/preview`.
pub fn build_preview(site: &Site) -> Result<BuildReport> {
    build_preview_into(site, site.cache_dir().join("preview"))
}

fn build_preview_into(site: &Site, out: PathBuf) -> Result<BuildReport> {
    let mut last = PREVIEW.lock().unwrap_or_else(|e| e.into_inner());
    let fp = fingerprint(&site.root);
    let r = build(site, &BuildOptions { drafts: true, out_dir: Some(out) })?;
    *last = Some(fp);
    Ok(r)
}

/// A hash of every source file's path, size and modification time. Dot folders (`.gw-cache`,
/// `.git`), build output and story workspaces don't affect the preview, so they are skipped.
fn fingerprint(root: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let skip = |e: &walkdir::DirEntry| {
        let n = e.file_name().to_string_lossy();
        e.depth() > 0 && (n.starts_with('.') || (e.depth() == 1 && matches!(n.as_ref(), "dist" | "stories")) || n == "node_modules")
    };
    for e in walkdir::WalkDir::new(root).sort_by_file_name().into_iter().filter_entry(|e| !skip(e)).flatten() {
        if let Ok(m) = e.metadata() {
            e.path().hash(&mut h);
            m.len().hash(&mut h);
            m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos()).hash(&mut h);
        }
    }
    h.finish()
}

/// Build the draft-inclusive preview once (errors are reported, not fatal).
fn initial_preview(root: &Path) {
    match Site::open(Some(root)).and_then(|s| build_preview(&s)) {
        Ok(r) => eprintln!("preview: built {} pages", r.pages),
        Err(e) => eprintln!("preview: build failed: {e:#}"),
    }
}

/// Rebuild the preview whenever the site's files change on disk, however they changed: the app,
/// an editor, `git pull`, a merge. Waits for the files to stop changing first, so a checkout that
/// touches many files builds once.
fn watch_preview(st: AppState) {
    std::thread::spawn(move || {
        let mut seen: Option<(PathBuf, u64)> = None;
        loop {
            std::thread::sleep(Duration::from_secs(2));
            let Ok(root) = st.root() else { continue };
            let fp = fingerprint(&root);
            let built = *PREVIEW.lock().unwrap_or_else(|e| e.into_inner());
            let settled = matches!(&seen, Some((r, f)) if *r == root && *f == fp);
            seen = Some((root.clone(), fp));
            if built.is_some() && built != Some(fp) && settled {
                match Site::open(Some(&root)).and_then(|s| build_preview(&s)) {
                    Ok(r) => eprintln!("preview: files changed, rebuilt {} pages", r.pages),
                    Err(e) => eprintln!("preview: rebuild failed: {e:#}"),
                }
            }
        }
    });
}

/// Serve the REST API at /api, the management UI at /app, and the built site at /preview.
pub async fn run(site_root: Option<PathBuf>, port: u16, open: bool) -> Result<()> {
    run_with(site_root, port, open, Some(crate::companion::StartOptions::default())).await
}

/// [`run`] with companion options (`gw serve --no-companion`, `--companion-port`, `--companion-auto-approve`).
pub async fn run_with(site_root: Option<PathBuf>, port: u16, open: bool, companion: Option<crate::companion::StartOptions>) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port))).await?;
    serve_with(site_root, listener, open, companion).await
}

/// Like [`run`], on an already-bound listener (the desktop app picks a free port itself).
/// Starts the companion gateway too.
pub async fn serve_on(site_root: Option<PathBuf>, listener: tokio::net::TcpListener, open: bool) -> Result<()> {
    serve_with(site_root, listener, open, Some(crate::companion::StartOptions::default())).await
}

/// [`serve_on`] with companion options (`None`: no companion).
pub async fn serve_with(
    site_root: Option<PathBuf>,
    listener: tokio::net::TcpListener,
    open: bool,
    companion: Option<crate::companion::StartOptions>,
) -> Result<()> {
    stories::set_actor("app");
    if let Some(root) = site_root.clone() {
        tokio::task::spawn_blocking(move || initial_preview(&root)).await?;
    }
    let addr = listener.local_addr()?;
    sc::UI_PORT.store(addr.port(), std::sync::atomic::Ordering::Relaxed);
    let companion_port = match companion {
        Some(mut o) => {
            o.ui_port = Some(addr.port());
            match crate::companion::start(o).await {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("companion: not started: {e:#}");
                    None
                }
            }
        }
        None => None,
    };
    sc::write_server_file(addr.port(), companion_port);
    let url = format!("http://{addr}/app/");
    eprintln!("gitwebsite: {url}  (API at http://{addr}/api, preview at http://{addr}/preview/)");
    if site_root.is_none() {
        eprintln!("gitwebsite: no site yet; the app opens on its setup screen");
    }
    if open {
        let _ = open::that(&url);
    }
    let st = AppState { root: Arc::new(RwLock::new(site_root)) };
    watch_preview(st.clone());
    axum::serve(listener, router_for(st)).await?;
    Ok(())
}
