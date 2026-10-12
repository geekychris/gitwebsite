//! The provenance registry (`registry.yaml`): every piece of work the site knows about and
//! where it came from — books and documents published elsewhere on the domain, imported
//! WordPress posts, repositories, SmugMug albums, external articles.
//!
//! Registry entries of kind `publication` drive the publications page; others are provenance
//! records that content items point at with `:source: <id>`.

use anyhow::{bail, Result};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Registry {
    #[serde(default)]
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    /// publication | repo | wordpress | smugmug | app | external | document
    pub kind: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub subtitle: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Where the source of truth lives: a repo URL, a path, a WordPress post id, an album key.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub origin: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<Link>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub meta: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub areas: Vec<String>,
    /// Group heading on the publications page (e.g. Books, Strategy).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub group: String,
    /// public | unlisted
    #[serde(default = "public")]
    pub visibility: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added: Option<NaiveDate>,
    /// Content item id generated from or describing this entry, if any.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Link {
    pub label: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub primary: bool,
}

fn public() -> String {
    "public".into()
}

const HEADER: &str = "# Provenance registry: what the site knows about and where it came from.\n# Managed by gitwebsite (`gw registry ...`); hand edits are fine.\n";

impl Registry {
    pub fn load(path: &Path) -> Result<Self> {
        crate::store::load(path)
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        crate::store::save(path, HEADER, self)
    }
    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }
    pub fn upsert(&mut self, e: Entry) {
        if let Some(x) = self.entries.iter_mut().find(|x| x.id == e.id) {
            *x = e;
        } else {
            self.entries.push(e);
        }
    }
    pub fn remove(&mut self, id: &str) -> Result<()> {
        let n = self.entries.len();
        self.entries.retain(|e| e.id != id);
        if self.entries.len() == n {
            bail!("no registry entry {id}");
        }
        Ok(())
    }
}
