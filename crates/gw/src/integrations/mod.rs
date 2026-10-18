//! External services: SmugMug (media hosting), WordPress (import and cross-posting), and
//! Meshy.ai (3D models, synced into a git repository by that repository's own tool).
//! Credentials live in the macOS Keychain, entered by the user in a terminal — never in
//! arguments, files, or chat.

pub mod html2adoc;
pub mod keychain;
pub mod meshy;
pub mod model_assets;
pub mod oauth1;
pub mod photos;
pub mod site_models;
pub mod smugmug;
pub mod wordpress;
pub mod xmlrpc;

use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum SmugmugCmd {
    /// Authorise gitwebsite with SmugMug. Interactive by default; or in two steps:
    /// `auth --credentials-file f --start` (opens the approval page), then `auth --verifier CODE`.
    Auth {
        /// File holding "API Key: ..." and "API Secret: ..." lines; stored in the Keychain.
        #[arg(long)]
        credentials_file: Option<PathBuf>,
        /// Step 1: get a request token and open SmugMug's approval page.
        #[arg(long, conflicts_with = "verifier")]
        start: bool,
        /// Step 2: the six-digit code SmugMug showed after approval.
        #[arg(long)]
        verifier: Option<String>,
    },
    /// Show the authorised SmugMug user.
    Whoami,
    /// List your albums (name, AlbumKey, web address).
    Albums,
    /// Create an album. Unlisted by default: anyone with the link can view it, but it is not
    /// listed on your SmugMug pages or in SmugMug or Google search.
    CreateAlbum {
        /// Album name (e.g. "geekychris.github.io media").
        name: String,
        /// URL name (default: derived from the name).
        #[arg(long)]
        url_name: Option<String>,
        /// unlisted | public | private
        #[arg(long, default_value = "unlisted")]
        privacy: String,
        /// Make it the site's media album: written to site.yaml as `smugmug: { album: KEY }`, and
        /// `gw smugmug upload` uses it when --album is not given.
        #[arg(long)]
        default: bool,
    },
    /// Upload an image or video to an album and register it in the media library.
    Upload {
        file: PathBuf,
        /// AlbumKey (e.g. "ZbcdEf") or an album URI (/api/v2/album/ZbcdEf). Default: the site's
        /// media album (`smugmug: { album: KEY }` in site.yaml, set by `create-album --default`).
        #[arg(long)]
        album: Option<String>,
        /// Media key to register it under (default: derived from the file name).
        #[arg(long)]
        key: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        caption: Option<String>,
        #[arg(long)]
        alt: Option<String>,
    },
    /// Register an existing SmugMug image (ImageKey or URL) without uploading.
    Import {
        /// ImageKey, or a SmugMug URL ending in /i-<ImageKey>.
        image: String,
        #[arg(long)]
        key: String,
        #[arg(long)]
        alt: Option<String>,
        #[arg(long)]
        caption: Option<String>,
    },
    /// Look a registered SmugMug media item up again, e.g. once a video has finished processing
    /// and has a playable URL. Keeps its alt text and caption.
    Refresh {
        /// The media key (as in `media:<key>`).
        key: String,
    },
    /// Register every image in an album as <prefix>-1, <prefix>-2, ...
    SyncAlbum {
        album: String,
        #[arg(long)]
        prefix: String,
    },
    /// Remove stored SmugMug credentials from the Keychain.
    Logout,
    /// Signed, read-only GET of an API path (for debugging), e.g. `/api/v2!authuser`.
    #[command(hide = true)]
    Get { path: String },
}

#[derive(Subcommand, Debug)]
pub enum WordpressCmd {
    /// Store WordPress credentials for cross-posting (interactive; run in a terminal).
    Auth {
        /// Site host, e.g. myblog.wordpress.com or example.com.
        #[arg(long)]
        host: String,
    },
    /// Import public posts as AsciiDoc articles (drafts by default).
    Import {
        /// Site host or URL to fetch from (WordPress.com public API, then /wp-json).
        #[arg(long, conflicts_with = "wxr")]
        from: Option<String>,
        /// Import from a WordPress export (WXR) file instead.
        #[arg(long)]
        wxr: Option<PathBuf>,
        /// Only posts on or after this date (YYYY-MM-DD).
        #[arg(long)]
        since: Option<String>,
        /// draft or published.
        #[arg(long, default_value = "draft")]
        status: String,
        /// Download images into the article bundle instead of linking to them.
        #[arg(long)]
        download_images: bool,
        /// Stop after this many posts.
        #[arg(long)]
        limit: Option<usize>,
        /// Show what would happen; write nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Cross-post a published item to WordPress (as a draft there unless --status publish).
    Crosspost {
        id: String,
        #[arg(long, default_value = "draft")]
        status: String,
    },
    /// Remove stored WordPress credentials from the Keychain.
    Logout,
}

#[derive(Subcommand, Debug)]
pub enum MeshyCmd {
    /// Last sync, next scheduled one, credit balance, and counts: models, new ones available
    /// free, expired ones not recovered (and what recovering them would cost).
    Status,
    /// Run the models repository's sync tool now and record the result. Without --recover it
    /// only downloads new models still inside Meshy's free 3-day window.
    Sync {
        /// Also recover expired models — a paid remesh, 5 credits each. Needs --max-credits.
        #[arg(long, requires = "max_credits")]
        recover: bool,
        /// The most this run may spend (with --recover). The sync tool also never lets the
        /// balance fall below its --min-balance (250).
        #[arg(long)]
        max_credits: Option<u64>,
        /// List and plan only; change nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// The models in the repository (project/slug, triangles, print size, parts, name).
    Models {
        #[arg(long)]
        project: Option<String>,
    },
    /// Show or change the settings: the repository, the schedule (hours), on or off.
    Config {
        /// The Meshy models repository (holding tools/sync).
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Hours between scheduled syncs in `gw serve` / the app (default 24).
        #[arg(long)]
        interval: Option<u32>,
        /// Turn the scheduled sync on.
        #[arg(long, conflicts_with = "disable")]
        enable: bool,
        /// Turn the scheduled sync off (on-demand syncs still work).
        #[arg(long)]
        disable: bool,
    },
}
