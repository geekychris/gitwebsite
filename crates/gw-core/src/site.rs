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
    /// media keys resolve, registry sources resolve. Returns every problem (the errors of
    /// [`Site::check_report`] first); a site that cannot be read at all is `Err`.
    pub fn check(&self) -> Result<Vec<String>> {
        let r = self.check_report()?;
        Ok(r.errors.into_iter().chain(r.warnings).collect())
    }

    /// [`Site::check`] by severity: errors are references that cannot work (a `:works-with:`
    /// naming no tool, a malformed entry; a `tool::` block naming no tool or an `example=` its
    /// tool does not declare; a `:redirect-from:` path that is already a page); everything else
    /// is a warning.
    pub fn check_report(&self) -> Result<CheckReport> {
        let items = self.items()?;
        let media = self.media()?;
        let reg = self.registry()?;
        let mut problems = vec![];
        let projects: Vec<&str> = items.iter().filter(|i| i.kind == Kind::Project).map(|i| i.slug.as_str()).collect();
        let tool_catalog = self.tool_catalog(&items);
        let mut errors = redirect_problems(&self.config, &items);
        for it in &items {
            for a in &it.areas {
                if self.config.resolve_area(a).is_none() {
                    problems.push(format!("{}: unknown area {a:?}", it.id));
                }
            }
            if it.attrs.contains_key("hero-crop") && it.hero_crop().is_none() {
                problems.push(format!("{}: :hero-crop: wants left, top, width, height in percent of the image (e.g. 10, 20, 80, 45)", it.id));
            }
            if let Some(p) = &it.project {
                if !projects.contains(&p.as_str()) {
                    problems.push(format!("{}: project {p:?} does not exist", it.id));
                }
            }
            if it.kind.is_post() && it.date.is_none() {
                problems.push(format!("{}: posts need a :date:", it.id));
            }
            if it.kind == Kind::Exploration && it.date.is_none() {
                problems.push(format!("{}: explorations need a :date: (the Lab lists them newest first)", it.id));
            }
            if it.attrs.contains_key("lab") && it.kind != Kind::Tool {
                problems.push(format!("{}: :lab: only lists tool pages on the Lab page", it.id));
            }
            if let Some(src) = &it.source {
                if !src.starts_with("repo:") && reg.get(src).is_none() {
                    problems.push(format!("{}: :source: {src} is not in the registry", it.id));
                }
            }
            let text = std::fs::read_to_string(&it.path)?;
            let (e, w) = crate::toolembed::problems(&text, &tool_catalog);
            errors.extend(e.into_iter().map(|p| format!("{}: {p}", it.id)));
            problems.extend(w.into_iter().map(|p| format!("{}: {p}", it.id)));
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
        problems.extend(crate::gallery::check(&self.root, &self.static_dir(), &media));
        problems.extend(crate::codecat::check(&self.root, &self.static_dir()));
        let (ww_errors, ww_warnings) = works_with_problems(&items);
        errors.extend(ww_errors);
        problems.extend(ww_warnings);
        Ok(CheckReport { errors, warnings: problems })
    }

    /// The site's tools by slug, read from their pages as the tool catalogue reads them (their
    /// `?example=` presets and `#` tabs; see [`crate::toolcat`]), drafts included.
    pub fn tool_catalog(&self, items: &[Item]) -> std::collections::BTreeMap<String, crate::toolcat::Tool> {
        items
            .iter()
            .filter(|i| i.kind == Kind::Tool)
            .map(|i| {
                let text = std::fs::read_to_string(&i.path).unwrap_or_default();
                // A page in content/tools/ without `:type: tool` is still a tool.
                let text = if !i.attrs.contains_key("type") { text.replacen('\n', "\n:type: tool\n", 1) } else { text };
                let t = crate::toolcat::parse(&i.slug, &text, &i.path)
                    .unwrap_or_else(|| crate::toolcat::Tool { slug: i.slug.clone(), title: i.title.clone(), ..Default::default() });
                (i.slug.clone(), t)
            })
            .collect()
    }
}

/// Addresses the build writes besides the items' own pages (listings, area and tag pages).
pub fn generated_urls(cfg: &SiteConfig, items: &[Item]) -> std::collections::BTreeSet<String> {
    let mut urls: std::collections::BTreeSet<String> =
        ["/", "/articles/", "/projects/", "/areas/", "/lab/", "/tools/", "/publications/", "/models/", "/code/"].iter().map(|s| s.to_string()).collect();
    urls.extend(cfg.flat_areas().into_iter().map(|(p, _)| format!("/areas/{p}/")));
    urls.extend(items.iter().flat_map(|i| i.tags.iter()).map(|t| format!("/tags/{}/", content::slugify(t))));
    urls
}

/// `:redirect-from:` paths that would replace a real page — an item, a listing, an area or tag
/// page, or another item's redirect — are errors (the redirect page would hide it).
pub fn redirect_problems(cfg: &SiteConfig, items: &[Item]) -> Vec<String> {
    let mut taken: std::collections::BTreeMap<String, String> = items.iter().map(|i| (i.url.clone(), format!("the page of {}", i.id))).collect();
    for u in generated_urls(cfg, items) {
        taken.entry(u.clone()).or_insert_with(|| format!("the generated page {u}"));
    }
    let mut errors = vec![];
    for it in items {
        for old in it.redirect_from() {
            // `/x/` and `/x/index.html` are the same file.
            let key = old.strip_suffix("index.html").unwrap_or(&old).to_string();
            if key == it.url {
                errors.push(format!("{}: :redirect-from: {old} is this page's own address", it.id));
            } else if let Some(what) = taken.get(&key) {
                errors.push(format!("{}: :redirect-from: {old} is already {what}", it.id));
            } else {
                taken.insert(key, format!("a redirect to {}", it.url));
            }
        }
    }
    errors
}

/// The result of [`Site::check_report`].
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct CheckReport {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

/// `:works-with:` on tool pages: an entry naming no tool or a malformed entry is an error; a
/// tool naming itself, or the same tool twice, is a warning; the attribute anywhere but a tool
/// page is a warning (only tool pages show it). Returns (errors, warnings).
pub fn works_with_problems(items: &[Item]) -> (Vec<String>, Vec<String>) {
    let tools: std::collections::BTreeSet<&str> = items.iter().filter(|i| i.kind == Kind::Tool).map(|i| i.slug.as_str()).collect();
    let (mut errors, mut warnings) = (vec![], vec![]);
    for it in items.iter().filter(|i| i.attrs.contains_key("works-with")) {
        if it.kind != Kind::Tool {
            warnings.push(format!("{}: :works-with: is only shown on tool pages", it.id));
            continue;
        }
        let mut seen = std::collections::BTreeSet::new();
        for e in it.works_with() {
            match e {
                Err(m) => errors.push(format!("{}: :works-with: {m}", it.id)),
                Ok(w) if w.slug == it.slug => warnings.push(format!("{}: :works-with: names the tool itself", it.id)),
                Ok(w) if !tools.contains(w.slug.as_str()) => errors.push(format!("{}: :works-with: no tool {:?} (content/tools/{}.adoc)", it.id, w.slug, w.slug)),
                Ok(w) if !seen.insert(w.slug.clone()) => warnings.push(format!("{}: :works-with: {:?} is listed twice", it.id, w.slug)),
                Ok(_) => {}
            }
        }
    }
    (errors, warnings)
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
