//! The media library (`media.yaml`): named images and videos that content refers to as
//! `media:<key>` — `image::media:bench-front[Bench, front view]`, `video::media:demo[]`,
//! or `:hero: media:bench-front`. A key can point at a file in the site, any URL, or a
//! SmugMug image (whose served URL is filled in by the SmugMug integration).

use anyhow::{bail, Result};
use regex::{Captures, Regex};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Library {
    #[serde(default)]
    pub media: BTreeMap<String, Media>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Media {
    /// The URL served to readers (absolute, or site-relative like `/media/x.jpg`).
    pub url: String,
    /// image | video
    #[serde(default = "image")]
    pub kind: String,
    /// local | smugmug | external
    #[serde(default = "external")]
    pub provider: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thumb: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub alt: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub caption: String,
    /// Provider-specific identifiers (SmugMug image key, album key, WebUri...).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ids: BTreeMap<String, String>,
}

fn image() -> String {
    "image".into()
}
fn external() -> String {
    "external".into()
}

const HEADER: &str = "# Media library: content refers to these as media:<key>.\n# Managed by `gw media ...`.\n";

fn token_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"media:([A-Za-z0-9][A-Za-z0-9_.\-]*)").unwrap())
}

impl Library {
    pub fn load(path: &Path) -> Result<Self> {
        crate::store::load(path)
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        crate::store::save(path, HEADER, self)
    }
    pub fn resolve(&self, key: &str) -> Option<&Media> {
        self.media.get(key)
    }
    /// Replace every `media:<key>` with its URL. Unknown keys are an error, so a broken
    /// reference fails the build instead of shipping a broken image.
    pub fn expand(&self, text: &str) -> Result<String> {
        let mut missing = vec![];
        let out = token_re().replace_all(text, |c: &Captures| match self.media.get(&c[1]) {
            Some(m) => m.url.clone(),
            None => {
                missing.push(c[1].to_string());
                c[0].to_string()
            }
        });
        if !missing.is_empty() {
            bail!("unknown media key(s): {}", missing.join(", "));
        }
        Ok(out.into_owned())
    }
}
