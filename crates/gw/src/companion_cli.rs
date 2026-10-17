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
    /// Approve a pairing request or a first-use approval (LAN host, TCP target, S3 delete) by
    /// its 6-digit code (shown on the page / in the app) or id.
    Approve { code: String },
    /// Deny a pairing request or a first-use approval.
    Deny { code: String },
    /// First-use approvals waiting for a decision (LAN hosts, TCP targets, S3 deletions).
    Approvals,
    /// The relay/lan and relay/ws LAN allow-list: hosts (any port) or host:port.
    #[command(subcommand)]
    Lan(ListCmd),
    /// Public wss:// hosts relay/ws may reach (host or host:port).
    #[command(subcommand)]
    Ws(ListCmd),
    /// The TCP relay's allow-list (host:port).
    #[command(subcommand)]
    Tcp(ListCmd),
    /// S3-compatible profiles for relay/s3 (credentials: `gw companion secret set s3:<name>`).
    #[command(subcommand)]
    S3(S3Cmd),
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
    /// Secrets in the keychain (digikey, mouser, nexar, s3:<profile>).
    #[command(subcommand)]
    Secret(SecretCmd),
}

#[derive(Subcommand)]
pub enum ListCmd {
    /// The entries.
    List,
    /// Add an entry.
    Add { entry: String },
    /// Remove an entry.
    Remove { entry: String },
    /// Approve a waiting first-use request for this host (or target), or by its code.
    Approve { which: String },
}

#[derive(Subcommand)]
pub enum S3Cmd {
    /// The profiles (and whether their credentials are set).
    List,
    /// Add or replace a profile: gw companion s3 add minio --endpoint http://nas.local:9000 --region us-east-1 --path-style
    Add {
        name: String,
        #[arg(long)]
        endpoint: String,
        #[arg(long, default_value = "us-east-1")]
        region: String,
        /// endpoint/bucket/key (MinIO and most self-hosted stores) instead of bucket.endpoint/key.
        #[arg(long)]
        path_style: bool,
        /// Accept a self-signed certificate (LAN endpoints only).
        #[arg(long)]
        insecure: bool,
    },
    /// Remove a profile (and its stored credentials).
    Remove { name: String },
}

#[derive(Subcommand)]
pub enum SecretCmd {
    /// Which secrets are present (never their values).
    List,
    /// Store secrets: `digikey` (asks for client id and secret), `mouser`, `nexar`,
    /// `s3:<profile>` (asks for the access key id and secret access key), or one field name
    /// (digikey_client_id, s3:<profile>:secret_access_key, …) with --value. Values are read
    /// from stdin when not given.
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

/// Change the settings: through the running app (so the gateway sees it at once), else in
/// companion.toml (and the audit log) directly.
fn change(f: impl FnOnce(&mut Settings) -> Result<Vec<&'static str>>) -> Result<Value> {
    match appclient::find_port() {
        Some(_) => {
            let cur: Settings = serde_json::from_value(appclient::get("/api/companion")?["settings"].clone()).context("reading the app's companion settings")?;
            let mut new = cur.clone();
            let keys = f(&mut new)?;
            let v = serde_json::to_value(&new)?;
            let patch: serde_json::Map<String, Value> = keys.iter().map(|k| (k.to_string(), v[*k].clone())).collect();
            appclient::send("PUT", "/api/companion/settings", &Value::Object(patch))
        }
        None => {
            let mut s = Settings::load()?;
            let keys = f(&mut s)?;
            s.validate()?;
            s.save()?;
            crate::companion::audit_append(json!({"event": "settings", "keys": keys, "by": "cli"}));
            Ok(json!({ "settings": s }))
        }
    }
}

fn list_field<'a>(s: &'a mut Settings, which: &str) -> &'a mut Vec<String> {
    match which {
        "lan_hosts" => &mut s.lan_hosts,
        "ws_allow" => &mut s.ws_allow,
        _ => &mut s.tcp_allow,
    }
}

fn run_list(key: &'static str, kind: &str, c: ListCmd) -> Result<Value> {
    match c {
        ListCmd::List => {
            let s = match appclient::get("/api/companion") {
                Ok(v) => serde_json::from_value(v["settings"].clone())?,
                Err(_) => Settings::load()?,
            };
            Ok(json!(list_field(&mut s.clone(), key).clone()))
        }
        ListCmd::Add { entry } => change(|s| {
            let e = entry.trim().to_ascii_lowercase();
            let l = list_field(s, key);
            if !l.iter().any(|x| x.eq_ignore_ascii_case(&e)) {
                l.push(e);
            }
            Ok(vec![key])
        }),
        ListCmd::Remove { entry } => change(|s| {
            let l = list_field(s, key);
            let before = l.len();
            l.retain(|x| !x.eq_ignore_ascii_case(entry.trim()));
            if l.len() == before {
                bail!("{entry:?} is not on {key}");
            }
            Ok(vec![key])
        }),
        ListCmd::Approve { which } => {
            if kind.is_empty() {
                bail!("public wss:// hosts are never prompted for: use `gw companion ws add <host>`");
            }
            appclient::send("POST", &format!("/api/companion/approvals/{}?kind={kind}", urlencoding::encode(&which)), &json!({ "allow": true }))
        }
    }
}

/// Approve or deny by code: a pairing request, else a first-use approval.
fn decide(code: &str, allow: bool) -> Result<Value> {
    match appclient::send("POST", &format!("/api/companion/pair/{code}"), &json!({ "allow": allow })) {
        Ok(v) => Ok(v),
        Err(e) if format!("{e:#}").contains("no pending pairing") => {
            appclient::send("POST", &format!("/api/companion/approvals/{}", urlencoding::encode(code)), &json!({ "allow": allow }))
        }
        Err(e) => Err(e),
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
        CompanionCmd::Approve { code } => decide(&code, true)?,
        CompanionCmd::Deny { code } => decide(&code, false)?,
        CompanionCmd::Approvals => appclient::get("/api/companion")?["approvals"].clone(),
        CompanionCmd::Lan(c) => run_list("lan_hosts", "lan", c)?,
        CompanionCmd::Ws(c) => run_list("ws_allow", "", c)?,
        CompanionCmd::Tcp(c) => run_list("tcp_allow", "tcp", c)?,
        CompanionCmd::S3(c) => match c {
            S3Cmd::List => match appclient::get("/api/companion") {
                Ok(v) => v["s3"].clone(),
                Err(_) => {
                    let store = secrets::Store::user();
                    Value::Array(Settings::load()?.s3_profiles.iter().map(|(n, p)| json!({"name": n, "endpoint": p.endpoint, "region": p.region,
                        "path_style": p.path_style, "insecure": p.insecure, "credentials": store.holds(&format!("s3:{n}"))})).collect())
                }
            },
            S3Cmd::Add { name, endpoint, region, path_style, insecure } => {
                let mut v = change(|s| {
                    s.s3_profiles.insert(name.clone(), crate::companion::settings::S3Profile { endpoint: endpoint.trim_end_matches('/').to_string(), region, path_style, insecure });
                    Ok(vec!["s3_profiles"])
                })?;
                if !secrets::Store::user().holds(&format!("s3:{name}")) {
                    v["next"] = json!(format!("store the credentials: gw companion secret set s3:{name}"));
                }
                v
            }
            S3Cmd::Remove { name } => {
                let v = change(|s| {
                    if s.s3_profiles.remove(&name).is_none() {
                        bail!("no S3 profile {name:?}");
                    }
                    Ok(vec!["s3_profiles"])
                })?;
                let store = secrets::Store::user();
                for f in secrets::fields(&format!("s3:{name}")).unwrap_or_default() {
                    let _ = store.delete(&f);
                }
                v
            }
        },
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
                    let fields: Vec<String> = secrets::fields(&name).unwrap_or_else(|| vec![name.clone()]);
                    if value.is_some() && fields.len() > 1 {
                        bail!("{name} has {} fields; set them one at a time ({}) with --value, or omit --value to be asked", fields.len(), fields.join(", "));
                    }
                    for f in &fields {
                        let v = match &value {
                            Some(v) => v.clone(),
                            None => read_line(f)?,
                        };
                        if v.is_empty() {
                            bail!("empty value for {f}");
                        }
                        store.set(f, &v)?;
                    }
                    secret_audit(&fields, "set");
                    store.presence()
                }
                SecretCmd::Delete { name } => {
                    let fields: Vec<String> = secrets::fields(&name).unwrap_or_else(|| vec![name.clone()]);
                    for f in &fields {
                        store.delete(f)?;
                    }
                    secret_audit(&fields, "deleted");
                    store.presence()
                }
            }
        }
    })
}

/// Secret changes are audit-logged (names only, never values).
fn secret_audit(names: &[String], action: &str) {
    crate::companion::audit_append(json!({"event": "secret", "names": names, "action": action, "by": "cli"}));
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
