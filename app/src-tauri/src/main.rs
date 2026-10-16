//! gitwebsite desktop app: starts the gitwebsite server (REST API, management UI, preview)
//! on a background thread, shows the management UI in a native window, and lives in the
//! menu bar. Closing the window hides it; the menu bar icon (or the Dock icon) brings it back.
//!
//! Site: $GW_SITE, else the current site in the user config (`gw config`); with neither, the
//! window opens on the setup screen (open an existing site or create one).
//! Port: $GW_PORT, else 8790, or any free port if that is taken.
//! Companion gateway (relays for the site's tools): 127.0.0.1:7317 (fallbacks 7318–7320, or
//! $GW_COMPANION_PORT); pairing requests are approved in a dialog. Tool windows (`/api/view`,
//! MCP `tool_open`) are native windows on the preview.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod views;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};

fn site_root() -> Option<PathBuf> {
    gw_core::userconfig::resolve_site().ok().filter(|p| p.join("site.yaml").exists())
}

/// The site's public address (site.yaml `base_url`), asked of the server so it follows a
/// switch of site.
fn live_url(port: u16) -> Option<String> {
    let body = call(port, "GET", "/api/site", "").ok()?;
    let v: serde_json::Value = serde_json::from_str(&body).ok()?;
    v["base_url"].as_str().filter(|u| !u.is_empty()).map(str::to_string)
}

fn bind() -> std::io::Result<TcpListener> {
    let port = std::env::var("GW_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8790);
    TcpListener::bind(("127.0.0.1", port)).or_else(|_| TcpListener::bind(("127.0.0.1", 0)))
}

/// Minimal HTTP call to the in-process server; returns the response body.
fn call(port: u16, method: &str, path: &str, body: &str) -> std::io::Result<String> {
    let mut s = TcpStream::connect(("127.0.0.1", port))?;
    s.set_read_timeout(Some(Duration::from_secs(120)))?;
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    let mut out = String::new();
    s.read_to_string(&mut out)?;
    Ok(out.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or(out))
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Edit ▸ Start Dictation…, sent to the first responder: the focused text field in the web
/// view. The UI's 🎤 buttons focus a field and then ask for this (POST /api/dictate).
#[cfg(target_os = "macos")]
fn start_dictation() {
    use objc2::{sel, MainThreadMarker};
    use objc2_app_kit::NSApplication;
    if let Some(mtm) = MainThreadMarker::new() {
        let app = NSApplication::sharedApplication(mtm);
        // SAFETY: a valid selector, nil target (the responder chain) and nil sender.
        unsafe {
            app.sendAction_to_from(sel!(startDictation:), None, None);
        }
    }
}

/// Ask the user about a companion pairing request (a page wants to use the relays).
fn ask_pairing(app: &AppHandle, req: gw::companion::pairing::PairRequest) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
    let who = if req.origin.is_empty() { "a program on this Mac (no web origin)".to_string() } else { req.origin.clone() };
    let tool = if req.tool.is_empty() { "(not named)".to_string() } else { req.tool.clone() };
    let what = |s: &str| match s {
        "relay.http" => "reach web APIs (aviation, parts suppliers, your receiver / Kafka REST)",
        "relay.tcp" => "open TCP connections to hosts on your allow-list",
        "relay.docker" => "read Docker (containers, images — read only)",
        "relay.kube" => "read your Kubernetes clusters (read only)",
        "relay.git" => "clone and fetch git repositories",
        "view" => "open tool windows in the app",
        _ => "",
    };
    let scopes: Vec<String> = req.scopes.iter().map(|s| format!("  • {s} — {}", what(s))).collect();
    let msg = format!(
        "Tool:   {tool}\nPage:   {who}\n\nwants to use the gitwebsite companion to:\n{}\n\nCode {} — check that the page shows the same code.\n\nAllow only pages you trust; you can revoke it later in Companion settings.",
        scopes.join("\n"),
        req.code
    );
    let id = req.id.clone();
    app.dialog()
        .message(msg)
        .title(&format!("Allow {} to use the companion?", if req.tool.is_empty() { "this page" } else { req.tool.as_str() }))
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom("Allow".into(), "Deny".into()))
        .show(move |allow| {
            if let Some(g) = gw::companion::global() {
                let _ = g.pairing.decide(&id, allow, &g.tokens);
            }
        });
}

fn open_url(url: &str) {
    let _ = std::process::Command::new("open").arg(url).status();
}

fn main() {
    let root = site_root();
    let listener = bind().expect("cannot bind a local port");
    listener.set_nonblocking(true).expect("nonblocking");
    let port = listener.local_addr().unwrap().port();

    let server_root = root.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        rt.block_on(async move {
            let l = tokio::net::TcpListener::from_std(listener).expect("listener");
            if let Err(e) = gw::server::serve_on(server_root, l, false).await {
                eprintln!("gitwebsite server stopped: {e:#}");
            }
        });
    });

    // The server builds the preview before it accepts connections; wait for it.
    let deadline = Instant::now() + Duration::from_secs(60);
    while TcpStream::connect(("127.0.0.1", port)).is_err() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    let base = format!("http://127.0.0.1:{port}");
    let app_url = format!("{base}/app/");

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(app_url.parse()?))
                .title("gitwebsite")
                .inner_size(1400.0, 900.0)
                .min_inner_size(900.0, 600.0)
                // Let the page handle drops itself (photos onto a story, from Finder or Photos);
                // Tauri's own handler would swallow them.
                .disable_drag_drop_handler()
                // The window only ever shows the app. Anything else — a file dropped where the
                // page doesn't take it (WebKit would open it in place, with no way back), or a
                // link to another site — opens in the default browser instead.
                .on_navigation({
                    let base = base.clone();
                    move |url| {
                        if url.as_str().starts_with(&base) || matches!(url.scheme(), "about" | "blob") {
                            return true;
                        }
                        if matches!(url.scheme(), "http" | "https") {
                            let u = url.to_string();
                            std::thread::spawn(move || open_url(&u));
                        }
                        false
                    }
                })
                .build()?;

            #[cfg(target_os = "macos")]
            {
                let handle = app.handle().clone();
                gw::server::set_dictation_hook(move || {
                    let _ = handle.run_on_main_thread(start_dictation);
                });
            }


            // Tool windows for /api/view and MCP tool_open.
            gw::companion::view::set_view_host(views::Views::new(app.handle().clone(), base.clone()));

            // Companion pairing requests: a dialog (the gateway starts with the server).
            {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    let deadline = Instant::now() + Duration::from_secs(300);
                    while Instant::now() < deadline {
                        if let Some(g) = gw::companion::global() {
                            let h = handle.clone();
                            g.set_pair_hook(move |req| {
                                let h2 = h.clone();
                                let _ = h.run_on_main_thread(move || ask_pairing(&h2, req));
                            });
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(200));
                    }
                });
            }

            // Menu bar icon and menu.
            let status = MenuItem::with_id(app, "status", "gitwebsite is running", false, None::<&str>)?;
            let open = MenuItem::with_id(app, "open", "Open gitwebsite", true, Some("CmdOrCtrl+O"))?;
            let preview = MenuItem::with_id(app, "preview", "Preview site in browser", true, None::<&str>)?;
            let live = MenuItem::with_id(app, "live", "Open live site", true, None::<&str>)?;
            let rebuild = MenuItem::with_id(app, "rebuild", "Rebuild preview", true, None::<&str>)?;
            let check = MenuItem::with_id(app, "check", "Check site", true, None::<&str>)?;
            let publish = MenuItem::with_id(app, "publish", "Build & Publish…", true, None::<&str>)?;
            let companion = MenuItem::with_id(app, "companion", "Companion settings…", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit gitwebsite", true, Some("CmdOrCtrl+Q"))?;
            let sep = || PredefinedMenuItem::separator(app);
            let menu = Menu::with_items(
                app,
                &[&status, &sep()?, &open, &preview, &live, &sep()?, &rebuild, &check, &publish, &companion, &sep()?, &quit],
            )?;

            let status_item = status.clone();
            let base = base.clone();
            TrayIconBuilder::with_id("gitwebsite")
                .icon(Image::from_bytes(include_bytes!("../icons/tray-icon.png"))?)
                .icon_as_template(true)
                .tooltip("gitwebsite")
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(move |app, event| {
                    let set = |t: &str| {
                        let _ = status_item.set_text(t);
                    };
                    match event.id.as_ref() {
                        "open" => show_main(app),
                        "preview" => open_url(&format!("{base}/preview/")),
                        "live" => match live_url(port) {
                            Some(u) => open_url(&u),
                            None => show_main(app),
                        },
                        "companion" => {
                            show_main(app);
                            if let Some(w) = app.get_webview_window("main") {
                                let _ = w.eval("location.hash = '#companion'");
                            }
                        }
                        "publish" => {
                            show_main(app);
                            if let Some(w) = app.get_webview_window("main") {
                                let _ = w.eval("location.hash = '#publish'");
                            }
                        }
                        "rebuild" => {
                            set("Rebuilding preview…");
                            let s = status_item.clone();
                            std::thread::spawn(move || {
                                let t = match call(port, "POST", "/api/build", r#"{"drafts":true}"#) {
                                    Ok(b) if b.contains("\"error\"") => "Preview build failed — see the app".to_string(),
                                    Ok(_) => "Preview rebuilt ✓".to_string(),
                                    Err(e) => format!("Preview build failed: {e}"),
                                };
                                let _ = s.set_text(t);
                            });
                        }
                        "check" => {
                            set("Checking site…");
                            let s = status_item.clone();
                            std::thread::spawn(move || {
                                let t = match call(port, "GET", "/api/check", "") {
                                    Ok(b) if b.contains("\"ok\":true") => "Site check: no problems ✓".to_string(),
                                    Ok(b) => {
                                        let n = b.matches("\",\"").count() + 1;
                                        format!("Site check: {n} problem(s) — open the app")
                                    }
                                    Err(e) => format!("Site check failed: {e}"),
                                };
                                let _ = s.set_text(t);
                            });
                        }
                        "quit" => app.exit(0),
                        _ => {}
                    }
                })
                .build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the main window keeps the app in the menu bar; tool windows just close.
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() != "main" {
                    return;
                }
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building gitwebsite");

    app.run(|app, event| {
        // Clicking the Dock icon brings the window back.
        if let RunEvent::Reopen { .. } = event {
            show_main(app);
        }
    });
}
