//! The scripts the app injects at document start into tool windows: the bridge
//! (`window.__GW_APP__`, `window.__gwNative`), then the Web API shims, each of which installs
//! itself only where WebKit lacks the API.

/// Shims run after the bridge, in this order. Add `include_str!("../inject/<name>.js")`.
pub const SHIMS: &[&str] = &[];

const BRIDGE: &str = include_str!("../inject/bridge.js");

/// The script for window `id`.
pub fn script(id: &str) -> String {
    let base = gw::companion::global()
        .map(|g| g.port.load(std::sync::atomic::Ordering::Relaxed))
        .filter(|p| *p != 0)
        .map(|p| format!("http://127.0.0.1:{p}"))
        .unwrap_or_default();
    let mut s = BRIDGE
        .replace("__GW_WINDOW__", &serde_json::to_string(id).unwrap())
        .replace("__GW_COMPANION__", &serde_json::to_string(&base).unwrap());
    for shim in SHIMS {
        s.push_str("\n;\n");
        s.push_str(shim);
    }
    s
}
