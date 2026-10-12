//! `gw init`: scaffold a new site — `site.yaml`, the empty stores, the default theme (copied so
//! it is easy to change), starter content, and a `.gitignore`.

use crate::theme;
use crate::userconfig::config_dir;
use anyhow::{bail, Result};
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InitOptions {
    pub title: String,
    pub author: String,
    /// Where the site is served, e.g. `https://octocat.github.io`.
    pub base_url: String,
    #[serde(default)]
    pub tagline: String,
    #[serde(default)]
    pub description: String,
    /// GitHub Pages repository as `owner/name` (optional; can be set later in site.yaml).
    #[serde(default)]
    pub pages_remote: String,
    #[serde(default)]
    pub branch: String,
    /// Local checkout of the Pages repository; default: `<config dir>/pages/<name>`.
    #[serde(default)]
    pub pages_repo: String,
    /// Copy the default theme's templates and assets into the site (they work from the
    /// engine without copying; copies are for changing them).
    #[serde(default = "yes")]
    pub copy_theme: bool,
    /// Leave out the starter pages and posts.
    #[serde(default)]
    pub no_starter: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Serialize)]
pub struct InitReport {
    pub root: PathBuf,
    pub files: Vec<String>,
}

fn yaml_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// `https://octocat.github.io` from `octocat/octocat.github.io`, for a default base URL.
pub fn pages_url(remote: &str) -> Option<String> {
    let (owner, name) = remote.split_once('/')?;
    if name.eq_ignore_ascii_case(&format!("{owner}.github.io")) {
        Some(format!("https://{}", name.to_lowercase()))
    } else {
        Some(format!("https://{}.github.io/{name}", owner.to_lowercase()))
    }
}

fn site_yaml(o: &InitOptions) -> String {
    let pages_repo = if !o.pages_repo.is_empty() {
        o.pages_repo.clone()
    } else if let Some((_, name)) = o.pages_remote.split_once('/') {
        config_dir().join("pages").join(name).display().to_string()
    } else {
        String::new()
    };
    let branch = if o.branch.is_empty() { "main" } else { &o.branch };
    format!(
        r#"# Site settings: identity, navigation, information architecture, build and publish.
# Areas form a tree; content lists them with :areas: (a full path like notes/reading, or a
# unique id like reading). An item can sit in several areas.

site:
  title: {title}
  tagline: {tagline}
  base_url: {base_url}
  author: {author}
  description: {description}
  links: []
  #  - {{ label: GitHub, href: "https://github.com/<you>" }}

nav:
  - {{ label: Articles, href: /articles/ }}
  - {{ label: Projects, href: /projects/ }}
  - {{ label: Areas, href: /areas/ }}
  - {{ label: About, href: /about/ }}

areas:
  - id: work
    title: Work
    description: Things I build and how they work.
    children:
      - {{ id: software, title: Software, description: "Programs, tools, and libraries." }}
  - id: notes
    title: Notes
    description: Things I learn along the way.
    children:
      - {{ id: reading, title: Reading, description: "Books and papers worth sharing." }}

# Listing pages. /projects/ is grouped into sections by each project's first area's top-level
# area (in the order of the areas above); projects without areas go under "Other".
articles: {{}}
#  intro: Articles and project updates, newest first.
#  group_by_area: false
projects: {{}}
#  intro: Things I have built.
#  group_by_area: true
#  group_labels: {{ work: Software }}
#  other_label: Other

build:
  out_dir: dist
  ruby: ruby
  # Folders under static/ that only preview builds include.
  preview_only: []

publish:
  # A local checkout of the GitHub Pages repository (cloned from `remote` if missing).
  pages_repo: {pages_repo}
  remote: {remote}
  branch: {branch}
  # Top-level paths in the Pages repository that gitwebsite must never touch.
  preserve:
    - README.md

# AsciiDoc kept in other repositories, pulled in at build time.
sources: []
#  - repo: ~/code/my-project
#    include: docs/*.adoc
#    label: my-project

tools:
  categories: []
"#,
        title = yaml_str(&o.title),
        tagline = yaml_str(&o.tagline),
        base_url = yaml_str(o.base_url.trim_end_matches('/')),
        author = yaml_str(&o.author),
        description = yaml_str(&o.description),
        pages_repo = yaml_str(&pages_repo),
        remote = yaml_str(&o.pages_remote),
        branch = yaml_str(branch),
    )
}

fn starter(o: &InitOptions) -> Vec<(String, String)> {
    let today = Local::now().format("%Y-%m-%d");
    let who = if o.author.is_empty() { "me" } else { o.author.as_str() };
    vec![
        (
            "content/pages/home.adoc".into(),
            format!(
                "= Home\n:type: page\n:slug: home\n\n{}This is the home page. Edit `content/pages/home.adoc` to say who you are and what you\nwrite about; the latest articles and featured projects are listed below it.\n",
                // The theme already shows the tagline above this text; only greet when there is none.
                if o.tagline.is_empty() { format!("Welcome to {}.\n\n", o.title) } else { String::new() }
            ),
        ),
        (
            "content/pages/about.adoc".into(),
            format!("= About\n:type: page\n:summary: Who writes this site.\n:status: published\n\nHi, I'm {who}. Say a little about yourself here (`content/pages/about.adoc`).\n"),
        ),
        (
            "content/articles/hello-world/index.adoc".into(),
            format!(
                "= Hello, world\n:type: article\n:date: {today}\n:areas: notes\n:tags: meta\n:summary: The first post: how this site is made.\n:status: published\n\nThis site is written in AsciiDoc and built with gitwebsite. Each article is a folder under\n`content/articles/` (images can sit beside the text) or a single `.adoc` file.\n\n== Writing\n\n* `gw new article \"My next post\" --area notes` starts a draft.\n* `gw serve --open` shows a live preview and the management app.\n* `gw publish` builds the site and pushes it to GitHub Pages.\n"
            ),
        ),
        (
            "content/projects/example-project/index.adoc".into(),
            format!(
                "= Example project\n:type: project\n:date: {today}\n:areas: work/software\n:tags: example\n:summary: A project page: what it is, why it exists, and how it works.\n:repo: https://github.com/octocat/Hello-World\n:status: published\n\nProject pages describe something you have built. Link its repository with `:repo:` and\nadd dated progress posts under `updates/` with `gw new update \"...\" --project example-project`.\n\nTIP: In the desktop app, *Enhance with Claude* reads the repository and interviews you to\nimprove a page like this one.\n"
            ),
        ),
    ]
}

const GITIGNORE: &str = "# Build output and caches\n/dist/\n/.gw-cache/\n# Story photos can be large; they stay local\n/stories/*/images/\n.DS_Store\n";

/// Create a site in `dir` (which must be empty or not exist).
pub fn init(dir: &Path, o: &InitOptions) -> Result<InitReport> {
    if o.title.trim().is_empty() {
        bail!("a site needs a title");
    }
    if o.author.trim().is_empty() {
        bail!("a site needs an author");
    }
    let mut o = o.clone();
    if o.base_url.trim().is_empty() {
        o.base_url = pages_url(&o.pages_remote).unwrap_or_default();
    }
    if !o.base_url.starts_with("http://") && !o.base_url.starts_with("https://") {
        bail!("base_url must be a full URL such as https://octocat.github.io (or give a Pages repo to derive it)");
    }
    if dir.join("site.yaml").exists() {
        bail!("{} already has a site.yaml", dir.display());
    }
    if dir.exists() && std::fs::read_dir(dir)?.flatten().any(|e| !e.file_name().to_string_lossy().starts_with('.')) {
        bail!("{} is not empty; choose a new or empty folder", dir.display());
    }
    std::fs::create_dir_all(dir)?;
    let mut files = vec![];
    let mut put = |rel: &str, body: &[u8]| -> Result<()> {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap())?;
        std::fs::write(&p, body)?;
        files.push(rel.to_string());
        Ok(())
    };
    put("site.yaml", site_yaml(&o).as_bytes())?;
    put("registry.yaml", b"# Provenance registry: where content came from (repos, publications, imports).\n# Managed by `gw registry ...`; hand edits are fine.\nentries: []\n")?;
    put("ideas.yaml", b"# Ideas and work in progress for the site. Managed by `gw idea ...`.\nideas: []\n")?;
    put("media.yaml", b"# Media library: content refers to these as media:<key>. Managed by `gw media ...`.\nmedia: {}\n")?;
    put(".gitignore", GITIGNORE.as_bytes())?;
    if o.copy_theme {
        for (name, text) in theme::TEMPLATES {
            put(&format!("templates/{name}"), text.as_bytes())?;
        }
        for (rel, bytes) in theme::STATIC {
            put(&format!("static/{rel}"), bytes)?;
        }
    }
    if !o.no_starter {
        for (rel, body) in starter(&o) {
            put(&rel, body.as_bytes())?;
        }
    }
    std::fs::create_dir_all(dir.join("content/articles"))?;
    std::fs::create_dir_all(dir.join("content/projects"))?;
    Ok(InitReport { root: dir.to_path_buf(), files })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::{build, BuildOptions};
    use crate::Site;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("gw-init-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn new_site_checks_and_builds_cleanly() {
        for copy_theme in [true, false] {
            let d = tmp(if copy_theme { "copy" } else { "builtin" });
            let o = InitOptions {
                title: "Octo's notes".into(),
                author: "Mona Octocat".into(),
                pages_remote: "octocat/octocat.github.io".into(),
                copy_theme,
                ..Default::default()
            };
            let r = init(&d, &o).unwrap();
            assert!(r.files.contains(&"site.yaml".to_string()));
            let site = Site::open(Some(&d)).unwrap();
            assert_eq!(site.config.site.base_url, "https://octocat.github.io");
            assert_eq!(site.config.publish.remote, "octocat/octocat.github.io");
            let problems = site.check().unwrap();
            assert!(problems.is_empty(), "{problems:?}");
            let rep = build(&site, &BuildOptions { drafts: false, out_dir: Some(d.join("dist")) }).unwrap();
            assert!(rep.warnings.is_empty(), "{:?}", rep.warnings);
            for f in ["index.html", "about/index.html", "articles/hello-world/index.html", "projects/example-project/index.html", "css/site.css", "images/favicon.svg", "feed.xml", "sitemap.xml"] {
                assert!(d.join("dist").join(f).exists(), "missing {f}");
            }
            let home = std::fs::read_to_string(d.join("dist/index.html")).unwrap();
            assert!(home.contains("Octo&#39;s notes") || home.contains("Octo's notes"));
            // Refuses to overwrite.
            assert!(init(&d, &o).is_err());
            let _ = std::fs::remove_dir_all(&d);
        }
    }

    #[test]
    fn projects_grouped_by_primary_area() {
        let d = tmp("groups");
        let o = InitOptions { title: "T".into(), author: "A".into(), base_url: "https://x.test".into(), copy_theme: false, no_starter: true, ..Default::default() };
        init(&d, &o).unwrap();
        // Rename the "work" group, as a site would in site.yaml.
        let y = d.join("site.yaml");
        let text = std::fs::read_to_string(&y).unwrap().replace("projects: {}", "projects:\n  group_labels: { work: Software }");
        std::fs::write(&y, text).unwrap();
        let proj = |slug: &str, areas: &str| {
            let p = d.join(format!("content/projects/{slug}.adoc"));
            let a = if areas.is_empty() { String::new() } else { format!(":areas: {areas}\n") };
            std::fs::write(p, format!("= {slug}\n:type: project\n:date: 2026-01-01\n{a}:status: published\n\nText.\n")).unwrap();
        };
        proj("tool", "software, reading");
        proj("diary", "notes/reading");
        proj("loose", "");
        let site = Site::open(Some(&d)).unwrap();
        assert!(site.check().unwrap().is_empty());
        build(&site, &BuildOptions { drafts: false, out_dir: Some(d.join("dist")) }).unwrap();
        let h = std::fs::read_to_string(d.join("dist/projects/index.html")).unwrap();
        let sw = h.find(">Software</h2>").expect("work group, relabelled");
        let notes = h.find(">Notes</h2>").expect("notes group");
        let other = h.find(">Other</h2>").expect("no-area group last");
        assert!(sw < notes && notes < other, "groups follow the area order, Other last");
        // Each project once; its other areas as chips; filter data has ancestors.
        assert_eq!(h.matches(">tool</a></h3>").count(), 1);
        assert!(h.contains(r#"data-areas="work work/software notes notes/reading""#));
        let tool_card = &h[h.find(">tool</a></h3>").unwrap()..];
        let tool_card = &tool_card[..tool_card.find("</article>").unwrap()];
        assert!(tool_card.contains(">Reading</a>") && !tool_card.contains(">Software</a>"));
        assert!(h.contains(r#"data-area="notes/reading""#), "sub-area filter button");
        // Grouping off: a plain grid.
        let text = std::fs::read_to_string(&y).unwrap().replace("group_labels: { work: Software }", "group_by_area: false");
        std::fs::write(&y, text).unwrap();
        build(&Site::open(Some(&d)).unwrap(), &BuildOptions { drafts: false, out_dir: Some(d.join("dist")) }).unwrap();
        let h = std::fs::read_to_string(d.join("dist/projects/index.html")).unwrap();
        assert!(!h.contains("data-area-filter") && h.contains(">tool</a></h3>"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn pages_urls() {
        assert_eq!(pages_url("octocat/octocat.github.io").as_deref(), Some("https://octocat.github.io"));
        assert_eq!(pages_url("octocat/blog").as_deref(), Some("https://octocat.github.io/blog"));
    }
}
