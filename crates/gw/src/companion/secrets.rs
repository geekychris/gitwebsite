//! API secrets for the parts suppliers (Digi-Key, Mouser, Nexar), held by the companion and
//! injected server-side so they never reach a page. Stored in the macOS Keychain (Windows
//! Credential Manager) under the service `gitwebsite-companion`; environment variables
//! (`DIGIKEY_CLIENT_ID`, … as for parts-relay.py) take precedence. Values are never returned
//! by any API — only whether each one is present.

use anyhow::{bail, Result};
use std::collections::HashMap;
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

/// The fields a supplier needs.
pub fn fields(supplier: &str) -> Option<&'static [&'static str]> {
    Some(match supplier {
        "digikey" => &["digikey_client_id", "digikey_client_secret"],
        "mouser" => &["mouser_api_key"],
        "nexar" => &["nexar_client_id", "nexar_client_secret"],
        _ => return None,
    })
}

fn check(name: &str) -> Result<()> {
    if NAMES.iter().any(|(n, _)| *n == name) {
        Ok(())
    } else {
        bail!("unknown secret {name:?}; one of {}", NAMES.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", "))
    }
}

pub enum Store {
    /// Environment, then the OS keychain.
    System,
    /// In memory (tests).
    Memory(Mutex<HashMap<String, String>>),
}

impl Store {
    pub fn memory(pairs: &[(&str, &str)]) -> Self {
        Store::Memory(Mutex::new(pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()))
    }

    pub fn get(&self, name: &str) -> Option<String> {
        match self {
            Store::Memory(m) => m.lock().unwrap().get(name).cloned(),
            Store::System => {
                let env = NAMES.iter().find(|(n, _)| *n == name).map(|(_, e)| *e)?;
                if let Ok(v) = std::env::var(env) {
                    if !v.is_empty() {
                        return Some(v);
                    }
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
            Store::System => keyring::Entry::new(SERVICE, name)?.set_password(value)?,
        }
        Ok(())
    }

    pub fn delete(&self, name: &str) -> Result<()> {
        check(name)?;
        match self {
            Store::Memory(m) => {
                m.lock().unwrap().remove(name);
            }
            Store::System => match keyring::Entry::new(SERVICE, name)?.delete_credential() {
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
