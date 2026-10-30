//! Live tool views in any page: the `tool::` block macro.
//!
//! ```asciidoc
//! tool::kafka-sim[query="example=unclean", tab=sim, height=560, caption="Kill the leader and watch"]
//! tool::graph[query="f=sin(x)*x", caption=The curve the fit produces]
//! ```
//!
//! Before rendering, [`expand`] replaces each macro line with a passthrough block: a lazy-loaded
//! iframe of the tool in viewer mode (`/tools/<slug>/?bare=1&embed=1&<query>#<tab>` — no site header,
//! footer or help), a "Loading…" placeholder behind it until it paints, the caption, and an
//! *Open full tool →* link to the normal page. The frame's styles come with the first view on a
//! page, so it needs nothing from the site's stylesheet beyond its colour variables.
//!
//! Attributes: `query` (URL parameters, `?` optional), `tab` (the `#` part), `height` (px, default
//! [`DEFAULT_HEIGHT`]; phones cap it at 75% of the screen), `caption` (or the first positional
//! attribute). `gw check` reports an unknown tool and an `example=` the tool does not declare as
//! errors ([`problems`]).

use crate::model::{esc, macro_lines, parse_attrs};
use crate::toolcat::Tool;
use regex::Regex;
use std::collections::BTreeMap;
use std::sync::OnceLock;

pub const DEFAULT_HEIGHT: u32 = 520;
const NAMES: &[&str] = &["query", "tab", "height", "caption"];

/// One `tool::` line in a page.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Block {
    /// 1-based line number in the file.
    pub line: usize,
    pub slug: String,
    /// URL parameters without the leading `?`.
    pub query: String,
    /// The tab, without the `#`.
    pub tab: String,
    pub height: u32,
    pub caption: String,
    /// Bad attribute values and unknown attribute names (warnings).
    pub notes: Vec<String>,
}

impl Block {
    /// The address the frame loads: viewer mode (`bare=1`) and embedded (`embed=1`: the tool keeps
    /// its settings in memory only, never the reader's saved state, so the frame shows exactly its
    /// query or the defaults).
    pub fn frame_url(&self) -> String {
        let q = if self.query.is_empty() { String::new() } else { format!("&{}", self.query) };
        format!("/tools/{}/?bare=1&embed=1{q}{}", self.slug, self.hash())
    }
    /// The tool's normal page, showing the same thing.
    pub fn page_url(&self) -> String {
        let q = if self.query.is_empty() { String::new() } else { format!("?{}", self.query) };
        format!("/tools/{}/{q}{}", self.slug, self.hash())
    }
    fn hash(&self) -> String {
        if self.tab.is_empty() { String::new() } else { format!("#{}", self.tab) }
    }
    /// The value of a URL parameter in `query`.
    pub fn param(&self, name: &str) -> Option<&str> {
        self.query.split('&').filter_map(|kv| kv.split_once('=')).find(|(k, _)| *k == name).map(|(_, v)| v)
    }
}

fn macro_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^tool::([a-z0-9][a-z0-9_-]*)\[(.*)\]\s*$").unwrap())
}

fn block(line: usize, slug: &str, attrs: &str) -> Block {
    let (pos, named) = parse_attrs(attrs);
    let mut notes = vec![];
    for k in named.keys().filter(|k| !NAMES.contains(&k.as_str())) {
        notes.push(format!("unknown attribute {k:?} (use {})", NAMES.join(", ")));
    }
    let height = match named.get("height").map(|h| h.trim().trim_end_matches("px").trim()) {
        None | Some("") => DEFAULT_HEIGHT,
        Some(h) => match h.parse::<u32>() {
            Ok(n) if (120..=2000).contains(&n) => n,
            _ => {
                notes.push(format!("height={h:?} wants a number of pixels between 120 and 2000"));
                DEFAULT_HEIGHT
            }
        },
    };
    // Passed through as written (already URL-encoded; never re-encoded or shortened), less the
    // `bare=`/`embed=` the frame adds itself.
    let query = named.get("query").map(|q| q.trim().trim_start_matches('?').trim_matches('&').to_string()).unwrap_or_default();
    let ours = |kv: &str| kv.starts_with("bare=") || kv.starts_with("embed=") || kv == "bare" || kv == "embed";
    if query.split('&').any(ours) {
        notes.push("query: leave out bare= and embed= (the frame adds them)".into());
    }
    Block {
        line,
        slug: slug.to_string(),
        query: query.split('&').filter(|kv| !kv.is_empty() && !ours(kv)).collect::<Vec<_>>().join("&"),
        tab: named.get("tab").map(|t| t.trim().trim_start_matches('#').to_string()).unwrap_or_default(),
        height,
        caption: named.get("caption").cloned().or_else(|| pos.first().cloned()).unwrap_or_default().trim().to_string(),
        notes,
    }
}

/// The page's `tool::` lines, in order (verbatim blocks skipped, as [`expand`] does).
pub fn blocks(text: &str) -> Vec<Block> {
    let mut out = vec![];
    for (n, (line, live)) in macro_lines(text).into_iter().enumerate() {
        let bare = line.trim_end_matches(['\n', '\r']);
        if let Some(c) = live.then(|| macro_re().captures(bare)).flatten() {
            out.push(block(n + 1, &c[1], &c[2]));
        }
    }
    out
}

/// The tools a page shows, each once, in the order they first appear.
pub fn slugs(text: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for b in blocks(text) {
        if !out.contains(&b.slug) {
            out.push(b.slug);
        }
    }
    out
}

const STYLE: &str = "<style>\
.tool-embed{margin:1.6rem 0}\
.tool-embed .te-frame{position:relative;height:var(--te-h,520px);max-height:90vh;border:1px solid var(--line,#d9d6d0);border-radius:10px;overflow:hidden;background:var(--wash,#f3efe9)}\
@media (max-width:640px){.tool-embed .te-frame{height:min(var(--te-h,520px),75vh);border-radius:8px}}\
.tool-embed iframe{position:absolute;inset:0;width:100%;height:100%;border:0;background:transparent}\
.tool-embed .te-loading{position:absolute;inset:0;margin:0;display:grid;place-items:center;color:var(--faint,#8a9096);font:.9rem var(--sans,system-ui,sans-serif)}\
.tool-embed .te-ready .te-loading{visibility:hidden}\
.tool-embed figcaption{display:flex;flex-wrap:wrap;justify-content:space-between;align-items:baseline;gap:.25rem 1rem;margin-top:.45rem;font-size:.86rem;color:var(--muted,#5f646b)}\
.tool-embed .te-open{white-space:nowrap;font-weight:600;margin-left:auto}\
.tool-embed-missing{padding:.8rem 1rem;border:1px dashed var(--line,#d9d6d0);border-radius:8px;color:var(--muted,#5f646b)}\
</style>";

/// The frame for one block; `title` is the tool's title.
pub fn figure(b: &Block, title: &str, with_style: bool) -> String {
    let cap = if b.caption.is_empty() { format!("<span class=\"te-cap\">{}</span>", esc(title)) } else { format!("<span class=\"te-cap\">{}</span>", esc(&b.caption)) };
    format!(
        "{style}<figure class=\"tool-embed\" data-tool=\"{slug}\">\n<div class=\"te-frame\" style=\"--te-h:{h}px\">\
<p class=\"te-loading\" aria-hidden=\"true\">Loading {t}…</p>\
<iframe src=\"{src}\" title=\"{t} (live tool)\" loading=\"lazy\" allow=\"clipboard-write; fullscreen\" onload=\"this.parentNode.classList.add('te-ready')\"></iframe></div>\n\
<figcaption>{cap}<a class=\"te-open\" href=\"{page}\">Open full tool →</a></figcaption>\n</figure>",
        style = if with_style { STYLE } else { "" },
        slug = esc(&b.slug),
        h = b.height,
        t = esc(title),
        src = esc(&b.frame_url()),
        page = esc(&b.page_url()),
    )
}

/// Replace every `tool::…[…]` line (outside listing/literal/comment/passthrough blocks) with its
/// frame. `title_of` gives a tool's title, or `None` when the tool is not in this build: then the
/// page shows a notice and the problem is returned.
pub fn expand(text: &str, title_of: impl Fn(&str) -> Option<String>) -> (String, Vec<String>) {
    let mut lines: Vec<String> = vec![];
    let mut problems = vec![];
    let mut styled = false;
    for (n, (line, live)) in macro_lines(text).into_iter().enumerate() {
        let bare = line.trim_end_matches(['\n', '\r']);
        let Some(c) = live.then(|| macro_re().captures(bare)).flatten() else {
            lines.push(line.to_string());
            continue;
        };
        let b = block(n + 1, &c[1], &c[2]);
        let html = match title_of(&b.slug) {
            Some(t) => figure(&b, &t, !styled),
            None => {
                problems.push(format!("tool::{}: no such tool in this build", b.slug));
                format!("{}<p class=\"tool-embed-missing\">Tool unavailable: {}</p>", if styled { "" } else { STYLE }, esc(&b.slug))
            }
        };
        styled = true;
        lines.push(format!("++++\n{html}\n++++\n"));
    }
    (lines.concat(), problems)
}

/// `gw check` for a page's `tool::` blocks against the site's tools (by slug): an unknown tool, or
/// an `example=` that a tool declaring presets does not have, is an error; a bad attribute or a
/// tab the tool does not list is a warning. Returns (errors, warnings), each prefixed `tool::<slug>`.
pub fn problems(text: &str, tools: &BTreeMap<String, Tool>) -> (Vec<String>, Vec<String>) {
    let (mut errors, mut warnings) = (vec![], vec![]);
    for b in blocks(text) {
        let at = format!("line {}: tool::{}", b.line, b.slug);
        warnings.extend(b.notes.iter().map(|n| format!("{at}: {n}")));
        let Some(t) = tools.get(&b.slug) else {
            let near: Vec<&str> = tools.keys().filter(|s| s.contains(&b.slug) || b.slug.contains(s.as_str())).map(String::as_str).take(5).collect();
            errors.push(format!("{at}: no tool {:?} (content/tools/{}.adoc){}", b.slug, b.slug, if near.is_empty() { String::new() } else { format!(" — did you mean {}?", near.join(", ")) }));
            continue;
        };
        if let Some(ex) = b.param("example") {
            if !t.presets.is_empty() && !t.presets.iter().any(|p| p.name == ex) {
                let names: Vec<&str> = t.presets.iter().map(|p| p.name.as_str()).collect();
                errors.push(format!("{at}: {:?} has no example {ex:?} (it has {})", b.slug, names.join(", ")));
            }
        }
        if !b.tab.is_empty() && !t.tabs.is_empty() && !t.tabs.contains(&b.tab) {
            warnings.push(format!("{at}: tab {:?} is not one the tool lists ({})", b.tab, t.tabs.join(", ")));
        }
    }
    (errors, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::toolcat::Preset;

    #[test]
    fn parses_attributes() {
        let b = &blocks("Intro.\n\ntool::kafka-sim[query=\"?example=wal&acks=all\", tab=#sim, height=600px, caption=\"Kill it, then watch\"]\n")[0];
        assert_eq!((b.line, b.slug.as_str(), b.query.as_str(), b.tab.as_str(), b.height), (3, "kafka-sim", "example=wal&acks=all", "sim", 600));
        assert_eq!(b.caption, "Kill it, then watch");
        assert_eq!(b.frame_url(), "/tools/kafka-sim/?bare=1&embed=1&example=wal&acks=all#sim");
        assert_eq!(b.page_url(), "/tools/kafka-sim/?example=wal&acks=all#sim");
        assert_eq!(b.param("example"), Some("wal"));
        assert!(b.notes.is_empty());
        let d = &blocks("tool::graph[A caption]\n")[0];
        assert_eq!((d.height, d.caption.as_str(), d.frame_url().as_str(), d.page_url().as_str()), (DEFAULT_HEIGHT, "A caption", "/tools/graph/?bare=1&embed=1", "/tools/graph/"));
        let bad = &blocks("tool::graph[height=tall, size=3, query=bare=1&f=x&embed=1]\n")[0];
        assert_eq!(bad.height, DEFAULT_HEIGHT);
        assert_eq!(bad.query, "f=x");
        assert_eq!(bad.notes.len(), 3, "{:?}", bad.notes);
        // Not inside listings, and only whole lines.
        assert!(blocks("----\ntool::graph[]\n----\nsee tool::graph[] inline\n").is_empty());
    }

    #[test]
    fn long_encoded_queries_pass_through() {
        // ~2.4 KB of URL-encoded source with %, &, = and + — kept byte for byte.
        let chunk = "src=fn%20main()%20%7B%0A%20%20let%20x%20%3D%201%2B2%3B%20%2F%2F%20a%26b%3Dc%0A%7D&lang=rust+2024&";
        let q = chunk.repeat(2400 / chunk.len() + 1);
        let q = q.trim_end_matches('&');
        assert!(q.len() > 2400);
        let b = &blocks(&format!("tool::sandbox[query=\"{q}\", tab=run]\n"))[0];
        assert_eq!(b.query, q);
        assert_eq!(b.frame_url(), format!("/tools/sandbox/?bare=1&embed=1&{q}#run"));
        assert_eq!(b.page_url(), format!("/tools/sandbox/?{q}#run"));
        let (t, _) = expand(&format!("tool::sandbox[query=\"{q}\"]\n"), |_| Some("Sandbox".into()));
        // In HTML only `&` is escaped; the percent-encoding is untouched (not double-encoded).
        let attr = q.replace('&', "&amp;");
        assert!(t.contains(&format!("src=\"/tools/sandbox/?bare=1&amp;embed=1&amp;{attr}\"")));
        assert!(t.contains(&format!("href=\"/tools/sandbox/?{attr}\"")));
        assert!(!t.contains("%25"));
    }

    #[test]
    fn expands_to_a_lazy_frame() {
        let (t, p) = expand("A\n\ntool::graph[query=\"f=x<2\", caption=\"Plot <it>\"]\n\ntool::graph[]\n\ntool::nope[]\n", |s| (s == "graph").then(|| "Function grapher".to_string()));
        assert_eq!(p, vec!["tool::nope: no such tool in this build".to_string()]);
        assert_eq!(t.matches("<style>").count(), 1, "{t}");
        assert_eq!(t.matches("++++\n").count(), 6);
        assert!(t.contains("<iframe src=\"/tools/graph/?bare=1&amp;embed=1&amp;f=x&lt;2\" title=\"Function grapher (live tool)\" loading=\"lazy\""), "{t}");
        assert!(t.contains("style=\"--te-h:520px\""));
        assert!(t.contains("Loading Function grapher…"));
        assert!(t.contains("<span class=\"te-cap\">Plot &lt;it&gt;</span><a class=\"te-open\" href=\"/tools/graph/?f=x&lt;2\">Open full tool →</a>"), "{t}");
        // Without a caption the tool's title is the caption.
        assert!(t.contains("<span class=\"te-cap\">Function grapher</span>"));
        assert!(t.contains("Tool unavailable: nope"));
        assert_eq!(slugs("tool::b[]\ntool::a[]\ntool::b[]\n"), vec!["b", "a"]);
    }

    #[test]
    fn check_reports_unknown_tools_examples_and_tabs() {
        let mut tools = BTreeMap::new();
        tools.insert("kafka-sim".to_string(), Tool { slug: "kafka-sim".into(), presets: vec![Preset { name: "wal".into(), shows: String::new() }], tabs: vec!["sim".into()], ..Default::default() });
        tools.insert("graph".to_string(), Tool { slug: "graph".into(), ..Default::default() });
        let text = "tool::kafka-sim[query=example=wal, tab=sim]\ntool::kafka-sim[query=example=nope]\ntool::kafka[]\ntool::graph[query=example=anything, tab=x]\ntool::kafka-sim[tab=keys, width=3]\n";
        let (e, w) = problems(text, &tools);
        assert_eq!(e.len(), 2, "{e:?}");
        assert!(e[0].starts_with("line 2: tool::kafka-sim") && e[0].contains("no example \"nope\" (it has wal)"), "{e:?}");
        assert!(e[1].contains("no tool \"kafka\"") && e[1].contains("did you mean kafka-sim"), "{e:?}");
        assert_eq!(w.len(), 2, "{w:?}");
        assert!(w.iter().any(|x| x.contains("unknown attribute \"width\"")));
        assert!(w.iter().any(|x| x.contains("tab \"keys\"")));
    }
}
