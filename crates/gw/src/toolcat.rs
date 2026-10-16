//! The tool catalogue: what the site's browser tools are and how to link to them, read from
//! their AsciiDoc pages (`content/tools/*.adoc`, `:type: tool`). For each tool: slug, title,
//! summary, category, tags, the URL parameters from its "Linking to an example" table, the
//! `?example=` presets, the tabs (`#…`), and the example links on the page — so an assistant can
//! open a tool showing exactly what it means (MCP `tools_list`, `tool_info`, `tool_open`).

use anyhow::{Context, Result};
use regex::Regex;
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct Param {
    pub name: String,
    pub meaning: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub example: String,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct Preset {
    pub name: String,
    pub shows: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Tool {
    pub slug: String,
    pub title: String,
    pub summary: String,
    pub category: String,
    pub tags: Vec<String>,
    /// The page documents hand-written links (`:deeplink: true`).
    pub deeplink: bool,
    /// The tool talks to the network (some features need the companion's relays).
    pub network: bool,
    pub draft: bool,
    pub params: Vec<Param>,
    pub presets: Vec<Preset>,
    pub tabs: Vec<String>,
    /// Example links found on the page (site-relative, e.g. /tools/graph/?f=sin(x)).
    pub links: Vec<String>,
    pub path: String,
}

fn re(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).unwrap())
}

/// Plain text of a table cell or list item: `code` unquoted, link:url[text] → text.
fn plain(s: &str) -> String {
    static LINK: OnceLock<Regex> = OnceLock::new();
    let s = re(&LINK, r"link:([^\[\s]+)\[([^\]]*)\]").replace_all(s, |c: &regex::Captures| {
        let text = c[2].trim().trim_matches('`');
        if text.is_empty() || text == "open" { String::new() } else { text.to_string() }
    });
    let s = s.replace('`', "").replace("()", "");
    s.split_whitespace().collect::<Vec<_>>().join(" ").trim().to_string()
}

/// The cells of an AsciiDoc table body (`|===` … `|===`): cells start at `|` at the start of a
/// line or after whitespace.
fn cells(body: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur: Option<String> = None;
    let mut in_code = false;
    let chars: Vec<char> = body.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c == '`' {
            in_code = !in_code;
        }
        let at_start = i == 0 || chars[i - 1].is_whitespace();
        if c == '|' && at_start && !in_code {
            if let Some(x) = cur.take() {
                out.push(x.trim().to_string());
            }
            cur = Some(String::new());
            continue;
        }
        if c == '\n' {
            in_code = false;
        }
        if let Some(x) = cur.as_mut() {
            x.push(c);
        }
    }
    if let Some(x) = cur {
        out.push(x.trim().to_string());
    }
    out
}

/// Tables in a text: (column count, cells).
fn tables(text: &str) -> Vec<(usize, Vec<String>)> {
    static COLS: OnceLock<Regex> = OnceLock::new();
    let cols_re = re(&COLS, r#"cols="([^"]+)""#);
    let lines: Vec<&str> = text.lines().collect();
    let mut out = vec![];
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() == "|===" {
            let mut ncols = 0;
            if i > 0 {
                if let Some(c) = cols_re.captures(lines[i - 1]) {
                    ncols = c[1].split(',').count();
                }
            }
            let start = i + 1;
            let mut j = start;
            while j < lines.len() && lines[j].trim() != "|===" {
                j += 1;
            }
            let body = lines[start..j.min(lines.len())].join("\n");
            if ncols == 0 {
                // From the first line's cells.
                ncols = cells(lines.get(start).copied().unwrap_or("")).len().max(1);
            }
            out.push((ncols, cells(&body)));
            i = j + 1;
        } else {
            i += 1;
        }
    }
    out
}

/// The text of a `== Heading` section (up to the next level-2 heading).
fn section<'a>(body: &'a str, heading: &str) -> Option<&'a str> {
    let start = body.find(&format!("\n== {heading}"))? + 1;
    let rest = &body[start..];
    let after_title = rest.find('\n').map(|n| n + 1).unwrap_or(rest.len());
    let end = rest[after_title..].find("\n== ").map(|n| after_title + n).unwrap_or(rest.len());
    Some(&rest[..end])
}

/// Read a tool page.
pub fn parse(slug: &str, text: &str, path: &Path) -> Option<Tool> {
    let text = text.replace("\r\n", "\n");
    let mut lines = text.lines();
    let title = lines.next()?.strip_prefix("= ")?.trim().to_string();
    let mut attrs = std::collections::HashMap::new();
    let mut header_len = title.len() + 3;
    for l in lines.by_ref() {
        header_len += l.len() + 1;
        if l.trim().is_empty() {
            break;
        }
        if let Some(rest) = l.strip_prefix(':') {
            if let Some((k, v)) = rest.split_once(':') {
                attrs.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
    if attrs.get("type").map(String::as_str) != Some("tool") {
        return None;
    }
    let body = format!("\n{}", text.get(header_len.min(text.len())..).unwrap_or(""));
    let mut t = Tool {
        slug: slug.to_string(),
        title,
        summary: attrs.get("summary").cloned().unwrap_or_default(),
        category: attrs.get("category").cloned().unwrap_or_default(),
        tags: attrs.get("tags").map(|t| t.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()).unwrap_or_default(),
        deeplink: attrs.get("deeplink").map(|v| v == "true").unwrap_or(false),
        network: attrs.get("network").map(|v| v == "true").unwrap_or(false),
        draft: attrs.get("status").map(|v| v == "draft").unwrap_or(false),
        path: path.display().to_string(),
        ..Default::default()
    };
    let linking = section(&body, "Linking to an example");
    if let Some(sec) = linking {
        for (ncols, c) in tables(sec) {
            if c.is_empty() || ncols == 0 {
                continue;
            }
            let head = c[0].to_ascii_lowercase();
            let rows = c.chunks(ncols).skip(1);
            if head.contains("parameter") {
                for r in rows {
                    let name = plain(&r[0]);
                    if name.is_empty() {
                        continue;
                    }
                    let meaning = plain(r.get(1).map(String::as_str).unwrap_or(""));
                    let example = plain(r.get(2).map(String::as_str).unwrap_or(""));
                    // "`x`, `y`" and "`a`, `b` and `c`" document several parameters at once.
                    let names: Vec<String> = name.replace(" and ", ", ").split(", ").map(|n| n.trim().to_string()).collect();
                    if names.len() > 1 && names.iter().all(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')) {
                        for n in names {
                            t.params.push(Param { name: n, meaning: meaning.clone(), example: example.clone() });
                        }
                    } else {
                        t.params.push(Param { name, meaning, example });
                    }
                }
            } else if head.contains("example=") || head.contains("preset") || head.contains("scenario") {
                for r in rows {
                    let name = plain(&r[0]);
                    if !name.is_empty() {
                        t.presets.push(Preset { name, shows: plain(r.get(1).map(String::as_str).unwrap_or("")) });
                    }
                }
            }
        }
        // Presets as a list: "* link:/tools/x/?example=NAME[`NAME`] — what it shows".
        static ITEM: OnceLock<Regex> = OnceLock::new();
        let item = re(&ITEM, r"(?m)^\*+ .*?[?&]example=([A-Za-z0-9._-]+)[^\n]*?(?:—|–| - )\s*(.+)$");
        for c in item.captures_iter(sec) {
            let name = c[1].to_string();
            if !t.presets.iter().any(|p| p.name == name) {
                t.presets.push(Preset { name, shows: plain(&c[2]) });
            }
        }
    }
    // Tabs: "Add a tab after `#`: `sim` (the default), `keys` or `assign`."
    static TABS: OnceLock<Regex> = OnceLock::new();
    static CODE: OnceLock<Regex> = OnceLock::new();
    if let Some(m) = re(&TABS, r"(?s)tab after `?#`?[:,]?(.{0,400}?)\.(\s|$)").captures(&body) {
        for c in re(&CODE, r"`([A-Za-z0-9_-]+)`").captures_iter(&m[1]) {
            if !t.tabs.contains(&c[1].to_string()) {
                t.tabs.push(c[1].to_string());
            }
        }
    }
    // Parameters mentioned only in prose (`?dot=…&engine=neato`), when there is no table.
    static QP: OnceLock<Regex> = OnceLock::new();
    if t.params.is_empty() {
        let mut prose = section(&body, "Sharing").map(str::to_string).unwrap_or_default() + linking.unwrap_or("");
        if prose.trim().is_empty() {
            prose = body.clone();
        }
        for c in re(&QP, r"`[^`]*?[?&]([a-z][A-Za-z0-9_]*)=[^`]*`").captures_iter(&prose) {
            let n = c[1].to_string();
            if n != "s" && !t.params.iter().any(|p| p.name == n) {
                t.params.push(Param { name: n, meaning: "(mentioned in the page; see tool_info help)".into(), example: String::new() });
            }
        }
        // All names in a backticked query, e.g. `?dot=…&engine=neato`.
        static QALL: OnceLock<Regex> = OnceLock::new();
        static NAMES: OnceLock<Regex> = OnceLock::new();
        for q in re(&QALL, r"`\?([^`]+)`").captures_iter(&prose) {
            for n in re(&NAMES, r"(?:^|&)([a-z][A-Za-z0-9_]*)=").captures_iter(&q[1]) {
                let n = n[1].to_string();
                if n != "s" && !t.params.iter().any(|p| p.name == n) {
                    t.params.push(Param { name: n, meaning: "(mentioned in the page; see tool_info help)".into(), example: String::new() });
                }
            }
        }
    }
    // Example links to this tool anywhere on the page.
    static LINKS: OnceLock<Regex> = OnceLock::new();
    let own = format!("/tools/{slug}/");
    for c in re(&LINKS, r"link:(/tools/[a-z0-9_-]+/[^\[\s]*)\[").captures_iter(&body) {
        let l = c[1].to_string();
        if l.starts_with(&own) && l.len() > own.len() && !t.links.contains(&l) && t.links.len() < 12 {
            t.links.push(l);
        }
    }
    // Parameters used in the page's own example links but not documented in a table.
    static LQ: OnceLock<Regex> = OnceLock::new();
    for l in t.links.clone() {
        if let Some(q) = l.split_once('?').map(|(_, q)| q.split('#').next().unwrap_or("")) {
            for n in re(&LQ, r"(?:^|&)([a-z][A-Za-z0-9_]*)=").captures_iter(q) {
                let n = n[1].to_string();
                let known = (n.len() == 1 && t.params.iter().any(|p| p.name.contains('…'))) || t.params.iter().any(|p| p.name == n || p.name.split(|c: char| !c.is_ascii_alphanumeric() && c != '_').any(|x| x == n));
                if n != "s" && !known {
                    t.params.push(Param { name: n, meaning: "(used in an example link on the page)".into(), example: String::new() });
                }
            }
        }
    }
    Some(t)
}

pub fn tools_dir(site_root: &Path) -> PathBuf {
    site_root.join("content").join("tools")
}

/// Every tool page of the site, sorted by slug.
pub fn load(site_root: &Path) -> Result<Vec<Tool>> {
    let dir = tools_dir(site_root);
    let mut out = vec![];
    let rd = std::fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))?;
    for e in rd.flatten() {
        let p = e.path();
        let (slug, file) = if p.is_dir() {
            (e.file_name().to_string_lossy().to_string(), p.join("index.adoc"))
        } else if p.extension().and_then(|x| x.to_str()) == Some("adoc") {
            (p.file_stem().unwrap_or_default().to_string_lossy().to_string(), p.clone())
        } else {
            continue;
        };
        if slug == "index" || slug.starts_with('_') {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        if let Some(t) = parse(&slug, &text, &file) {
            out.push(t);
        }
    }
    out.sort_by(|a, b| a.slug.cmp(&b.slug));
    Ok(out)
}

/// One tool, with the page's text (help) when asked.
pub fn info(site_root: &Path, slug: &str) -> Result<Value> {
    let tools = load(site_root)?;
    let t = tools.iter().find(|t| t.slug == slug).with_context(|| {
        let near: Vec<&str> = tools.iter().filter(|t| t.slug.contains(slug) || slug.contains(&t.slug)).map(|t| t.slug.as_str()).take(8).collect();
        format!("no tool {slug:?}{}", if near.is_empty() { String::new() } else { format!(" (did you mean {}?)", near.join(", ")) })
    })?;
    let text = std::fs::read_to_string(&t.path).unwrap_or_default();
    let help = text.split_once("\n\n").map(|(_, b)| b).unwrap_or("");
    let help: String = help.chars().take(16_000).collect();
    let mut v = serde_json::to_value(t)?;
    v["url"] = json!(format!("/tools/{}/", t.slug));
    v["help"] = json!(help);
    Ok(v)
}

/// The compact list for `tools_list` (optionally filtered by category or a search).
pub fn list(site_root: &Path, category: Option<&str>, query: Option<&str>) -> Result<Value> {
    let q = query.map(|q| q.to_lowercase());
    let tools: Vec<Value> = load(site_root)?
        .into_iter()
        .filter(|t| category.is_none_or(|c| t.category == c))
        .filter(|t| {
            q.as_ref().is_none_or(|q| {
                let hay = format!("{} {} {} {}", t.slug, t.title, t.summary, t.tags.join(" ")).to_lowercase();
                q.split_whitespace().all(|w| hay.contains(w))
            })
        })
        .map(|t| {
            json!({
                "slug": t.slug, "title": t.title, "category": t.category, "summary": t.summary, "tags": t.tags,
                "params": t.params.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
                "presets": t.presets.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
                "tabs": t.tabs, "network": t.network,
            })
        })
        .collect();
    Ok(json!({ "count": tools.len(), "tools": tools }))
}

fn cut(s: &str, n: usize) -> String {
    let s = s.replace('|', "/");
    if s.chars().count() <= n {
        s
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>().trim_end())
    }
}

/// `tools-index.md` for the show-with-tools skill: one line per tool, grouped by category.
pub fn index_markdown(site_root: &Path, base_url: &str) -> Result<String> {
    let tools = load(site_root)?;
    let mut cats: Vec<(String, String)> = vec![];
    if let Ok(site) = gw_core::Site::open(Some(site_root)) {
        if let Some(c) = serde_json::to_value(&site.config).ok().and_then(|v| v["tools"]["categories"].as_array().cloned()) {
            for x in c {
                cats.push((x["id"].as_str().unwrap_or("").into(), x["title"].as_str().unwrap_or("").into()));
            }
        }
    }
    for t in &tools {
        if !cats.iter().any(|(id, _)| *id == t.category) {
            cats.push((t.category.clone(), t.category.clone()));
        }
    }
    let mut md = String::new();
    md.push_str("# Which tool for which rendering\n\n");
    md.push_str(&format!(
        "Generated by `gw tools index` from the site's tool pages ({} tools). Do not edit by hand — regenerate.\n\
         Columns: slug · what it shows · URL parameters (exact names; get meanings with `tool_info`) · presets (`example=`).\n\
         Public URL: {}/tools/<slug>/\n",
        tools.len(),
        base_url.trim_end_matches('/')
    ));
    for (id, title) in cats {
        let in_cat: Vec<&Tool> = tools.iter().filter(|t| t.category == id && !t.draft).collect();
        if in_cat.is_empty() {
            continue;
        }
        md.push_str(&format!("\n## {title}\n\n| tool | shows | params | presets |\n|---|---|---|---|\n"));
        for t in in_cat {
            let params: Vec<&str> = t.params.iter().map(|p| p.name.as_str()).collect();
            let presets: Vec<&str> = t.presets.iter().map(|p| p.name.as_str()).collect();
            let mut pre = presets.iter().take(8).cloned().collect::<Vec<_>>().join(", ");
            if presets.len() > 8 {
                pre.push_str(&format!(" +{}", presets.len() - 8));
            }
            md.push_str(&format!(
                "| `{}` | {} | {} | {} |\n",
                t.slug,
                cut(&format!("{} — {}", t.title, t.summary), 150),
                cut(&params.join(" "), 90),
                cut(&pre, 120)
            ));
        }
    }
    Ok(md)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "= Kafka simulator\n:type: tool\n:date: 2026-10-01\n:category: infra\n:tags: kafka, streaming\n:deeplink: true\n:summary: Partitions and consumer groups.\n\nIntro.\n\n== Linking to an example\n\nLinks can be written by hand. Add a tab after `#`: `sim` (the default), `keys` or\n`throughput`. More text.\n\n[cols=\"1,3,2\"]\n|===\n| Parameter | Meaning | Example\n\n| `example` | A scenario | `example=unclean`\n| `acks` | `0`, `1` or `all` | `acks=all`\n| `re` | a regex like `a|b` | `re=a%7Cb`\n|===\n\nScenarios:\n\n[cols=\"2,4\"]\n|===\n| `example=` | Shows\n\n| `unclean` | Unclean leader election (link:/tools/kafka-sim/?example=unclean[open])\n| `rebalance` | A rebalance\n|===\n\n== Sharing\n\nShare.\n";

    #[test]
    fn parses_tables_tabs_and_links() {
        let t = parse("kafka-sim", SAMPLE, Path::new("x.adoc")).unwrap();
        assert_eq!(t.title, "Kafka simulator");
        assert_eq!(t.category, "infra");
        assert_eq!(t.params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), vec!["example", "acks", "re"]);
        assert_eq!(t.params[2].meaning, "a regex like a|b");
        assert_eq!(t.params[1].example, "acks=all");
        assert_eq!(t.presets, vec![Preset { name: "unclean".into(), shows: "Unclean leader election".into() }, Preset { name: "rebalance".into(), shows: "A rebalance".into() }]);
        assert_eq!(t.tabs, vec!["sim", "keys", "throughput"]);
        assert_eq!(t.links, vec!["/tools/kafka-sim/?example=unclean"]);
        assert!(parse("x", "= X\n:type: article\n\nbody", Path::new("x")).is_none());
    }

    /// The real site's tool pages, when the checkout is next to this one.
    fn real_site() -> Option<PathBuf> {
        let candidates = [
            std::env::var("GW_TEST_SITE").unwrap_or_default(),
            "../../../geekychris-site/site".into(),
            format!("{}/code/claude_world/geekychris-site/site", std::env::var("HOME").unwrap_or_default()),
        ];
        candidates.iter().map(PathBuf::from).find(|p| !p.as_os_str().is_empty() && p.join("content/tools/redis.adoc").exists())
    }

    #[test]
    fn real_tool_pages() {
        let Some(site) = real_site() else {
            eprintln!("skipping: no site checkout (set GW_TEST_SITE)");
            return;
        };
        let tools = load(&site).unwrap();
        assert!(tools.len() > 100, "{}", tools.len());
        let get = |s: &str| tools.iter().find(|t| t.slug == s).cloned().unwrap();
        let redis = get("redis");
        assert!(redis.params.iter().any(|p| p.name == "example") && redis.params.iter().any(|p| p.name == "cmd"));
        assert!(redis.presets.iter().any(|p| p.name == "leaderboard"));
        assert!(redis.tabs.contains(&"console".to_string()));
        let graph = get("graph");
        assert!(graph.params.iter().any(|p| p.name == "f"));
        assert!(graph.presets.iter().any(|p| p.name == "ch05-slope-intercept"), "{:?}", graph.presets);
        let k8s = get("k8s-sim");
        assert!(k8s.presets.iter().any(|p| p.name == "rolling-update"));
        assert!(k8s.tabs.contains(&"formulas".to_string()));
        let diagrams = get("diagrams");
        assert!(diagrams.params.iter().any(|p| p.name == "dot") && diagrams.params.iter().any(|p| p.name == "mmd"), "{:?}", diagrams.params);
        let md = index_markdown(&site, "https://geekychris.github.io").unwrap();
        assert!(md.contains("| `kafka-sim` |"));
        let i = info(&site, "redis").unwrap();
        assert!(i["help"].as_str().unwrap().contains("redis-cli"));
        assert!(info(&site, "nope-nope").is_err());
    }
}
