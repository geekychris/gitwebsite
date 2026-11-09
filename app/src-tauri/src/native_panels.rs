//! Open and save panels for `native.fs` (filled in by the files work).

use anyhow::{bail, Result};
use gw::companion::native::host::PanelOptions;
use std::path::PathBuf;
use tauri::AppHandle;

/// Host features this file provides (`fs.panels` once the panels work).
pub const FEATURES: &[&str] = &[];

pub fn open_panel(_app: &AppHandle, _o: &PanelOptions) -> Result<Vec<PathBuf>> {
    bail!("open panels are not supported yet")
}

pub fn save_panel(_app: &AppHandle, _o: &PanelOptions) -> Result<Option<PathBuf>> {
    bail!("save panels are not supported yet")
}
