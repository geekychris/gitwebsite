//! Native features for the site's tools: what a web page cannot do, done by the app on this
//! Mac, behind the same pairing, origin checks and audit log as the relays. Mounted at
//! `/v1/native/…`; see docs/companion.adoc (Native).
//!
//! | scope          | module     | what                                                        |
//! |----------------|------------|-------------------------------------------------------------|
//! | `native.fs`    | [`fs`]     | open/save panels, reading and writing files the user chose   |
//! | `native.device`| [`device`] | serial, Bluetooth LE, USB, HID and MIDI (shims for Web APIs) |
//! | `native.exec`  | [`exec`]   | a fixed catalogue of system commands (launchctl, defaults …) |
//! | `native.mac`   | [`mac`]    | Apple frameworks through the helper (Vision, PDFKit, Speech) |
//! | `native.app`   | [`app`]    | window title/menus, notifications, menu-bar items, palette   |
//! | `native.store` | [`store`]  | per-tool storage in Application Support                      |
//!
//! Each module adds its routes in `router()` and lists what is available in `capabilities()`
//! (shown in `/v1/hello`; a page tests for a capability, e.g. `native.device.serial`).
//! Things only the app can do on its main thread (panels, menus, notifications) go through
//! the [`host::NativeHost`] the desktop app installs; `gw serve` alone has none, and those
//! capabilities are then not listed.

use super::Gateway;
use axum::Router;
use std::sync::Arc;

pub mod app;
pub mod device;
pub mod exec;
pub mod fs;
pub mod host;
pub mod mac;
pub mod store;

pub const SCOPES: &[&str] = &["native.fs", "native.device", "native.exec", "native.mac", "native.app", "native.store"];

pub fn router() -> Router<Arc<Gateway>> {
    Router::new()
        .merge(fs::router())
        .merge(device::router())
        .merge(exec::router())
        .merge(mac::router())
        .merge(app::router())
        .merge(store::router())
}

/// What is available now (for `/v1/hello`).
pub fn capabilities(gw: &Gateway) -> Vec<String> {
    let mut v = vec![];
    v.extend(fs::capabilities(gw));
    v.extend(device::capabilities(gw));
    v.extend(exec::capabilities(gw));
    v.extend(mac::capabilities(gw));
    v.extend(app::capabilities(gw));
    v.extend(store::capabilities(gw));
    v
}

/// The window a request comes from: the app's tool windows send `X-GW-Window` (from
/// `window.__GW_APP__.window`, set by the app). Empty for an ordinary browser tab.
pub fn window_of(h: &axum::http::HeaderMap) -> String {
    h.get("x-gw-window")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(64).collect())
        .unwrap_or_default()
}
