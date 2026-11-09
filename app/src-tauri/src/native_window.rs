//! Window chrome, menus, notifications and menu-bar items for `native.app` (filled in by the
//! app-shell work).

use anyhow::{bail, Result};
use gw::companion::native::host::{Notice, WindowPatch};
use serde_json::Value;
use tauri::AppHandle;

/// Host features this file provides (`app.window`, `app.notify`, `app.status`, `app.palette`).
pub const FEATURES: &[&str] = &[];

pub fn notify(_app: &AppHandle, _n: &Notice) -> Result<()> {
    bail!("notifications are not supported yet")
}

pub fn window(_app: &AppHandle, _id: &str, _p: &WindowPatch) -> Result<()> {
    bail!("window changes are not supported yet")
}

pub fn status_item(_app: &AppHandle, _id: &str, _text: Option<&str>, _menu: Option<&Value>) -> Result<()> {
    bail!("menu-bar items are not supported yet")
}
