//! The code catalog: the site's git projects at `/code/` (a grid with categories, a filter and
//! tags, like the tools page) and `/code/<slug>/` (one page per project, with its logo, its
//! description, tags, and related projects).
//!
//! The catalog is the site's own file, `code.yaml` beside `site.yaml`:
//!
//! ```yaml
//! title: Code
//! intro: Things I have built and published on GitHub.
//! owner: geekychris                # whose repositories `gw code sync` reads
//! logo_style: Flat vector icon …   # the house style every logo prompt ends with
//! categories:                      # display order; a project names one with `category:`
//!   - { id: ai, title: AI and agents, description: … }
//! projects:
//!   - slug: amiga-mcp              # /code/amiga-mcp/
//!     repo: geekychris/amiga_mcp
//!     title: Amiga DevBench        # editorial fields, edited in the app or by hand …
//!     summary: One line for the tile.
//!     description: |               # paragraphs; `code`, [links](https://…) and - bullets
//!       …
//!     category: retro
//!     tags: [amiga, mcp]
//!     related: [amiga-games]       # slugs; shown both ways
//!     hidden: false                # left off the website
//!     featured: false              # first in its category
//!     logo: amiga-mcp.png          # in static/code-logos/ (found by sync, or imported)
//!     logo_prompt: …               # a hand-written image prompt (else one is composed)
//!     github: { … }                # … and facts refreshed by `gw code sync` (not edited)
//! ```
//!
//! `gw code sync` (and the app's Code page) adds new repositories, refreshes the `github:` facts,
//! and copies each repository's logo into `static/code-logos/`, so the build reads nothing but the
//! site. Projects without a logo get an image prompt ([`logo_prompt`]), handed out in batches
//! ([`logo_batches`]); a rendered logo is imported back, and [`readme_with_logo`] puts it at the
//! top of the repository's README.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The catalog file, beside site.yaml.
pub const FILE: &str = "code.yaml";
/// Under `static/`: the projects' logos, served at `/code-logos/<file>`.
pub const LOGO_DIR: &str = "code-logos";
/// Where the catalog lives on the website.
pub const URL: &str = "/code/";

pub const DEFAULT_TITLE: &str = "Code";
pub const DEFAULT_INTRO: &str = "Projects I have built and published on GitHub — tools, experiments, retro computing and more. Filter by category or search; open one for what it does and where to find it.";
pub const DEFAULT_LOGO_STYLE: &str = "Style: a clean, modern flat vector logo mark — a single bold, simple symbol that still reads at 64 pixels, \
centred on a transparent background (or plain white if transparency is not possible), with a limited palette of two or three \
harmonious colours, soft geometric shapes, no gradients heavier than a subtle shade, no text, letters or words, no mock-ups, \
no frames or borders. Square 1:1 image, 1024 × 1024.";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Catalog {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub intro: String,
    /// The GitHub user or organisation whose repositories sync reads.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub owner: String,
    /// Appended to every composed logo prompt, so the logos look like a family.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub logo_style: String,
    #[serde(default)]
    pub categories: Vec<Category>,
    /// Logo families: one mark shared by every project that names the family.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub families: Vec<Family>,
    #[serde(default)]
    pub projects: Vec<Project>,
}

/// A logo family: projects that share one mark (e.g. the modules of one framework). Its logo is
/// rendered once (`family-<id>.png`) and used by each member without a logo of its own.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Family {
    pub id: String,
    pub title: String,
    /// What the family is, for the prompt.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// The mark's idea, e.g. "a stylised red bull".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub concept: String,
    /// A hand-written prompt (else one is composed from the above and the house style).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub logo_prompt: String,
    /// File name in `static/code-logos/` (`family-<id>.<ext>`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub logo: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Category {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Project {
    /// The page's URL segment: `/code/<slug>/`.
    pub slug: String,
    /// `owner/name` on GitHub.
    pub repo: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    /// One line for the tile (default: the GitHub description).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    /// The project page's text: paragraphs, `code`, [links](url), `- ` bullets.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub category: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Slugs of related projects (shown on both projects' pages).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub featured: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<i64>,
    /// File name in `static/code-logos/`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub logo: String,
    /// Where the logo came from: `readme`, `repo` (a logo file in the repository), `imported`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub logo_source: String,
    /// A hand-written prompt for the logo (else [`logo_prompt`] composes one).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub logo_prompt: String,
    /// The pull request that adds the logo to the repository's README.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub logo_pr: String,
    /// The post that writes the project up (content id); else found by its `:repo:`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub post: String,
    /// The logo family it belongs to (its mark is used when the project has no logo of its own).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub family: String,
    /// When the text was last improved with Claude (YYYY-MM-DD).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub enhanced: String,
    /// When it was added to the catalog (YYYY-MM-DD).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub added: String,
    #[serde(default)]
    pub github: GitHub,
}

/// Facts from GitHub, refreshed by sync.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct GitHub {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub language: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub topics: Vec<String>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub stars: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub homepage: String,
    /// Last push (YYYY-MM-DD).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pushed: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub created: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub license: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub default_branch: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub fork: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub archived: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub private: bool,
    /// No longer on GitHub (or no longer readable).
    #[serde(default, skip_serializing_if = "is_false")]
    pub gone: bool,
    /// The logo's path in the repository, when it has one.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub logo_path: String,
}

fn is_false(b: &bool) -> bool {
    !*b
}
fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// A slug: lower-case letters, digits and `-`.
pub fn safe_slug(s: &str) -> bool {
    !s.is_empty() && s.len() <= 100 && !s.starts_with('-') && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// `amiga_mcp` → `amiga-mcp`, `3DO.Dev` → `3do-dev`.
pub fn slug_for(repo_name: &str) -> String {
    let mut out = String::new();
    for c in repo_name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() { "project".into() } else { out }
}

/// `owner/name`, checked.
pub fn parse_repo(repo: &str) -> Result<(&str, &str)> {
    let ok = |s: &str| !s.is_empty() && !s.starts_with('.') && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    match repo.split_once('/') {
        Some((o, n)) if ok(o) && ok(n) => Ok((o, n)),
        _ => bail!("repo {repo:?}: use owner/name"),
    }
}

/// A logo file name in `static/code-logos/`: `<slug>.<png|jpg|jpeg|webp|svg|gif>`.
pub fn safe_logo(name: &str) -> bool {
    match name.rsplit_once('.') {
        Some((stem, ext)) => safe_slug(stem) && LOGO_EXTS.contains(&ext),
        None => false,
    }
}
pub const LOGO_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp", "svg", "gif"];

/// `amiga_mcp` → `Amiga MCP`-ish: words capitalised (short all-consonant words upper-cased).
pub fn title_for(repo_name: &str) -> String {
    repo_name
        .split(|c: char| c == '-' || c == '_' || c == '.' || c.is_whitespace())
        .filter(|w| !w.is_empty())
        .map(|w| {
            let lower = w.to_ascii_lowercase();
            if w.chars().any(|c| c.is_ascii_uppercase()) && w.chars().any(|c| c.is_ascii_lowercase()) {
                w.to_string() // already cased: GitWebsite, macOS
            } else if lower.len() <= 4 && !lower.chars().any(|c| "aeiouy".contains(c)) && lower.chars().all(|c| c.is_ascii_alphanumeric()) {
                lower.to_ascii_uppercase() // mcp, sql, k8s
            } else {
                let mut c = lower.chars();
                c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

impl Project {
    pub fn name(&self) -> &str {
        self.repo.split_once('/').map(|(_, n)| n).unwrap_or(&self.repo)
    }
    pub fn display_title(&self) -> String {
        if self.title.trim().is_empty() { title_for(self.name()) } else { self.title.trim().to_string() }
    }
    pub fn display_summary(&self) -> String {
        if self.summary.trim().is_empty() { self.github.description.trim().to_string() } else { self.summary.trim().to_string() }
    }
    pub fn repo_url(&self) -> String {
        format!("https://github.com/{}", self.repo)
    }
    /// On the website: not hidden, and still on GitHub and public.
    pub fn listed(&self) -> bool {
        !self.hidden && !self.github.gone && !self.github.private
    }
}

impl Catalog {
    pub fn path(site_root: &Path) -> PathBuf {
        site_root.join(FILE)
    }

    pub fn load(site_root: &Path) -> Result<Option<Catalog>> {
        let p = Self::path(site_root);
        if !p.exists() {
            return Ok(None);
        }
        let t = std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
        Ok(Some(Self::parse(&t).with_context(|| format!("parsing {}", p.display()))?))
    }

    pub fn parse(text: &str) -> Result<Catalog> {
        if text.trim().is_empty() {
            return Ok(Catalog::default());
        }
        let c: Catalog = serde_yaml::from_str(text)?;
        c.validate()?;
        Ok(c)
    }

    /// Hard errors: bad or duplicate slugs and repos, bad logo names, bad category ids.
    pub fn validate(&self) -> Result<()> {
        let mut slugs = BTreeSet::new();
        let mut repos = BTreeSet::new();
        for p in &self.projects {
            if !safe_slug(&p.slug) {
                bail!("project slug {:?}: lower-case letters, digits and '-' only", p.slug);
            }
            parse_repo(&p.repo)?;
            if !slugs.insert(p.slug.as_str()) {
                bail!("project {} is listed twice", p.slug);
            }
            if !repos.insert(p.repo.to_ascii_lowercase()) {
                bail!("repository {} is listed twice", p.repo);
            }
            if !p.logo.is_empty() && !safe_logo(&p.logo) {
                bail!("{}: logo {:?} must be <slug>.png|jpg|webp|svg|gif in {LOGO_DIR}/", p.slug, p.logo);
            }
        }
        let mut fams = BTreeSet::new();
        for f in &self.families {
            if !safe_slug(&f.id) {
                bail!("family id {:?}: lower-case letters, digits and '-' only", f.id);
            }
            if !fams.insert(f.id.as_str()) {
                bail!("family {} is listed twice", f.id);
            }
            if !f.logo.is_empty() && !safe_logo(&f.logo) {
                bail!("family {}: logo {:?} must be family-<id>.png|jpg|webp|svg|gif", f.id, f.logo);
            }
        }
        for p in self.projects.iter().filter(|p| !p.family.is_empty()) {
            if !fams.contains(p.family.as_str()) {
                bail!("{}: family {} is not in families", p.slug, p.family);
            }
        }
        let mut ids = BTreeSet::new();
        for c in &self.categories {
            if !safe_slug(&c.id) {
                bail!("category id {:?}: lower-case letters, digits and '-' only", c.id);
            }
            if !ids.insert(c.id.as_str()) {
                bail!("category {} is listed twice", c.id);
            }
        }
        Ok(())
    }

    pub fn to_yaml(&self) -> Result<String> {
        Ok(format!(
            "# The code catalog (/code/): the site's git projects. Edit in the gitwebsite app's Code page or\n\
             # by hand; `gw code sync` adds new repositories and refreshes each project's github: facts and\n\
             # logo (static/{LOGO_DIR}/). Editorial fields: title, summary, description, category, tags,\n\
             # related (slugs), hidden, featured, order, logo_prompt, post, family (a logo family's id).\n{}",
            serde_yaml::to_string(self)?
        ))
    }

    pub fn save(&self, site_root: &Path) -> Result<()> {
        self.validate()?;
        let p = Self::path(site_root);
        let tmp = p.with_extension("yaml.tmp");
        std::fs::write(&tmp, self.to_yaml()?)?;
        std::fs::rename(&tmp, &p)?;
        Ok(())
    }

    pub fn get(&self, slug: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.slug == slug)
    }
    pub fn get_mut(&mut self, slug: &str) -> Option<&mut Project> {
        self.projects.iter_mut().find(|p| p.slug == slug)
    }
    pub fn by_repo(&self, repo: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.repo.eq_ignore_ascii_case(repo))
    }

    /// A free slug for a new repository (`name`, then `name-2`, …).
    pub fn new_slug(&self, repo_name: &str) -> String {
        let base = slug_for(repo_name);
        let mut s = base.clone();
        let mut n = 2;
        while self.get(&s).is_some() {
            s = format!("{base}-{n}");
            n += 1;
        }
        s
    }

    pub fn family(&self, id: &str) -> Option<&Family> {
        self.families.iter().find(|f| f.id == id)
    }

    /// The logo a project shows: its own, else its family's. (file, from the family)
    pub fn logo_of<'a>(&'a self, p: &'a Project) -> Option<(&'a str, bool)> {
        if !p.logo.is_empty() {
            return Some((&p.logo, false));
        }
        self.family(&p.family).filter(|f| !f.logo.is_empty()).map(|f| (f.logo.as_str(), true))
    }

    pub fn style(&self) -> &str {
        if self.logo_style.trim().is_empty() { DEFAULT_LOGO_STYLE } else { self.logo_style.trim() }
    }

    pub fn category_title(&self, id: &str) -> String {
        self.categories.iter().find(|c| c.id == id).map(|c| c.title.clone()).unwrap_or_else(|| if id.is_empty() { "Other".into() } else { title_for(id) })
    }

    /// Related projects of `slug`, both ways (its own list, then those that list it), listed only.
    pub fn related_of(&self, slug: &str) -> Vec<&Project> {
        let Some(me) = self.get(slug) else { return vec![] };
        let mut out: Vec<&Project> = vec![];
        let mut seen = BTreeSet::from([slug]);
        for r in &me.related {
            if let Some(p) = self.get(r).filter(|p| p.listed()) {
                if seen.insert(p.slug.as_str()) {
                    out.push(p);
                }
            }
        }
        for p in self.projects.iter().filter(|p| p.listed() && p.related.iter().any(|r| r == slug)) {
            if seen.insert(p.slug.as_str()) {
                out.push(p);
            }
        }
        out
    }

    /// Listed projects in display order: by category (as `categories` lists them, then the
    /// rest), then featured, `order`, title.
    pub fn ordered(&self) -> Vec<&Project> {
        let rank = |c: &str| self.categories.iter().position(|x| x.id == c).unwrap_or(usize::MAX);
        let mut v: Vec<&Project> = self.projects.iter().filter(|p| p.listed()).collect();
        v.sort_by_key(|p| (rank(&p.category), !p.featured, p.order.unwrap_or(i64::MAX), p.display_title().to_lowercase()));
        v
    }
}

// ------------------------------------------------------------------ logos

/// The image prompt for a project's logo: its own `logo_prompt`, or one composed from what the
/// project is, ending with the catalog's house style.
pub fn logo_prompt(cat: &Catalog, p: &Project) -> String {
    if !p.logo_prompt.trim().is_empty() {
        return p.logo_prompt.trim().to_string();
    }
    let title = p.display_title();
    let mut what = clip_sentences(&p.display_summary(), 300);
    if what.is_empty() {
        what = first_paragraph(&p.description);
    }
    let mut s = format!("Design a logo for \"{title}\", a software project on GitHub.");
    if !what.is_empty() {
        s.push_str(&format!(" What it is: {}", what.trim_end_matches('.')));
        s.push('.');
    }
    let mut hints: Vec<String> = vec![];
    if !p.github.language.is_empty() {
        hints.push(format!("written in {}", p.github.language));
    }
    let tags: Vec<&String> = p.tags.iter().chain(p.github.topics.iter()).collect::<BTreeSet<_>>().into_iter().take(6).collect();
    if !tags.is_empty() {
        hints.push(format!("themes: {}", tags.iter().map(|t| t.as_str()).collect::<Vec<_>>().join(", ")));
    }
    if !hints.is_empty() {
        s.push_str(&format!(" ({}.)", hints.join("; ")));
    }
    s.push_str(" Use a symbol or metaphor that suggests what the project does, not a generic code or gear icon. ");
    s.push_str(cat.style());
    s
}

/// At most `n` characters, cut at a sentence end when there is one.
fn clip_sentences(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let cut: String = s.chars().take(n).collect();
    match cut.rfind(". ") {
        Some(i) if i > n / 3 => cut[..=i].to_string(),
        _ => format!("{}…", cut.trim_end()),
    }
}

fn first_paragraph(text: &str) -> String {
    let p = text.split("\n\n").map(str::trim).find(|p| !p.is_empty()).unwrap_or("");
    let one: String = p.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > 300 { one.chars().take(300).collect::<String>() + "…" } else { one }
}

/// One logo to render: which project, the file name to save it as, and the prompt.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PromptItem {
    pub slug: String,
    pub title: String,
    pub repo: String,
    /// What to save the rendered image as (import matches on it).
    pub file: String,
    pub prompt: String,
}

/// The prompt for a family's shared mark.
pub fn family_prompt(cat: &Catalog, f: &Family) -> String {
    if !f.logo_prompt.trim().is_empty() {
        return f.logo_prompt.trim().to_string();
    }
    let n = cat.projects.iter().filter(|p| p.family == f.id && p.listed()).count();
    let mut s = format!("Design a logo mark for \"{}\", a family of {n} related software projects on GitHub that will all share this one mark.", f.title);
    if !f.description.trim().is_empty() {
        s.push_str(&format!(" What it is: {}.", f.description.trim().trim_end_matches('.')));
    }
    if !f.concept.trim().is_empty() {
        s.push_str(&format!(" The mark: {}.", f.concept.trim().trim_end_matches('.')));
    } else {
        s.push_str(" Use a symbol or metaphor that suggests what the family does, not a generic code or gear icon.");
    }
    s.push(' ');
    s.push_str(cat.style());
    s
}

/// The file a family's rendered mark is saved as.
pub fn family_file(id: &str) -> String {
    format!("family-{id}.png")
}

/// The logos still needed — each family without a mark (if it has listed members), then the
/// listed projects with no logo and no family mark — in display order, split into batches.
pub fn logo_batches(cat: &Catalog, size: usize) -> Vec<Vec<PromptItem>> {
    let size = size.max(1);
    let mut items: Vec<PromptItem> = cat
        .families
        .iter()
        .filter(|f| f.logo.is_empty() && cat.projects.iter().any(|p| p.family == f.id && p.listed()))
        .map(|f| PromptItem { slug: format!("family-{}", f.id), title: format!("{} (family mark)", f.title), repo: String::new(), file: family_file(&f.id), prompt: family_prompt(cat, f) })
        .collect();
    items.extend(
        cat.ordered()
            .into_iter()
            .filter(|p| p.logo.is_empty() && (p.family.is_empty() || cat.family(&p.family).is_none()))
            .map(|p| PromptItem { slug: p.slug.clone(), title: p.display_title(), repo: p.repo.clone(), file: format!("{}.png", p.slug), prompt: logo_prompt(cat, p) }),
    );
    items.chunks(size).map(|c| c.to_vec()).collect()
}

/// Match a rendered image's file name to a family: `family-hitorro.png`, `Family_HiTorro (1).png`.
pub fn match_family_file<'a>(cat: &'a Catalog, file_name: &str) -> Option<&'a Family> {
    let (stem, ext) = file_name.rsplit_once('.')?;
    if !LOGO_EXTS.contains(&ext.to_ascii_lowercase().as_str()) {
        return None;
    }
    let s = slug_for(stem);
    let id = s.strip_prefix("family-")?;
    cat.family(id).or_else(|| id.rsplit_once('-').filter(|(_, n)| n.len() <= 2 && n.chars().all(|c| c.is_ascii_digit())).and_then(|(a, _)| cat.family(a)))
}

/// A batch as text to paste into ChatGPT: one numbered prompt per logo, each naming its file.
pub fn batch_text(batch: &[PromptItem], number: usize, of: usize) -> String {
    let mut s = format!(
        "Logo batch {number} of {of}: {} logo{}. Make one separate square image per numbered item below — do not combine them. \
         After each image, tell me the file name given for it.\n",
        batch.len(),
        if batch.len() == 1 { "" } else { "s" }
    );
    for (i, it) in batch.iter().enumerate() {
        s.push_str(&format!("\n{}. File name: {}\n{}\n", i + 1, it.file, it.prompt));
    }
    s
}

/// Match a rendered image's file name to a project: `amiga-mcp.png`, `amiga-mcp (1).png`,
/// `Amiga_MCP-logo.webp` all match `amiga-mcp`.
pub fn match_logo_file<'a>(cat: &'a Catalog, file_name: &str) -> Option<&'a Project> {
    let (stem, ext) = file_name.rsplit_once('.')?;
    if !LOGO_EXTS.contains(&ext.to_ascii_lowercase().as_str()) {
        return None;
    }
    let mut s = slug_for(stem);
    // Strip a copy counter and a trailing "logo".
    for suffix in ["-logo", "-icon"] {
        if let Some(x) = s.strip_suffix(suffix) {
            s = x.to_string();
        }
    }
    let strip_count = |s: &str| -> String {
        match s.rsplit_once('-') {
            Some((a, n)) if !a.is_empty() && n.len() <= 2 && n.chars().all(|c| c.is_ascii_digit()) && cat.get(s).is_none() => a.to_string(),
            _ => s.to_string(),
        }
    };
    let s2 = strip_count(&s);
    cat.get(&s).or_else(|| cat.get(&s2)).or_else(|| cat.projects.iter().find(|p| slug_for(p.name()) == s || slug_for(p.name()) == s2))
}

/// The README with the logo centred at the top (above the first heading). Unchanged when the
/// README already shows that image.
pub fn readme_with_logo(readme: &str, img_path: &str, alt: &str) -> String {
    if readme.contains(img_path) {
        return readme.to_string();
    }
    let block = format!(
        "<p align=\"center\">\n  <img src=\"{img_path}\" alt=\"{}\" width=\"180\">\n</p>\n\n",
        alt.replace('"', "&quot;")
    );
    // Keep front matter first.
    let lead = readme.strip_prefix("---\n").and_then(|rest| rest.find("\n---\n")).map(|i| i + 4 + 5).unwrap_or(0);
    let (head, rest) = readme.split_at(lead);
    format!("{head}{block}{}", rest.trim_start_matches('\n'))
}

/// Where a logo goes in a repository: `assets/logo.<ext>`, unless the repository already keeps
/// its logo somewhere.
pub fn repo_logo_path(p: &Project, ext: &str) -> String {
    if !p.github.logo_path.is_empty() && p.github.logo_path.ends_with(&format!(".{ext}")) {
        return p.github.logo_path.clone();
    }
    format!("assets/logo.{ext}")
}

/// A logo made fit for the site: raster images scaled to fit `max_px` (never enlarged) and saved
/// as PNG; SVG kept as it is, unless it carries script. Returns the bytes and the extension.
pub fn normalise_logo(bytes: &[u8], max_px: u32) -> Result<(Vec<u8>, &'static str)> {
    // Raster first: PNGs can carry metadata (C2PA) that mentions SVG.
    let raster = image::guess_format(bytes).is_ok();
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).trim_start_matches('\u{feff}').trim_start().to_ascii_lowercase();
    let xmlish = head.starts_with("<?xml") || head.starts_with("<!--") || head.starts_with("<!doctype svg");
    if !raster && (head.starts_with("<svg") || (xmlish && String::from_utf8_lossy(bytes).to_ascii_lowercase().contains("<svg"))) {
        if bytes.len() > 300 * 1024 {
            bail!("the SVG is {} KB (an embedded picture?); use a PNG", bytes.len() / 1024);
        }
        let all = String::from_utf8_lossy(bytes).to_ascii_lowercase();
        static ON: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let on = ON.get_or_init(|| regex::Regex::new(r"\son[a-z]+\s*=").unwrap());
        if all.contains("<script") || all.contains("javascript:") || all.contains("<foreignobject") || on.is_match(&all) {
            bail!("the SVG carries script; use a PNG");
        }
        return Ok((bytes.to_vec(), "svg"));
    }
    let img = image::load_from_memory(bytes).context("not an image gitwebsite can read (PNG, JPEG, WebP, GIF or SVG)")?;
    let img = if img.width() > max_px || img.height() > max_px { img.resize(max_px, max_px, image::imageops::FilterType::Lanczos3) } else { img };
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png)?;
    Ok((out.into_inner(), "png"))
}

// ------------------------------------------------------------------ text

/// A small, safe Markdown subset for descriptions: paragraphs, `- ` bullets, `code`,
/// **bold**, [text](http…) links. Everything else is text.
pub fn md_html(text: &str) -> String {
    let mut out = vec![];
    for block in text.replace("\r\n", "\n").split("\n\n").map(str::trim).filter(|b| !b.is_empty()) {
        let lines: Vec<&str> = block.lines().collect();
        if lines.iter().all(|l| l.trim_start().starts_with("- ") || l.trim_start().starts_with("* ")) {
            let items: Vec<String> = lines.iter().map(|l| format!("<li>{}</li>", inline(l.trim_start()[2..].trim()))).collect();
            out.push(format!("<ul>{}</ul>", items.join("")));
        } else if let Some(h) = block.strip_prefix("### ").or_else(|| block.strip_prefix("## ")).filter(|_| lines.len() == 1) {
            out.push(format!("<h3>{}</h3>", inline(h)));
        } else {
            out.push(format!("<p>{}</p>", inline(&lines.join(" "))));
        }
    }
    out.join("\n")
}

fn inline(s: &str) -> String {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"`([^`]+)`|\*\*([^*]+)\*\*|\[([^\]]+)\]\((https?://[^\s)]+|/[^\s)]*)\)").unwrap());
    let mut out = String::new();
    let mut last = 0;
    for c in re.captures_iter(s) {
        let m = c.get(0).unwrap();
        out.push_str(&crate::model::esc(&s[last..m.start()]));
        if let Some(code) = c.get(1) {
            out.push_str(&format!("<code>{}</code>", crate::model::esc(code.as_str())));
        } else if let Some(b) = c.get(2) {
            out.push_str(&format!("<strong>{}</strong>", crate::model::esc(b.as_str())));
        } else {
            out.push_str(&format!("<a href=\"{}\">{}</a>", crate::model::esc(&c[4]), crate::model::esc(&c[3])));
        }
        last = m.end();
    }
    out.push_str(&crate::model::esc(&s[last..]));
    out
}

// ------------------------------------------------------------------ check

/// Problems with the catalog, for `gw check`.
pub fn check(site_root: &Path, static_dir: &Path) -> Vec<String> {
    let c = match Catalog::load(site_root) {
        Ok(Some(c)) => c,
        Ok(None) => return vec![],
        Err(e) => return vec![format!("{FILE}: {e:#}")],
    };
    let mut out = vec![];
    let cats: BTreeSet<&str> = c.categories.iter().map(|x| x.id.as_str()).collect();
    for p in &c.projects {
        if !p.category.is_empty() && !cats.is_empty() && !cats.contains(p.category.as_str()) {
            out.push(format!("{FILE}: {}: category {} is not in categories", p.slug, p.category));
        }
        for r in &p.related {
            if c.get(r).is_none() {
                out.push(format!("{FILE}: {}: related project {r} is not in the catalog", p.slug));
            }
        }
        if !p.logo.is_empty() && !static_dir.join(LOGO_DIR).join(&p.logo).is_file() {
            out.push(format!("{FILE}: {}: logo {LOGO_DIR}/{} is missing (gw code sync)", p.slug, p.logo));
        }
    }
    for f in c.families.iter().filter(|f| !f.logo.is_empty()) {
        if !static_dir.join(LOGO_DIR).join(&f.logo).is_file() {
            out.push(format!("{FILE}: family {}: logo {LOGO_DIR}/{} is missing", f.id, f.logo));
        }
    }
    out
}

// ------------------------------------------------------------------ pages

/// A project as the templates see it.
#[derive(Debug, Clone, Serialize)]
pub struct Card {
    pub slug: String,
    pub repo: String,
    pub url: String,
    pub title: String,
    pub summary: String,
    pub description_html: String,
    /// `/code-logos/<file>`, or empty (then a monogram).
    pub logo: String,
    pub monogram: String,
    /// 0–359, for the monogram's colour.
    pub hue: u32,
    pub category: String,
    pub category_title: String,
    pub tags: Vec<String>,
    pub language: String,
    pub stars: u64,
    pub license: String,
    pub pushed_human: String,
    pub pushed: String,
    pub repo_url: String,
    pub homepage: String,
    pub archived: bool,
    pub featured: bool,
    /// The post that writes it up.
    pub post_url: String,
    pub post_title: String,
    pub related: Vec<Mini>,
    /// What the filter matches.
    pub search_text: String,
    /// Absolute logo URL (Open Graph).
    pub og_image: String,
}

/// A related project's small tile.
#[derive(Debug, Clone, Serialize)]
pub struct Mini {
    pub url: String,
    pub title: String,
    pub summary: String,
    pub logo: String,
    pub monogram: String,
    pub hue: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Group {
    pub id: String,
    pub title: String,
    pub description: String,
    pub cards: Vec<Card>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TagCount {
    pub tag: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Pages {
    pub title: String,
    pub intro: String,
    pub groups: Vec<Group>,
    pub tags: Vec<TagCount>,
    pub count: usize,
}

/// A post that may write a project up: its id, URL, title and `:repo:`.
pub struct PostRef<'a> {
    pub id: &'a str,
    pub url: &'a str,
    pub title: &'a str,
    pub repo: Option<&'a str>,
}

fn monogram(title: &str) -> String {
    let words: Vec<&str> = title.split_whitespace().filter(|w| w.chars().next().is_some_and(|c| c.is_alphanumeric())).collect();
    match words.len() {
        0 => "?".into(),
        1 => {
            let mut c = words[0].chars();
            c.next().map(|f| f.to_uppercase().chain(c.take(1).flat_map(|x| x.to_lowercase())).collect()).unwrap_or_default()
        }
        _ => words.iter().take(2).filter_map(|w| w.chars().next()).flat_map(|c| c.to_uppercase()).collect(),
    }
}

fn hue(s: &str) -> u32 {
    s.bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619)) % 360
}

fn human_date(iso: &str) -> String {
    chrono::NaiveDate::parse_from_str(iso, "%Y-%m-%d").map(|d| d.format("%B %Y").to_string()).unwrap_or_default()
}

fn norm_repo_url(u: &str) -> String {
    u.trim().trim_end_matches('/').trim_end_matches(".git").to_ascii_lowercase().replace("http://", "https://").replace("://www.", "://")
}

/// Build the catalog's pages: groups by category, tags, and every listed project's card.
pub fn pages(c: &Catalog, static_dir: &Path, posts: &[PostRef], base_url: &str, warnings: &mut Vec<String>) -> Pages {
    let base = base_url.trim_end_matches('/');
    let logo_url = |p: &Project| -> String {
        // A missing file is reported by check(); the tile falls back to a monogram.
        match c.logo_of(p) {
            Some((f, _)) if static_dir.join(LOGO_DIR).join(f).is_file() => format!("/{LOGO_DIR}/{f}"),
            _ => String::new(),
        }
    };
    let mini = |p: &Project, logo: String| Mini { url: format!("{URL}{}/", p.slug), title: p.display_title(), summary: p.display_summary(), logo, monogram: monogram(&p.display_title()), hue: hue(&p.slug) };
    let logos: BTreeMap<String, String> = c.projects.iter().filter(|p| p.listed()).map(|p| (p.slug.clone(), logo_url(p))).collect();
    let mut groups: Vec<Group> = vec![];
    let mut tag_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut count = 0;
    for p in c.ordered() {
        let title = p.display_title();
        let logo = logos.get(&p.slug).cloned().unwrap_or_default();
        let post = if p.post.is_empty() {
            let mine = norm_repo_url(&p.repo_url());
            posts.iter().find(|x| x.repo.is_some_and(|r| norm_repo_url(r) == mine))
        } else {
            let found = posts.iter().find(|x| x.id == p.post);
            if found.is_none() {
                warnings.push(format!("code catalog: {}: post {} is not a published post", p.slug, p.post));
            }
            found
        };
        let tags: Vec<String> = {
            let mut seen = BTreeSet::new();
            p.tags.iter().map(|t| t.trim().to_lowercase()).filter(|t| !t.is_empty() && seen.insert(t.clone())).collect()
        };
        for t in &tags {
            *tag_counts.entry(t.clone()).or_default() += 1;
        }
        let related: Vec<Mini> = c.related_of(&p.slug).into_iter().map(|r| mini(r, logos.get(&r.slug).cloned().unwrap_or_default())).collect();
        let summary = p.display_summary();
        let card = Card {
            url: format!("{URL}{}/", p.slug),
            monogram: monogram(&title),
            hue: hue(&p.slug),
            category_title: c.category_title(&p.category),
            category: if p.category.is_empty() { "other".into() } else { p.category.clone() },
            search_text: format!("{title} {} {summary} {} {} {}", p.name(), tags.join(" "), p.github.language, p.github.topics.join(" ")).to_lowercase(),
            og_image: if logo.is_empty() { String::new() } else { format!("{base}{logo}") },
            description_html: md_html(&p.description),
            language: p.github.language.clone(),
            stars: p.github.stars,
            license: p.github.license.clone(),
            pushed_human: human_date(&p.github.pushed),
            pushed: p.github.pushed.clone(),
            repo_url: p.repo_url(),
            homepage: p.github.homepage.clone(),
            archived: p.github.archived,
            featured: p.featured,
            post_url: post.map(|x| x.url.to_string()).unwrap_or_default(),
            post_title: post.map(|x| x.title.to_string()).unwrap_or_default(),
            slug: p.slug.clone(),
            repo: p.repo.clone(),
            title,
            summary,
            logo,
            tags,
            related,
        };
        count += 1;
        match groups.iter_mut().find(|g| g.id == card.category) {
            Some(g) => g.cards.push(card),
            None => {
                let info = c.categories.iter().find(|x| x.id == card.category);
                groups.push(Group {
                    id: card.category.clone(),
                    title: card.category_title.clone(),
                    description: info.map(|x| x.description.clone()).unwrap_or_default(),
                    cards: vec![card],
                })
            }
        }
    }
    let mut tags: Vec<TagCount> = tag_counts.into_iter().map(|(tag, count)| TagCount { tag, count }).collect();
    tags.sort_by(|a, b| b.count.cmp(&a.count).then(a.tag.cmp(&b.tag)));
    Pages {
        title: if c.title.is_empty() { DEFAULT_TITLE.into() } else { c.title.clone() },
        intro: if c.intro.is_empty() { DEFAULT_INTRO.into() } else { c.intro.clone() },
        groups,
        tags,
        count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!("gw-codecat-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const SAMPLE: &str = "title: Code\nowner: me\ncategories:\n  - { id: retro, title: Retro computing }\n  - { id: tools, title: Tools }\nprojects:\n  - slug: amiga-mcp\n    repo: me/amiga_mcp\n    title: Amiga DevBench\n    summary: Debug Amigas from Claude.\n    category: retro\n    tags: [Amiga, mcp]\n    related: [amiga-games]\n    logo: amiga-mcp.png\n    github: { language: C, stars: 12, pushed: 2026-10-01, topics: [amiga] }\n  - slug: amiga-games\n    repo: me/amiga_games\n    category: retro\n    tags: [amiga]\n    github: { description: Games for the Amiga. }\n  - slug: gw\n    repo: me/gitwebsite\n    category: tools\n    featured: true\n  - slug: secret\n    repo: me/secret\n    hidden: true\n    related: [amiga-mcp]\n";

    #[test]
    fn parses_round_trips_and_validates() {
        let c = Catalog::parse(SAMPLE).unwrap();
        assert_eq!(c.projects.len(), 4);
        assert_eq!(Catalog::parse(&c.to_yaml().unwrap()).unwrap(), c);
        let y = c.to_yaml().unwrap();
        assert!(!y.contains("hidden: false") && !y.contains("stars: 0") && !y.contains("gone"));
        assert!(Catalog::parse("projects:\n  - { slug: a, repo: me/a }\n  - { slug: a, repo: me/b }\n").unwrap_err().to_string().contains("twice"));
        assert!(Catalog::parse("projects:\n  - { slug: a, repo: me/a }\n  - { slug: b, repo: ME/A }\n").is_err());
        for bad in ["../x", "A", "a b", "", "-x"] {
            assert!(Catalog::parse(&format!("projects:\n  - {{ slug: {bad:?}, repo: me/a }}\n")).is_err(), "{bad}");
        }
        for bad in ["me", "me/../x", "/x", "me/a b", "me/.git"] {
            assert!(Catalog::parse(&format!("projects:\n  - {{ slug: a, repo: {bad:?} }}\n")).is_err(), "{bad}");
        }
        assert!(Catalog::parse("projects:\n  - { slug: a, repo: me/a, logo: ../../etc/passwd }\n").is_err());
        assert!(Catalog::parse("projects:\n  - { slug: a, repo: me/a, logo: a.exe }\n").is_err());
        assert_eq!(Catalog::parse("").unwrap(), Catalog::default());
        assert!(Catalog::load(&tmp("none")).unwrap().is_none());
    }

    #[test]
    fn names_and_slugs() {
        assert_eq!(slug_for("amiga_mcp"), "amiga-mcp");
        assert_eq!(slug_for("3DO.Dev"), "3do-dev");
        assert_eq!(slug_for("--x__"), "x");
        assert_eq!(title_for("amiga_mcp"), "Amiga MCP");
        assert_eq!(title_for("code-graph-search"), "Code Graph Search");
        assert_eq!(title_for("GitWebsite"), "GitWebsite");
        let c = Catalog::parse(SAMPLE).unwrap();
        assert_eq!(c.new_slug("amiga_mcp"), "amiga-mcp-2");
        assert_eq!(c.new_slug("Fresh"), "fresh");
        assert_eq!(monogram("Amiga DevBench"), "AD");
        assert_eq!(monogram("gitwebsite"), "Gi");
    }

    #[test]
    fn order_and_related_both_ways() {
        let c = Catalog::parse(SAMPLE).unwrap();
        let o: Vec<&str> = c.ordered().iter().map(|p| p.slug.as_str()).collect();
        assert_eq!(o, ["amiga-mcp", "amiga-games", "gw"], "by category, then title (Amiga DevBench, Amiga Games); hidden left out");
        let r: Vec<&str> = c.related_of("amiga-games").iter().map(|p| p.slug.as_str()).collect();
        assert_eq!(r, ["amiga-mcp"], "related shows both ways");
        let r: Vec<&str> = c.related_of("amiga-mcp").iter().map(|p| p.slug.as_str()).collect();
        assert_eq!(r, ["amiga-games"], "hidden projects are not shown as related");
    }

    #[test]
    fn logo_prompts_and_batches() {
        let mut c = Catalog::parse(SAMPLE).unwrap();
        let p = logo_prompt(&c, c.get("amiga-games").unwrap());
        assert!(p.contains("\"Amiga Games\"") && p.contains("Games for the Amiga.") && p.contains("themes: amiga") && p.contains("no text"));
        let b = logo_batches(&c, 1);
        assert_eq!(b.len(), 2, "amiga-mcp has a logo; secret is hidden");
        assert_eq!(b[0][0].file, "amiga-games.png");
        c.logo_style = "House style.".into();
        c.get_mut("gw").unwrap().logo_prompt = "My own prompt".into();
        let b = logo_batches(&c, 10);
        assert_eq!(b.len(), 1);
        assert!(b[0][0].prompt.ends_with("House style."));
        assert_eq!(b[0][1].prompt, "My own prompt");
        let t = batch_text(&b[0], 1, 1);
        assert!(t.contains("Logo batch 1 of 1: 2 logos") && t.contains("1. File name: amiga-games.png") && t.contains("2. File name: gw.png"));
    }

    #[test]
    fn logo_families_share_one_mark() {
        let mut c = Catalog::parse(&format!("{SAMPLE}families:\n  - {{ id: amiga, title: Amiga tools, concept: a bouncing ball }}\n")).unwrap();
        for s in ["amiga-mcp", "amiga-games"] {
            c.get_mut(s).unwrap().family = "amiga".into();
        }
        c.validate().unwrap();
        let items: Vec<PromptItem> = logo_batches(&c, 50).concat();
        assert_eq!(items[0].file, "family-amiga.png");
        assert!(items[0].prompt.contains("family of 2 related") && items[0].prompt.contains("The mark: a bouncing ball."));
        assert!(!items.iter().any(|i| i.slug == "amiga-games"), "members wait for the family mark");
        assert_eq!(c.logo_of(c.get("amiga-games").unwrap()), None);
        c.families[0].logo = "family-amiga.png".into();
        assert_eq!(c.logo_of(c.get("amiga-games").unwrap()), Some(("family-amiga.png", true)));
        assert_eq!(c.logo_of(c.get("amiga-mcp").unwrap()), Some(("amiga-mcp.png", false)), "its own logo wins");
        assert!(!logo_batches(&c, 50).concat().iter().any(|i| i.slug.starts_with("family-")));
        assert_eq!(match_family_file(&c, "Family_Amiga (1).png").map(|f| f.id.as_str()), Some("amiga"));
        assert!(match_family_file(&c, "amiga-games.png").is_none());
        let mut bad = c.clone();
        bad.get_mut("gw").unwrap().family = "nope".into();
        assert!(bad.validate().is_err());
    }

    #[test]
    fn matching_rendered_files() {
        let c = Catalog::parse(SAMPLE).unwrap();
        for f in ["amiga-games.png", "amiga-games (1).png", "Amiga_Games-logo.webp", "amiga-games-2.jpg", "AMIGA_GAMES.PNG"] {
            assert_eq!(match_logo_file(&c, f).map(|p| p.slug.as_str()), Some("amiga-games"), "{f}");
        }
        assert_eq!(match_logo_file(&c, "gitwebsite.png").map(|p| p.slug.as_str()), Some("gw"), "by repository name");
        assert!(match_logo_file(&c, "amiga-games.txt").is_none());
        assert!(match_logo_file(&c, "nothing.png").is_none());
    }

    #[test]
    fn readme_logo_goes_on_top_once() {
        let r = readme_with_logo("# Amiga Games\n\nText.\n", "assets/logo.png", "Amiga \"Games\" logo");
        assert!(r.starts_with("<p align=\"center\">\n  <img src=\"assets/logo.png\" alt=\"Amiga &quot;Games&quot; logo\" width=\"180\">\n</p>\n\n# Amiga Games\n"));
        assert_eq!(readme_with_logo(&r, "assets/logo.png", "x"), r);
        let fm = readme_with_logo("---\ntitle: x\n---\n# T\n", "assets/logo.png", "x");
        assert!(fm.starts_with("---\ntitle: x\n---\n<p align"));
        let mut p = Project::default();
        assert_eq!(repo_logo_path(&p, "png"), "assets/logo.png");
        p.github.logo_path = "docs/img/logo.png".into();
        assert_eq!(repo_logo_path(&p, "png"), "docs/img/logo.png");
        assert_eq!(repo_logo_path(&p, "webp"), "assets/logo.webp");
    }

    #[test]
    fn logos_are_normalised() {
        let img = image::RgbaImage::from_pixel(1200, 600, image::Rgba([200, 30, 30, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let (out, ext) = normalise_logo(&png.into_inner(), 256).unwrap();
        assert_eq!(ext, "png");
        let back = image::load_from_memory(&out).unwrap();
        assert_eq!((back.width(), back.height()), (256, 128));
        let small = image::RgbaImage::from_pixel(40, 40, image::Rgba([0, 0, 0, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(small).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let (out, _) = normalise_logo(&png.into_inner(), 256).unwrap();
        assert_eq!(image::load_from_memory(&out).unwrap().width(), 40, "never enlarged");
        assert_eq!(normalise_logo(b"<svg xmlns='http://www.w3.org/2000/svg'><circle r='4'/></svg>", 256).unwrap().1, "svg");
        assert!(normalise_logo(b"<svg><script>alert(1)</script></svg>", 256).is_err());
        assert!(normalise_logo(b"<svg><a onclick = 'x'/></svg>", 256).is_err());
        assert!(normalise_logo(b"not an image", 256).is_err());
        // A PNG whose metadata mentions <svg is still a PNG.
        let mut tagged = image::RgbaImage::from_pixel(8, 8, image::Rgba([0, 0, 0, 255]));
        tagged.put_pixel(0, 0, image::Rgba([1, 1, 1, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(tagged).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let mut bytes = png.into_inner();
        bytes.splice(33..33, b"\0\0\0\x10tEXtc2pa\0<svg></svg>....".iter().cloned());
        assert_eq!(normalise_logo(&bytes, 256).map(|x| x.1).unwrap_or("png"), "png");
        let big = format!("<svg>{}</svg>", "x".repeat(400 * 1024));
        assert!(normalise_logo(big.as_bytes(), 256).is_err());
    }

    #[test]
    fn markdown_subset_is_safe() {
        let h = md_html("Hello `x<y>` and **bold** [site](https://a.b/c?d=1) [bad](javascript:alert(1)).\n\n- one\n- two <b>\n\n## Why");
        assert!(h.contains("<code>x&lt;y&gt;</code>") && h.contains("<strong>bold</strong>") && h.contains("<a href=\"https://a.b/c?d=1\">site</a>"));
        assert!(h.contains("[bad](javascript:alert(1))") && !h.contains("href=\"javascript"));
        assert!(h.contains("<ul><li>one</li><li>two &lt;b&gt;</li></ul>") && h.contains("<h3>Why</h3>"));
    }

    #[test]
    fn build_writes_the_catalog() {
        let root = tmp("site");
        std::fs::create_dir_all(root.join("content/projects")).unwrap();
        std::fs::create_dir_all(root.join("static").join(LOGO_DIR)).unwrap();
        std::fs::write(root.join("static").join(LOGO_DIR).join("amiga-mcp.png"), b"png").unwrap();
        std::fs::write(root.join("site.yaml"), "site:\n  title: T\n  base_url: https://example.org\n  author: A\n").unwrap();
        std::fs::write(root.join("content/projects/devbench.adoc"), "= DevBench write-up\n:type: project\n:date: 2026-01-01\n:status: published\n:repo: https://github.com/me/amiga_mcp\n\nText.\n").unwrap();
        std::fs::write(root.join(FILE), SAMPLE).unwrap();
        let site = crate::Site::open(Some(&root)).unwrap();
        assert!(site.check().unwrap().is_empty(), "{:?}", site.check().unwrap());
        let out = root.join("dist");
        crate::build::build(&site, &crate::build::BuildOptions { drafts: false, out_dir: Some(out.clone()) }).unwrap();
        let read = |p: &str| std::fs::read_to_string(out.join(p)).unwrap_or_else(|_| panic!("{p} missing"));
        let index = read("code/index.html");
        assert!(index.contains("Retro computing") && index.contains("Tools") && index.contains("data-code-filter"));
        assert!(index.contains("/code/amiga-mcp/") && index.contains("/code-logos/amiga-mcp.png") && index.contains("/css/code.css"));
        assert!(index.find("/code/amiga-mcp/").unwrap() < index.find("/code/gw/").unwrap(), "categories in order");
        assert!(!index.contains("/code/secret/"), "hidden projects are left out");
        assert!(index.contains("data-tag=\"amiga\""), "tags are filters");
        let mcp = read("code/amiga-mcp/index.html");
        assert!(mcp.contains("Amiga DevBench") && mcp.contains("Debug Amigas from Claude.") && mcp.contains("https://github.com/me/amiga_mcp"));
        assert!(mcp.contains("/projects/devbench/") && mcp.contains("DevBench write-up"), "the write-up is found by :repo:");
        assert!(mcp.contains("og:image\" content=\"https://example.org/code-logos/amiga-mcp.png"));
        assert!(mcp.contains("/code/amiga-games/"), "related");
        let games = read("code/amiga-games/index.html");
        assert!(games.contains("/code/amiga-mcp/"), "related both ways");
        assert!(games.contains("code-mono"), "a monogram when there is no logo");
        assert!(!out.join("code/secret/index.html").exists());
        assert!(out.join("code-logos/amiga-mcp.png").is_file());
        let sm = read("sitemap.xml");
        assert!(sm.contains("https://example.org/code/</loc>") && sm.contains("/code/gw/</loc>") && !sm.contains("/code/secret/"));
        // A missing logo and an unknown category are reported.
        std::fs::remove_file(root.join("static").join(LOGO_DIR).join("amiga-mcp.png")).unwrap();
        std::fs::write(root.join(FILE), SAMPLE.replace("category: tools", "category: nope")).unwrap();
        let problems = site.check().unwrap();
        assert!(problems.iter().any(|p| p.contains("amiga-mcp") && p.contains("missing")), "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("nope")), "{problems:?}");
        // No code.yaml → no catalog.
        std::fs::remove_file(root.join(FILE)).unwrap();
        let _ = std::fs::remove_dir_all(&out);
        crate::build::build(&site, &crate::build::BuildOptions { drafts: false, out_dir: Some(out.clone()) }).unwrap();
        assert!(!out.join("code/index.html").exists());
    }
}
