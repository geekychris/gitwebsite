//! WordPress: import existing posts as AsciiDoc articles, and cross-post new ones.
//!
//! Import reads public posts — no credentials — from the WordPress.com public API, falling
//! back to a self-hosted site's `/wp-json` REST API, or from a WXR export file. Every
//! imported post gets a registry entry recording where it came from.
//!
//! Cross-posting uses XML-RPC (`wp.newPost`), which WordPress.com and most self-hosted
//! sites support, with credentials from `gw wordpress auth` (Keychain service
//! `gitwebsite-wordpress`).

use super::html2adoc::{self, decode_entities, plain};
use super::{keychain, xmlrpc, WordpressCmd};
use anyhow::{bail, Context, Result};
use chrono::{Local, NaiveDate};
use gw_core::content::{slugify, Kind};
use gw_core::registry::Entry;
use gw_core::render::{render_one, Doc};
use gw_core::Site;
use quick_xml::events::Event;
use quick_xml::Reader;
use regex::Regex;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const SERVICE: &str = "gitwebsite-wordpress";

#[derive(Debug, Clone)]
struct Post {
    id: String,
    title: String,
    date: NaiveDate,
    slug: String,
    link: String,
    html: String,
    excerpt_html: String,
    tags: Vec<String>,
    categories: Vec<String>,
    featured: Option<String>,
}

fn host_of(s: &str) -> String {
    let s = s.trim().trim_start_matches("https://").trim_start_matches("http://");
    s.split('/').next().unwrap_or(s).to_string()
}

fn http() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder().user_agent("gitwebsite/0.1").timeout(std::time::Duration::from_secs(60)).build()?)
}

fn day(s: &str) -> Result<NaiveDate> {
    let s = s.trim();
    NaiveDate::parse_from_str(s.get(..10).unwrap_or(s), "%Y-%m-%d").with_context(|| format!("bad post date {s:?}"))
}

fn names(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Object(m)) => m.values().filter_map(|x| x.get("name").and_then(|n| n.as_str())).map(decode_entities).collect(),
        Some(Value::Array(a)) => a.iter().filter_map(|x| x.get("name").and_then(|n| n.as_str())).map(decode_entities).collect(),
        _ => vec![],
    }
}

// ------------------------------------------------------------------ fetching

/// WordPress.com public API. `Ok(None)` if the site is not on WordPress.com.
fn fetch_wpcom(host: &str, limit: usize) -> Result<Option<Vec<Post>>> {
    let c = http()?;
    let mut out = vec![];
    for page in 1.. {
        let url = format!("https://public-api.wordpress.com/rest/v1.1/sites/{host}/posts/?number=100&page={page}&order_by=date&order=DESC");
        let r = c.get(&url).send()?;
        let status = r.status();
        let v: Value = r.json().unwrap_or(Value::Null);
        if !status.is_success() {
            if page == 1 {
                return Ok(None);
            }
            bail!("{url}: HTTP {status}");
        }
        let posts = v.get("posts").and_then(|p| p.as_array()).cloned().unwrap_or_default();
        if posts.is_empty() {
            break;
        }
        for p in posts {
            let s = |k: &str| p.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
            out.push(Post {
                id: p.get("ID").map(|x| x.to_string()).unwrap_or_default(),
                title: plain(&s("title")),
                date: day(&s("date"))?,
                slug: s("slug"),
                link: s("URL"),
                html: s("content"),
                excerpt_html: s("excerpt"),
                tags: names(p.get("tags")),
                categories: names(p.get("categories")),
                featured: Some(s("featured_image")).filter(|x| !x.is_empty()),
            });
            if out.len() >= limit {
                return Ok(Some(out));
            }
        }
        let found = v.get("found").and_then(|f| f.as_u64()).unwrap_or(0) as usize;
        if out.len() >= found {
            break;
        }
    }
    Ok(Some(out))
}

/// Core REST API of a self-hosted WordPress.
fn fetch_wpjson(host: &str, limit: usize) -> Result<Vec<Post>> {
    let c = http()?;
    let mut out = vec![];
    for page in 1.. {
        let url = format!("https://{host}/wp-json/wp/v2/posts?per_page=100&page={page}&_embed=1");
        let r = c.get(&url).send()?;
        let status = r.status();
        if status.as_u16() == 400 && page > 1 {
            break; // past the last page
        }
        if !status.is_success() {
            bail!("{url}: HTTP {status} — is this a WordPress site with the REST API enabled? Try --wxr with an export file.");
        }
        let total_pages = r.headers().get("X-WP-TotalPages").and_then(|h| h.to_str().ok()).and_then(|s| s.parse::<u32>().ok()).unwrap_or(1);
        let posts: Vec<Value> = r.json()?;
        for p in &posts {
            let rendered = |k: &str| p.pointer(&format!("/{k}/rendered")).and_then(|x| x.as_str()).unwrap_or("").to_string();
            let terms = p.pointer("/_embedded/wp:term").and_then(|t| t.as_array()).cloned().unwrap_or_default();
            let mut cats = vec![];
            let mut tags = vec![];
            for group in terms {
                for t in group.as_array().cloned().unwrap_or_default() {
                    let name = t.get("name").and_then(|n| n.as_str()).map(decode_entities).unwrap_or_default();
                    match t.get("taxonomy").and_then(|x| x.as_str()) {
                        Some("category") => cats.push(name),
                        Some("post_tag") => tags.push(name),
                        _ => {}
                    }
                }
            }
            out.push(Post {
                id: p.get("id").map(|x| x.to_string()).unwrap_or_default(),
                title: plain(&rendered("title")),
                date: day(p.get("date").and_then(|x| x.as_str()).unwrap_or(""))?,
                slug: p.get("slug").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                link: p.get("link").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                html: rendered("content"),
                excerpt_html: rendered("excerpt"),
                tags,
                categories: cats,
                featured: p.pointer("/_embedded/wp:featuredmedia/0/source_url").and_then(|x| x.as_str()).map(|s| s.to_string()),
            });
            if out.len() >= limit {
                return Ok(out);
            }
        }
        if page >= total_pages || posts.is_empty() {
            break;
        }
    }
    Ok(out)
}

/// WordPress export (WXR) file: published posts only.
fn read_wxr(path: &Path) -> Result<(String, Vec<Post>)> {
    let xml = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut r = Reader::from_str(&xml);
    let mut posts = vec![];
    let mut site_link = String::new();
    let mut cur: Option<BTreeMap<String, String>> = None;
    let mut cats: Vec<(String, String)> = vec![];
    let mut path_stack: Vec<String> = vec![];
    let mut cat_domain = String::new();
    let mut text = String::new();
    loop {
        match r.read_event()? {
            Event::Start(e) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if name == "item" {
                    cur = Some(BTreeMap::new());
                    cats.clear();
                }
                if name == "category" {
                    cat_domain = e
                        .attributes()
                        .flatten()
                        .find(|a| a.key.as_ref() == b"domain")
                        .map(|a| String::from_utf8_lossy(&a.value).to_string())
                        .unwrap_or_default();
                }
                path_stack.push(name);
                text.clear();
            }
            Event::Text(t) => text.push_str(&t.unescape()?.into_owned()),
            Event::CData(t) => text.push_str(&String::from_utf8_lossy(&t)),
            Event::End(_) => {
                let name = path_stack.pop().unwrap_or_default();
                let parent = path_stack.last().cloned().unwrap_or_default();
                if name == "item" {
                    if let Some(m) = cur.take() {
                        let g = |k: &str| m.get(k).cloned().unwrap_or_default();
                        if g("wp:post_type") == "post" && g("wp:status") == "publish" {
                            posts.push(Post {
                                id: g("wp:post_id"),
                                title: plain(&g("title")),
                                date: day(&g("wp:post_date"))?,
                                slug: g("wp:post_name"),
                                link: g("link"),
                                html: g("content:encoded"),
                                excerpt_html: g("excerpt:encoded"),
                                tags: cats.iter().filter(|(d, _)| d == "post_tag").map(|(_, n)| n.clone()).collect(),
                                categories: cats.iter().filter(|(d, _)| d == "category").map(|(_, n)| n.clone()).collect(),
                                featured: None,
                            });
                        }
                    }
                } else if let Some(m) = cur.as_mut() {
                    if parent == "item" {
                        if name == "category" {
                            cats.push((cat_domain.clone(), text.trim().to_string()));
                        } else {
                            m.insert(name, text.clone());
                        }
                    }
                } else if name == "link" && parent == "channel" && site_link.is_empty() {
                    site_link = text.trim().to_string();
                }
                text.clear();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok((host_of(&site_link), posts))
}

// ------------------------------------------------------------------ writing

fn file_name_from_url(u: &str) -> String {
    let path = u.split(['?', '#']).next().unwrap_or(u);
    let name = path.rsplit('/').next().unwrap_or("image");
    let name: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || ".-_".contains(c) { c } else { '-' }).collect();
    if name.is_empty() { "image".into() } else { name }
}

fn header_line(s: &str) -> String {
    s.replace('\n', " ").trim().to_string()
}

struct Planned {
    post: Post,
    path: PathBuf,
    item_id: String,
    reg_id: String,
}

fn import(site: &Site, host: &str, posts: Vec<Post>, since: Option<NaiveDate>, status: &str, download: bool, dry_run: bool) -> Result<Value> {
    if !matches!(status, "draft" | "published") {
        bail!("--status must be draft or published");
    }
    let hslug = slugify(host);
    let mut registry = site.registry()?;
    let existing_urls: Vec<String> = site.items()?.into_iter().map(|i| i.url).collect();
    let mut planned = vec![];
    let mut skipped = vec![];
    for p in posts {
        if since.map(|s| p.date < s).unwrap_or(false) {
            continue;
        }
        let reg_id = format!("wordpress-{hslug}-{}", p.id);
        let slug = if p.slug.is_empty() { slugify(&p.title) } else { slugify(&p.slug) };
        if registry.get(&reg_id).is_some() {
            skipped.push(json!({ "title": p.title, "reason": "already imported", "registry": reg_id }));
            continue;
        }
        if existing_urls.contains(&format!("/articles/{slug}/")) {
            skipped.push(json!({ "title": p.title, "reason": format!("an article already uses /articles/{slug}/") }));
            continue;
        }
        let images = html2adoc::image_urls(&p.html);
        let bundle = download && !images.is_empty();
        let dir = site.content_dir().join("articles");
        let (path, item_id) = if bundle {
            (dir.join(&slug).join("index.adoc"), format!("articles/{slug}"))
        } else {
            (dir.join(format!("{slug}.adoc")), format!("articles/{slug}"))
        };
        planned.push(Planned { post: p, path, item_id, reg_id });
    }

    if dry_run {
        return Ok(json!({
            "source": host, "dry_run": true,
            "would_import": planned.iter().map(|x| json!({
                "title": x.post.title, "date": x.post.date.to_string(), "path": x.path.display().to_string(),
                "registry": x.reg_id, "images": html2adoc::image_urls(&x.post.html).len(),
            })).collect::<Vec<_>>(),
            "skipped": skipped,
        }));
    }

    let today = Local::now().date_naive();
    let client = http()?;
    let mut imported = vec![];
    for x in &planned {
        let p = &x.post;
        let mut body = html2adoc::convert(&p.html).with_context(|| format!("converting {}", p.title))?;
        if x.path.ends_with("index.adoc") {
            let dir = x.path.parent().unwrap();
            std::fs::create_dir_all(dir)?;
            for u in html2adoc::image_urls(&p.html) {
                let name = file_name_from_url(&u);
                match client.get(&u).send().and_then(|r| r.error_for_status()).and_then(|r| r.bytes()) {
                    Ok(b) => {
                        std::fs::write(dir.join(&name), &b)?;
                        body = body.replace(&u, &name);
                        let bare = u.split('?').next().unwrap_or(&u).to_string();
                        body = body.replace(&bare, &name);
                    }
                    Err(e) => eprintln!("warning: could not download {u}: {e}; keeping the link"),
                }
            }
        }
        let mut areas = vec![];
        for c in p.categories.iter() {
            if let Some(a) = site.config.resolve_area(&slugify(c)) {
                areas.push(a);
            }
        }
        let mut tags: Vec<String> = p.tags.iter().chain(p.categories.iter()).cloned().collect();
        tags.sort();
        tags.dedup();
        tags.retain(|t| !t.eq_ignore_ascii_case("uncategorized"));
        let mut summary = header_line(&plain(&p.excerpt_html));
        if summary.chars().count() > 220 {
            summary = summary.chars().take(217).collect::<String>().trim_end().to_string() + "…";
        }
        let mut h = format!("= {}\n:type: article\n:date: {}\n:status: {status}\n", header_line(&p.title), p.date);
        if !summary.is_empty() {
            h += &format!(":summary: {summary}\n");
        }
        if !tags.is_empty() {
            h += &format!(":tags: {}\n", tags.iter().map(|t| t.replace(',', " ")).collect::<Vec<_>>().join(", "));
        }
        if !areas.is_empty() {
            h += &format!(":areas: {}\n", areas.join(", "));
        }
        if let Some(f) = &p.featured {
            h += &format!(":hero: {f}\n");
        }
        h += &format!(":source: {}\n", x.reg_id);
        if areas.is_empty() {
            h += "// TODO areas: choose from `gw areas` and add an :areas: line\n";
        }
        h += &format!("// Imported from {} on {today}\n", p.link);
        std::fs::create_dir_all(x.path.parent().unwrap())?;
        std::fs::write(&x.path, format!("{h}\n{body}"))?;
        registry.upsert(Entry {
            id: x.reg_id.clone(),
            kind: "wordpress".into(),
            title: p.title.clone(),
            subtitle: String::new(),
            url: p.link.clone(),
            description: String::new(),
            origin: format!("{host} post {}", p.id),
            links: vec![],
            meta: vec![p.date.to_string()],
            areas: areas.clone(),
            group: String::new(),
            visibility: "public".into(),
            added: Some(today),
            content: x.item_id.clone(),
        });
        imported.push(json!({ "title": p.title, "path": x.path.display().to_string(), "registry": x.reg_id }));
    }
    let site_id = format!("wordpress-site-{hslug}");
    if registry.get(&site_id).is_none() {
        registry.upsert(Entry {
            id: site_id,
            kind: "wordpress".into(),
            title: format!("WordPress site {host}"),
            subtitle: String::new(),
            url: format!("https://{host}/"),
            description: "Source of imported posts.".into(),
            origin: host.to_string(),
            links: vec![],
            meta: vec![],
            areas: vec![],
            group: String::new(),
            visibility: "public".into(),
            added: Some(today),
            content: String::new(),
        });
    }
    registry.save(&site.registry_path())?;
    Ok(json!({ "source": host, "imported": imported, "skipped": skipped, "status": status }))
}

// ------------------------------------------------------------------ auth and cross-posting

struct WpAuth {
    host: String,
    user: String,
    password: String,
    blog_id: i64,
}

fn load_auth() -> Result<WpAuth> {
    let g = |a: &str| keychain::get(SERVICE, a);
    match (g("site"), g("username"), g("password")) {
        (Some(host), Some(user), Some(password)) => Ok(WpAuth {
            host,
            user,
            password,
            blog_id: g("blog-id").and_then(|b| b.parse().ok()).unwrap_or(1),
        }),
        _ => bail!("WordPress credentials are not set. Run `gw wordpress auth --host <your-site>` in a terminal."),
    }
}

fn endpoint(host: &str) -> String {
    format!("https://{host}/xmlrpc.php")
}

fn auth_flow(host: &str) -> Result<Value> {
    let host = host_of(host);
    eprintln!("WordPress credentials for {host}. On WordPress.com use your username and an application password");
    eprintln!("(https://wordpress.com/me/security/application-passwords); on self-hosted sites, an application password from your profile.");
    let user = keychain::prompt("Username")?;
    let password = keychain::prompt_secret("Application password")?;
    if user.is_empty() || password.is_empty() {
        bail!("username and password are required");
    }
    let blogs = xmlrpc::call(&endpoint(&host), "wp.getUsersBlogs", &[json!(user), json!(password)])
        .context("checking the credentials with wp.getUsersBlogs")?;
    let blog = blogs.as_array().and_then(|a| {
        a.iter().find(|b| b.get("url").and_then(|u| u.as_str()).map(|u| u.contains(&host)).unwrap_or(false)).or_else(|| a.first())
    });
    let blog_id = blog.and_then(|b| b.get("blogid")).map(|v| v.as_str().map(|s| s.to_string()).unwrap_or_else(|| v.to_string())).unwrap_or_else(|| "1".into());
    keychain::set(SERVICE, "site", &host)?;
    keychain::set(SERVICE, "username", &user)?;
    keychain::set(SERVICE, "password", &password)?;
    keychain::set(SERVICE, "blog-id", &blog_id)?;
    Ok(json!({ "authorised": true, "site": host, "blog_id": blog_id, "blog": blog.and_then(|b| b.get("blogName")) }))
}

fn absolutise(html: &str, base: &str, page_url: &str) -> String {
    let re = Regex::new(r#"(src|href)="([^"]+)""#).unwrap();
    re.replace_all(html, |c: &regex::Captures| {
        let v = &c[2];
        let abs = if v.starts_with("http") || v.starts_with('#') || v.starts_with("mailto:") || v.starts_with("data:") {
            v.to_string()
        } else if v.starts_with('/') {
            format!("{base}{v}")
        } else {
            format!("{base}{page_url}{v}")
        };
        format!("{}=\"{}\"", &c[1], abs)
    })
    .into_owned()
}

fn crosspost(site: &Site, id: &str, status: &str) -> Result<Value> {
    let status = match status {
        "draft" => "draft",
        "publish" | "published" => "publish",
        s => bail!("--status must be draft or publish, not {s}"),
    };
    let it = site.item(id)?;
    if it.draft {
        bail!("{} is a draft here; publish it on the site first (`gw release {}`)", it.id, it.id);
    }
    if it.unlisted {
        bail!("{} is unlisted; cross-posting would make it public", it.id);
    }
    if !matches!(it.kind, Kind::Article | Kind::Update | Kind::Project) {
        bail!("only articles, project updates, and projects are cross-posted");
    }
    let reg_id = format!("wordpress-crosspost-{}", it.slug);
    let mut registry = site.registry()?;
    if let Some(e) = registry.get(&reg_id) {
        bail!("already cross-posted: {} ({})", e.url, reg_id);
    }
    let auth = load_auth()?;
    let raw = std::fs::read_to_string(&it.path)?;
    let lib = site.media()?;
    // WordPress runs no page scripts: 3-D models go over as their poster and a download link.
    let raw = gw_core::model::expand(&raw, &gw_core::model::Env {
        lib: Some(&lib),
        bundle_dir: it.bundle_dir.as_deref(),
        static_dir: Some(&site.static_dir()),
        static_html: true,
    })
    .text;
    let text = lib.expand(&raw)?;
    let mut attrs = BTreeMap::new();
    attrs.insert("site-url".into(), site.config.site.base_url.clone());
    let html = render_one(
        &site.config.build.ruby,
        &site.cache_dir().join("render"),
        Doc { id: it.id.clone(), text, base_dir: it.path.parent().unwrap().to_path_buf(), attrs },
    )?;
    let base = site.config.site.base_url.trim_end_matches('/').to_string();
    let canonical = format!("{base}{}", it.url);
    let content = format!(
        "{}\n<p><em>Originally published at <a href=\"{canonical}\">{canonical}</a>.</em></p>",
        absolutise(&html, &base, &it.url)
    );
    let post = json!({
        "post_type": "post",
        "post_status": status,
        "post_title": it.title,
        "post_content": content,
        "post_excerpt": it.summary,
        "terms_names": { "post_tag": it.tags },
    });
    let ep = endpoint(&auth.host);
    let pid = xmlrpc::call(&ep, "wp.newPost", &[json!(auth.blog_id), json!(auth.user), json!(auth.password), post])?;
    let pid = pid.as_str().map(|s| s.to_string()).unwrap_or_else(|| pid.to_string());
    let link = xmlrpc::call(&ep, "wp.getPost", &[json!(auth.blog_id), json!(auth.user), json!(auth.password), json!(pid.parse::<i64>().unwrap_or(0))])
        .ok()
        .and_then(|p| p.get("link").and_then(|l| l.as_str()).map(|s| s.to_string()))
        .unwrap_or_else(|| format!("https://{}/?p={pid}", auth.host));
    registry.upsert(Entry {
        id: reg_id.clone(),
        kind: "wordpress".into(),
        title: it.title.clone(),
        subtitle: String::new(),
        url: link.clone(),
        description: format!("Cross-post of {canonical} ({status} on WordPress)."),
        origin: format!("{} post {pid}", auth.host),
        links: vec![],
        meta: vec![],
        areas: vec![],
        group: String::new(),
        visibility: "public".into(),
        added: Some(Local::now().date_naive()),
        content: it.id.clone(),
    });
    registry.save(&site.registry_path())?;
    Ok(json!({ "posted": true, "status": status, "wordpress_post_id": pid, "url": link, "registry": reg_id }))
}

pub fn run(site: &Site, cmd: WordpressCmd) -> Result<Value> {
    match cmd {
        WordpressCmd::Auth { host } => auth_flow(&host),
        WordpressCmd::Logout => {
            for a in ["site", "username", "password", "blog-id"] {
                keychain::delete(SERVICE, a);
            }
            Ok(json!({ "logged_out": true }))
        }
        WordpressCmd::Import { from, wxr, since, status, download_images, limit, dry_run } => {
            let since = since.as_deref().map(day).transpose()?;
            let limit = limit.unwrap_or(usize::MAX);
            let (host, posts) = match (from, wxr) {
                (_, Some(path)) => {
                    let (h, mut p) = read_wxr(&path)?;
                    p.truncate(limit);
                    (if h.is_empty() { "wordpress-export".to_string() } else { h }, p)
                }
                (Some(f), None) => {
                    let host = host_of(&f);
                    let posts = match fetch_wpcom(&host, limit)? {
                        Some(p) => p,
                        None => fetch_wpjson(&host, limit)?,
                    };
                    (host, posts)
                }
                (None, None) => bail!("pass --from <site> or --wxr <export.xml>"),
            };
            import(site, &host, posts, since, &status, download_images, dry_run)
        }
        WordpressCmd::Crosspost { id, status } => crosspost(site, &id, &status),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_and_files() {
        assert_eq!(host_of("https://someone.wordpress.com/2020/01/x/"), "someone.wordpress.com");
        assert_eq!(host_of("example.com"), "example.com");
        assert_eq!(file_name_from_url("https://i0.wp.com/a/b/My%20Pic.jpg?w=1024"), "My-20Pic.jpg");
    }

    #[test]
    fn absolute_links() {
        let h = absolutise(r#"<img src="bench.jpg"><a href="/about/">a</a><a href="https://x.y">x</a>"#, "https://s.io", "/articles/a/");
        assert!(h.contains(r#"src="https://s.io/articles/a/bench.jpg""#));
        assert!(h.contains(r#"href="https://s.io/about/""#));
        assert!(h.contains(r#"href="https://x.y""#));
    }

    #[test]
    fn wxr_parsing() {
        let xml = r#"<?xml version="1.0"?><rss xmlns:wp="http://wordpress.org/export/1.2/" xmlns:content="http://purl.org/rss/1.0/modules/content/" xmlns:excerpt="http://wordpress.org/export/1.2/excerpt/"><channel><title>Blog</title><link>https://myblog.example.com</link>
<item><title>Hello &amp; welcome</title><link>https://myblog.example.com/hello/</link><content:encoded><![CDATA[<p>Hi <b>there</b></p>]]></content:encoded><excerpt:encoded><![CDATA[Short]]></excerpt:encoded><wp:post_id>12</wp:post_id><wp:post_date>2019-05-04 10:00:00</wp:post_date><wp:post_name>hello</wp:post_name><wp:status>publish</wp:status><wp:post_type>post</wp:post_type><category domain="category" nicename="woodworking"><![CDATA[Woodworking]]></category><category domain="post_tag" nicename="oak"><![CDATA[oak]]></category></item>
<item><title>A page</title><wp:post_type>page</wp:post_type><wp:status>publish</wp:status><wp:post_date>2019-05-04 10:00:00</wp:post_date></item>
</channel></rss>"#;
        let dir = std::env::temp_dir().join(format!("gw-wxr-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("x.xml");
        std::fs::write(&f, xml).unwrap();
        let (host, posts) = read_wxr(&f).unwrap();
        assert_eq!(host, "myblog.example.com");
        assert_eq!(posts.len(), 1);
        let p = &posts[0];
        assert_eq!((p.id.as_str(), p.title.as_str(), p.slug.as_str()), ("12", "Hello & welcome", "hello"));
        assert_eq!(p.categories, vec!["Woodworking"]);
        assert_eq!(p.tags, vec!["oak"]);
        assert_eq!(p.date.to_string(), "2019-05-04");
    }
}
