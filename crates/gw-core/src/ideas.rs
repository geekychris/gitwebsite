//! The curation list (`ideas.yaml`): things worth writing up or building, and their progress.

use anyhow::{bail, Result};
use chrono::{Local, NaiveDate};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const STATUSES: &[&str] = &["idea", "planned", "drafting", "review", "published", "dropped"];

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Ideas {
    #[serde(default)]
    pub ideas: Vec<Idea>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Idea {
    pub id: u32,
    pub title: String,
    #[serde(default)]
    pub status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub areas: Vec<String>,
    /// Kind of content it should become (article, project, update, app, playground).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// Where the material is: repo path, URL, SmugMug album...
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    /// Content item id once drafted.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content: String,
    pub created: NaiveDate,
    pub updated: NaiveDate,
}

const HEADER: &str = "# Curation list: ideas and work in progress for the site.\n# Statuses: idea, planned, drafting, review, published, dropped. Managed by `gw idea ...`.\n";

impl Ideas {
    pub fn load(path: &Path) -> Result<Self> {
        crate::store::load(path)
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        crate::store::save(path, HEADER, self)
    }
    pub fn add(&mut self, title: &str, notes: &str, areas: Vec<String>, kind: &str, source: &str) -> &Idea {
        let id = self.ideas.iter().map(|i| i.id).max().unwrap_or(0) + 1;
        let today = Local::now().date_naive();
        self.ideas.push(Idea {
            id,
            title: title.into(),
            status: "idea".into(),
            notes: notes.into(),
            areas,
            kind: kind.into(),
            source: source.into(),
            content: String::new(),
            created: today,
            updated: today,
        });
        self.ideas.last().unwrap()
    }
    pub fn get_mut(&mut self, id: u32) -> Result<&mut Idea> {
        match self.ideas.iter_mut().find(|i| i.id == id) {
            Some(i) => Ok(i),
            None => bail!("no idea #{id}"),
        }
    }
    pub fn set_status(&mut self, id: u32, status: &str) -> Result<()> {
        if !STATUSES.contains(&status) {
            bail!("status must be one of {}", STATUSES.join(", "));
        }
        let i = self.get_mut(id)?;
        i.status = status.into();
        i.updated = Local::now().date_naive();
        Ok(())
    }
}
