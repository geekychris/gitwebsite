//! Downloads from the app's windows (a tool's *Save* or *Export* in the preview, a file link in the
//! management UI). WKWebView refuses every download unless the webview has a handler, so without
//! this they failed silently. Files go to ~/Downloads under the page's suggested name — wry picks
//! the folder and appends " (1)", " (2)" … like a browser — and a notification says where.
//!
//! A save panel was considered: the destination has to be decided synchronously inside WebKit's
//! callback on the main thread, where a modal panel from tauri-plugin-dialog can deadlock.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::webview::DownloadEvent;
use tauri::{Runtime, Webview};

/// A download handler for a window on `base` (http://127.0.0.1:<port>): downloads of the app's own
/// pages, and blob: / data: URLs made by them, are saved; anything else is refused.
pub fn handler<R: Runtime>(base: String) -> impl Fn(Webview<R>, DownloadEvent<'_>) -> bool + Send + Sync + 'static {
    // url → destination: on macOS the Finished event carries no path
    let pending: Arc<Mutex<HashMap<String, PathBuf>>> = Arc::default();
    move |_webview, event| match event {
        DownloadEvent::Requested { url, destination } => {
            let u = url.as_str();
            let ours = u.starts_with(&base) || u.starts_with("data:") || u.strip_prefix("blob:").is_some_and(|b| b.starts_with(&base));
            if !ours {
                eprintln!("gitwebsite: download refused (not from the app): {u}");
                return false;
            }
            pending.lock().unwrap().insert(u.to_string(), destination.clone());
            true
        }
        DownloadEvent::Finished { url, path, success } => {
            let dest = pending.lock().unwrap().remove(url.as_str()).or(path);
            match (success, dest) {
                (true, Some(p)) => notify_saved(&p),
                (true, None) => {}
                (false, p) => eprintln!("gitwebsite: download failed: {} → {}", url, p.map(|p| p.display().to_string()).unwrap_or_default()),
            }
            true
        }
        _ => true,
    }
}

/// A notification "Saved <name> in Downloads" (macOS); a line on stderr elsewhere.
fn notify_saved(path: &std::path::Path) {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let folder = path.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    eprintln!("gitwebsite: saved {}", path.display());
    #[cfg(target_os = "macos")]
    {
        let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        let script = format!("display notification \"{}\" with title \"gitwebsite\" subtitle \"Saved in {}\"", esc(&name), esc(&folder));
        std::thread::spawn(move || {
            let _ = std::process::Command::new("osascript").arg("-e").arg(script).status();
        });
    }
}
