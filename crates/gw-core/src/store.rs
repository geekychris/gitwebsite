//! Small helper for the YAML list stores (registry, ideas, media).

use anyhow::{Context, Result};
use serde::{de::DeserializeOwned, Serialize};
use std::path::Path;

pub fn load<T: DeserializeOwned + Default>(path: &Path) -> Result<T> {
    if !path.exists() {
        return Ok(T::default());
    }
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    if text.trim().is_empty() {
        return Ok(T::default());
    }
    serde_yaml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

pub fn save<T: Serialize>(path: &Path, header: &str, value: &T) -> Result<()> {
    let body = serde_yaml::to_string(value)?;
    std::fs::write(path, format!("{header}{body}")).with_context(|| format!("writing {}", path.display()))
}
