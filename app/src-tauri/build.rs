use std::path::Path;
use std::process::Command;

fn main() {
    mac_helper();
    tauri_build::build()
}

/// Build the Swift helper (app/mac-helper → target/gw-mac-helper); the bundle copies it to
/// Contents/MacOS (tauri.conf.json `bundle.macOS.files`). Without swiftc the app still builds,
/// with no `native.mac` features.
fn mac_helper() {
    let src = Path::new("../mac-helper");
    println!("cargo:rerun-if-changed=../mac-helper");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let mut files: Vec<_> = std::fs::read_dir(src).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "swift")).collect();
    files.sort();
    for f in &files {
        println!("cargo:rerun-if-changed={}", f.display());
    }
    let out = Path::new("../../target/gw-mac-helper");
    let _ = std::fs::create_dir_all("../../target");
    let status = Command::new("xcrun").args(["swiftc", "-O", "-target", "arm64-apple-macos13", "-o"]).arg(out).args(&files).status();
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => println!("cargo:warning=gw-mac-helper did not build (swiftc exited {s}); native.mac features are off"),
        Err(e) => println!("cargo:warning=gw-mac-helper did not build ({e}); native.mac features are off"),
    }
    if !out.exists() {
        // The bundle step needs the file; an empty stand-in lists no ops.
        let _ = std::fs::write(out, "#!/bin/sh\necho '{\"ops\":[]}'\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(out, std::fs::Permissions::from_mode(0o755));
        }
    }
}
