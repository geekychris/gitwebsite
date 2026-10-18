//! The static build: content + templates + stores → `dist/`.
//!
//! Output pages:
//! - one page per item at its URL (template by kind: `home`, `page`, `article`, `project`,
//!   `app`, `playground`)
//! - `/articles/`, `/projects/`, `/areas/` and `/areas/<path>/`, `/tags/<tag>/`, `/lab/`
//! - `/publications/` from registry entries of kind `publication`
//! - `/models/<project>/<slug>/`: the 3-D models gallery's model pages, and `/models/`: its grid,
//!   or — when a post has a `models::gallery[]` block — a redirect to that post (see [`crate::gallery`])
//! - `feed.xml` (Atom), `sitemap.xml`, `search.json`, `robots.txt`, `404.html`
//!
//! Drafts are left out unless `drafts` is set (for local preview). Unlisted items are built
//! but carry `noindex` and are kept out of every listing, feed, and the sitemap.

use crate::content::{Item, Kind};
use crate::media::Library;
use crate::registry::Entry;
use crate::render::{Doc, Renderer};
use crate::site::Site;
use anyhow::{Context, Result};
use chrono::NaiveDate;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use tera::{Context as Ctx, Tera};
use walkdir::WalkDir;

#[derive(Debug, Clone, Default)]
pub struct BuildOptions {
    pub drafts: bool,
    pub out_dir: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
pub struct BuildReport {
    pub out_dir: PathBuf,
    pub pages: usize,
    pub items: usize,
    pub drafts_included: bool,
    pub files: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct AreaLink {
    path: String,
    title: String,
    url: String,
}

/// What templates see for an item.
#[derive(Debug, Clone, Serialize)]
struct View {
    #[serde(flatten)]
    item: Item,
    kind_label: &'static str,
    date_human: String,
    date_iso: String,
    updated_human: String,
    area_links: Vec<AreaLink>,
    hero_url: String,
    /// The hero image already appears in the body, so pages need not repeat it.
    hero_in_body: bool,
    reading_minutes: usize,
    html: String,
    project_title: String,
    project_url: String,
}

/// A card in a grouped listing: the item, its areas outside its own section (as chips), and
/// every area path it is in, ancestors included (what the filter buttons match).
#[derive(Debug, Clone, Serialize)]
struct GroupEntry {
    #[serde(flatten)]
    view: View,
    chips: Vec<AreaLink>,
    filter: String,
}

#[derive(Debug, Clone, Serialize)]
struct Group {
    id: String,
    title: String,
    entries: Vec<GroupEntry>,
}

#[derive(Debug, Clone, Serialize)]
struct FilterButton {
    path: String,
    title: String,
    depth: usize,
}

/// Sections by each entry's primary top-level area (the top of its first area), in the order of
/// the top-level areas in site.yaml; entries without areas go last under `other_label`. Each entry
/// appears once. Also the filter buttons: each top-level group present, then the sub-areas used.
fn group_by_area(cfg: &crate::config::SiteConfig, lc: &crate::config::ListingConfig, entries: &[View]) -> (Vec<Group>, Vec<FilterButton>) {
    let top = |p: &str| p.split('/').next().unwrap_or(p).to_string();
    let flat = cfg.flat_areas();
    let title_of = |p: &str| flat.iter().find(|(fp, _)| fp == p).map(|(_, a)| a.title.clone()).unwrap_or_else(|| p.to_string());
    let label = |id: &str| lc.group_labels.get(id).cloned().unwrap_or_else(|| title_of(id));
    let mut groups: Vec<Group> = cfg.areas.iter().map(|a| Group { id: a.id.clone(), title: label(&a.id), entries: vec![] }).collect();
    let mut other = Group { id: "other".into(), title: lc.other_label.clone().unwrap_or_else(|| "Other".into()), entries: vec![] };
    let mut used: BTreeSet<String> = BTreeSet::new();
    for v in entries {
        let paths: Vec<String> = v.area_links.iter().map(|a| a.path.clone()).collect();
        let mut filter: Vec<String> = vec![];
        for p in &paths {
            let parts: Vec<&str> = p.split('/').collect();
            for i in 1..=parts.len() {
                let anc = parts[..i].join("/");
                if !filter.contains(&anc) {
                    filter.push(anc.clone());
                }
                used.insert(anc);
            }
        }
        let primary = paths.first().map(|p| top(p));
        let chips = v.area_links.iter().filter(|a| Some(top(&a.path)) != primary).cloned().collect();
        let e = GroupEntry { view: v.clone(), chips, filter: filter.join(" ") };
        match primary.and_then(|t| groups.iter_mut().find(|g| g.id == t)) {
            Some(g) => g.entries.push(e),
            None => other.entries.push(e),
        }
    }
    groups.retain(|g| !g.entries.is_empty());
    if !other.entries.is_empty() {
        groups.push(other);
    }
    let mut buttons = vec![];
    for (p, a) in &flat {
        if !used.contains(p) {
            continue;
        }
        let depth = p.matches('/').count();
        let title = if depth == 0 { label(&a.id) } else { a.title.clone() };
        buttons.push(FilterButton { path: p.clone(), title, depth });
    }
    (groups, buttons)
}

#[derive(Debug, Clone, Serialize)]
struct AreaNode {
    path: String,
    id: String,
    title: String,
    description: String,
    url: String,
    count: usize,
    children: Vec<AreaNode>,
}

fn human(d: Option<NaiveDate>) -> String {
    d.map(|d| d.format("%-d %B %Y").to_string()).unwrap_or_default()
}

fn kind_label(k: Kind) -> &'static str {
    match k {
        Kind::Page => "Page",
        Kind::Article => "Article",
        Kind::Project => "Project",
        Kind::Update => "Project update",
        Kind::App => "App",
        Kind::Playground => "Playground",
        Kind::Tool => "Tool",
    }
}

fn hero_url(it: &Item, media: &Library) -> String {
    let Some(h) = &it.hero else { return String::new() };
    if let Some(key) = h.strip_prefix("media:") {
        return media.resolve(key).map(|m| if m.thumb.is_empty() { m.url.clone() } else { m.url.clone() }).unwrap_or_default();
    }
    if h.starts_with("http") || h.starts_with('/') {
        return h.clone();
    }
    format!("{}{}", it.url, h)
}

fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

pub fn build(site: &Site, opts: &BuildOptions) -> Result<BuildReport> {
    let cfg = &site.config;
    let out = opts.out_dir.clone().unwrap_or_else(|| site.out_dir());
    let media = site.media()?;
    let registry = site.registry()?;
    let mut warnings = site.check()?;

    let all = site.items()?;
    let items: Vec<Item> = all.into_iter().filter(|i| opts.drafts || !i.draft).collect();

    // Templates.
    // The built-in theme first; the site's own templates/ replace files of the same name.
    let mut tera = Tera::default();
    tera.add_raw_templates(crate::theme::TEMPLATES.iter().map(|(n, t)| (n.to_string(), t.to_string())).collect::<Vec<_>>())
        .context("loading the built-in templates")?;
    let tdir = site.templates_dir();
    if tdir.is_dir() {
        let mut own = vec![];
        for e in WalkDir::new(&tdir).min_depth(1) {
            let e = e?;
            let rel = e.path().strip_prefix(&tdir)?.to_string_lossy().replace('\\', "/");
            if e.file_type().is_file() && !rel.split('/').any(|c| c.starts_with('.')) {
                own.push((e.path().to_path_buf(), Some(rel)));
            }
        }
        tera.add_template_files(own).with_context(|| format!("loading templates from {}", tdir.display()))?;
    }
    tera.autoescape_on(vec![".html", ".xml"]);
    // Tera's default escaper also rewrites '/' as &#x2F;, which makes every URL unreadable.
    tera.set_escape_fn(escape_html);

    // The 3-D models gallery (models.yaml + static/model-gallery/), shown at /models/ or, when a
    // post has a models::gallery[] block, in that post ("the gallery's home").
    let st_dir = site.static_dir();
    let gallery = crate::gallery::Gallery::load(&site.root)?.filter(|g| !g.models.is_empty());
    // Its problems are already among the warnings (from `site.check()`).
    let mut gpages = gallery.as_ref().map(|g| crate::gallery::pages(g, &st_dir, &media, &cfg.site.base_url, &mut vec![])).filter(|p| !p.cards.is_empty());
    let mut gallery_home: Option<&Item> = None;
    // The media keys of the models each post shows (for "Appears in" on the model pages).
    let mut shows_models: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // Posts showing the gallery: the grid is their picture, so the hero is not repeated above it.
    let mut shows_gallery: BTreeSet<String> = BTreeSet::new();

    // Render every item's body in one Asciidoctor run.
    let mut docs = vec![];
    for it in &items {
        let raw = std::fs::read_to_string(&it.path)?;
        shows_models.insert(it.id.clone(), crate::model::embeds(&raw).into_iter().filter_map(|e| e.key).collect());
        // The models gallery block (a passthrough of models_grid.html).
        let gallery_blocks = crate::gallery::embeds(&raw);
        let raw = if gallery_blocks.is_empty() {
            raw
        } else {
            shows_gallery.insert(it.id.clone());
            if gallery_home.is_none() && gallery_blocks.iter().any(|e| matches!(e, Ok(e) if e.project.is_none())) {
                gallery_home = Some(it);
            }
            let (text, problems) = crate::gallery::expand(&raw, |e| {
                let Some(gp) = &gpages else { anyhow::bail!("no models on the website yet (models.yaml; `gw models on`)") };
                let gp = match &e.project {
                    Some(p) => crate::gallery::only_project(gp, p),
                    None => gp.clone(),
                };
                let mut c = Ctx::new();
                c.insert("cards", &gp.cards);
                c.insert("projects", &gp.projects);
                c.insert("show_chips", &(e.chips && e.project.is_none()));
                c.insert("embedded", &true);
                let grid = tera.render("models_grid.html", &c).context("rendering models_grid.html")?;
                Ok(format!("<link rel=\"stylesheet\" href=\"/css/models.css\">\n{grid}\n<script src=\"/js/models.js\" defer></script>"))
            });
            warnings.extend(problems.into_iter().map(|p| format!("{}: {p}", it.id)));
            text
        };
        // 3-D models first (they become passthrough HTML), then media:<key> references.
        let models = crate::model::expand(&raw, &crate::model::Env {
            lib: Some(&media),
            bundle_dir: it.bundle_dir.as_deref(),
            static_dir: Some(&st_dir),
            static_html: false,
        });
        let raw = if models.count > 0 || !models.problems.is_empty() { models.text } else { raw };
        let text = media.expand(&raw).unwrap_or(raw);
        let mut attrs = BTreeMap::new();
        attrs.insert("site-url".into(), cfg.site.base_url.clone());
        docs.push(Doc {
            id: it.id.clone(),
            text,
            base_dir: it.path.parent().unwrap().to_path_buf(),
            attrs,
        });
    }
    let renderer = Renderer { ruby: cfg.build.ruby.clone(), cache_dir: site.cache_dir().join("render") };
    let html = renderer.render_all(&docs)?;

    // Area paths each item belongs to (resolved), and project lookup.
    let projects: BTreeMap<String, &Item> =
        items.iter().filter(|i| i.kind == Kind::Project).map(|i| (i.slug.clone(), i)).collect();
    let area_titles: BTreeMap<String, String> =
        cfg.flat_areas().into_iter().map(|(p, a)| (p, a.title)).collect();

    let living = gallery_home.map(|h| h.id.clone()).zip(gallery.as_ref().and_then(|g| crate::gallery::latest_change(g, &st_dir)));
    let view = |it: &Item| -> View {
        let mut it = it.clone();
        if let Some((_, d)) = living.as_ref().filter(|(id, _)| *id == it.id) {
            if it.date.is_none_or(|x| *d > x) && it.updated.is_none_or(|x| *d > x) {
                it.updated = Some(*d);
            }
        }
        let it = &it;
        let body = html.get(&it.id).cloned().unwrap_or_default();
        // Reading time is for the text: a models gallery's cards are left out.
        let prose = match (body.find("<div class=\"mg mg-embed\""), body.rfind("</dialog>")) {
            (Some(a), Some(b)) if b > a => format!("{}{}", &body[..a], &body[b..]),
            _ => body.clone(),
        };
        let words = strip_tags(&prose).split_whitespace().count();
        let area_links = it
            .areas
            .iter()
            .filter_map(|a| cfg.resolve_area(a))
            .map(|p| AreaLink { title: area_titles.get(&p).cloned().unwrap_or_default(), url: format!("/areas/{p}/"), path: p })
            .collect();
        let (pt, pu) = it
            .project
            .as_ref()
            .and_then(|p| projects.get(p))
            .map(|p| (p.title.clone(), p.url.clone()))
            .unwrap_or_default();
        let hero = hero_url(it, &media);
        let hero_in_body = !hero.is_empty() && (body.contains(hero.rsplit('/').next().unwrap_or(&hero)) || shows_gallery.contains(&it.id));
        View {
            item: it.clone(),
            kind_label: kind_label(it.kind),
            date_human: human(it.date),
            date_iso: it.date.map(|d| d.to_string()).unwrap_or_default(),
            updated_human: human(it.updated),
            area_links,
            hero_url: hero,
            hero_in_body,
            reading_minutes: (words / 220).max(1),
            html: body,
            project_title: pt,
            project_url: pu,
        }
    };
    let views: Vec<View> = items.iter().map(view).collect();
    let by_date = |mut v: Vec<View>| {
        v.sort_by(|a, b| b.item.date.cmp(&a.item.date).then(a.item.title.cmp(&b.item.title)));
        v
    };
    // In a preview build drafts are listed too, so the whole site can be reviewed before release.
    let listed: Vec<View> = views.iter().filter(|v| !v.item.unlisted && (opts.drafts || !v.item.draft)).cloned().collect();
    let posts = by_date(listed.iter().filter(|v| v.item.kind.is_post()).cloned().collect());
    let project_views = by_date(listed.iter().filter(|v| v.item.kind == Kind::Project).cloned().collect());
    let apps: Vec<View> = listed.iter().filter(|v| v.item.kind == Kind::App).cloned().collect();
    // The home page: pinned items first (by their `:pinned:` number, then newest), then featured,
    // then the latest posts and projects — each section leaves out what an earlier one shows.
    let mut pinned = by_date(listed.iter().filter(|v| v.item.pinned && v.item.kind != Kind::Page).cloned().collect());
    pinned.sort_by_key(|v| v.item.pin_order.unwrap_or(i64::MAX));
    let featured = by_date(listed.iter().filter(|v| v.item.featured && !v.item.pinned && v.item.kind != Kind::Page).cloned().collect());
    let on_top: BTreeSet<&str> = pinned.iter().chain(&featured).map(|v| v.item.id.as_str()).collect();
    let home_posts: Vec<&View> = posts.iter().filter(|v| !on_top.contains(v.item.id.as_str())).take(6).collect();
    let home_projects: Vec<&View> = project_views.iter().filter(|v| !on_top.contains(v.item.id.as_str())).collect();
    let home = |c: &mut Ctx| {
        c.insert("pinned", &pinned);
        c.insert("featured", &featured);
        c.insert("posts", &home_posts);
        c.insert("projects", &home_projects);
        c.insert("all_posts", &posts);
        c.insert("all_projects", &project_views);
    };
    let playgrounds = by_date(listed.iter().filter(|v| v.item.kind == Kind::Playground).cloned().collect());
    let mut tools: Vec<View> = listed.iter().filter(|v| v.item.kind == Kind::Tool).cloned().collect();
    tools.sort_by(|a, b| a.item.title.to_lowercase().cmp(&b.item.title.to_lowercase()));
    // Tool categories in the order site.yaml lists them (tools: categories), then any others.
    let mut tool_groups: Vec<(String, Vec<View>)> = vec![];
    for cat in &cfg.tools.categories {
        let v: Vec<View> = tools.iter().filter(|t| t.item.attrs.get("category").map(|c| c == &cat.id).unwrap_or(false)).cloned().collect();
        if !v.is_empty() {
            tool_groups.push((cat.title.clone(), v));
        }
    }
    let known: Vec<&str> = cfg.tools.categories.iter().map(|c| c.id.as_str()).collect();
    let other: Vec<View> = tools.iter().filter(|t| !t.item.attrs.get("category").map(|c| known.contains(&c.as_str())).unwrap_or(false)).cloned().collect();
    if !other.is_empty() {
        tool_groups.push(("Other".into(), other));
    }

    // Area membership includes descendants: an item in making/woodworking is in making too.
    let in_area = |v: &View, path: &str| v.area_links.iter().any(|a| a.path == path || a.path.starts_with(&format!("{path}/")));
    fn tree(prefix: &str, list: &[crate::config::Area], counter: &dyn Fn(&str) -> usize) -> Vec<AreaNode> {
        list.iter()
            .map(|a| {
                let p = if prefix.is_empty() { a.id.clone() } else { format!("{prefix}/{}", a.id) };
                AreaNode {
                    count: counter(&p),
                    children: tree(&p, &a.children, counter),
                    url: format!("/areas/{p}/"),
                    path: p,
                    id: a.id.clone(),
                    title: a.title.clone(),
                    description: a.description.clone(),
                }
            })
            .collect()
    }
    let counter = |p: &str| listed.iter().filter(|v| v.item.kind != Kind::Page && in_area(v, p)).count();
    let areas_tree = tree("", &cfg.areas, &counter);

    let base = |path: &str, title: &str, noindex: bool| -> Ctx {
        let mut c = Ctx::new();
        c.insert("site", &cfg.site);
        c.insert("analytics_id", &cfg.site.analytics_id());
        c.insert("nav", &cfg.nav);
        c.insert("page_path", path);
        c.insert("page_title", title);
        c.insert("noindex", &noindex);
        c.insert("areas_tree", &areas_tree);
        c.insert("build_year", &chrono::Local::now().format("%Y").to_string());
        c
    };

    if out.exists() {
        std::fs::remove_dir_all(&out).with_context(|| format!("clearing {}", out.display()))?;
    }
    std::fs::create_dir_all(&out)?;
    let mut files: Vec<String> = vec![];
    let write = |rel: &str, body: String, files: &mut Vec<String>| put(&out, rel, body, files);
    let page_file = |url: &str| format!("{}index.html", url.trim_start_matches('/'));

    // Item pages.
    for v in &views {
        let it = &v.item;
        let tpl = match it.kind {
            Kind::Page if it.url == "/" => "home.html",
            Kind::Page => "page.html",
            Kind::Article | Kind::Update => "article.html",
            Kind::Project => "project.html",
            Kind::App => "app.html",
            Kind::Playground => "playground.html",
            Kind::Tool => "tool.html",
        };
        let mut c = base(&it.url, &it.title, it.unlisted || it.draft);
        c.insert("item", v);
        c.insert("posts", &posts.iter().take(6).collect::<Vec<_>>());
        c.insert("featured", &featured);
        c.insert("projects", &project_views);
        if tpl == "home.html" {
            home(&mut c);
        }
        c.insert("apps", &apps);
        c.insert("tools", &tools);
        if it.kind == Kind::Project {
            let ups = by_date(listed.iter().filter(|x| x.item.project.as_deref() == Some(&it.slug)).cloned().collect());
            c.insert("updates", &ups);
        }
        if it.kind.is_post() {
            let related: Vec<&View> = posts
                .iter()
                .filter(|x| x.item.id != it.id && x.area_links.iter().any(|a| v.area_links.iter().any(|b| a.path == b.path)))
                .take(3)
                .collect();
            c.insert("related", &related);
        }
        write(&page_file(&it.url), tera.render(tpl, &c).with_context(|| format!("rendering {} with {tpl}", it.id))?, &mut files)?;
        if let Some(dir) = &it.bundle_dir {
            for e in WalkDir::new(dir).min_depth(1) {
                let e = e?;
                let rel = e.path().strip_prefix(dir)?;
                if e.file_type().is_file() && e.path().extension().map(|x| x != "adoc").unwrap_or(true)
                    && !rel.starts_with("updates")
                {
                    let dest = format!("{}{}", it.url.trim_start_matches('/'), rel.to_string_lossy());
                    let p = out.join(&dest);
                    std::fs::create_dir_all(p.parent().unwrap())?;
                    std::fs::copy(e.path(), &p)?;
                    files.push(dest);
                }
            }
        }
    }
    if !views.iter().any(|v| v.item.url == "/") {
        warnings.push("no home page (content/pages/home.adoc); using the template alone".into());
        let mut c = base("/", &cfg.site.title, false);
        c.insert("item", &serde_json::json!({"title": cfg.site.title, "html": ""}));
        home(&mut c);
        c.insert("apps", &apps);
        c.insert("tools", &tools);
        write("index.html", tera.render("home.html", &c)?, &mut files)?;
    }

    // Listings.
    let list_page = |url: &str, title: &str, intro: &str, entries: &[View], files: &mut Vec<String>| -> Result<()> {
        let mut c = base(url, title, false);
        c.insert("heading", title);
        c.insert("intro", intro);
        c.insert("entries", entries);
        write(&page_file(url), tera.render("list.html", &c)?, files)?;
        Ok(())
    };
    // A listing from site.yaml (`articles:`, `projects:`): its intro, and grouped by area if set.
    let listing = |url: &str, title: &str, lc: &crate::config::ListingConfig, default_intro: &str, group_default: bool, entries: &[View], files: &mut Vec<String>| -> Result<()> {
        let intro = lc.intro.clone().unwrap_or_else(|| default_intro.to_string());
        if !lc.group_by_area.unwrap_or(group_default) {
            return list_page(url, title, &intro, entries, files);
        }
        let (groups, buttons) = group_by_area(cfg, lc, entries);
        let mut c = base(url, title, false);
        c.insert("heading", title);
        c.insert("intro", &intro);
        c.insert("entries", entries);
        c.insert("groups", &groups);
        c.insert("filters", &buttons);
        write(&page_file(url), tera.render("list.html", &c)?, files)?;
        Ok(())
    };
    listing("/articles/", "Articles", &cfg.articles, "Articles and project updates, newest first.", false, &posts, &mut files)?;
    listing("/projects/", "Projects", &cfg.projects, "Things built, and being built.", true, &project_views, &mut files)?;

    let mut tags: BTreeSet<String> = BTreeSet::new();
    for v in &listed {
        tags.extend(v.item.tags.iter().cloned());
    }
    for t in &tags {
        let entries: Vec<View> = by_date(listed.iter().filter(|v| v.item.tags.contains(t) && v.item.kind != Kind::Page).cloned().collect());
        list_page(&format!("/tags/{}/", crate::content::slugify(t)), &format!("Tagged “{t}”"), "", &entries, &mut files)?;
    }

    {
        let mut c = base("/areas/", "Areas", false);
        write("areas/index.html", tera.render("areas.html", &c)?, &mut files)?;
        for (p, a) in cfg.flat_areas() {
            let entries: Vec<View> = by_date(listed.iter().filter(|v| v.item.kind != Kind::Page && in_area(v, &p)).cloned().collect());
            c = base(&format!("/areas/{p}/"), &a.title, false);
            let node = find_node(&areas_tree, &p).cloned();
            c.insert("area", &node);
            c.insert("entries", &entries);
            let crumbs: Vec<AreaLink> = (1..=p.matches('/').count())
                .map(|n| p.splitn(n + 1, '/').take(n).collect::<Vec<_>>().join("/"))
                .map(|q| AreaLink { title: area_titles.get(&q).cloned().unwrap_or_default(), url: format!("/areas/{q}/"), path: q })
                .collect();
            c.insert("crumbs", &crumbs);
            write(&page_file(&format!("/areas/{p}/")), tera.render("area.html", &c)?, &mut files)?;
            }
    }

    {
        let mut c = base("/lab/", "Lab", false);
        c.insert("apps", &apps);
        c.insert("tools", &tools);
        c.insert("playgrounds", &playgrounds);
        write("lab/index.html", tera.render("lab.html", &c)?, &mut files)?;
    }

    {
        let mut c = base("/tools/", "Tools", false);
        c.insert("groups", &tool_groups);
        c.insert("count", &tools.len());
        write("tools/index.html", tera.render("tools.html", &c)?, &mut files)?;
    }

    // The 3-D models gallery: one page per model, and /models/ (the grid, or a redirect to the
    // gallery's home post).
    let mut gallery_urls: Vec<String> = vec![];
    if let Some(gp) = gpages.as_mut() {
        let home_view = gallery_home.and_then(|h| listed.iter().find(|v| v.item.id == h.id));
        let (index_url, heading) = match home_view {
            Some(v) => (v.item.url.clone(), v.item.title.clone()),
            None => ("/models/".to_string(), gp.title.clone()),
        };
        // Which listed posts show each model (by the model's key in the models repository).
        for c in gp.cards.iter_mut() {
            c.used_in = listed
                .iter()
                .filter(|v| v.item.kind != Kind::Page || v.item.url != "/")
                .filter(|v| shows_models.get(&v.item.id).is_some_and(|keys| keys.iter().any(|k| media.resolve(k).and_then(|m| m.ids.get("model")) == Some(&c.key))))
                .map(|v| crate::gallery::NavLink { url: v.item.url.clone(), name: v.item.title.clone() })
                .collect();
        }
        if let Some(v) = home_view {
            write("models/index.html", redirect_page(&v.item.url, &v.item.title), &mut files)?;
        } else {
            let mut c = base("/models/", &gp.title, false);
            c.insert("heading", &gp.title);
            c.insert("intro", &gp.intro);
            c.insert("cards", &gp.cards);
            c.insert("projects", &gp.projects);
            c.insert("show_chips", &true);
            c.insert("embedded", &false);
            c.insert("og_image", &gp.cards.first().map(|m| m.og_image.clone()).unwrap_or_default());
            write("models/index.html", tera.render("models.html", &c).context("rendering models.html")?, &mut files)?;
            gallery_urls.push("/models/".into());
        }
        for m in &gp.cards {
            let mut c = base(&m.url, &m.name, false);
            c.insert("heading", &heading);
            c.insert("index_url", &index_url);
            c.insert("m", m);
            c.insert("viewer_js", crate::model::VIEWER_JS);
            write(&page_file(&m.url), tera.render("model.html", &c).with_context(|| format!("rendering model.html for {}", m.key))?, &mut files)?;
            gallery_urls.push(m.url.clone());
        }
    }

    {
        let pubs: Vec<&Entry> = registry.entries.iter().filter(|e| e.kind == "publication").collect();
        let mut groups: Vec<(String, Vec<&Entry>)> = vec![];
        for e in pubs {
            let g = if e.group.is_empty() { "Publications".to_string() } else { e.group.clone() };
            match groups.iter_mut().find(|(n, _)| *n == g) {
                Some((_, v)) => v.push(e),
                None => groups.push((g, vec![e])),
            }
        }
        let all_public = registry.entries.iter().filter(|e| e.kind == "publication").all(|e| e.visibility == "public");
        let mut c = base("/publications/", "Publications", !all_public);
        c.insert("groups", &groups);
        write("publications/index.html", tera.render("publications.html", &c)?, &mut files)?;
    }

    // Feed, sitemap, search index, robots, 404.
    {
        let mut c = base("/feed.xml", &cfg.site.title, false);
        c.insert("entries", &posts.iter().take(30).collect::<Vec<_>>());
        c.insert("updated", &posts.first().map(|p| p.date_iso.clone()).unwrap_or_default());
        write("feed.xml", tera.render("feed.xml", &c)?, &mut files)?;
        let mut urls: Vec<String> = vec!["/".into(), "/articles/".into(), "/projects/".into(), "/areas/".into(), "/lab/".into(), "/tools/".into()];
        urls.extend(listed.iter().map(|v| v.item.url.clone()));
        urls.extend(gallery_urls.iter().cloned());
        urls.extend(cfg.flat_areas().into_iter().map(|(p, _)| format!("/areas/{p}/")));
        let pubs_public = registry.entries.iter().any(|e| e.kind == "publication")
            && registry.entries.iter().filter(|e| e.kind == "publication").all(|e| e.visibility == "public");
        if pubs_public {
            urls.push("/publications/".into());
        }
        urls.sort();
        urls.dedup();
        let mut c = base("/sitemap.xml", "", false);
        c.insert("urls", &urls);
        write("sitemap.xml", tera.render("sitemap.xml", &c)?, &mut files)?;
        let idx: Vec<serde_json::Value> = listed
            .iter()
            .filter(|v| v.item.kind != Kind::Page || v.item.url != "/")
            .map(|v| serde_json::json!({
                "title": v.item.title, "url": v.item.url, "kind": v.kind_label, "summary": v.item.summary,
                "date": v.date_iso, "areas": v.area_links.iter().map(|a| a.title.clone()).collect::<Vec<_>>(),
                "tags": v.item.tags,
            }))
            .collect();
        write("search.json", serde_json::to_string(&idx)?, &mut files)?;
        write("robots.txt", format!("User-agent: *\nAllow: /\nSitemap: {}/sitemap.xml\n", cfg.site.base_url.trim_end_matches('/')), &mut files)?;
        let c = base("/404.html", "Not found", true);
        write("404.html", tera.render("404.html", &c)?, &mut files)?;
        write(".nojekyll", String::new(), &mut files)?;
    }

    // Static assets. build.preview_only folders go into draft builds only, never into a publish.
    let st = site.static_dir();
    let held_back: Vec<PathBuf> = if opts.drafts { Vec::new() } else { cfg.build.preview_only.iter().map(|d| st.join(d.trim_matches('/'))).collect() };
    // The built-in theme's assets; a site's own static/ files of the same name replace them below.
    for (rel, bytes) in crate::theme::STATIC {
        if held_back.iter().any(|d| st.join(rel).starts_with(d)) {
            continue;
        }
        let p = out.join(rel);
        std::fs::create_dir_all(p.parent().unwrap())?;
        std::fs::write(&p, bytes)?;
        files.push(rel.to_string());
    }
    if st.exists() {
        for e in WalkDir::new(&st).min_depth(1) {
            let e = e?;
            if e.file_type().is_file() && !held_back.iter().any(|d| e.path().starts_with(d)) {
                let rel = e.path().strip_prefix(&st)?.to_string_lossy().to_string();
                let p = out.join(&rel);
                std::fs::create_dir_all(p.parent().unwrap())?;
                std::fs::copy(e.path(), &p)?;
                files.push(rel);
            }
        }
    }
    files.sort();
    files.dedup();
    write_manifest(&out, &files)?;
    let pages = files.iter().filter(|f| f.ends_with(".html")).count();
    Ok(BuildReport { out_dir: out, pages, items: items.len(), drafts_included: opts.drafts, files, warnings })
}

/// A page that sends readers on to `url` (keeping the `#project` filter), for an address that
/// moved: /models/ when the gallery lives in a post.
fn redirect_page(url: &str, title: &str) -> String {
    let u = escape_html(url);
    let js = serde_json::to_string(url).unwrap_or_default().replace('<', "\\u003c");
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><title>{t}</title>\n<link rel=\"canonical\" href=\"{u}\">\n<meta name=\"robots\" content=\"noindex\">\n<meta http-equiv=\"refresh\" content=\"0; url={u}\">\n<script>location.replace({js} + location.hash)</script></head>\n<body><p>This page has moved to <a href=\"{u}\">{t}</a>.</p></body></html>\n",
        t = escape_html(title)
    )
}

fn escape_html(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            c => o.push(c),
        }
    }
    o
}

fn put(out: &Path, rel: &str, body: String, files: &mut Vec<String>) -> Result<()> {
    let p = out.join(rel.trim_start_matches('/'));
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(&p, body)?;
    files.push(rel.trim_start_matches('/').to_string());
    Ok(())
}

fn find_node<'a>(list: &'a [AreaNode], path: &str) -> Option<&'a AreaNode> {
    for n in list {
        if n.path == path {
            return Some(n);
        }
        if let Some(x) = find_node(&n.children, path) {
            return Some(x);
        }
    }
    None
}

pub const MANIFEST: &str = ".gitwebsite-manifest.json";

fn write_manifest(out: &Path, files: &[String]) -> Result<()> {
    std::fs::write(out.join(MANIFEST), serde_json::to_string_pretty(files)?)?;
    Ok(())
}
