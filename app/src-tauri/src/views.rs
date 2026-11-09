//! Tool windows for the viewer (`POST /api/view`, MCP `tool_open`): each shows one of the site's
//! tools from the app's own preview. Windows are reused per tool unless a new one is asked for;
//! links leaving the preview open in the default browser. Screenshots use WKWebView's snapshot.

use anyhow::{anyhow, bail, Result};
use gw::companion::view::{ViewHost, ViewWindow};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

pub struct Views {
    pub app: AppHandle,
    pub base: String,
    wins: Mutex<Vec<ViewWindow>>,
    seq: AtomicU32,
}

impl Views {
    pub fn new(app: AppHandle, base: String) -> Self {
        Views { app, base, wins: Mutex::new(vec![]), seq: AtomicU32::new(0) }
    }

    /// Forget windows the user has closed.
    fn prune(&self) {
        self.wins.lock().unwrap().retain(|w| self.app.get_webview_window(&w.id).is_some());
    }
}

impl ViewHost for Views {
    fn open(&self, tool: &str, url: &str, title: &str, target: &str) -> Result<ViewWindow> {
        self.prune();
        let parsed: tauri::Url = url.parse()?;
        let existing = {
            let wins = self.wins.lock().unwrap();
            match target {
                "new" => None,
                "reuse" | "" => wins.iter().rev().find(|w| w.tool == tool).cloned(),
                id => Some(wins.iter().find(|w| w.id == id).cloned().ok_or_else(|| anyhow!("no window {id:?} (see tool_windows)"))?),
            }
        };
        if let Some(mut rec) = existing {
            let w = self.app.get_webview_window(&rec.id).ok_or_else(|| anyhow!("window {} has closed", rec.id))?;
            w.navigate(parsed)?;
            let _ = w.set_title(title);
            let _ = w.show();
            let _ = w.unminimize();
            let _ = w.set_focus();
            rec.tool = tool.into();
            rec.title = title.into();
            rec.url = url.into();
            let mut wins = self.wins.lock().unwrap();
            if let Some(r) = wins.iter_mut().find(|r| r.id == rec.id) {
                *r = rec.clone();
            }
            return Ok(rec);
        }
        let id = format!("view-{}", self.seq.fetch_add(1, Ordering::Relaxed) + 1);
        let base = self.base.clone();
        WebviewWindowBuilder::new(&self.app, &id, WebviewUrl::External(parsed))
            .title(title)
            .inner_size(1200.0, 860.0)
            .min_inner_size(480.0, 360.0)
            .on_navigation(move |u| {
                if u.as_str().starts_with(&base) || matches!(u.scheme(), "about" | "blob" | "data") {
                    return true;
                }
                if matches!(u.scheme(), "http" | "https") {
                    let s = u.to_string();
                    std::thread::spawn(move || {
                        let _ = std::process::Command::new("open").arg(s).status();
                    });
                }
                false
            })
            .build()?;
        let rec = ViewWindow { id, tool: tool.into(), title: title.into(), url: url.into() };
        self.wins.lock().unwrap().push(rec.clone());
        Ok(rec)
    }

    fn list(&self) -> Vec<ViewWindow> {
        self.prune();
        let mut v = self.wins.lock().unwrap().clone();
        for w in v.iter_mut() {
            if let Some(win) = self.app.get_webview_window(&w.id) {
                if let Ok(u) = win.url() {
                    w.url = u.to_string();
                }
            }
        }
        v
    }

    fn close(&self, id: &str) -> Result<()> {
        if !id.starts_with("view-") {
            bail!("no window {id:?}");
        }
        let w = self.app.get_webview_window(id).ok_or_else(|| anyhow!("no window {id:?}"))?;
        w.destroy()?;
        self.wins.lock().unwrap().retain(|r| r.id != id);
        Ok(())
    }

    fn screenshot(&self, id: &str) -> Result<Vec<u8>> {
        if !id.starts_with("view-") {
            bail!("no window {id:?}");
        }
        let w = self.app.get_webview_window(id).ok_or_else(|| anyhow!("no window {id:?}"))?;
        snapshot(&w)
    }
}

#[cfg(target_os = "macos")]
fn snapshot(w: &tauri::WebviewWindow) -> Result<Vec<u8>> {
    use std::sync::mpsc;
    let (tx, rx) = mpsc::channel::<Result<Vec<u8>, String>>();
    w.with_webview(move |pw| {
        use block2::RcBlock;
        use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage};
        use objc2_foundation::{NSDictionary, NSError};
        use objc2_web_kit::WKWebView;
        // SAFETY: wry hands us its WKWebView; we are on the main thread inside with_webview.
        let view: &WKWebView = unsafe { &*(pw.inner() as *const WKWebView) };
        let tx = tx.clone();
        let block = RcBlock::new(move |img: *mut NSImage, err: *mut NSError| {
            let r = (|| -> Result<Vec<u8>, String> {
                if img.is_null() {
                    let msg = if err.is_null() { "snapshot failed".to_string() } else { unsafe { (*err).localizedDescription().to_string() } };
                    return Err(msg);
                }
                let img: &NSImage = unsafe { &*img };
                let tiff = img.TIFFRepresentation().ok_or("no image data")?;
                let rep = NSBitmapImageRep::imageRepWithData(&tiff).ok_or("cannot read the snapshot")?;
                let props = NSDictionary::new();
                let png = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) }.ok_or("PNG encoding failed")?;
                Ok(png.to_vec())
            })();
            let _ = tx.send(r);
        });
        unsafe { view.takeSnapshotWithConfiguration_completionHandler(None, &block) };
    })?;
    match rx.recv_timeout(std::time::Duration::from_secs(15)) {
        Ok(Ok(png)) => Ok(png),
        Ok(Err(e)) => bail!("snapshot: {e}"),
        Err(_) => bail!("snapshot timed out"),
    }
}

#[cfg(not(target_os = "macos"))]
fn snapshot(_w: &tauri::WebviewWindow) -> Result<Vec<u8>> {
    bail!("not supported: screenshots are only implemented on macOS")
}
