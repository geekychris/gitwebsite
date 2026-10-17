//! API secrets for the parts suppliers (Digi-Key, Mouser, Nexar), held by the companion and
//! injected server-side so they never reach a page. Stored in the macOS Keychain (Windows
//! Credential Manager) under the service `gitwebsite-companion`; environment variables
//! (`DIGIKEY_CLIENT_ID`, … as for parts-relay.py) take precedence. Values are never returned
//! by any API — only whether each one is present.
//!
//! A non-default config directory (`GW_CONFIG_DIR` pointing anywhere but the user's real one:
//! scratch and test instances) never touches the Keychain: its secrets live in
//! `<dir>/companion-secrets.json` (mode 0600), so a test instance can neither read nor
//! overwrite the user's real supplier keys.

use anyhow::{bail, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const SERVICE: &str = "gitwebsite-companion";

/// Every secret the companion knows, with the environment variable that can supply it.
pub const NAMES: &[(&str, &str)] = &[
    ("digikey_client_id", "DIGIKEY_CLIENT_ID"),
    ("digikey_client_secret", "DIGIKEY_CLIENT_SECRET"),
    ("mouser_api_key", "MOUSER_API_KEY"),
    ("nexar_client_id", "NEXAR_CLIENT_ID"),
    ("nexar_client_secret", "NEXAR_CLIENT_SECRET"),
];

/// The two fields of an S3 profile's credentials.
pub const S3_FIELDS: &[&str] = &["access_key_id", "secret_access_key"];

/// The fields a supplier (or `s3:<profile>`) needs.
pub fn fields(supplier: &str) -> Option<Vec<String>> {
    let f: &[&str] = match supplier {
        "digikey" => &["digikey_client_id", "digikey_client_secret"],
        "mouser" => &["mouser_api_key"],
        "nexar" => &["nexar_client_id", "nexar_client_secret"],
        _ => {
            let p = supplier.strip_prefix("s3:").filter(|p| super::settings::s3_name_ok(p))?;
            return Some(S3_FIELDS.iter().map(|f| format!("s3:{p}:{f}")).collect());
        }
    };
    Some(f.iter().map(|s| s.to_string()).collect())
}

/// `s3:<profile>:access_key_id` / `s3:<profile>:secret_access_key`.
fn s3_field(name: &str) -> bool {
    name.strip_prefix("s3:")
        .and_then(|r| r.rsplit_once(':'))
        .is_some_and(|(p, f)| super::settings::s3_name_ok(p) && S3_FIELDS.contains(&f))
}

fn check(name: &str) -> Result<()> {
    if NAMES.iter().any(|(n, _)| *n == name) || s3_field(name) {
        Ok(())
    } else {
        bail!("unknown secret {name:?}; one of {}, or s3:<profile>:access_key_id / s3:<profile>:secret_access_key", NAMES.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", "))
    }
}

pub enum Store {
    /// Environment, then the OS keychain (the user's real config directory).
    Keychain,
    /// Environment, then a 0600 JSON file (a non-default config directory).
    File(PathBuf),
    /// In memory (tests).
    Memory(Mutex<HashMap<String, String>>),
}

fn env_value(name: &str) -> Option<String> {
    let env = NAMES.iter().find(|(n, _)| *n == name).map(|(_, e)| *e)?;
    std::env::var(env).ok().filter(|v| !v.is_empty())
}

fn read_file(p: &Path) -> HashMap<String, String> {
    std::fs::read_to_string(p).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn write_file(p: &Path, m: &HashMap<String, String>) -> Result<()> {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(m)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, p)?;
    Ok(())
}

impl Store {
    /// The store for the config directory in use: the Keychain for the user's real one, a file
    /// in the directory for any other (`GW_CONFIG_DIR`).
    pub fn user() -> Self {
        if gw_core::userconfig::is_default_config_dir() {
            Store::Keychain
        } else {
            Store::File(gw_core::userconfig::config_dir().join("companion-secrets.json"))
        }
    }

    pub fn memory(pairs: &[(&str, &str)]) -> Self {
        Store::Memory(Mutex::new(pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()))
    }

    /// Where the secrets are kept, for the status page.
    pub fn backend(&self) -> String {
        match self {
            Store::Keychain => format!("keychain:{SERVICE}"),
            Store::File(p) => format!("file:{}", p.display()),
            Store::Memory(_) => "memory".into(),
        }
    }

    pub fn get(&self, name: &str) -> Option<String> {
        match self {
            Store::Memory(m) => m.lock().unwrap().get(name).cloned(),
            Store::File(p) => env_value(name).or_else(|| read_file(p).get(name).cloned().filter(|v| !v.is_empty())),
            Store::Keychain => {
                if let Some(v) = env_value(name) {
                    return Some(v);
                }
                keyring::Entry::new(SERVICE, name).ok()?.get_password().ok().filter(|v| !v.is_empty())
            }
        }
    }

    pub fn set(&self, name: &str, value: &str) -> Result<()> {
        check(name)?;
        match self {
            Store::Memory(m) => {
                m.lock().unwrap().insert(name.into(), value.into());
            }
            Store::File(p) => {
                let mut m = read_file(p);
                m.insert(name.into(), value.into());
                write_file(p, &m)?;
            }
            Store::Keychain => keyring::Entry::new(SERVICE, name)?.set_password(value)?,
        }
        Ok(())
    }

    pub fn delete(&self, name: &str) -> Result<()> {
        check(name)?;
        match self {
            Store::Memory(m) => {
                m.lock().unwrap().remove(name);
            }
            Store::File(p) => {
                let mut m = read_file(p);
                if m.remove(name).is_some() {
                    write_file(p, &m)?;
                }
            }
            Store::Keychain => match keyring::Entry::new(SERVICE, name)?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => {}
                Err(e) => return Err(e.into()),
            },
        }
        Ok(())
    }

    /// Which secrets are present (never their values).
    pub fn presence(&self) -> serde_json::Value {
        let mut m = serde_json::Map::new();
        for (n, _) in NAMES {
            m.insert(n.to_string(), self.get(n).is_some().into());
        }
        serde_json::Value::Object(m)
    }

    /// Whether every field of a supplier is present.
    pub fn holds(&self, supplier: &str) -> bool {
        fields(supplier).map(|f| f.iter().all(|n| self.get(n).is_some())).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_store_round_trip_0600() {
        let d = std::env::temp_dir().join(format!("gw-sec-{}", std::process::id()));
        let p = d.join("companion-secrets.json");
        let s = Store::File(p.clone());
        // Uses a name with no environment variable set in CI (mouser).
        if std::env::var("MOUSER_API_KEY").is_err() {
            assert!(s.get("mouser_api_key").is_none());
            s.set("mouser_api_key", "k1").unwrap();
            assert_eq!(s.get("mouser_api_key").as_deref(), Some("k1"));
            assert!(s.holds("mouser"));
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
            }
            s.delete("mouser_api_key").unwrap();
            assert!(s.get("mouser_api_key").is_none());
        }
        assert!(s.set("nope", "x").is_err());
        s.set("s3:minio:secret_access_key", "SK").unwrap();
        s.set("s3:minio:access_key_id", "AK").unwrap();
        assert!(s.holds("s3:minio"));
        assert!(s.set("s3:minio:other", "x").is_err() && s.set("s3:a b:access_key_id", "x").is_err());
        assert_eq!(fields("s3:minio").unwrap(), vec!["s3:minio:access_key_id", "s3:minio:secret_access_key"]);
        assert!(s.backend().starts_with("file:"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
