//! A site on disk: `site.yaml`, `content/`, `templates/`, `static/`, and the YAML stores.

use crate::config::{expand_home, SiteConfig};
use crate::content::{self, Item, Kind};
use crate::ideas::Ideas;
use crate::media::Library;
use crate::registry::Registry;
use anyhow::{bail, Context, Result};
use regex::Regex;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub struct Site {
    pub root: PathBuf,
    pub config: SiteConfig,
}

impl Site {
    /// Open the site at `root`, or find it: `GW_SITE`, then `site.yaml` here or above (a
    /// `site/site.yaml` counts too), then the current site in the user config
    /// (see [`crate::userconfig`]).
    pub fn open(root: Option<&Path>) -> Result<Self> {
        let root = match root {
            Some(r) => r.to_path_buf(),
            None => find_root()?,
        };
        let cfg_path = root.join("site.yaml");
        if !cfg_path.exists() {
            bail!("{} has no site.yaml", root.display());
        }
        let config = SiteConfig::load(&cfg_path)?;
        Ok(Site { root, config })
    }

    pub fn content_dir(&self) -> PathBuf { self.root.join("content") }
    pub fn templates_dir(&self) -> PathBuf { self.root.join("templates") }
    pub fn static_dir(&self) -> PathBuf { self.root.join("static") }
    pub fn out_dir(&self) -> PathBuf { self.root.join(&self.config.build.out_dir) }
    pub fn cache_dir(&self) -> PathBuf { self.root.join(".gw-cache") }
    pub fn registry_path(&self) -> PathBuf { self.root.join("registry.yaml") }
    pub fn ideas_path(&self) -> PathBuf { self.root.join("ideas.yaml") }
    pub fn media_path(&self) -> PathBuf { self.root.join("media.yaml") }

    pub fn registry(&self) -> Result<Registry> { Registry::load(&self.registry_path()) }
    pub fn ideas(&self) -> Result<Ideas> { Ideas::load(&self.ideas_path()) }
    pub fn media(&self) -> Result<Library> { Library::load(&self.media_path()) }

    /// All content: the site's own `content/` plus every configured external source.
    pub fn items(&self) -> Result<Vec<Item>> {
        let mut items = content::scan(&self.content_dir())?;
        for src in &self.config.sources {
            let repo = expand_home(&src.repo);
            if !repo.exists() {
                eprintln!("warning: source repo {} not found; skipping", repo.display());
                continue;
            }
            let label = if src.label.is_empty() {
                repo.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
            } else {
                src.label.clone()
            };
            let re = glob_re(&src.include)?;
            for e in WalkDir::new(&repo).into_iter().filter_entry(|e| e.file_name() != ".git") {
                let e = e?;
                let rel = e.path().strip_prefix(&repo)?.to_string_lossy().replace('\\', "/");
                if e.file_type().is_file() && re.is_match(&rel) {
                    let name = e.path().file_name().unwrap();
                    let mut it = content::load_item(e.path(), &Path::new("articles").join(name))?;
                    it.id = format!("ext:{label}/{rel}");
                    if it.source.is_none() {
                        it.source = Some(format!("repo:{label}"));
                    }
                    items.push(it);
                }
            }
        }
        // URLs must be unique.
        let mut seen = std::collections::BTreeMap::new();
        for it in &items {
            if let Some(prev) = seen.insert(it.url.clone(), it.id.clone()) {
                bail!("two items have the URL {}: {} and {}", it.url, prev, it.id);
            }
        }
        Ok(items)
    }

    pub fn item(&self, id: &str) -> Result<Item> {
        self.items()?
            .into_iter()
            .find(|i| i.id == id || i.url == id || i.slug == id)
            .with_context(|| format!("no content item {id}"))
    }

    /// Check the site: content parses, areas exist, projects referenced by updates exist,
    /// media keys resolve, registry sources resolve. Returns warnings; errors are `Err`.
    pub fn check(&self) -> Result<Vec<String>> {
        let items = self.items()?;
        let media = self.media()?;
        let reg = self.registry()?;
        let mut problems = vec![];
        let projects: Vec<&str> = items.iter().filter(|i| i.kind == Kind::Project).map(|i| i.slug.as_str()).collect();
        for it in &items {
            for a in &it.areas {
                if self.config.resolve_area(a).is_none() {
                    problems.push(format!("{}: unknown area {a:?}", it.id));
                }
            }
            if let Some(p) = &it.project {
                if !projects.contains(&p.as_str()) {
                    problems.push(format!("{}: project {p:?} does not exist", it.id));
                }
            }
            if it.kind.is_post() && it.date.is_none() {
                problems.push(format!("{}: posts need a :date:", it.id));
            }
            if let Some(src) = &it.source {
                if !src.starts_with("repo:") && reg.get(src).is_none() {
                    problems.push(format!("{}: :source: {src} is not in the registry", it.id));
                }
            }
            let text = std::fs::read_to_string(&it.path)?;
            let models = crate::model::expand(&text, &crate::model::Env {
                lib: Some(&media),
                bundle_dir: it.bundle_dir.as_deref(),
                static_dir: Some(&self.static_dir()),
                static_html: false,
            });
            problems.extend(models.problems.iter().map(|p| format!("{}: {p}", it.id)));
            if let Err(e) = media.expand(&models.text) {
                problems.push(format!("{}: {e}", it.id));
            }
            if it.kind == Kind::App && it.app_url.is_none() {
                problems.push(format!("{}: apps need a :url:", it.id));
            }
        }
        Ok(problems)
    }
}

fn find_root() -> Result<PathBuf> {
    crate::userconfig::resolve_site()
}

/// Convert a simple glob (`*`, `**`, `?`) to an anchored regex.
pub fn glob_re(glob: &str) -> Result<Regex> {
    let mut re = String::from("^");
    let mut chars = glob.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
                if chars.peek() == Some(&'/') {
                    chars.next();
                    re.push_str("(?:.*/)?");
                } else {
                    re.push_str(".*");
                }
            }
            '*' => re.push_str("[^/]*"),
            '?' => re.push_str("[^/]"),
            c => re.push_str(&regex::escape(&c.to_string())),
        }
    }
    re.push('$');
    Ok(Regex::new(&re)?)
}
