//! What only the desktop app can do (on its main thread): panels, window chrome, menus,
//! notifications, menu-bar items. The app installs a [`NativeHost`] at start-up; `gw serve`
//! alone has none, so [`host()`] is `None` and the capabilities that need it are not listed.
//! Every method has a default that says it is not supported, so a host implements what it can.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::OnceLock;

/// An open or save panel.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PanelOptions {
    #[serde(default)]
    pub title: String,
    /// Save: the suggested file name.
    #[serde(default)]
    pub name: String,
    /// File extensions without the dot (`["pdf", "png"]`); empty means any.
    #[serde(default)]
    pub types: Vec<String>,
    /// Open: choose folders instead of files.
    #[serde(default)]
    pub directory: bool,
    /// Open: allow several.
    #[serde(default)]
    pub multiple: bool,
    /// The window to attach the panel to (its id), if any.
    #[serde(default)]
    pub window: String,
}

/// A notification.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Notice {
    pub title: String,
    #[serde(default)]
    pub body: String,
    /// The tool it comes from (clicking it opens that tool).
    #[serde(default)]
    pub tool: String,
    #[serde(default)]
    pub sound: bool,
}

/// Changes to a tool window. `None` leaves a field as it is.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct WindowPatch {
    pub title: Option<String>,
    /// The "unsaved changes" dot in the close button.
    pub edited: Option<bool>,
    /// The file the window shows (title-bar proxy icon, Open Recent).
    pub file: Option<String>,
    /// The tool's menus: `[{ "title": "File", "items": [{ "id", "title", "key", "enabled" } | "-"] }]`.
    pub menu: Option<Value>,
}

pub trait NativeHost: Send + Sync {
    fn open_panel(&self, _o: &PanelOptions) -> Result<Vec<PathBuf>> {
        bail!("open panels are not supported here")
    }
    /// `None` when the user cancels.
    fn save_panel(&self, _o: &PanelOptions) -> Result<Option<PathBuf>> {
        bail!("save panels are not supported here")
    }
    fn notify(&self, _n: &Notice) -> Result<()> {
        bail!("notifications are not supported here")
    }
    fn window(&self, _id: &str, _p: &WindowPatch) -> Result<()> {
        bail!("window changes are not supported here")
    }
    /// A menu-bar item for a tool: `text` None removes it; `menu` as in [`WindowPatch::menu`].
    fn status_item(&self, _id: &str, _text: Option<&str>, _menu: Option<&Value>) -> Result<()> {
        bail!("menu-bar items are not supported here")
    }
    /// Deliver an event to a window's page (`window.__gwNative.emit(event, data)`); "*" for all.
    fn emit(&self, _window: &str, _event: &str, _data: &Value) -> Result<()> {
        bail!("events are not supported here")
    }
    /// What this host supports, as capability names without the `native.` prefix
    /// (`fs.panels`, `app.window`, `app.notify`, `app.status`, `app.palette` …).
    fn features(&self) -> Vec<String> {
        vec![]
    }
}

static HOST: OnceLock<Box<dyn NativeHost>> = OnceLock::new();

/// Called once by the desktop app.
pub fn set_native_host(h: impl NativeHost + 'static) {
    let _ = HOST.set(Box::new(h));
}

pub fn host() -> Option<&'static dyn NativeHost> {
    HOST.get().map(|b| b.as_ref())
}

/// Does the host support `feature` (e.g. `fs.panels`)?
pub fn has(feature: &str) -> bool {
    host().is_some_and(|h| h.features().iter().any(|f| f == feature))
}
