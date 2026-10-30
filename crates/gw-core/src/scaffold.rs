//! Create new content from templates.

use crate::content::{slugify, Kind};
use crate::site::Site;
use anyhow::{bail, Result};
use chrono::Local;
use std::path::PathBuf;

pub struct NewItem<'a> {
    pub kind: Kind,
    pub title: &'a str,
    pub areas: Vec<String>,
    pub tags: Vec<String>,
    pub project: Option<&'a str>,
    pub summary: &'a str,
    pub repo: Option<&'a str>,
    pub url: Option<&'a str>,
    pub body: Option<&'a str>,
    /// Make a bundle directory (`slug/index.adoc`) so images can sit beside the text.
    pub bundle: bool,
}

pub fn path_for(site: &Site, n: &NewItem) -> Result<PathBuf> {
    let slug = slugify(n.title);
    if slug.is_empty() {
        bail!("title has no usable characters for a slug");
    }
    let c = site.content_dir();
    let base = match n.kind {
        Kind::Page => c.join("pages"),
        Kind::Article => c.join("articles"),
        Kind::Project => c.join("projects"),
        Kind::Update => {
            let p = n.project.ok_or_else(|| anyhow::anyhow!("updates need --project"))?;
            if !c.join("projects").join(p).exists() {
                bail!("no project {p} (content/projects/{p}/)");
            }
            c.join("projects").join(p).join("updates")
        }
        Kind::App => c.join("lab/apps"),
        Kind::Playground => c.join("lab/playground"),
        Kind::Tool => c.join("tools"),
        Kind::Exploration => c.join("lab/explorations"),
    };
    Ok(if n.bundle || n.kind == Kind::Project {
        base.join(&slug).join("index.adoc")
    } else {
        base.join(format!("{slug}.adoc"))
    })
}

pub fn create(site: &Site, n: &NewItem) -> Result<PathBuf> {
    for a in &n.areas {
        if site.config.resolve_area(a).is_none() {
            bail!("unknown area {a:?}; see `gw areas`");
        }
    }
    let path = path_for(site, n)?;
    if path.exists() {
        bail!("{} already exists", path.display());
    }
    let today = Local::now().date_naive();
    let mut h = format!("= {}\n:type: {}\n", n.title, n.kind.as_str());
    if n.kind != Kind::Page {
        h += &format!(":date: {today}\n");
    }
    h += ":status: draft\n";
    if !n.areas.is_empty() {
        h += &format!(":areas: {}\n", n.areas.join(", "));
    }
    if !n.tags.is_empty() {
        h += &format!(":tags: {}\n", n.tags.join(", "));
    }
    if let Some(p) = n.project {
        h += &format!(":project: {p}\n");
    }
    h += &format!(":summary: {}\n", n.summary);
    if let Some(r) = n.repo {
        h += &format!(":repo: {r}\n");
    }
    if let Some(u) = n.url {
        h += &format!(":url: {u}\n");
    }
    let body = n.body.map(|b| b.to_string()).unwrap_or_else(|| match n.kind {
        Kind::Project => "What it is and why I built it.\n\n== What it does\n\n== How it is built\n\n== Status and next steps\n".into(),
        Kind::Update => "What changed, what I learned, and what is next.\n".into(),
        Kind::App => "What the app does and how to use it.\n\n== How it works\n".into(),
        Kind::Playground => "What this playground shows.\n\n[source.playground,javascript]\n----\nconsole.log('Edit me and press Run');\n----\n".into(),
        Kind::Tool => "How to use the tool. The tool itself is static/tools/<slug>.js; set :tool: and :category:.\n".into(),
        Kind::Exploration => "The question this exploration asks, and what to watch for.\n\n\
// A live tool view: tool::<slug>[query=\"example=…\", tab=…, height=520, caption=\"…\"]\n\
// Runnable code: [source.playground,python] on a ---- block.\n\n\
== Try it\n\n== What happened, and why\n".into(),
        _ => "Start writing here.\n".into(),
    });
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, format!("{h}\n{body}"))?;
    Ok(path)
}

/// Flip `:status:` to published (and stamp `:date:` with today if it is missing).
pub fn set_status(path: &std::path::Path, status: &str) -> Result<()> {
    if !matches!(status, "draft" | "published") {
        bail!("status must be draft or published");
    }
    let text = std::fs::read_to_string(path)?;
    let mut out = vec![];
    let mut done = false;
    let mut in_header = true;
    for (i, line) in text.lines().enumerate() {
        if in_header && i > 0 && line.trim().is_empty() {
            if !done {
                out.push(format!(":status: {status}"));
                done = true;
            }
            in_header = false;
        }
        if in_header && line.starts_with(":status:") {
            out.push(format!(":status: {status}"));
            done = true;
            continue;
        }
        out.push(line.to_string());
    }
    std::fs::write(path, out.join("\n") + "\n")?;
    Ok(())
}
