//! `gw` — the gitwebsite command line. Also starts the REST server (`gw serve`) and the MCP
//! server (`gw mcp`).

use gw::{companion, companion_cli, integrations, mcp, ops, server, setup, stories};

use anyhow::Result;
use clap::{Parser, Subcommand};
use gw_core::media::Media;
use gw_core::registry::Entry;
use gw_core::Site;
use serde_json::{json, Value};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "gw", version, about = "gitwebsite: build, curate, and publish a website from AsciiDoc in git")]
struct Cli {
    /// Site directory (holding site.yaml). Default: $GW_SITE, then site.yaml here or above, then
    /// the current site in the user config (`gw config`).
    #[arg(long, global = true)]
    site: Option<PathBuf>,
    /// Print machine-readable JSON.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show the site: title, author, address, folder, and where it publishes.
    Site,
    /// Build the site into dist/.
    Build {
        /// Include drafts (for local preview only).
        #[arg(long)]
        drafts: bool,
    },
    /// Validate content, areas, media references, and registry sources.
    Check,
    /// List content.
    List {
        #[arg(long)]
        kind: Option<String>,
        #[arg(long)]
        area: Option<String>,
        #[arg(long)]
        tag: Option<String>,
        /// Only drafts.
        #[arg(long)]
        drafts: bool,
    },
    /// Show one item (id, slug, or URL) with its source.
    Show { id: String },
    /// Create a draft: gw new article "Title" --area woodworking
    New {
        /// page | article | project | update | app | playground | tool
        kind: String,
        title: String,
        #[arg(long = "area")]
        areas: Vec<String>,
        #[arg(long = "tag")]
        tags: Vec<String>,
        #[arg(long)]
        project: Option<String>,
        #[arg(long, default_value = "")]
        summary: String,
        #[arg(long)]
        repo: Option<String>,
        /// For apps: where the app runs.
        #[arg(long)]
        url: Option<String>,
        /// Create a bundle directory so images can sit beside the text.
        #[arg(long)]
        bundle: bool,
    },
    /// Mark an item published (it appears at the next `gw publish`).
    Release { id: String },
    /// Mark an item as a draft again.
    Unrelease { id: String },
    /// Show the information architecture (areas).
    Areas,
    /// Ideas and work to curate.
    #[command(subcommand)]
    Idea(IdeaCmd),
    /// The provenance registry.
    #[command(subcommand)]
    Registry(RegistryCmd),
    /// The media library (media:<key> references).
    #[command(subcommand)]
    Media(MediaCmd),
    /// Serve the REST API, the management UI (/app), and a live preview of the site.
    Serve {
        #[arg(long, default_value_t = 8790)]
        port: u16,
        /// Open the management UI in the browser.
        #[arg(long)]
        open: bool,
        /// Don't start the companion gateway (relays and tool windows for the site's tools).
        #[arg(long)]
        no_companion: bool,
        /// Companion port (default 7317 or companion.toml; fallbacks +1…+3).
        #[arg(long)]
        companion_port: Option<u16>,
        /// Approve every companion pairing request without asking (headless use only).
        #[arg(long)]
        companion_auto_approve: bool,
    },
    /// The companion gateway: pairing approvals, tokens, settings, keychain secrets.
    #[command(subcommand)]
    Companion(companion_cli::CompanionCmd),
    /// The site's browser tools: list, describe, index for the show-with-tools skill, open in a window.
    #[command(subcommand)]
    Tools(companion_cli::ToolsCmd),
    /// Run the MCP server on stdio (for Claude Code and other MCP clients).
    Mcp,
    /// SmugMug: media hosting for images and video.
    #[command(subcommand)]
    Smugmug(integrations::SmugmugCmd),
    /// WordPress: import posts and cross-post.
    #[command(subcommand)]
    Wordpress(integrations::WordpressCmd),
    /// Meshy.ai 3D models: sync the models repository, its status, the schedule in `gw serve`.
    #[command(subcommand)]
    Meshy(integrations::MeshyCmd),
    /// 3-D models on the website: the models gallery (which Meshy models are on it — models.yaml —
    /// and copying their pictures into the site, `sync-site`) and the models in posts (in, add, remove).
    #[command(subcommand)]
    Models(integrations::site_models::ModelsCmd),
    /// Project stories: photos + notes + an interview → a post (see `/project-story`).
    #[command(subcommand)]
    Story(StoryCmd),
    /// Create a new site: gw init ~/sites/blog --title "My notes" --author "Mona" --pages-repo octocat/octocat.github.io
    Init {
        /// Folder for the site (new or empty).
        dir: PathBuf,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        author: Option<String>,
        /// Where the site is served; default: derived from --pages-repo.
        #[arg(long)]
        base_url: Option<String>,
        #[arg(long, default_value = "")]
        tagline: String,
        #[arg(long, default_value = "")]
        description: String,
        /// GitHub Pages repository, owner/name (e.g. octocat/octocat.github.io).
        #[arg(long)]
        pages_repo: Option<String>,
        #[arg(long, default_value = "main")]
        branch: String,
        /// Local checkout of the Pages repository (default: under the gitwebsite config folder).
        #[arg(long)]
        pages_checkout: Option<String>,
        /// Don't copy the default theme into the site (it still builds with the built-in one).
        #[arg(long)]
        no_theme_copy: bool,
        /// Leave out the starter pages and posts.
        #[arg(long)]
        no_starter: bool,
        /// Make the site folder a git repository with a first commit.
        #[arg(long)]
        git: bool,
        /// Create the GitHub Pages repository with `gh` (public).
        #[arg(long)]
        create_pages_repo: bool,
        /// Don't make it the current site in the user config.
        #[arg(long)]
        no_current: bool,
    },
    /// Your gitwebsite settings: sites, the current one, program paths, repo search folders.
    #[command(subcommand)]
    Config(ConfigCmd),
    /// Build without drafts and publish to GitHub Pages.
    Publish {
        /// Show what would change; touch nothing.
        #[arg(long)]
        dry_run: bool,
        /// Commit but do not push.
        #[arg(long)]
        no_push: bool,
        /// Skip the confirmation prompt.
        #[arg(long, short)]
        yes: bool,
        #[arg(long, short)]
        message: Option<String>,
    },
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Show the settings and where they are kept.
    Show,
    /// Add an existing site (a folder holding site.yaml).
    AddSite {
        path: PathBuf,
        #[arg(long)]
        name: Option<String>,
        /// Make it the current site.
        #[arg(long)]
        current: bool,
    },
    /// Switch the current site (by name or path).
    Use {
        /// The site's name or path
        #[arg(value_name = "SITE")]
        which: String,
    },
    /// Forget a site (its files are untouched).
    Remove {
        /// The site's name or path
        #[arg(value_name = "SITE")]
        which: String,
    },
    /// Set repo-roots (comma-separated folders), claude-bin, or gw-bin; an empty value clears it.
    Set { key: String, value: String },
}

#[derive(Subcommand)]
enum StoryCmd {
    /// Start a story: gw story new "Creating the Amiga 40th exhibit" --area amiga
    New {
        title: String,
        /// project | article | update
        #[arg(long, default_value = "project")]
        target: String,
        /// For --target update: the project it belongs to.
        #[arg(long)]
        project: Option<String>,
        #[arg(long = "area")]
        areas: Vec<String>,
        #[arg(long = "tag")]
        tags: Vec<String>,
        #[arg(long)]
        notes: Option<String>,
        #[arg(long)]
        slug: Option<String>,
    },
    /// List stories.
    List,
    /// Show a story: settings, notes, images (with files to look at), interview, outline.
    Show { slug: String },
    /// Change a story's title, target, project, areas, tags, or status.
    Set {
        slug: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        target: Option<String>,
        #[arg(long)]
        project: Option<String>,
        #[arg(long = "area")]
        areas: Option<Vec<String>>,
        #[arg(long = "tag")]
        tags: Option<Vec<String>>,
        #[arg(long)]
        status: Option<String>,
    },
    /// Add images: files, URLs, SmugMug pages (`…/i-KEY`, album or gallery URLs, `smugmug:KEY`,
    /// `album:KEY`), or `media:<key>`. SmugMug images need --comment unless added as --candidate.
    Add {
        slug: String,
        #[arg(required = true)]
        sources: Vec<String>,
        #[arg(long)]
        caption: Option<String>,
        /// What's happening here / why it matters (required for SmugMug images).
        #[arg(long)]
        comment: Option<String>,
        /// Offer them as candidates to choose from (nothing is copied until selected).
        #[arg(long)]
        candidate: bool,
    },
    /// macOS: bring photos in from Apple Photos — the ones selected in Photos now (added as chosen
    /// photos), or an album's (offered as candidates). Exports current versions with edits as JPEG.
    AddPhotos {
        slug: String,
        /// An album: its name (as `gw story photos-albums` lists it) or id. Without it, the
        /// photos selected in Photos.
        #[arg(long)]
        album: Option<String>,
        /// Add as candidates to choose from (the default for an album).
        #[arg(long, conflicts_with = "chosen")]
        candidate: bool,
        /// Add as chosen photos (the default for a selection).
        #[arg(long)]
        chosen: bool,
        /// At most this many photos from an album (default 300).
        #[arg(long)]
        limit: Option<usize>,
    },
    /// macOS: list the albums in Apple Photos.
    PhotosAlbums,
    /// Offer every photo in folders as candidates (left where they are until selected; videos
    /// are reported, not added). Photos are numbered for `contact-sheet` and `select`.
    AddDir {
        slug: String,
        #[arg(required = true)]
        dirs: Vec<String>,
        /// Look in subfolders too.
        #[arg(long, short)]
        recursive: bool,
        /// Only photos taken on or after this date (YYYY-MM-DD).
        #[arg(long)]
        since: Option<String>,
        /// At most this many photos per folder (oldest first).
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Offer the photos on SmugMug pages (album, gallery, or photo links; `album:KEY`) as
    /// candidates. With --comment they are selected instead.
    AddSmugmug {
        slug: String,
        #[arg(required = true)]
        targets: Vec<String>,
        /// Select them now with this comment, instead of offering them as candidates.
        #[arg(long)]
        comment: Option<String>,
    },
    /// Render a numbered contact sheet (JPEG) to choose from: candidates by default.
    ContactSheet {
        slug: String,
        /// Candidates (the default).
        #[arg(long)]
        candidates: bool,
        /// Selected photos.
        #[arg(long)]
        selected: bool,
        /// Rejected photos.
        #[arg(long)]
        rejected: bool,
        /// Everything.
        #[arg(long)]
        all: bool,
        #[arg(long, default_value_t = 1)]
        page: usize,
        #[arg(long, default_value_t = 24)]
        per_page: usize,
        /// Open the sheet in Preview.
        #[arg(long)]
        open: bool,
    },
    /// Render a contact sheet and open it in Preview (same options as contact-sheet).
    OpenSheet {
        slug: String,
        #[arg(long)]
        selected: bool,
        #[arg(long)]
        rejected: bool,
        #[arg(long)]
        all: bool,
        #[arg(long, default_value_t = 1)]
        page: usize,
        #[arg(long, default_value_t = 24)]
        per_page: usize,
    },
    /// Select photos by number (3,7-9,12), id, or `all`. Each selected photo needs a comment:
    /// give --comment (applies to all of them) or add them one by one with `comment`.
    Select {
        slug: String,
        refs: String,
        #[arg(long)]
        comment: Option<String>,
    },
    /// Put selected photos back among the candidates.
    Unselect { slug: String, refs: String },
    /// Reject photos (they stay listed as rejected; `select` brings them back).
    Reject { slug: String, refs: String },
    /// The story so far, in the author's words: appended by default, --set to replace.
    /// Use - to read from stdin.
    Narrative {
        slug: String,
        text: String,
        #[arg(long)]
        set: bool,
    },
    /// Record a question you are waiting on; it can be answered here (`answer`) or in the app.
    Ask { slug: String, question: String },
    /// Answer the oldest pending question (or --id N); records it in the transcript.
    Answer {
        slug: String,
        answer: String,
        #[arg(long)]
        id: Option<u32>,
    },
    /// Questions waiting for an answer.
    Pending { slug: String },
    /// What changed after a revision (who, what, when): the app, the CLI, or MCP.
    Changes {
        slug: String,
        #[arg(long, default_value_t = 0)]
        since: u64,
    },
    /// Wait for a change made elsewhere (e.g. an answer typed in the app).
    Wait {
        slug: String,
        /// Revision to wait from (default: now).
        #[arg(long)]
        since: Option<u64>,
        /// Give up after this many seconds.
        #[arg(long, default_value_t = 300)]
        timeout: u64,
        /// Wait until no question is pending.
        #[arg(long)]
        answer: bool,
    },
    /// Open the story's page in the gitwebsite app (in your browser while the app runs).
    OpenApp { slug: String },
    /// Set an image's caption.
    Caption { slug: String, id: String, text: String },
    /// Set an image's comment (what's happening, why it matters).
    Comment { slug: String, id: String, text: String },
    /// Leave an image out of the post (or --undo to include it again).
    Exclude {
        slug: String,
        id: String,
        #[arg(long)]
        undo: bool,
    },
    /// Remove an image from the story (deletes a local original's copy in the story).
    Remove { slug: String, id: String },
    /// Put images in this order (ids not listed follow in their current order).
    Reorder {
        slug: String,
        #[arg(required = true)]
        ids: Vec<String>,
    },
    /// Append to the story's notes (or --replace them).
    Note {
        slug: String,
        text: String,
        #[arg(long)]
        replace: bool,
    },
    /// Record one interview question and answer.
    Qa {
        slug: String,
        #[arg(long = "q")]
        question: String,
        #[arg(long = "a")]
        answer: String,
    },
    /// Save the agreed outline (from a file, - for stdin, or --text).
    Outline {
        slug: String,
        #[arg(long)]
        file: Option<PathBuf>,
        #[arg(long)]
        text: Option<String>,
    },
    /// Write the post from AsciiDoc that uses image::story:<id>[…] (and model::story:<id>[…] for 3-D models); rebuilds the preview.
    Draft {
        slug: String,
        #[arg(long)]
        file: Option<PathBuf>,
        #[arg(long)]
        text: Option<String>,
        /// Skip rebuilding the preview.
        #[arg(long)]
        no_preview: bool,
    },
    /// Fetch/make a review thumbnail for every image and print the files to look at.
    ReviewImages { slug: String },
    /// List images on a SmugMug page (album or gallery URL, AlbumKey, folder, or image URL).
    SmugmugBrowse {
        target: String,
        #[arg(long, default_value_t = 1)]
        start: usize,
        #[arg(long, default_value_t = 100)]
        count: usize,
    },
    /// Your SmugMug albums (for the picker).
    SmugmugAlbums,
    /// Enhance an existing post with Claude: start (or reopen) a story tied to it (see /enhance-post).
    Enhance {
        /// Content item id, slug, or URL (e.g. articles/my-first-post)
        item: String,
    },
    /// What Claude reads for an enhancement: the post, the author's answers, the repository.
    Context { slug: String },
    /// Answer one of the key questions (inspiration, why, how, hardest, learned, next, audience).
    Key { slug: String, id: String, answer: String },
    /// Point an enhancement at a repository URL and/or local checkout.
    Repo {
        slug: String,
        #[arg(long, default_value = "")]
        url: String,
        #[arg(long)]
        path: Option<String>,
    },
    /// An enhancement's proposal, as a diff against the post.
    Proposal { slug: String },
    /// Write the proposal over the post (keeps its path, status, and date; sets :updated:).
    Accept {
        slug: String,
        #[arg(long)]
        yes: bool,
    },
    /// Throw the proposal away.
    Discard { slug: String },
    /// Send the proposal back with what to change.
    RequestChanges { slug: String, text: String },
    /// Start, stop, or check a Claude session on an enhancement (as the app's Start Claude button).
    Claude {
        slug: String,
        /// start | stop | status
        #[arg(default_value = "status")]
        action: String,
    },
    /// Delete a story workspace (originals, transcript). A drafted post is kept.
    Delete {
        slug: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum IdeaCmd {
    List {
        #[arg(long)]
        status: Option<String>,
    },
    Add {
        title: String,
        #[arg(long, default_value = "")]
        notes: String,
        #[arg(long = "area")]
        areas: Vec<String>,
        #[arg(long, default_value = "")]
        kind: String,
        #[arg(long, default_value = "")]
        source: String,
    },
    /// Change status: idea, planned, drafting, review, published, dropped.
    Set { id: u32, status: String },
    /// Link an idea to the content item drafted for it.
    Link { id: u32, content: String },
}

#[derive(Subcommand)]
enum RegistryCmd {
    List {
        #[arg(long)]
        kind: Option<String>,
    },
    Show { id: String },
    /// Add or replace an entry from a YAML or JSON file.
    Put { file: PathBuf },
    Remove { id: String },
}

#[derive(Subcommand)]
enum MediaCmd {
    List,
    /// Register a URL (or site path) under a key.
    Add {
        key: String,
        url: String,
        #[arg(long, default_value = "image")]
        kind: String,
        #[arg(long, default_value = "")]
        alt: String,
        #[arg(long, default_value = "")]
        caption: String,
    },
    /// Add a 3-D model from the Meshy models repository: makes a web GLB (decimated, textured,
    /// Draco-compressed), a USDZ for AR on iOS, a poster, and optionally the print file into
    /// static/models/<key>/, and registers media:<key> for `model::media:<key>[]`. Needs Blender.
    AddModel {
        /// The model as <project>/<slug> (see `gw meshy models`).
        model: String,
        /// Media key (default: the slug).
        #[arg(long)]
        key: Option<String>,
        /// Offer readers the print file: stl (the print or plate STL), 3mf, or none.
        #[arg(long, default_value = "none", value_parser = ["stl", "3mf", "none"])]
        download: String,
        /// Triangles in the web model.
        #[arg(long, default_value_t = 150_000)]
        polycount: u64,
        /// Decimate the downloadable STL to this many triangles (≈50 bytes each).
        #[arg(long)]
        download_polycount: Option<u64>,
        /// Longest texture edge (px).
        #[arg(long, default_value_t = 2048)]
        max_texture: u32,
        /// Plain GLB (no Draco compression).
        #[arg(long)]
        no_draco: bool,
        /// No USDZ (no AR Quick Look on iOS).
        #[arg(long)]
        no_usdz: bool,
        /// Replace an existing key that holds something else.
        #[arg(long)]
        force: bool,
    },
}

fn print(json: bool, v: &Value) {
    if json {
        println!("{}", serde_json::to_string_pretty(v).unwrap());
    } else {
        println!("{}", human(v));
    }
}

/// Compact human rendering of the JSON results.
fn human(v: &Value) -> String {
    match v {
        Value::Array(items) => items
            .iter()
            .map(|i| {
                if let Some(t) = i.get("title") {
                    let id = i.get("id").map(|x| x.to_string().trim_matches('"').to_string()).unwrap_or_default();
                    let kind = i.get("kind").and_then(|x| x.as_str()).unwrap_or("");
                    let status = i.get("status").and_then(|x| x.as_str())
                        .or_else(|| i.get("draft").and_then(|d| d.as_bool()).map(|d| if d { "draft" } else { "" }))
                        .unwrap_or("");
                    format!("{id:<40} {kind:<11} {status:<9} {}", t.as_str().unwrap_or(""))
                } else if let Some(p) = i.get("path") {
                    format!("{:<34} {}", p.as_str().unwrap_or(""), i.get("title").and_then(|x| x.as_str()).unwrap_or(""))
                } else {
                    i.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => serde_yaml::to_string(v).unwrap_or_default().trim_end().to_string(),
    }
}

/// Human-readable story: what Claude (or the author) needs at a glance.
fn print_story(v: &Value) {
    let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
    println!("{}  [{}]  target: {}{}", s("title"), s("status"), s("target"),
        v["project"].as_str().map(|p| format!(" (project {p})")).unwrap_or_default());
    println!("slug: {}   folder: {}", s("slug"), s("dir"));
    let list = |k: &str| v[k].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
    println!("areas: {}   tags: {}", list("areas"), list("tags"));
    if let Some(d) = v.get("draft_item").filter(|d| !d.is_null()) {
        println!("draft: {} ({})  preview: {}", d["id"].as_str().unwrap_or(""), if d["draft"] == true { "draft" } else { "published" }, d["preview_url"].as_str().unwrap_or(""));
    }
    println!("rev {}   app: {}", v["rev"], s("app_url"));
    if !s("notes").trim().is_empty() {
        println!("\nNotes:\n{}", s("notes").trim());
    }
    if !s("narrative").trim().is_empty() {
        println!("\nThe story so far:\n{}", s("narrative").trim());
    }
    let all = v["images"].as_array().cloned().unwrap_or_default();
    let c = &v["counts"];
    println!("\nPhotos: {} selected ({} need a comment), {} candidates, {} rejected", c["selected"], c["needs_comment"], c["candidates"], c["rejected"]);
    if c["candidates"].as_u64().unwrap_or(0) > 0 {
        println!("  candidates: see `gw story contact-sheet {}`", s("slug"));
    }
    let imgs: Vec<_> = all.into_iter().filter(|im| im["state"] == "selected").collect();
    for im in imgs {
        let g = |k: &str| im[k].as_str().unwrap_or("").to_string();
        println!("  #{} {} {}{}{}  {}{}", im["number"], g("id"), if im["needs_comment"] == true { "(NEEDS COMMENT) " } else { "" }, if im["include"] == false { "(left out) " } else { "" }, g("source"),
            g("filename"), im["taken"].as_str().map(|t| format!("  taken {t}")).unwrap_or_default());
        if !g("caption").is_empty() { println!("      caption: {}", g("caption")); }
        if !g("notes").is_empty() { println!("      comment: {}", g("notes")); }
        if let Some(p) = im["review_path"].as_str() { println!("      look at: {p}"); }
    }
    if !s("interview").trim().is_empty() {
        println!("\n{}", s("interview").trim());
    }
    if !s("outline").trim().is_empty() {
        println!("\nOutline:\n{}", s("outline").trim());
    }
    for q in v["pending"].as_array().cloned().unwrap_or_default() {
        println!("\nWaiting for an answer: Q{} {}", q["id"], q["q"].as_str().unwrap_or(""));
    }
    println!("\nInterview in Claude Code: {}", s("interview_command"));
}

fn print_sheet(v: &Value) {
    println!("Contact sheet ({}, page {}/{}, {} in all): {}", v["which"].as_str().unwrap_or(""), v["page"], v["pages"], v["total"], v["path"].as_str().unwrap_or(""));
    for it in v["items"].as_array().cloned().unwrap_or_default() {
        let st = it["state"].as_str().unwrap_or("");
        let mark = match (st, it["needs_comment"] == true) { ("selected", true) => "  [selected, needs comment]", ("selected", false) => "  [selected]", ("rejected", _) => "  [rejected]", _ => "" };
        println!("  #{:<4} {}  {}{}", it["number"], it["filename"].as_str().unwrap_or(""), it["taken"].as_str().unwrap_or(""), mark);
    }
    println!("Select with: {}", v["select_with"].as_str().unwrap_or(""));
}

fn print_changes(v: &Value) {
    let ch = v["changes"].as_array().cloned().unwrap_or_default();
    if ch.is_empty() {
        println!("No changes since rev {} (now {}).", v["since"], v["rev"]);
    }
    for c in ch {
        println!("rev {:<4} {}  {:<4} {:<10} {}", c["rev"], c["at"].as_str().unwrap_or(""), c["by"].as_str().unwrap_or(""), c["kind"].as_str().unwrap_or(""), c["what"].as_str().unwrap_or(""));
    }
    for q in v["pending"].as_array().cloned().unwrap_or_default() {
        println!("waiting: Q{} {}", q["id"], q["q"].as_str().unwrap_or(""));
    }
    println!("now at rev {}", v["rev"]);
}

fn confirm(prompt: &str) -> bool {
    use std::io::Write;
    print!("{prompt} [y/N] ");
    std::io::stdout().flush().ok();
    let mut s = String::new();
    std::io::stdin().read_line(&mut s).ok();
    matches!(s.trim(), "y" | "Y" | "yes")
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    stories::set_actor(match cli.cmd { Cmd::Serve { .. } => "app", Cmd::Mcp => "mcp", _ => "cli" });
    let j = cli.json;
    // Commands that work without a site.
    match cli.cmd {
        Cmd::Companion(c) => {
            let v = companion_cli::run_companion(c)?;
            print(j, &v);
            return Ok(());
        }
        Cmd::Meshy(c) => {
            let v = integrations::meshy::run_cli(c, j)?;
            if !v.is_null() {
                print(j, &v);
            }
            return Ok(());
        }
        Cmd::Init { .. } | Cmd::Config(_) => {
            let v = match cli.cmd {
                Cmd::Init { dir, title, author, base_url, tagline, description, pages_repo, branch, pages_checkout, no_theme_copy, no_starter, git, create_pages_repo, no_current } => {
                    setup::init(setup::InitArgs { dir, title, author, base_url, tagline, description, pages_repo, branch, pages_checkout, no_theme_copy, no_starter, git, create_pages_repo, no_current })?
                }
                Cmd::Config(c) => match c {
                    ConfigCmd::Show => setup::show()?,
                    ConfigCmd::AddSite { path, name, current } => setup::add_site(&path, name.as_deref(), current)?,
                    ConfigCmd::Use { which } => setup::use_site(&which)?,
                    ConfigCmd::Remove { which } => setup::remove_site(&which)?,
                    ConfigCmd::Set { key, value } => setup::set(&key, &value)?,
                },
                _ => unreachable!(),
            };
            print(j, &v);
            return Ok(());
        }
        Cmd::Serve { port, open, no_companion, companion_port, companion_auto_approve } => {
            // Without a site the app opens on its setup screen.
            let root = match &cli.site {
                Some(p) => Some(Site::open(Some(p))?.root),
                None => gw_core::userconfig::resolve_site().ok(),
            };
            let rt = tokio::runtime::Runtime::new()?;
            let comp = (!no_companion).then(|| companion::StartOptions { ui_port: None, port: companion_port, auto_approve: companion_auto_approve });
            return rt.block_on(server::run_with(root, port, open, comp));
        }
        _ => {}
    }
    let site = Site::open(cli.site.as_deref())?;
    let v = match cli.cmd {
        Cmd::Serve { .. } | Cmd::Init { .. } | Cmd::Config(_) | Cmd::Companion(_) | Cmd::Meshy(_) => unreachable!(),
        Cmd::Tools(c) => {
            let v = companion_cli::run_tools(&site.root, &site.config.site.base_url, c)?;
            if v.is_null() {
                return Ok(());
            }
            v
        }
        Cmd::Mcp => return mcp::run(site.root.clone()),
        Cmd::Site => {
            let c = &site.config;
            json!({
                "title": c.site.title, "author": c.site.author, "tagline": c.site.tagline, "base_url": c.site.base_url,
                "root": site.root.display().to_string(), "content": site.content_dir().display().to_string(),
                "publish": c.publish, "areas": c.flat_areas().iter().map(|(p, _)| p.clone()).collect::<Vec<_>>(),
            })
        }
        Cmd::Smugmug(c) => integrations::smugmug::run(&site, c)?,
        Cmd::Models(c) => integrations::site_models::run_cli(&site, c)?,
        Cmd::Wordpress(c) => integrations::wordpress::run(&site, c)?,
        Cmd::Build { drafts } => ops::do_build(&site, drafts)?,
        Cmd::Check => {
            let v = ops::check(&site)?;
            if !j && v["ok"].as_bool() == Some(true) {
                println!("ok");
                return Ok(());
            }
            v
        }
        Cmd::List { kind, area, tag, drafts } => ops::list(&site, &ops::ListArgs { kind, area, tag, drafts_only: drafts })?,
        Cmd::Show { id } => {
            let v = ops::show(&site, &id)?;
            if !j {
                println!("{}\n\n{}", v["path"].as_str().unwrap_or(""), v["source_text"].as_str().unwrap_or(""));
                return Ok(());
            }
            v
        }
        Cmd::New { kind, title, areas, tags, project, summary, repo, url, bundle } => ops::new_item(
            &site,
            &ops::NewArgs { kind, title, areas, tags, project, summary, repo, url, body: None, bundle },
        )?,
        Cmd::Release { id } => ops::set_status(&site, &id, "published")?,
        Cmd::Unrelease { id } => ops::set_status(&site, &id, "draft")?,
        Cmd::Areas => ops::areas(&site)?,
        Cmd::Story(c) => match c {
            StoryCmd::New { title, target, project, areas, tags, notes, slug } => stories::create(
                &site,
                &gw_core::story::NewStory { title, slug, target: Some(target), project, areas, tags, notes },
            )?,
            StoryCmd::List => {
                let v = stories::list(&site)?;
                if !j {
                    for s in v.as_array().unwrap() {
                        println!("{:<40} {:<12} {:<8} {:>3} images  {}", s["slug"].as_str().unwrap_or(""), s["status"].as_str().unwrap_or(""),
                            s["target"].as_str().unwrap_or(""), s["images"], s["title"].as_str().unwrap_or(""));
                    }
                    return Ok(());
                }
                v
            }
            StoryCmd::Show { slug } => {
                let v = stories::show(&site, &slug)?;
                if !j {
                    print_story(&v);
                    return Ok(());
                }
                v
            }
            StoryCmd::Set { slug, title, target, project, areas, tags, status } => stories::update(
                &site,
                &slug,
                &gw_core::story::StoryUpdate { title, target, project, areas, tags, notes: None, status },
            )?,
            StoryCmd::Add { slug, sources, caption, comment, candidate } => stories::add(&site, &slug, &stories::AddArgs { sources, caption, comment, candidate })?,
            StoryCmd::AddPhotos { slug, album, candidate, chosen, limit } => {
                // An album may be given by name or id.
                let album = match album {
                    Some(a) if !a.contains("/L0/") => {
                        let all = gw::integrations::photos::albums()?;
                        let hits: Vec<_> = all.iter().filter(|x| x.name.eq_ignore_ascii_case(&a) || x.name.rsplit(" / ").next().is_some_and(|n| n.eq_ignore_ascii_case(&a))).collect();
                        match hits.as_slice() {
                            [one] => Some(one.id.clone()),
                            [] => anyhow::bail!("no album called \"{a}\" in Photos (gw story photos-albums lists them)"),
                            _ => anyhow::bail!("several albums are called \"{a}\": give the full path or the id (gw story photos-albums)"),
                        }
                    }
                    other => other,
                };
                let cand = if candidate { Some(true) } else if chosen { Some(false) } else { None };
                let v = stories::add_photos(&site, &slug, &stories::AddPhotosArgs { album, candidate: cand, limit })?;
                if !j {
                    let nums = v["numbers"].as_str().unwrap_or("");
                    println!("Added {} {} from Apple Photos{}{}.", v["added"], if v["as"] == "candidates" { "candidate photo(s)" } else { "photo(s)" },
                        if nums.is_empty() { String::new() } else { format!(" {nums}") },
                        if v["already_in_story"].as_u64().unwrap_or(0) > 0 { format!(" ({} already in the story)", v["already_in_story"]) } else { String::new() });
                    if v["videos_skipped"].as_u64().unwrap_or(0) > 0 { println!("{} video(s) skipped (stories use photos).", v["videos_skipped"]); }
                    println!("Next: {}", v["next"].as_str().unwrap_or(""));
                    return Ok(());
                }
                v
            }
            StoryCmd::PhotosAlbums => {
                let v = stories::photos_albums()?;
                if !j {
                    for a in v["albums"].as_array().into_iter().flatten() {
                        println!("{:>5}  {}", a["count"], a["name"].as_str().unwrap_or(""));
                    }
                    return Ok(());
                }
                v
            }
            StoryCmd::AddDir { slug, dirs, recursive, since, limit } => {
                let v = stories::add_dir(&site, &slug, &stories::AddDirArgs { dirs, recursive, since, limit })?;
                if !j {
                    println!("Added {} candidate photo(s) {} ({} already in the story, {} left out by --since/--limit).",
                        v["added"], v["numbers"].as_str().unwrap_or(""), v["already_in_story"], v["filtered_out"]);
                    let vids = v["videos_not_added"].as_array().map(|a| a.len()).unwrap_or(0);
                    if vids > 0 { println!("{vids} video(s) found and not added (stories use photos)."); }
                    println!("Next: {}", v["next"].as_str().unwrap_or(""));
                    return Ok(());
                }
                v
            }
            StoryCmd::AddSmugmug { slug, targets, comment } => {
                let candidate = comment.is_none();
                stories::add(&site, &slug, &stories::AddArgs { sources: targets, caption: None, comment, candidate })?
            }
            StoryCmd::ContactSheet { slug, candidates: _, selected, rejected, all, page, per_page, open } => {
                let which = if all { "all" } else if selected { "selected" } else if rejected { "rejected" } else { "candidates" };
                let v = stories::contact_sheet(&site, &slug, which, page, per_page)?;
                if open {
                    let _ = ::open::that(v["path"].as_str().unwrap_or(""));
                }
                if !j {
                    print_sheet(&v);
                    return Ok(());
                }
                v
            }
            StoryCmd::OpenSheet { slug, selected, rejected, all, page, per_page } => {
                let which = if all { "all" } else if selected { "selected" } else if rejected { "rejected" } else { "candidates" };
                let v = stories::contact_sheet(&site, &slug, which, page, per_page)?;
                ::open::that(v["path"].as_str().unwrap_or(""))?;
                if !j {
                    print_sheet(&v);
                    return Ok(());
                }
                v
            }
            StoryCmd::Select { slug, refs, comment } => {
                let v = stories::select(&site, &slug, &refs, comment.as_deref())?;
                if !j {
                    let sel = v["selected"].as_array().cloned().unwrap_or_default();
                    println!("Selected {}.", sel.iter().map(|x| format!("#{}", x["number"])).collect::<Vec<_>>().join(", "));
                    let need = v["needs_comment"].as_array().cloned().unwrap_or_default();
                    if need.is_empty() { println!("Every selected photo has a comment."); }
                    else { println!("Still needing a comment: {}\n{}", need.iter().map(|n| format!("#{n}")).collect::<Vec<_>>().join(", "), v["hint"].as_str().unwrap_or("")); }
                    return Ok(());
                }
                v
            }
            StoryCmd::Unselect { slug, refs } if !j => {
                let v = stories::set_state(&site, &slug, &refs, "candidate")?;
                println!("Moved back to the candidates: {}.", v["numbers"].as_str().unwrap_or(""));
                return Ok(());
            }
            StoryCmd::Reject { slug, refs } if !j => {
                let v = stories::set_state(&site, &slug, &refs, "rejected")?;
                println!("Rejected {}.", v["numbers"].as_str().unwrap_or(""));
                return Ok(());
            }
            StoryCmd::Unselect { slug, refs } => stories::set_state(&site, &slug, &refs, "candidate")?,
            StoryCmd::Reject { slug, refs } => stories::set_state(&site, &slug, &refs, "rejected")?,
            StoryCmd::Narrative { slug, text, set } if !j => {
                let text = if text == "-" { stories::read_text_arg(Some(std::path::Path::new("-")), None)? } else { text };
                let v = stories::narrative(&site, &slug, &text, !set, None)?;
                println!("{} the story so far ({} characters, rev {}).", if set { "Replaced" } else { "Added to" }, v["narrative"].as_str().map(|n| n.chars().count()).unwrap_or(0), v["rev"]);
                return Ok(());
            }
            StoryCmd::Narrative { slug, text, set } => {
                let text = if text == "-" { stories::read_text_arg(Some(std::path::Path::new("-")), None)? } else { text };
                stories::narrative(&site, &slug, &text, !set, None)?
            }
            StoryCmd::Ask { slug, question } => {
                let v = stories::ask(&site, &slug, &question)?;
                if !j {
                    println!("Waiting on Q{} (rev {}). It can be answered here (`gw story answer {slug} \"…\"`) or in the app: {}", v["id"], v["rev"], v["answer_in_app"].as_str().unwrap_or(""));
                    return Ok(());
                }
                v
            }
            StoryCmd::Answer { slug, answer, id } => {
                let v = stories::answer(&site, &slug, id, &answer)?;
                if !j {
                    let left = v["pending"].as_array().map(|a| a.len()).unwrap_or(0);
                    println!("Recorded the answer to Q{} (transcript Q{}).{}", v["answered"], v["recorded"], if left > 0 { format!(" {left} question(s) still waiting.") } else { String::new() });
                    return Ok(());
                }
                v
            }
            StoryCmd::Pending { slug } => {
                let v = stories::pending(&site, &slug)?;
                if !j {
                    let p = v["pending"].as_array().cloned().unwrap_or_default();
                    if p.is_empty() { println!("No questions waiting (rev {}).", v["rev"]); }
                    for q in p { println!("Q{} (asked {}): {}", q["id"], q["asked"].as_str().unwrap_or(""), q["q"].as_str().unwrap_or("")); }
                    return Ok(());
                }
                v
            }
            StoryCmd::Changes { slug, since } => {
                let v = stories::changes(&site, &slug, since)?;
                if !j {
                    print_changes(&v);
                    return Ok(());
                }
                v
            }
            StoryCmd::Wait { slug, since, timeout, answer } => {
                let v = stories::wait(&site, &slug, since, timeout, answer)?;
                if !j {
                    if v["changed"] == true { print_changes(&v); } else { println!("Nothing changed in {timeout}s (rev {}).", v["rev"]); }
                    return Ok(());
                }
                v
            }
            StoryCmd::OpenApp { slug } => {
                let s = stories::show(&site, &slug)?;
                let url = s["app_url"].as_str().unwrap_or("").to_string();
                ::open::that(&url)?;
                json!({ "opened": url })
            }
            StoryCmd::Caption { slug, id, text } => stories::image_update(&site, &slug, &id, &stories::ImageUpdate { caption: Some(text), ..Default::default() })?,
            StoryCmd::Comment { slug, id, text } => {
                let v = stories::comment(&site, &slug, &id, &text)?;
                if !j {
                    println!("Comment saved on #{} ({}).", v["number"], v["id"].as_str().unwrap_or(""));
                    let s = stories::show(&site, &slug)?;
                    let need: Vec<String> = s["needs_comment"].as_array().cloned().unwrap_or_default().iter().map(|n| format!("#{}", n["number"])).collect();
                    if need.is_empty() { println!("Every selected photo has a comment."); } else { println!("Still needing a comment: {}", need.join(", ")); }
                    return Ok(());
                }
                v
            }
            StoryCmd::Exclude { slug, id, undo } => stories::image_update(&site, &slug, &id, &stories::ImageUpdate { include: Some(undo), ..Default::default() })?,
            StoryCmd::Remove { slug, id } => stories::image_remove(&site, &slug, &id)?,
            StoryCmd::Reorder { slug, ids } => stories::reorder(&site, &slug, &ids)?,
            StoryCmd::Note { slug, text, replace } => stories::note(&site, &slug, &text, replace)?,
            StoryCmd::Qa { slug, question, answer } => stories::qa(&site, &slug, &question, &answer)?,
            StoryCmd::Outline { slug, file, text } => stories::outline(&site, &slug, &stories::read_text_arg(file.as_deref(), text.as_deref())?)?,
            StoryCmd::Draft { slug, file, text, no_preview } => {
                stories::draft(&site, &slug, &stories::read_text_arg(file.as_deref(), text.as_deref())?, !no_preview)?
            }
            StoryCmd::ReviewImages { slug } => stories::review_images(&site, &slug)?,
            StoryCmd::SmugmugBrowse { target, start, count } => stories::smugmug_browse(&target, start, count)?,
            StoryCmd::SmugmugAlbums => stories::smugmug_albums()?,
            StoryCmd::Enhance { item } => gw::enhance::start(&site, &item)?,
            StoryCmd::Context { slug } => gw::enhance::context(&site, &slug)?,
            StoryCmd::Key { slug, id, answer } => gw::enhance::key_answers(&site, &slug, &std::collections::BTreeMap::from([(id, answer)]))?,
            StoryCmd::Repo { slug, url, path } => gw::enhance::set_repo(&site, &slug, &url, path.as_deref())?,
            StoryCmd::Proposal { slug } if !j => {
                let v = gw::enhance::proposal(&site, &slug)?;
                for d in v["diff"].as_array().into_iter().flatten() {
                    let op = d["op"].as_str().unwrap_or(" ");
                    if op != " " {
                        println!("{op} {}", d["text"].as_str().unwrap_or(""));
                    }
                }
                println!("({} added, {} removed)", v["added"], v["removed"]);
                return Ok(());
            }
            StoryCmd::Proposal { slug } => gw::enhance::proposal(&site, &slug)?,
            StoryCmd::Accept { slug, yes } => {
                if !yes && !confirm(&format!("Write the proposal over the post for {slug}?")) {
                    println!("cancelled");
                    return Ok(());
                }
                gw::enhance::accept(&site, &slug)?
            }
            StoryCmd::Discard { slug } => gw::enhance::discard(&site, &slug)?,
            StoryCmd::RequestChanges { slug, text } => gw::enhance::request_changes(&site, &slug, &text)?,
            StoryCmd::Claude { slug, action } => match action.as_str() {
                "start" => gw::enhance::claude_start(&site, &slug)?,
                "stop" => gw::enhance::claude_stop(&site, &slug)?,
                "status" | "log" => gw::enhance::claude_status(&site, &slug, 200)?,
                a => anyhow::bail!("unknown action {a}: start, stop, or status"),
            },
            StoryCmd::Delete { slug, yes } => {
                if !yes && !confirm(&format!("Delete story {slug} (its originals and interview)? A drafted post is kept.")) {
                    println!("cancelled");
                    return Ok(());
                }
                stories::delete(&site, &slug)?
            }
        },
        Cmd::Idea(c) => match c {
            IdeaCmd::List { status } => ops::ideas_list(&site, status.as_deref())?,
            IdeaCmd::Add { title, notes, areas, kind, source } => {
                ops::idea_add(&site, &ops::IdeaArgs { title, notes, areas, kind, source })?
            }
            IdeaCmd::Set { id, status } => ops::idea_update(&site, id, &ops::IdeaUpdate { status: Some(status), ..Default::default() })?,
            IdeaCmd::Link { id, content } => ops::idea_update(&site, id, &ops::IdeaUpdate { content: Some(content), ..Default::default() })?,
        },
        Cmd::Registry(c) => match c {
            RegistryCmd::List { kind } => ops::registry_list(&site, kind.as_deref())?,
            RegistryCmd::Show { id } => serde_json::to_value(site.registry()?.get(&id).ok_or_else(|| anyhow::anyhow!("no entry {id}"))?)?,
            RegistryCmd::Put { file } => {
                let e: Entry = serde_yaml::from_str(&std::fs::read_to_string(file)?)?;
                ops::registry_upsert(&site, e)?
            }
            RegistryCmd::Remove { id } => ops::registry_remove(&site, &id)?,
        },
        Cmd::Media(c) => match c {
            MediaCmd::List => ops::media_list(&site)?,
            MediaCmd::Add { key, url, kind, alt, caption } => {
                let provider = if url.starts_with('/') { "local" } else { "external" };
                ops::media_add(&site, &key, Media { url, kind, provider: provider.into(), thumb: String::new(), alt, caption, ids: Default::default(), ..Default::default() })?
            }
            MediaCmd::AddModel { model, key, download, polycount, download_polycount, max_texture, no_draco, no_usdz, force } => {
                use gw::integrations::model_assets::{add_model, AddModel};
                if !j {
                    eprintln!("Making web assets for {model} with Blender…");
                }
                add_model(&site, &AddModel { model, key, download, polycount, download_polycount, max_texture, draco: !no_draco, no_usdz, force })?
            }
        },
        Cmd::Publish { dry_run, no_push, yes, message } => {
            let plan = ops::publish_plan(&site)?;
            let (a, u, r) = (plan["added"].as_array().map_or(0, |x| x.len()), plan["updated"].as_array().map_or(0, |x| x.len()), plan["removed"].as_array().map_or(0, |x| x.len()));
            if dry_run {
                plan
            } else {
                if !j {
                    println!("Publishing to {}: {a} added, {u} updated, {r} removed.", plan["pages_repo"].as_str().unwrap_or(""));
                }
                if !yes && !confirm(if no_push { "Commit to the Pages repo?" } else { "Commit and push to the live site?" }) {
                    println!("cancelled");
                    return Ok(());
                }
                ops::publish_site(&site, message.as_deref(), !no_push)?
            }
        }
    };
    print(j, &v);
    Ok(())
}

#[cfg(test)]
mod cli_tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    #[test]
    fn cli_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    /// A subcommand argument named like a global option (`--site`) made clap panic.
    #[test]
    fn config_use_and_remove_parse() {
        for argv in [["gw", "config", "use", "my-site"], ["gw", "config", "remove", "/tmp/x"]] {
            Cli::try_parse_from(argv).expect("parses");
        }
    }
}
