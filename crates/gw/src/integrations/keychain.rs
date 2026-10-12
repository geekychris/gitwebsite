//! macOS Keychain access through the `security` CLI, plus terminal prompts.
//!
//! Secrets are written via `security -i` on stdin so they never appear in a process list.

use anyhow::{bail, Context, Result};
use std::io::{BufRead, Write};
use std::process::{Command, Stdio};

pub fn get(service: &str, account: &str) -> Option<String> {
    let o = Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", service, "-a", account, "-w"])
        .output()
        .ok()?;
    if !o.status.success() {
        return None;
    }
    let v = String::from_utf8_lossy(&o.stdout).trim_end_matches('\n').to_string();
    (!v.is_empty()).then_some(v)
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

pub fn set(service: &str, account: &str, value: &str) -> Result<()> {
    let mut child = Command::new("/usr/bin/security")
        .arg("-i")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("running /usr/bin/security")?;
    let line = format!(
        "add-generic-password -U -s {} -a {} -w {}\n",
        quote(service),
        quote(account),
        quote(value)
    );
    child.stdin.take().unwrap().write_all(line.as_bytes())?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!("storing {service}/{account} in the Keychain failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(())
}

pub fn delete(service: &str, account: &str) {
    let _ = Command::new("/usr/bin/security")
        .args(["delete-generic-password", "-s", service, "-a", account])
        .output();
}

/// Read one line from the terminal.
pub fn prompt(label: &str) -> Result<String> {
    eprint!("{label}: ");
    std::io::stderr().flush().ok();
    let mut s = String::new();
    std::io::stdin().lock().read_line(&mut s)?;
    Ok(s.trim().to_string())
}

/// Read a secret from the terminal without echo.
pub fn prompt_secret(label: &str) -> Result<String> {
    let tty = std::fs::File::open("/dev/tty").context("a terminal is required to enter secrets")?;
    let stty = |arg: &str| {
        let _ = Command::new("stty").arg(arg).stdin(tty.try_clone().unwrap()).status();
    };
    eprint!("{label}: ");
    std::io::stderr().flush().ok();
    stty("-echo");
    let mut s = String::new();
    let r = std::io::BufReader::new(tty.try_clone()?).read_line(&mut s);
    stty("echo");
    eprintln!();
    r?;
    Ok(s.trim().to_string())
}
