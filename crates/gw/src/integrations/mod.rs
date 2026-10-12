//! External services: SmugMug (media hosting) and WordPress (import and cross-posting).
//! Credentials live in the macOS Keychain, entered by the user in a terminal — never in
//! arguments, files, or chat.

pub mod html2adoc;
pub mod keychain;
pub mod oauth1;
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
    /// Upload an image or video to an album and register it in the media library.
    Upload {
        file: PathBuf,
        /// AlbumKey (e.g. "ZbcdEf") or an album URI (/api/v2/album/ZbcdEf).
        #[arg(long)]
        album: String,
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
