//! The app's [`NativeHost`]: what the companion's /v1/native endpoints need from the app's main
//! thread. Panels and files are in native_panels.rs; window chrome, menus, notifications and
//! menu-bar items in native_window.rs.

use crate::{native_panels, native_window};
use anyhow::Result;
use gw::companion::native::host::{NativeHost, Notice, PanelOptions, WindowPatch};
use serde_json::Value;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

pub struct AppNative {
    pub app: AppHandle,
}

impl NativeHost for AppNative {
    fn open_panel(&self, o: &PanelOptions) -> Result<Vec<PathBuf>> {
        native_panels::open_panel(&self.app, o)
    }
    fn save_panel(&self, o: &PanelOptions) -> Result<Option<PathBuf>> {
        native_panels::save_panel(&self.app, o)
    }
    fn notify(&self, n: &Notice) -> Result<()> {
        native_window::notify(&self.app, n)
    }
    fn window(&self, id: &str, p: &WindowPatch) -> Result<()> {
        native_window::window(&self.app, id, p)
    }
    fn status_item(&self, id: &str, text: Option<&str>, menu: Option<&Value>) -> Result<()> {
        native_window::status_item(&self.app, id, text, menu)
    }
    fn emit(&self, window: &str, event: &str, data: &Value) -> Result<()> {
        emit(&self.app, window, event, data)
    }
    fn features(&self) -> Vec<String> {
        let mut v: Vec<String> = vec!["app.events".into()];
        v.extend(native_panels::FEATURES.iter().map(|s| s.to_string()));
        v.extend(native_window::FEATURES.iter().map(|s| s.to_string()));
        v
    }
}

/// Deliver an event to one tool window's page, or to all of them ("*").
pub fn emit(app: &AppHandle, window: &str, event: &str, data: &Value) -> Result<()> {
    let js = format!(
        "window.__gwNative && window.__gwNative.emit({}, {})",
        serde_json::to_string(event)?,
        serde_json::to_string(data)?
    );
    for (label, w) in app.webview_windows() {
        if (window == "*" && label.starts_with("view-")) || label == window {
            let _ = w.eval(&js);
        }
    }
    Ok(())
}
