//! The media library (`media.yaml`): named images and videos that content refers to as
//! `media:<key>` — `image::media:bench-front[Bench, front view]`, `video::media:demo[]`,
//! or `:hero: media:bench-front`. A key can point at a file in the site, any URL, or a
//! SmugMug image (whose served URL is filled in by the SmugMug integration).
//!
//! A `model` entry is a 3-D model for `model::media:<key>[]` (see [`crate::model`]): `url` is
//! the web GLB, `ios` the USDZ for AR Quick Look, `poster` the still shown before it loads, and
//! `download` an optional print file (STL/3MF) offered to readers. `gw media add-model` makes
//! these from the Meshy models repository into `static/models/<key>/`.

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

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Media {
    /// The URL served to readers (absolute, or site-relative like `/media/x.jpg`).
    pub url: String,
    /// image | video | model
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
    // ---- models (kind: model)
    /// USDZ for AR Quick Look on iOS.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ios: String,
    /// Still image shown until the model loads (and without JavaScript).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub poster: String,
    /// A print file readers may download (STL or 3MF).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub download: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub download_bytes: u64,
    /// Size of the web file (`url`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub ios_bytes: u64,
    /// Triangles in the web model.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub triangles: u64,
    /// Printed size, x × y × z in millimetres.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dimensions_mm: Vec<f64>,
    /// Separate printed parts (a multi-part print), when more than one.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub parts: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_entry_round_trip() {
        let dir = std::env::temp_dir().join(format!("gw-media-rt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("media.yaml");
        let mut lib = Library::default();
        let mut ids = BTreeMap::new();
        ids.insert("meshy_id".to_string(), "d137c11a".to_string());
        ids.insert("model".to_string(), "lemmings/wizard-lemming".to_string());
        let m = Media {
            url: "/models/wizard-lemming/wizard-lemming.glb".into(),
            kind: "model".into(),
            provider: "local".into(),
            thumb: "/models/wizard-lemming/poster.webp".into(),
            caption: "Wizard Lemming".into(),
            ids,
            ios: "/models/wizard-lemming/wizard-lemming.usdz".into(),
            poster: "/models/wizard-lemming/poster.webp".into(),
            download: "/models/wizard-lemming/wizard-lemming-plate.stl".into(),
            download_bytes: 32_361_984,
            bytes: 2_097_008,
            ios_bytes: 12_560_701,
            triangles: 150_000,
            dimensions_mm: vec![82.7, 60.2, 100.0],
            parts: 2,
            ..Default::default()
        };
        lib.media.insert("wizard-lemming".into(), m.clone());
        lib.media.insert("photo".into(), Media { url: "https://example.org/a.jpg".into(), kind: "image".into(), provider: "external".into(), ..Default::default() });
        lib.save(&p).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        // Images carry none of the model fields.
        let photo = &text[text.find("  photo:").unwrap()..text.find("  wizard-lemming:").unwrap()];
        assert!(!photo.contains("ios") && !photo.contains("bytes") && !photo.contains("dimensions"), "{photo}");
        let back = Library::load(&p).unwrap();
        assert_eq!(back.media["wizard-lemming"], m);
        assert_eq!(back.media["photo"].kind, "image");
        // Old entries without the new fields still load.
        std::fs::write(&p, "media:\n  old:\n    url: /a.jpg\n").unwrap();
        let old = Library::load(&p).unwrap();
        assert_eq!(old.media["old"].kind, "image");
        assert!(old.media["old"].ios.is_empty() && old.media["old"].dimensions_mm.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
