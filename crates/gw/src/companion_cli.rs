//! `gw companion …` and `gw tools …`.

use crate::companion::{secrets, settings::Settings};
use crate::{appclient, toolcat};
use anyhow::{bail, Context, Result};
use clap::Subcommand;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub enum CompanionCmd {
    /// Is the gateway running, on which port, what is paired, which secrets are present.
    Status,
    /// Pairing requests waiting for a decision.
    Pending,
    /// Approve a pairing request by its 6-digit code (shown on the page) or id.
    Approve { code: String },
    /// Deny a pairing request.
    Deny { code: String },
    /// Paired origins and tools (token ids, never tokens).
    Tokens,
    /// Revoke a token by id, or every token of an origin.
    Revoke { which: String },
    /// The last requests through the gateway.
    Audit {
        #[arg(long, default_value_t = 50)]
        n: usize,
    },
    /// Change a setting: gw companion set tcp_allow '["127.0.0.1:5432","pi.local:30005"]',
    /// gw companion set receiver http://piaware.local:8080, gw companion set relays.git false
    Set { key: String, value: String },
    /// Supplier API secrets in the keychain (digikey, mouser, nexar).
    #[command(subcommand)]
    Secret(SecretCmd),
}

#[derive(Subcommand)]
pub enum SecretCmd {
    /// Which secrets are present (never their values).
    List,
    /// Store secrets: `digikey` (asks for client id and secret), `mouser`, `nexar`, or one field
    /// name (digikey_client_id, …) with --value. Values are read from stdin when not given.
    Set {
        name: String,
        #[arg(long)]
        value: Option<String>,
    },
    /// Remove a supplier's secrets (or one field).
    Delete { name: String },
}

#[derive(Subcommand)]
pub enum ToolsCmd {
    /// The site's tools (slug, category, URL parameters, presets).
    List {
        #[arg(long)]
        category: Option<String>,
        /// Words that must all appear in the slug, title, summary or tags.
        #[arg(long, short)]
        q: Option<String>,
    },
    /// One tool: parameters, presets, tabs, example links, and the page's help text.
    Info { slug: String },
    /// Write the "which tool for which rendering" index for the show-with-tools skill.
    Index {
        /// Output file (default: skills/show-with-tools/tools-index.md here, or stdout with -).
        #[arg(long, default_value = "skills/show-with-tools/tools-index.md")]
        out: String,
    },
    /// Open a tool in an app window: gw tools open graph -p f=sin(x) -p x=-6,6 --hash graph
    Open {
        slug: String,
        /// URL parameter name=value (repeatable).
        #[arg(long = "param", short = 'p')]
        params: Vec<String>,
        #[arg(long)]
        example: Option<String>,
        #[arg(long)]
        hash: Option<String>,
        #[arg(long)]
        title: Option<String>,
        /// Always open a new window.
        #[arg(long)]
        new: bool,
    },
}

fn read_line(prompt: &str) -> Result<String> {
    use std::io::Write;
    eprint!("{prompt}: ");
    std::io::stderr().flush().ok();
    let mut s = String::new();
    std::io::stdin().read_line(&mut s)?;
    Ok(s.trim().to_string())
}

/// A settings change: `relays.git false`, `tcp_allow '[…]'`, `receiver http://…`.
fn patch_for(key: &str, value: &str) -> Value {
    let v: Value = serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_string()));
    match key.split_once('.') {
        Some((a, b)) => json!({ a: { b: v } }),
        None => json!({ key: v }),
    }
}

pub fn run_companion(c: CompanionCmd) -> Result<Value> {
    Ok(match c {
        CompanionCmd::Status => match appclient::get("/api/companion") {
            Ok(v) => v,
            Err(e) => json!({ "running": false, "note": format!("{e:#}"), "settings_file": crate::companion::settings::path().display().to_string(),
                "settings": Settings::load()?, "secrets": secrets::Store::user().presence() }),
        },
        CompanionCmd::Pending => appclient::get("/api/companion")?["pending"].clone(),
        CompanionCmd::Approve { code } => appclient::send("POST", &format!("/api/companion/pair/{code}"), &json!({ "allow": true }))?,
        CompanionCmd::Deny { code } => appclient::send("POST", &format!("/api/companion/pair/{code}"), &json!({ "allow": false }))?,
        CompanionCmd::Tokens => match appclient::get("/api/companion") {
            Ok(v) => v["paired"].clone(),
            Err(_) => {
                let t = crate::companion::pairing::Tokens::open(Some(crate::companion::settings::dir().join("companion-tokens.json")));
                Value::Array(t.list())
            }
        },
        CompanionCmd::Revoke { which } => match appclient::find_port() {
            Some(_) => appclient::send("DELETE", &format!("/api/companion/tokens/{}", urlencoding::encode(&which)), &json!({}))?,
            None => {
                let t = crate::companion::pairing::Tokens::open(Some(crate::companion::settings::dir().join("companion-tokens.json")));
                json!({ "revoked": t.revoke(&which) })
            }
        },
        CompanionCmd::Audit { n } => match appclient::get(&format!("/api/companion/audit?n={n}")) {
            Ok(v) => v,
            Err(_) => {
                let p = crate::companion::settings::dir().join("companion-audit.log");
                let t = std::fs::read_to_string(&p).unwrap_or_default();
                let entries: Vec<Value> = t.lines().rev().take(n).filter_map(|l| serde_json::from_str(l).ok()).collect();
                json!({ "entries": entries })
            }
        },
        CompanionCmd::Set { key, value } => {
            let patch = patch_for(&key, &value);
            match appclient::find_port() {
                Some(_) => appclient::send("PUT", "/api/companion/settings", &patch)?,
                None => {
                    let s = Settings::load()?.merged(&patch)?;
                    s.save()?;
                    json!({ "settings": s })
                }
            }
        }
        CompanionCmd::Secret(s) => {
            let store = secrets::Store::user();
            match s {
                SecretCmd::List => store.presence(),
                SecretCmd::Set { name, value } => {
                    let fields: Vec<&str> = match secrets::fields(&name) {
                        Some(f) => f.to_vec(),
                        None => vec![name.as_str()],
                    };
                    if value.is_some() && fields.len() > 1 {
                        bail!("{name} has {} fields; set them one at a time ({}) with --value, or omit --value to be asked", fields.len(), fields.join(", "));
                    }
                    for f in fields {
                        let v = match &value {
                            Some(v) => v.clone(),
                            None => read_line(f)?,
                        };
                        if v.is_empty() {
                            bail!("empty value for {f}");
                        }
                        store.set(f, &v)?;
                    }
                    store.presence()
                }
                SecretCmd::Delete { name } => {
                    let fields: Vec<&str> = secrets::fields(&name).map(|f| f.to_vec()).unwrap_or_else(|| vec![name.as_str()]);
                    for f in fields {
                        store.delete(f)?;
                    }
                    store.presence()
                }
            }
        }
    })
}

/// Parse `-p name=value` pairs.
pub fn params(list: &[String]) -> Result<serde_json::Map<String, Value>> {
    let mut m = serde_json::Map::new();
    for p in list {
        let (k, v) = p.split_once('=').with_context(|| format!("parameter {p:?} must be name=value"))?;
        m.insert(k.to_string(), Value::String(v.to_string()));
    }
    Ok(m)
}

pub fn run_tools(site_root: &Path, base_url: &str, c: ToolsCmd) -> Result<Value> {
    Ok(match c {
        ToolsCmd::List { category, q } => toolcat::list(site_root, category.as_deref(), q.as_deref())?,
        ToolsCmd::Info { slug } => toolcat::info(site_root, &slug)?,
        ToolsCmd::Index { out } => {
            let md = toolcat::index_markdown(site_root, base_url)?;
            if out == "-" {
                print!("{md}");
                return Ok(Value::Null);
            }
            let p = PathBuf::from(&out);
            if let Some(d) = p.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(d)?;
            }
            std::fs::write(&p, &md)?;
            json!({ "wrote": p.display().to_string(), "bytes": md.len(), "tools": toolcat::load(site_root)?.len() })
        }
        ToolsCmd::Open { slug, params: ps, example, hash, title, new } => {
            let body = json!({ "tool": slug, "params": params(&ps)?, "example": example, "hash": hash, "title": title,
                "window": if new { "new" } else { "reuse" } });
            appclient::send("POST", "/api/view", &body)?
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn patches() {
        assert_eq!(patch_for("relays.git", "false"), json!({"relays": {"git": false}}));
        assert_eq!(patch_for("receiver", "http://pi.local:8080"), json!({"receiver": "http://pi.local:8080"}));
        assert_eq!(patch_for("tcp_allow", r#"["a:1"]"#), json!({"tcp_allow": ["a:1"]}));
        assert!(params(&["x".into()]).is_err());
        assert_eq!(params(&["f=a=b".into()]).unwrap()["f"], "a=b");
    }
}
