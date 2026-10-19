//! MCP server over stdio (`gw mcp`): JSON-RPC 2.0, one message per line.
//!
//! Every tool maps onto a function in [`crate::ops`], so an AI client sees exactly what the
//! CLI and REST API do. A fresh [`Site`] is opened per call, so edits made elsewhere (the CLI,
//! the desktop app, a text editor) are always seen. Logging goes to stderr only; stdout
//! carries protocol messages and nothing else.

use crate::{enhance, ops, stories};
use anyhow::{bail, Context, Result};
use gw_core::media::Media;
use gw_core::registry::Entry;
use gw_core::Site;
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::PathBuf;

const SUPPORTED: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// Content conventions, repeated in tool descriptions so a client can write correct posts.
const CONVENTIONS: &str = "Content is AsciiDoc. A file starts with `= Title` then header attributes \
(one per line, `:name: value`) until the first blank line: :type: (page|article|project|update|app|playground|tool), \
:date: YYYY-MM-DD, :areas: comma-separated area paths or unique ids from list_areas (an item may sit in several), \
:tags:, :project: <project slug> (for updates), :summary: one line for cards and feeds, :hero: media:<key> or \
an image path/URL, :hero-crop: left, top, width, height (percent of the hero image that 16:9 cards show), :status: draft|published, :visibility: public|unlisted, :repo: URL, :url: (apps: where the app \
runs), :embed: true (apps: embed in an iframe), :source: <registry id>, :featured: true (home page Featured strip), :pinned: true or :pinned: <n> (pinned to the top of the home page, lowest n first; pinned, featured and latest never repeat each other). Images: \
image::media:<key>[Alt text] for library media (add_media first), image::photo.jpg[] for files beside a bundle's \
index.adoc, or image::https://...[] for external URLs; video::media:<key>[]; 3-D models: \
model::media:<key>[Caption, download=true|false] (add_model first, from the Meshy models repository; or post_add_model / post_remove_model), and the whole models gallery: models::gallery[] (or models::gallery[project=<id>]). Runnable code: a block preceded by \
[source.playground,javascript] (or python, html). New content is always created as a draft.";

fn tools() -> Value {
    let s = |props: Value, req: &[&str]| json!({"type": "object", "properties": props, "required": req});
    let str_ = |d: &str| json!({"type": "string", "description": d});
    let arr = |d: &str| json!({"type": "array", "items": {"type": "string"}, "description": d});
    let kinds = json!({"type": "string", "enum": ["page", "article", "project", "update", "app", "playground", "tool"],
        "description": "page: standalone page (/slug/). article: dated post (/articles/slug/). project: long-running work with updates (/projects/slug/). update: dated post belonging to a project (needs project). app: a publicly running app in the Lab (needs url). playground: page of runnable code in the Lab. tool: an interactive browser tool at /tools/slug/ whose UI is site/static/tools/<slug>.js (header :tool: script name, :category: one of the tools.categories ids in site.yaml)."});
    let entry_schema = json!({"type": "object", "description": "Registry entry",
        "properties": {
            "id": str_("Stable id, e.g. pub-my-book or wordpress-123"),
            "kind": str_("publication | repo | wordpress | smugmug | app | external | document"),
            "title": str_("Title"), "subtitle": str_("Optional subtitle"), "url": str_("Where it lives"),
            "description": str_("Paragraph describing it"),
            "origin": str_("Source of truth: repo URL, local path, WordPress post id, album key"),
            "links": {"type": "array", "items": {"type": "object", "properties": {
                "label": {"type": "string"}, "url": {"type": "string"}, "note": {"type": "string"}, "primary": {"type": "boolean"}},
                "required": ["label", "url"]}},
            "meta": arr("Short facts shown as chips, e.g. 'October 2026', 'Version 0.1'"),
            "areas": arr("Area paths"), "group": str_("Heading on the publications page, e.g. Books, Strategy"),
            "visibility": {"type": "string", "enum": ["public", "unlisted"]},
            "added": str_("YYYY-MM-DD"), "content": str_("Content item id describing this entry, if any")},
        "required": ["id", "kind", "title"]});
    json!([
        {"name": "site_info", "description": "The site's identity, navigation, publish target, and where its files are. Call first to orient.",
         "inputSchema": s(json!({}), &[])},
        {"name": "list_areas", "description": "The information architecture: every area as path (e.g. making/woodworking), id, title, description. Use these paths in :areas:.",
         "inputSchema": s(json!({}), &[])},
        {"name": "list_content", "description": "List content items (newest first), optionally filtered. Returns id, kind, title, url, date, areas, tags, draft, path.",
         "inputSchema": s(json!({"kind": kinds.clone(), "area": str_("Area path or id; includes sub-areas"), "tag": str_("Tag"),
            "drafts_only": {"type": "boolean", "description": "Only drafts"}}), &[])},
        {"name": "get_content", "description": "One item with its full AsciiDoc source (source_text). id may be the item id, slug, or URL.",
         "inputSchema": s(json!({"id": str_("Item id, slug, or URL")}), &["id"])},
        {"name": "create_content", "description": format!("Create a new draft item. Returns its id and file path. {CONVENTIONS}"),
         "inputSchema": s(json!({"kind": kinds, "title": str_("Title"), "areas": arr("Area paths or ids from list_areas"),
            "tags": arr("Tags"), "project": str_("Project slug (required for kind=update)"), "summary": str_("One-line summary"),
            "repo": str_("Repository URL"), "url": str_("For apps: where the app runs"),
            "body": str_("AsciiDoc body after the header (no title or attributes). Omit for a template body."),
            "bundle": {"type": "boolean", "description": "Create a directory (slug/index.adoc) so images can sit beside the text"}}),
            &["kind", "title"])},
        {"name": "update_content_source", "description": format!("Replace an item's entire AsciiDoc source (header and body). Validated before saving; a bad header is rejected and the file left unchanged. Get the current text with get_content first. {CONVENTIONS}"),
         "inputSchema": s(json!({"id": str_("Item id, slug, or URL"), "text": str_("Complete new AsciiDoc source starting with `= Title`")}), &["id", "text"])},
        {"name": "set_status", "description": "Mark an item draft or published. Published items go live at the next publish_site.",
         "inputSchema": s(json!({"id": str_("Item id, slug, or URL"), "status": {"type": "string", "enum": ["draft", "published"]}}), &["id", "status"])},
        {"name": "check_site", "description": "Validate content: headers parse, areas exist, update projects exist, media keys resolve, registry sources exist, apps have a url.",
         "inputSchema": s(json!({}), &[])},
        {"name": "build_site", "description": "Build the static site into dist/. Set drafts=true for a local preview that includes drafts. Does not publish.",
         "inputSchema": s(json!({"drafts": {"type": "boolean"}}), &[])},
        {"name": "publish_plan", "description": "Build without drafts and report what publishing would add, update, and remove in the GitHub Pages repo. Changes nothing. Show this to the user before publish_site.",
         "inputSchema": s(json!({}), &[])},
        {"name": "publish_site", "description": "Build without drafts, copy into the GitHub Pages repo, commit, and (unless push=false) push — this UPDATES THE LIVE PUBLIC WEBSITE. Always call publish_plan, show the user the changes, and get their explicit confirmation first; then pass confirm=true.",
         "inputSchema": s(json!({"confirm": {"type": "boolean", "description": "Must be true: the user has explicitly approved publishing"},
            "message": str_("Commit message"), "push": {"type": "boolean", "description": "Push to GitHub (default true)"}}), &["confirm"])},
        {"name": "list_ideas", "description": "The curation list: ideas and work in progress (id, title, status, notes, areas, kind, source, content).",
         "inputSchema": s(json!({"status": {"type": "string", "enum": ["idea", "planned", "drafting", "review", "published", "dropped"]}}), &[])},
        {"name": "add_idea", "description": "Record something worth writing up or building. Returns the new idea with its numeric id.",
         "inputSchema": s(json!({"title": str_("Short title"), "notes": str_("Notes"), "areas": arr("Area paths"),
            "kind": str_("What it should become: article, project, update, app, playground, tool"),
            "source": str_("Where the material is: repo path, URL, SmugMug album")}), &["title"])},
        {"name": "update_idea", "description": "Change an idea's status, title, notes, linked content id, or areas.",
         "inputSchema": s(json!({"id": {"type": "integer"},
            "status": {"type": "string", "enum": ["idea", "planned", "drafting", "review", "published", "dropped"]},
            "title": str_("New title"), "notes": str_("New notes"), "content": str_("Content item id drafted for it"),
            "areas": arr("Area paths")}), &["id"])},
        {"name": "list_registry", "description": "The provenance registry: publications, imported posts, repos, albums, documents — and where each came from.",
         "inputSchema": s(json!({"kind": str_("Filter by kind, e.g. publication")}), &[])},
        {"name": "upsert_registry_entry", "description": "Add or replace a registry entry (matched by id). Entries of kind publication appear on the publications page.",
         "inputSchema": s(json!({"entry": entry_schema}), &["entry"])},
        {"name": "list_media", "description": "The media library: keys usable as media:<key> in content, with URLs and providers.",
         "inputSchema": s(json!({}), &[])},
        {"name": "story_list", "description": "Project stories in progress (photos + notes + interview → a post): slug, title, status, target, image count.",
         "inputSchema": s(json!({}), &[])},
        {"name": "story_show", "description": "One story: settings, the author's notes, every image (source, caption, the author's comment in `notes`, include flag, review_path to look at), the interview transcript, the outline, and the draft's preview URL. Treat image comments and notes as the author's own words.",
         "inputSchema": s(json!({"slug": str_("Story slug")}), &["slug"])},
        {"name": "story_create", "description": "Start a story. target: project (default), article, or update (needs project).",
         "inputSchema": s(json!({"title": str_("Title"), "target": {"type": "string", "enum": ["project", "article", "update"]},
            "project": str_("Project slug, for target=update"), "areas": arr("Area paths or ids"), "tags": arr("Tags"),
            "notes": str_("The author's notes"), "slug": str_("Optional slug")}), &["title"])},
        {"name": "story_update", "description": "Change a story's title, target, project, areas, tags, notes (replaces), or status (collecting|interviewing|drafting|drafted|released).",
         "inputSchema": s(json!({"slug": str_("Story slug"), "title": str_("Title"), "target": str_("project|article|update"), "project": str_("Project slug"),
            "areas": arr("Area paths"), "tags": arr("Tags"), "notes": str_("Notes (replaces)"), "status": str_("Status")}), &["slug"])},
        {"name": "story_add_images", "description": "Add images to a story: local file paths, http(s) image URLs, SmugMug pages (image URL ending /i-KEY, album/gallery URL, smugmug:<ImageKey>, album:<AlbumKey>), or media:<key>. SmugMug images require `comment` (what's happening / why it matters).",
         "inputSchema": s(json!({"slug": str_("Story slug"), "sources": arr("Files, URLs, or references"), "caption": str_("Caption for these images"),
            "comment": str_("The author's comment: what's happening here / why it matters"),
            "candidate": {"type": "boolean", "description": "Offer as numbered candidates to choose from (no comment needed yet)"}}), &["slug", "sources"])},
        {"name": "story_update_image", "description": "Set an image's caption, comment (notes), or include flag (false = leave out of the post).",
         "inputSchema": s(json!({"slug": str_("Story slug"), "id": str_("Image id, e.g. img3"), "caption": str_("Caption"),
            "notes": str_("Comment"), "include": {"type": "boolean"}}), &["slug", "id"])},
        {"name": "story_remove_image", "description": "Remove an image from a story.",
         "inputSchema": s(json!({"slug": str_("Story slug"), "id": str_("Image id")}), &["slug", "id"])},
        {"name": "story_reorder", "description": "Put a story's images in this order (unlisted ids follow).",
         "inputSchema": s(json!({"slug": str_("Story slug"), "ids": arr("Image ids in order")}), &["slug", "ids"])},
        {"name": "story_note", "description": "Append to a story's notes (or replace=true).",
         "inputSchema": s(json!({"slug": str_("Story slug"), "text": str_("Text"), "replace": {"type": "boolean"}}), &["slug", "text"])},
        {"name": "story_qa", "description": "Record one interview question and the author's answer (verbatim or lightly tidied) in the story's transcript. Call after every answer.",
         "inputSchema": s(json!({"slug": str_("Story slug"), "q": str_("Question asked"), "a": str_("The author's answer")}), &["slug", "q", "a"])},
        {"name": "story_outline", "description": "Save the agreed outline (Markdown): sections, which images go where, title, summary, areas, tags.",
         "inputSchema": s(json!({"slug": str_("Story slug"), "text": str_("Outline")}), &["slug", "text"])},
        {"name": "story_draft", "description": format!("Write the story's post (a draft) from complete AsciiDoc that refers to the story's images as image::story:<id>[Alt text] (and :hero: story:<id>). Local photos are copied in at web size without EXIF/GPS; SmugMug photos become media keys. Missing :type:, :date:, :status: draft, :areas:, :tags: are filled from the story. Rebuilds the preview and returns preview_url. For an enhancement (story_enhance) the text is saved as a proposal the author reviews in the app; the post is not changed. {CONVENTIONS}"),
         "inputSchema": s(json!({"slug": str_("Story slug"), "text": str_("Complete AsciiDoc starting with `= Title`"),
            "preview": {"type": "boolean", "description": "Rebuild the preview (default true)"}}), &["slug", "text"])},
        {"name": "story_review_images", "description": "Make sure every image in a story has a review thumbnail on disk; returns each image's local `path` to look at, with caption and the author's comment.",
         "inputSchema": s(json!({"slug": str_("Story slug")}), &["slug"])},
        {"name": "smugmug_albums", "description": "The authorised user's SmugMug albums (name, album_key, web URL, image count). Read-only.",
         "inputSchema": s(json!({}), &[])},
        {"name": "smugmug_browse", "description": "List what is on a SmugMug page: an album's images (paged: start, count), a folder's albums, or one image. Accepts album/gallery/folder/image URLs (custom domains too) or an AlbumKey. Read-only.",
         "inputSchema": s(json!({"target": str_("SmugMug URL or AlbumKey"), "start": {"type": "integer"}, "count": {"type": "integer"}}), &["target"])},
        {"name": "story_smugmug_pick", "description": "Add, update, or remove SmugMug images in a story, as the app's picker does. Every kept pick needs a non-empty comment from the author.",
         "inputSchema": s(json!({"slug": str_("Story slug"), "picks": {"type": "array", "items": {"type": "object", "properties": {
            "image_key": {"type": "string"}, "album_key": {"type": "string"}, "caption": {"type": "string"},
            "comment": {"type": "string"}, "remove": {"type": "boolean"}}, "required": ["image_key"]}}}), &["slug", "picks"])},
        {"name": "story_add_dir", "description": "Offer every photo in local folders as numbered candidates (left where they are until selected; videos reported, not added). Then show a contact sheet and let the author choose by number.",
         "inputSchema": s(json!({"slug": str_("Story slug"), "dirs": arr("Absolute folder paths"), "recursive": {"type": "boolean"},
            "since": str_("Only photos taken on/after YYYY-MM-DD"), "limit": {"type": "integer", "description": "At most N per folder"}}), &["slug", "dirs"])},
        {"name": "story_add_photos", "description": "macOS: bring photos in from Apple Photos — the photos the author has selected in the Photos app (added as chosen photos, each still needing a comment), or an album by id from photos_albums (offered as numbered candidates). Photos exports current versions with edits as JPEG; videos are skipped. Ask the author to select the photos in Photos first.",
         "inputSchema": s(json!({"slug": str_("Story slug"), "album": str_("Album id from photos_albums; omit for the current selection in Photos"),
            "candidate": {"type": "boolean", "description": "Add as candidates (default for an album) or as chosen photos (default for a selection)"},
            "limit": {"type": "integer", "description": "At most N photos from an album (default 300)"}}), &["slug"])},
        {"name": "photos_albums", "description": "macOS: list the albums in Apple Photos (name with folder path, id, item count) for story_add_photos.",
         "inputSchema": s(json!({}), &[])},
        {"name": "story_add_smugmug", "description": "Offer the photos on SmugMug pages (album/gallery/photo URLs, album:<AlbumKey>) as numbered candidates. Read-only on SmugMug.",
         "inputSchema": s(json!({"slug": str_("Story slug"), "targets": arr("SmugMug URLs or album:<key>")}), &["slug", "targets"])},
        {"name": "story_contact_sheet", "description": "Render a numbered contact sheet JPEG (candidates by default) and return its `path` (open it with your image viewer/Read tool) and the number→image list. The author picks by number.",
         "inputSchema": s(json!({"slug": str_("Story slug"), "which": {"type": "string", "enum": ["candidates", "selected", "rejected", "all"]},
            "page": {"type": "integer"}, "per_page": {"type": "integer"}}), &["slug"])},
        {"name": "story_select", "description": "Select photos by number list (\"3,7-9,12\"), ids, or \"all\". Every selected photo needs the author's comment: pass `comment` (applies to all) or set them with story_comment. Returns which still need one.",
         "inputSchema": s(json!({"slug": str_("Story slug"), "refs": str_("Numbers/ranges/ids"), "comment": str_("The author's comment")}), &["slug", "refs"])},
        {"name": "story_unselect", "description": "Move photos back to the candidates.",
         "inputSchema": s(json!({"slug": str_("Story slug"), "refs": str_("Numbers/ranges/ids")}), &["slug", "refs"])},
        {"name": "story_reject", "description": "Reject photos (kept, marked rejected).",
         "inputSchema": s(json!({"slug": str_("Story slug"), "refs": str_("Numbers/ranges/ids")}), &["slug", "refs"])},
        {"name": "story_comment", "description": "Set one photo's comment (the author's words: what's happening, why it matters). `ref` is a number (#7) or id.",
         "inputSchema": s(json!({"slug": str_("Story slug"), "ref": str_("Number or id"), "text": str_("Comment")}), &["slug", "ref", "text"])},
        {"name": "story_narrative", "description": "The story so far, in the author's words: append (default) or replace (append=false). Pass `base` (narrative_rev you read) when replacing to avoid overwriting edits made in the app.",
         "inputSchema": s(json!({"slug": str_("Story slug"), "text": str_("Text"), "append": {"type": "boolean"}, "base": {"type": "integer"}}), &["slug", "text"])},
        {"name": "story_ask", "description": "Record a question you are waiting on. The author can answer in the chat (then call story_answer) or in the gitwebsite app; check story_pending / story_changes before the next round.",
         "inputSchema": s(json!({"slug": str_("Story slug"), "q": str_("Question")}), &["slug", "q"])},
        {"name": "story_answer", "description": "Answer a pending question (the oldest, or `id`) with the author's words; records it in the transcript.",
         "inputSchema": s(json!({"slug": str_("Story slug"), "a": str_("The author's answer"), "id": {"type": "integer"}}), &["slug", "a"])},
        {"name": "story_pending", "description": "Questions waiting for an answer.",
         "inputSchema": s(json!({"slug": str_("Story slug")}), &["slug"])},
        {"name": "story_changes", "description": "What changed after revision `since` — who (app, cli, mcp), what, when. Call before each interview round to pick up edits and answers made in the app.",
         "inputSchema": s(json!({"slug": str_("Story slug"), "since": {"type": "integer"}}), &["slug"])},
        {"name": "story_wait", "description": "Wait for the author: returns when something changes in the story elsewhere (e.g. an answer typed in the app), or after `timeout` seconds (default 240, max 600). With answer_only=true, returns once no question is pending or an answer arrives. Loop on it while waiting for answers.",
         "inputSchema": s(json!({"slug": str_("Story slug"), "since": {"type": "integer", "description": "Revision to wait from (default: now)"},
            "timeout": {"type": "integer"}, "answer_only": {"type": "boolean"}}), &["slug"])},
        {"name": "story_enhance", "description": "Start (or reopen) an enhancement: rework an existing post (article, project, update, page…) with the author. Creates a story `enhance-<slug>` tied to the item, with the item's title/areas/tags, the code repository behind it (url + local checkout, when found), and the standard key questions the author can answer in the app. Then follow the enhance-post skill.",
         "inputSchema": s(json!({"item": str_("Content item id, slug, or URL")}), &["item"])},
        {"name": "story_context", "description": "Everything for an enhancement: the post's current AsciiDoc, the author's key-question answers, narrative, notes and interview, pending questions, any proposal, selected photos, and the repository (url, local path, README, last 40 commits, first commit, top-level files). Read the repo further with your own tools.",
         "inputSchema": s(json!({"slug": str_("Story slug")}), &["slug"])},
        {"name": "story_key_answers", "description": "Save the author's answers to the standard key questions (ids: inspiration, why, how, hardest, learned, next, audience). Changed answers are added to the transcript.",
         "inputSchema": s(json!({"slug": str_("Story slug"), "answers": {"type": "object", "additionalProperties": {"type": "string"}, "description": "question id → the author's answer"}}), &["slug", "answers"])},
        {"name": "story_set_repo", "description": "Point an enhancement at a repository: its URL and/or a local checkout path (empty path: look for a checkout).",
         "inputSchema": s(json!({"slug": str_("Story slug"), "url": str_("Repository URL"), "path": str_("Local checkout")}), &["slug"])},
        {"name": "story_proposal", "description": "An enhancement's proposal: the revised text, the current post, and a line diff (op ' ', '-', '+').",
         "inputSchema": s(json!({"slug": str_("Story slug")}), &["slug"])},
        {"name": "story_accept", "description": "Write an enhancement's proposal over the post (keeps its path, :status:, :date:, and attributes the proposal omits; sets :updated:). ONLY when the author has explicitly said to accept it in this conversation; normally they accept in the app. Pass confirm=true.",
         "inputSchema": s(json!({"slug": str_("Story slug"), "confirm": {"type": "boolean"}}), &["slug", "confirm"])},
        {"name": "story_discard", "description": "Throw away an enhancement's proposal (only when the author asks).",
         "inputSchema": s(json!({"slug": str_("Story slug")}), &["slug"])},
        {"name": "story_request_changes", "description": "Record the author's feedback on the proposal (what to change); revise and story_draft again.",
         "inputSchema": s(json!({"slug": str_("Story slug"), "text": str_("The author's feedback")}), &["slug", "text"])},
        {"name": "tools_list", "description": "The site's browser tools (diagrams, graphs, simulators, calculators, viewers…): slug, title, category, summary, tags, URL parameter names, ?example= presets, tabs. Filter by category or words. Use it to pick a tool for showing something to the user, then tool_info for exact parameters.",
         "inputSchema": s(json!({"category": str_("Category id, e.g. math, distributed, containers, electronics"), "q": str_("Words that must all match the slug, title, summary or tags")}), &[])},
        {"name": "tool_info", "description": "One tool: every URL parameter with its meaning and an example, the ?example= presets and what each shows, the tabs (#hash), example links from its page, and the page's help text. Read this before tool_open — never guess parameter names.",
         "inputSchema": s(json!({"slug": str_("Tool slug from tools_list")}), &["slug"])},
        {"name": "tool_open", "description": "Show the user something in one of the site's tools: opens /tools/<slug>/?<params>#<hash> from the app's local preview in its own window (bare, no site chrome). Prefer a preset (example) when one fits; params override it. Keep the user's private data out of params unless they asked. Returns the window id and URL; if the app isn't running, returns the public link to give the user instead.",
         "inputSchema": s(json!({"slug": str_("Tool slug"), "params": {"type": "object", "description": "URL parameters by name (strings or numbers), exactly as tool_info lists them", "additionalProperties": true},
            "example": str_("A preset name for ?example="), "hash": str_("Tab to open (#hash), from tool_info's tabs"),
            "title": str_("Window title (default: the tool's name)"),
            "window": str_("reuse (default: the window already showing this tool), new, or a window id from tool_windows")}), &["slug"])},
        {"name": "tool_windows", "description": "The tool windows open in the gitwebsite app (id, tool, title, URL).",
         "inputSchema": s(json!({}), &[])},
        {"name": "tool_close", "description": "Close a tool window by id.",
         "inputSchema": s(json!({"id": str_("Window id from tool_open or tool_windows")}), &["id"])},
        {"name": "tool_screenshot", "description": "A PNG of a tool window's page, to check what the user is seeing (the gitwebsite app on macOS only).",
         "inputSchema": s(json!({"id": str_("Window id")}), &["id"])},
        {"name": "companion_status", "description": "The gitwebsite app and its companion gateway: running or not, its port, the relays (HTTP upstreams, TCP allow-list, Docker socket, Kubernetes contexts, git hosts, LAN hosts, wss allow-list, S3 profiles), paired tools, pending pairing requests and first-use approvals, which secrets are present (never values).",
         "inputSchema": s(json!({}), &[])},
        {"name": "meshy_status", "description": "Meshy.ai model sync: the models repository, last sync (time, result, new models), next scheduled sync, credit balance, model count, new models available free, expired models not recovered and what recovering them would cost. Read-only.",
         "inputSchema": s(json!({}), &[])},
        {"name": "meshy_sync", "description": "Start a sync of the Meshy models repository (runs its tools/sync in the background; follow it with meshy_status). Without recover it is free: it downloads new models still inside Meshy's 3-day window. recover=true PAYS to recover expired models (5 credits each) and needs max_credits, the most it may spend — only with the user's explicit go-ahead for that amount. dry_run=true only looks.",
         "inputSchema": s(json!({"recover": {"type": "boolean", "description": "Spend credits recovering expired models (needs max_credits and the user's consent)"},
            "max_credits": {"type": "integer", "minimum": 1, "description": "With recover: the most credits this run may spend"},
            "dry_run": {"type": "boolean", "description": "List and plan only; change nothing"}}), &[])},
        {"name": "meshy_models", "description": "The 3D models in the Meshy models repository: project, slug, name, description, created, triangle count, print dimensions (mm), parts (multi-part models come on build plates), image paths, folder. Give project to filter, or project+slug for one model with its full README.",
         "inputSchema": s(json!({"project": str_("Project id, e.g. amiga"), "slug": str_("Model slug (with project): one model in full")}), &[])},
        {"name": "add_media", "description": "Register an image or video URL (or site path like /images/x.jpg) under a key, for use as image::media:<key>[] or video::media:<key>[].",
         "inputSchema": s(json!({"key": str_("Letters, digits, - _ ."), "url": str_("Absolute URL or site path"),
            "kind": {"type": "string", "enum": ["image", "video"]}, "alt": str_("Alt text"), "caption": str_("Caption")}), &["key", "url"])},
        {"name": "add_model", "description": "Put a 3-D model from the Meshy models repository on the site: makes a web GLB (decimated, textured, Draco), a USDZ for AR on iOS, a poster and optionally the print file into static/models/<key>/ with Blender (10-60 s), and registers media:<key>. Show it with model::media:<key>[Caption, download=true]. Returns the files and their sizes.",
         "inputSchema": s(json!({"model": str_("<project>/<slug>, as meshy_models lists it"), "key": str_("Media key (default: the slug)"),
            "download": {"type": "string", "enum": ["stl", "3mf", "none"], "description": "Offer readers the print file (default none)"},
            "polycount": {"type": "integer", "description": "Triangles in the web model (default 150000)"},
            "download_polycount": {"type": "integer", "description": "Decimate the downloadable STL to this many triangles"},
            "force": {"type": "boolean", "description": "Replace an existing key holding something else"}}), &["model"])},
        {"name": "post_models", "description": "The 3-D models a post (article, project, update, page) shows: each model:: line with its index, line number, media key, caption, download setting, and the repository model behind it. Also how many models::gallery[] blocks it has.",
         "inputSchema": s(json!({"id": str_("Item id, slug, or URL")}), &["id"])},
        {"name": "post_add_model", "description": "Show a 3-D model in a post: adds a model::media:<key>[Caption] block (at the end, or after a line). model is a media key, or <project>/<slug> from meshy_models — then its web model is made first with Blender (10-60 s) if the site has none. Saves the post's file.",
         "inputSchema": s(json!({"id": str_("Item id, slug, or URL"), "model": str_("Media key or <project>/<slug>"), "caption": str_("Caption (default: the media entry's)"),
            "download": {"type": "boolean", "description": "Offer readers the print file (default: the media entry's setting)"},
            "after_line": {"type": "integer", "description": "Put it after this 1-based line (default: end of the post)"}}), &["id", "model"])},
        {"name": "post_remove_model", "description": "Take a 3-D model out of a post (its model:: line). Give the model (media key or <project>/<slug>) or its index from post_models. Saves the post's file.",
         "inputSchema": s(json!({"id": str_("Item id, slug, or URL"), "model": str_("Media key or <project>/<slug>"), "index": {"type": "integer", "description": "0-based, from post_models"}}), &["id"])},
        {"name": "code_list", "description": "The code catalog (/code/ on the website, code.yaml): the site's git projects with slug, repo, title, summary, category, tags, related, listed (on the website), logo; the categories; counts. Projects without a logo get one from ChatGPT (code_logo_prompts).",
         "inputSchema": s(json!({"need_logo": {"type": "boolean", "description": "Only listed projects without a logo"}}), &[])},
        {"name": "code_get", "description": "One code-catalog project, with its README from GitHub, the categories, the other projects (for related) and the house writing voice — what you need to improve its text. Then save with code_update.",
         "inputSchema": s(json!({"slug": str_("Project slug")}), &["slug"])},
        {"name": "code_update", "description": "Change a code-catalog project (only the fields given). description: 2-4 short paragraphs; `code`, **bold**, [text](https://…) links and '- ' bullets. tags: lower-case. related: slugs of other projects (shown both ways). category: an id from code_list (add one with code_category). hidden: leave it off the website. Set enhanced=true when you rewrote its text.",
         "inputSchema": s(json!({"slug": str_("Project slug"), "title": str_("Display name"), "summary": str_("One sentence for the tile, ≤140 chars"),
            "description": str_("The project page's text"), "category": str_("Category id"), "tags": arr("Tags (replaces)"), "related": arr("Related project slugs (replaces)"),
            "hidden": {"type": "boolean"}, "featured": {"type": "boolean"}, "logo_prompt": str_("A hand-written image prompt for its logo"),
            "post": str_("Content id of the post that writes it up (if not found by its :repo:)"), "enhanced": {"type": "boolean"},
            "family": str_("Logo family id (its shared mark is used when the project has no logo); \"\" to leave it")}), &["slug"])},
        {"name": "code_family", "description": "Add or change a code-catalog logo family: projects that share one mark (e.g. a framework's modules). Give the mark's idea as concept; add members by slug (add) or by category (categories). Its prompt then appears in code_logo_prompts as one item (file family-<id>.png); remove deletes the family.",
         "inputSchema": s(json!({"id": str_("Short kebab-case id"), "title": str_("Name"), "description": str_("What the family is"), "concept": str_("The mark's idea, e.g. a stylised red bull"),
            "logo_prompt": str_("A hand-written prompt instead"), "add": arr("Project slugs to add"), "categories": arr("Add every project in these categories"), "drop": arr("Project slugs to take out"), "remove": {"type": "boolean"}}), &["id"])},
        {"name": "code_category", "description": "Add or change a code-catalog category (id, title, description), or remove it (its projects lose the category).",
         "inputSchema": s(json!({"id": str_("Short kebab-case id"), "title": str_("Title"), "description": str_("One sentence"), "remove": {"type": "boolean"}}), &["id"])},
        {"name": "code_apply_scheme", "description": "Set the code catalog's categories and each project's category and tags at once. categories replaces the list (in display order). Projects keep a category they have unless overwrite=true; tags are added (replaced with overwrite).",
         "inputSchema": s(json!({"categories": {"type": "array", "items": {"type": "object", "properties": {"id": {"type": "string"}, "title": {"type": "string"}, "description": {"type": "string"}}, "required": ["id", "title"]}},
            "projects": {"type": "object", "description": "slug → {category, tags}", "additionalProperties": {"type": "object", "properties": {"category": {"type": "string"}, "tags": {"type": "array", "items": {"type": "string"}}}}},
            "overwrite": {"type": "boolean"}}), &[])},
        {"name": "code_sync", "description": "Read the owner's repositories from GitHub (gh CLI) into the code catalog: new public repositories are added (forks and archived ones hidden), facts refreshed, and logos found in READMEs copied into the site. Takes a minute or more.",
         "inputSchema": s(json!({"logos": {"type": "boolean", "description": "Look for logos (default true)"}}), &[])},
        {"name": "code_logo_prompts", "description": "Image prompts for the code-catalog logos still missing, in numbered batches (text to paste into ChatGPT; each item names the file to save the image as, <slug>.png). The user imports the images in the app's Code page or with `gw code import-logos`.",
         "inputSchema": s(json!({"size": {"type": "integer", "description": "Logos per batch (default 15)"},
            "write": {"type": "boolean", "description": "Also write every batch into the pickup folder's prompts/ (batch-NN.txt to paste, batch-NN.json with the order)"}}), &[])},
        {"name": "code_import_logos", "description": "Import rendered logos — by default everything in the pickup folder (images and zips): images named as the prompts asked (<slug>.png, family-<id>.png) match by name; with batch=N, images whose names match nothing are matched to that batch's missing logos in the order they were saved (only if the counts agree). Look at the images (they are moved to imported/) to check order matches before telling the user.",
         "inputSchema": s(json!({"paths": arr("Files or folders (default: the pickup folder)"), "batch": {"type": "integer", "description": "Batch number, to match unnamed images by order"}}), &[])},
        {"name": "code_new_projects", "description": "Capture new projects for the code catalog: sync from GitHub, have Claude write each new project's entry (title, summary, description, category, tags, related, logo prompt), and write ChatGPT logo prompt files into the pickup folder. New projects wait for review (off the website) until code_approve. Takes a few minutes.",
         "inputSchema": s(json!({}), &[])},
        {"name": "code_approve", "description": "Put code-catalog projects waiting for review on the website; with logo_prs (default true) also add each one's imported logo to its repository's README as a pull request, merged unless merge=false. Adding logos WRITES TO THE USER'S REPOSITORIES: get explicit confirmation, then pass confirm=true.",
         "inputSchema": s(json!({"slugs": arr("Project slugs"), "all": {"type": "boolean", "description": "Every project waiting for review"}, "logo_prs": {"type": "boolean"}, "merge": {"type": "boolean"}, "confirm": {"type": "boolean"}}), &[])},
        {"name": "code_dismiss", "description": "Keep code-catalog projects waiting for review off the website (hidden).",
         "inputSchema": s(json!({"slugs": arr("Project slugs")}), &["slugs"])},
        {"name": "code_logo_pr", "description": "Open a pull request on a project's GitHub repository adding its imported logo (assets/logo.png) at the top of its README, and with merge=true merge it. This WRITES TO THE USER'S REPOSITORIES: list what you will do and get explicit confirmation, then pass confirm=true.",
         "inputSchema": s(json!({"slugs": arr("Project slugs"), "all": {"type": "boolean", "description": "Every imported logo without a PR"}, "merge": {"type": "boolean", "description": "Also squash-merge each PR and delete its branch"}, "confirm": {"type": "boolean"}}), &["confirm"])},
    ])
}

fn sv<'a>(a: &'a Value, k: &str) -> Option<&'a str> {
    a.get(k).and_then(|v| v.as_str())
}
fn req<'a>(a: &'a Value, k: &str) -> Result<&'a str> {
    sv(a, k).with_context(|| format!("missing string argument `{k}`"))
}

fn call(root: &PathBuf, name: &str, a: &Value) -> Result<Value> {
    if let Some(v) = call_meshy(name, a) {
        return v;
    }
    let site = Site::open(Some(root))?;
    match name {
        "site_info" => Ok(json!({
            "root": site.root.display().to_string(),
            "content_dir": site.content_dir().display().to_string(),
            "site": site.config.site, "nav": site.config.nav,
            "publish": site.config.publish, "sources": site.config.sources,
            "conventions": CONVENTIONS,
        })),
        "list_areas" => ops::areas(&site),
        "list_content" => ops::list(&site, &serde_json::from_value(a.clone()).context("bad arguments")?),
        "get_content" => ops::show(&site, req(a, "id")?),
        "create_content" => ops::new_item(&site, &serde_json::from_value(a.clone()).context("bad arguments")?),
        "update_content_source" => ops::write_source(&site, req(a, "id")?, req(a, "text")?),
        "set_status" => ops::set_status(&site, req(a, "id")?, req(a, "status")?),
        "check_site" => ops::check(&site),
        "build_site" => ops::do_build(&site, a.get("drafts").and_then(|v| v.as_bool()).unwrap_or(false)),
        "publish_plan" => ops::publish_plan(&site),
        "publish_site" => {
            if a.get("confirm").and_then(|v| v.as_bool()) != Some(true) {
                bail!("refused: publishing updates the live website. Show the user publish_plan, get explicit confirmation, then call again with confirm=true");
            }
            ops::publish_site(&site, sv(a, "message"), a.get("push").and_then(|v| v.as_bool()).unwrap_or(true))
        }
        "list_ideas" => ops::ideas_list(&site, sv(a, "status")),
        "add_idea" => ops::idea_add(&site, &serde_json::from_value(a.clone()).context("bad arguments")?),
        "update_idea" => {
            let id = a.get("id").and_then(|v| v.as_u64()).context("missing integer argument `id`")? as u32;
            let u: ops::IdeaUpdate = serde_json::from_value(a.clone()).context("bad arguments")?;
            ops::idea_update(&site, id, &u)
        }
        "list_registry" => ops::registry_list(&site, sv(a, "kind")),
        "upsert_registry_entry" => {
            let e: Entry = serde_json::from_value(a.get("entry").cloned().context("missing `entry`")?).context("bad entry")?;
            ops::registry_upsert(&site, e)
        }
        "list_media" => ops::media_list(&site),
        "add_media" => {
            let url = req(a, "url")?.to_string();
            let provider = if url.starts_with('/') { "local" } else if url.contains("smugmug") { "smugmug" } else { "external" };
            ops::media_add(&site, req(a, "key")?, Media {
                url,
                kind: sv(a, "kind").unwrap_or("image").into(),
                provider: provider.into(),
                thumb: String::new(),
                alt: sv(a, "alt").unwrap_or("").into(),
                caption: sv(a, "caption").unwrap_or("").into(),
                ids: Default::default(),
                ..Default::default()
            })
        }
        "post_models" => crate::integrations::site_models::post_models(&site, req(a, "id")?),
        "code_list" | "code_get" | "code_update" | "code_category" | "code_family" | "code_apply_scheme" | "code_sync" | "code_logo_prompts" | "code_import_logos" | "code_logo_pr" | "code_new_projects" | "code_approve" | "code_dismiss" => {
            use crate::integrations::code_catalog as cc;
            let strs = |k: &str| a.get(k).and_then(|v| v.as_array()).map(|x| x.iter().filter_map(|s| s.as_str().map(str::to_string)).collect::<Vec<_>>());
            match name {
                "code_list" => cc::run_cli(&site, cc::CodeCmd::List { need_logo: a.get("need_logo").and_then(|v| v.as_bool()).unwrap_or(false) }),
                "code_get" => {
                    let slug = req(a, "slug")?;
                    let mut v = cc::context(&site, slug)?;
                    v["page"] = cc::show(&site, slug)?;
                    Ok(v)
                }
                "code_update" => {
                    let p = cc::Patch {
                        title: sv(a, "title").map(str::to_string), summary: sv(a, "summary").map(str::to_string),
                        description: sv(a, "description").map(str::to_string), category: sv(a, "category").map(str::to_string),
                        tags: strs("tags"), related: strs("related"),
                        hidden: a.get("hidden").and_then(|v| v.as_bool()), featured: a.get("featured").and_then(|v| v.as_bool()),
                        logo_prompt: sv(a, "logo_prompt").map(str::to_string), post: sv(a, "post").map(str::to_string),
                        enhanced: a.get("enhanced").and_then(|v| v.as_bool()), order: None,
                        family: sv(a, "family").map(str::to_string),
                    };
                    cc::update(&site, req(a, "slug")?, &p)
                }
                "code_family" => cc::family(&site, &serde_json::from_value(a.clone())?).map(|v| json!({ "added": v["added"], "families": v["families"] })),
                "code_category" => cc::category(&site, req(a, "id")?, sv(a, "title"), sv(a, "description"), a.get("remove").and_then(|v| v.as_bool()).unwrap_or(false)),
                "code_apply_scheme" => cc::apply_scheme(&site, &serde_json::from_value(a.clone())?).map(|v| json!({ "applied": v["applied"], "categories": v["categories"] })),
                "code_new_projects" => cc::new_projects(&site, &|l| eprintln!("{l}")),
                "code_approve" => {
                    let prs = a.get("logo_prs").and_then(|v| v.as_bool()).unwrap_or(true);
                    if prs && a.get("confirm").and_then(|v| v.as_bool()) != Some(true) {
                        bail!("adding logos to the repositories needs confirm=true, after the user has agreed (or logo_prs=false)");
                    }
                    let slugs = cc::review_slugs(&site, strs("slugs").unwrap_or_default(), a.get("all").and_then(|v| v.as_bool()) == Some(true))?;
                    cc::approve(&site, &slugs, prs, a.get("merge").and_then(|v| v.as_bool()).unwrap_or(true), &|l| eprintln!("{l}"))
                }
                "code_dismiss" => cc::dismiss(&site, &strs("slugs").unwrap_or_default()),
                "code_sync" => Ok(serde_json::to_value(cc::sync(&site, a.get("logos").and_then(|v| v.as_bool()).unwrap_or(true), false, &|l| eprintln!("{l}"))?)?),
                "code_logo_prompts" => {
                    let size = a.get("size").and_then(|v| v.as_u64()).unwrap_or(15).clamp(1, 50) as usize;
                    if a.get("write").and_then(|v| v.as_bool()) == Some(true) { cc::write_prompt_files(&site, size) } else { cc::logo_prompts(&site, size) }
                }
                "code_import_logos" => cc::import_logos(&site, &strs("paths").unwrap_or_default().into_iter().map(PathBuf::from).collect::<Vec<_>>(), a.get("batch").and_then(|v| v.as_u64()).map(|n| n as usize)),
                _ => {
                    if a.get("confirm").and_then(|v| v.as_bool()) != Some(true) {
                        bail!("opening pull requests needs confirm=true, after the user has agreed");
                    }
                    let slugs = if a.get("all").and_then(|v| v.as_bool()) == Some(true) { cc::awaiting_pr(&site)? } else { strs("slugs").unwrap_or_default() };
                    if slugs.is_empty() {
                        bail!("name slugs, or all=true");
                    }
                    cc::logo_prs(&site, &slugs, a.get("merge").and_then(|v| v.as_bool()) == Some(true), &|l| eprintln!("{l}"))
                }
            }
        }
        "post_add_model" | "post_remove_model" => {
            use crate::integrations::site_models::{post_change, PostChange};
            let ch = PostChange {
                action: if name == "post_add_model" { "add" } else { "remove" }.into(),
                model: sv(a, "model").map(str::to_string),
                index: a.get("index").and_then(|v| v.as_u64()).map(|n| n as usize),
                caption: sv(a, "caption").map(str::to_string),
                download: a.get("download").filter(|v| v.is_boolean()).cloned(),
                after_line: a.get("after_line").and_then(|v| v.as_u64()).map(|n| n as usize),
            };
            post_change(&site, req(a, "id")?, &ch)
        }
        "add_model" => {
            let mut o: crate::integrations::model_assets::AddModel = serde_json::from_value(a.clone()).context("bad arguments")?;
            if o.download.is_empty() {
                o.download = "none".into();
            }
            crate::integrations::model_assets::add_model(&site, &o)
        }
        "story_list" => stories::list(&site),
        "story_show" => stories::show(&site, req(a, "slug")?),
        "story_create" => stories::create(&site, &serde_json::from_value(a.clone()).context("bad arguments")?),
        "story_update" => {
            let mut u: gw_core::story::StoryUpdate = serde_json::from_value(a.clone()).context("bad arguments")?;
            if u.project.is_none() && a.get("project").is_some() { u.project = Some(String::new()); }
            stories::update(&site, req(a, "slug")?, &u)
        }
        "story_add_images" => stories::add(&site, req(a, "slug")?, &serde_json::from_value(a.clone()).context("bad arguments")?),
        "story_update_image" => stories::image_update(&site, req(a, "slug")?, req(a, "id")?, &serde_json::from_value(a.clone()).context("bad arguments")?),
        "story_remove_image" => stories::image_remove(&site, req(a, "slug")?, req(a, "id")?),
        "story_reorder" => {
            let ids: Vec<String> = serde_json::from_value(a.get("ids").cloned().context("missing `ids`")?).context("bad ids")?;
            stories::reorder(&site, req(a, "slug")?, &ids)
        }
        "story_note" => stories::note(&site, req(a, "slug")?, req(a, "text")?, a.get("replace").and_then(|v| v.as_bool()).unwrap_or(false)),
        "story_qa" => stories::qa(&site, req(a, "slug")?, req(a, "q")?, req(a, "a")?),
        "story_outline" => stories::outline(&site, req(a, "slug")?, req(a, "text")?),
        "story_draft" => stories::draft(&site, req(a, "slug")?, req(a, "text")?, a.get("preview").and_then(|v| v.as_bool()).unwrap_or(true)),
        "story_review_images" => stories::review_images(&site, req(a, "slug")?),
        "smugmug_albums" => stories::smugmug_albums(),
        "smugmug_browse" => stories::smugmug_browse(
            req(a, "target")?,
            a.get("start").and_then(|v| v.as_u64()).unwrap_or(1) as usize,
            a.get("count").and_then(|v| v.as_u64()).unwrap_or(100) as usize,
        ),
        "story_add_dir" => stories::add_dir(&site, req(a, "slug")?, &serde_json::from_value(a.clone()).context("bad arguments")?),
        "story_add_photos" => stories::add_photos(&site, req(a, "slug")?, &serde_json::from_value(a.clone()).context("bad arguments")?),
        "photos_albums" => stories::photos_albums(),
        "story_add_smugmug" => {
            let targets: Vec<String> = serde_json::from_value(a.get("targets").cloned().context("missing `targets`")?).context("bad targets")?;
            stories::add(&site, req(a, "slug")?, &stories::AddArgs { sources: targets, caption: None, comment: None, candidate: true })
        }
        "story_contact_sheet" => stories::contact_sheet(&site, req(a, "slug")?, sv(a, "which").unwrap_or("candidates"),
            a.get("page").and_then(|v| v.as_u64()).unwrap_or(1) as usize, a.get("per_page").and_then(|v| v.as_u64()).unwrap_or(24) as usize),
        "story_select" => stories::select(&site, req(a, "slug")?, req(a, "refs")?, sv(a, "comment")),
        "story_unselect" => stories::set_state(&site, req(a, "slug")?, req(a, "refs")?, "candidate"),
        "story_reject" => stories::set_state(&site, req(a, "slug")?, req(a, "refs")?, "rejected"),
        "story_comment" => stories::comment(&site, req(a, "slug")?, req(a, "ref")?, req(a, "text")?),
        "story_narrative" => stories::narrative(&site, req(a, "slug")?, req(a, "text")?, a.get("append").and_then(|v| v.as_bool()).unwrap_or(true), a.get("base").and_then(|v| v.as_u64())),
        "story_ask" => stories::ask(&site, req(a, "slug")?, req(a, "q")?),
        "story_answer" => stories::answer(&site, req(a, "slug")?, a.get("id").and_then(|v| v.as_u64()).map(|v| v as u32), req(a, "a")?),
        "story_pending" => stories::pending(&site, req(a, "slug")?),
        "story_changes" => stories::changes(&site, req(a, "slug")?, a.get("since").and_then(|v| v.as_u64()).unwrap_or(0)),
        "story_wait" => stories::wait(&site, req(a, "slug")?, a.get("since").and_then(|v| v.as_u64()),
            a.get("timeout").and_then(|v| v.as_u64()).unwrap_or(240).min(600), a.get("answer_only").and_then(|v| v.as_bool()).unwrap_or(false)),
        "story_enhance" => enhance::start(&site, req(a, "item")?),
        "story_context" => enhance::context(&site, req(a, "slug")?),
        "story_key_answers" => {
            let m: std::collections::BTreeMap<String, String> = serde_json::from_value(a.get("answers").cloned().context("missing `answers`")?).context("answers must map ids to strings")?;
            enhance::key_answers(&site, req(a, "slug")?, &m)
        }
        "story_set_repo" => enhance::set_repo(&site, req(a, "slug")?, sv(a, "url").unwrap_or(""), sv(a, "path")),
        "story_proposal" => enhance::proposal(&site, req(a, "slug")?),
        "story_accept" => {
            if a.get("confirm").and_then(|v| v.as_bool()) != Some(true) {
                bail!("refused: accepting rewrites the post. Only when the author has explicitly said so; then call again with confirm=true");
            }
            enhance::accept(&site, req(a, "slug")?)
        }
        "story_discard" => enhance::discard(&site, req(a, "slug")?),
        "story_request_changes" => enhance::request_changes(&site, req(a, "slug")?, req(a, "text")?),
        "story_smugmug_pick" => {
            let picks: Vec<stories::Pick> = serde_json::from_value(a.get("picks").cloned().context("missing `picks`")?).context("bad picks")?;
            stories::smugmug_pick(&site, req(a, "slug")?, &picks)
        }
        "tools_list" => crate::toolcat::list(&site.root, sv(a, "category"), sv(a, "q")),
        "tool_info" => crate::toolcat::info(&site.root, req(a, "slug")?),
        "tool_open" => tool_open(&site, a),
        "tool_windows" => crate::appclient::get("/api/view"),
        "tool_close" => crate::appclient::send("DELETE", &format!("/api/view/{}", urlencoding::encode(req(a, "id")?)), &json!({})),
        "tool_screenshot" => {
            use base64::Engine;
            let png = crate::appclient::post_bytes(&format!("/api/view/{}/screenshot", urlencoding::encode(req(a, "id")?)))?;
            Ok(json!({ IMAGE_KEY: base64::engine::general_purpose::STANDARD.encode(png) }))
        }
        "companion_status" => Ok(match crate::appclient::get("/api/companion") {
            Ok(mut v) => {
                v["app"] = json!({ "running": true, "url": crate::appclient::base().ok() });
                v
            }
            Err(e) => json!({ "app": { "running": false, "note": format!("{e:#}") }, "settings_file": crate::companion::settings::path().display().to_string() }),
        }),
        _ => bail!("unknown tool {name}"),
    }
}

/// A tool_screenshot result: the PNG, base64, sent as MCP image content.
const IMAGE_KEY: &str = "_image_png_base64";

fn tool_open(site: &Site, a: &Value) -> Result<Value> {
    let slug = req(a, "slug")?;
    let tool = crate::toolcat::load(&site.root)?.into_iter().find(|t| t.slug == slug).with_context(|| format!("no tool {slug:?}: see tools_list"))?;
    let mut r = crate::companion::view::ViewRequest {
        tool: slug.to_string(),
        params: a.get("params").and_then(|p| p.as_object()).cloned().unwrap_or_default(),
        example: sv(a, "example").map(str::to_string),
        hash: sv(a, "hash").map(str::to_string),
        title: sv(a, "title").map(str::to_string).or(Some(tool.title.clone())),
        window: sv(a, "window").map(str::to_string),
        ..Default::default()
    };
    if let Some(e) = &r.example {
        if !tool.presets.is_empty() && !tool.presets.iter().any(|p| &p.name == e) {
            bail!("{slug} has no preset {e:?}; presets: {}", tool.presets.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", "));
        }
    }
    let public = crate::companion::view::tool_url(&site.config.site.base_url, &r, false);
    let unknown: Vec<String> = r.params.keys().filter(|k| !tool.params.is_empty() && !tool.params.iter().any(|p| &p.name == *k || p.name.contains('…'))).cloned().collect();
    r.params.retain(|_, v| !v.is_null());
    match crate::appclient::send("POST", "/api/view", &serde_json::to_value(&r)?) {
        Ok(mut v) => {
            v["public_url"] = json!(public);
            if !unknown.is_empty() {
                v["warning"] = json!(format!("parameters not in the tool's documented list (check tool_info): {}", unknown.join(", ")));
            }
            Ok(v)
        }
        Err(e) => Ok(json!({
            "opened": false, "public_url": public,
            "note": format!("{e:#}. Give the user the public link instead."),
        })),
    }
}

/// The Meshy tools, which need no site. `None` for any other tool.
fn call_meshy(name: &str, a: &Value) -> Option<Result<Value>> {
    use crate::integrations::meshy;
    let run = || -> Result<Value> {
        let ctx = meshy::Ctx::load()?;
        match name {
            "meshy_status" => meshy::status(&ctx, true),
            "meshy_models" => match (sv(a, "project"), sv(a, "slug")) {
                (Some(p), Some(s)) => meshy::model(&ctx, p, s),
                (p, None) => meshy::models(&ctx, p),
                (None, Some(_)) => bail!("give project with slug"),
            },
            _ => {
                let opts = meshy::SyncOptions {
                    recover: a.get("recover").and_then(|v| v.as_bool()).unwrap_or(false),
                    max_credits: a.get("max_credits").and_then(|v| v.as_u64()),
                    dry_run: a.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(false),
                };
                opts.validate()?;
                // In the running app when there is one, so its Models page shows the progress;
                // otherwise in this process, in the background.
                match crate::appclient::send("POST", "/api/meshy/sync", &serde_json::to_value(&opts)?) {
                    Ok(mut v) => {
                        v["in"] = json!("app");
                        Ok(v)
                    }
                    Err(e) if format!("{e:#}").contains("not running") => {
                        let mut v = meshy::spawn_sync(ctx, opts, "mcp")?;
                        v["in"] = json!("mcp");
                        Ok(v)
                    }
                    Err(e) => Err(e),
                }
            }
        }
    };
    matches!(name, "meshy_status" | "meshy_sync" | "meshy_models").then(run)
}

/// "Mona Octocat's website, “Octo's notes” (https://octocat.github.io)", from site.yaml.
fn whose_site(root: &std::path::Path) -> String {
    match Site::open(Some(root)) {
        Ok(s) => {
            let c = &s.config.site;
            let host = c.base_url.trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/');
            if c.author.is_empty() {
                format!("the website “{}” ({host})", c.title)
            } else {
                format!("{}'s website, “{}” ({host})", c.author, c.title)
            }
        }
        Err(_) => "the gitwebsite site".into(),
    }
}

fn handle(root: &PathBuf, msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let result: std::result::Result<Value, (i64, String)> = match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(|v| v.as_str()).unwrap_or("");
            let v = if SUPPORTED.contains(&asked) { asked } else { SUPPORTED[0] };
            Ok(json!({
                "protocolVersion": v,
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "gitwebsite", "version": env!("CARGO_PKG_VERSION")},
                "instructions": format!("Manage {}: content, areas, ideas, registry, media, project stories (photos + interview → post), enhancements of existing posts (story_enhance), the code catalog of git projects at /code/ (code_* tools: code_new_projects captures new repositories for review, then code_approve; sync from GitHub, descriptions, categories, tags, related projects, ChatGPT logo prompts in batches; logo pull requests write to the user's repositories — confirm first), build, and publish; and show the user things in the site's browser tools (tools_list → tool_info → tool_open opens a tool window in the app). Publishing updates the live site — always get the user's confirmation. {CONVENTIONS}", whose_site(root)),
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tools()})),
        "tools/call" => {
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            match call(root, name, &args) {
                Ok(v) if v.get(IMAGE_KEY).is_some() => Ok(json!({"content": [{"type": "image", "data": v[IMAGE_KEY], "mimeType": "image/png"}], "isError": false})),
                Ok(v) => Ok(json!({"content": [{"type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default()}], "isError": false})),
                Err(e) => Ok(json!({"content": [{"type": "text", "text": format!("Error: {e:#}")}], "isError": true})),
            }
        }
        m if m.starts_with("notifications/") => return None,
        _ => Err((-32601, format!("method not found: {method}"))),
    };
    let id = id?; // notifications get no response
    Some(match result {
        Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
        Err((code, message)) => json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}),
    })
}

pub fn run(site_root: PathBuf) -> Result<()> {
    stories::set_actor("mcp");
    eprintln!("gitwebsite MCP server: site {}", site_root.display());
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(Value::Array(batch)) => {
                let r: Vec<Value> = batch.iter().filter_map(|m| handle(&site_root, m)).collect();
                if r.is_empty() { None } else { Some(Value::Array(r)) }
            }
            Ok(msg) => handle(&site_root, &msg),
            Err(e) => Some(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": format!("parse error: {e}")}})),
        };
        if let Some(r) = reply {
            writeln!(out, "{}", serde_json::to_string(&r)?)?;
            out.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_schemas_are_well_formed() {
        let tools = tools();
        let list = tools.as_array().unwrap();
        let mut names = std::collections::HashSet::new();
        for t in list {
            let name = t["name"].as_str().unwrap();
            assert!(names.insert(name.to_string()), "duplicate {name}");
            assert!(t["description"].as_str().is_some_and(|d| d.len() > 10), "{name}");
            let s = &t["inputSchema"];
            assert_eq!(s["type"], "object", "{name}");
            let props = s["properties"].as_object().unwrap();
            for r in s["required"].as_array().unwrap() {
                assert!(props.contains_key(r.as_str().unwrap()), "{name}: required {r} not in properties");
            }
        }
        for n in ["tools_list", "tool_info", "tool_open", "tool_windows", "tool_close", "tool_screenshot", "companion_status", "meshy_status", "meshy_sync", "meshy_models"] {
            assert!(names.contains(n), "missing {n}");
        }
        let open = list.iter().find(|t| t["name"] == "tool_open").unwrap();
        assert_eq!(open["inputSchema"]["required"], json!(["slug"]));
        assert_eq!(open["inputSchema"]["properties"]["params"]["type"], "object");
        // Recovering spends credits: never without an explicit max_credits.
        let e = call_meshy("meshy_sync", &json!({"recover": true})).unwrap().unwrap_err().to_string();
        assert!(e.contains("max_credits"), "{e}");
        assert!(call_meshy("list_content", &json!({})).is_none());
    }
}
