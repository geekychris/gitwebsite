//! 3-D models in content: the `model::` block macro.
//!
//! ```asciidoc
//! model::media:wizard-lemming[Wizard lemming, download=true, ar=true, autorotate=true, height=480]
//! model::story:img4[]            (in a story draft; `gw story draft` turns it into media:<key>)
//! model::wizard.glb[poster=wizard.webp, ios=wizard.usdz]   (a file beside the post)
//! model::/models/x/x.glb[]       (a site path) — or an https:// URL
//! ```
//!
//! Asciidoctor knows nothing of models, so before rendering, [`expand`] replaces each macro line
//! with a passthrough block holding a `<model-viewer>` web component (loaded from a CDN as an ES
//! module, once, on pages that use it): drag to rotate, auto-rotate, a poster until it loads
//! (and the only thing shown without JavaScript), AR on phones (Quick Look with the USDZ on iOS,
//! Scene Viewer on Android, WebXR), a caption with the printed size, and a download link for the
//! print file with its size.
//!
//! Attributes: the first positional one is the caption (default: the library entry's caption);
//! `alt=`, `download=true|false|<path>` (default: true when the entry has a download),
//! `ar=true|false` (true), `autorotate=true|false` (true), `height=` (`480`, `480px`, `30rem`,
//! `60vh`), `poster=`, `ios=` (direct paths), `reveal=auto|interaction` (auto: loads when it
//! scrolls near; interaction: waits for a click), `orbit=` (model-viewer `camera-orbit`).

use crate::media::{Library, Media};
use regex::Regex;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

/// The model-viewer web component (Google, Apache-2.0), major version 4.
pub const VIEWER_JS: &str = "https://cdn.jsdelivr.net/npm/@google/model-viewer@4/dist/model-viewer.min.js";

/// What [`expand`] needs to know beyond the text.
#[derive(Default)]
pub struct Env<'a> {
    pub lib: Option<&'a Library>,
    /// The post's bundle folder (for files beside it: sizes, a USDZ/poster with the same name).
    pub bundle_dir: Option<&'a Path>,
    /// The site's `static/` folder (for `/models/...` site paths).
    pub static_dir: Option<&'a Path>,
    /// Plain HTML only (poster image + links), for places that run no scripts (WordPress export).
    pub static_html: bool,
}

#[derive(Debug, Default)]
pub struct Expanded {
    pub text: String,
    /// Models rendered.
    pub count: usize,
    /// Broken references and bad attributes, for `gw check` and build warnings. A broken model
    /// renders as a visible notice instead of failing the build.
    pub problems: Vec<String>,
}

fn macro_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^model::([^\s\[\]]+)\[(.*)\]\s*$").unwrap())
}

/// Delimiters of blocks whose contents are not AsciiDoc (listing, literal, comment, passthrough).
fn verbatim_delimiter(line: &str) -> Option<&str> {
    let t = line.trim_end();
    if t.len() >= 4 && ["-", ".", "/", "+"].iter().any(|c| t.chars().all(|x| x.to_string() == *c)) {
        return Some(t);
    }
    if t.starts_with("```") {
        return Some("```");
    }
    None
}

/// The lines of `text` (with their line endings) and, for each, whether it is AsciiDoc a block
/// macro may stand on — false inside listing/literal/comment/passthrough blocks and on their
/// delimiters.
pub fn macro_lines(text: &str) -> Vec<(&str, bool)> {
    let mut out = vec![];
    let mut open: Option<String> = None;
    for line in text.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\n', '\r']);
        if let Some(d) = &open {
            if bare.trim_end() == d {
                open = None;
            }
            out.push((line, false));
            continue;
        }
        if let Some(d) = verbatim_delimiter(bare) {
            open = Some(d.to_string());
            out.push((line, false));
            continue;
        }
        out.push((line, true));
    }
    out
}

/// Replace every `model::…[…]` line (outside listing/literal/comment/passthrough blocks).
pub fn expand(text: &str, env: &Env) -> Expanded {
    let mut out = Expanded::default();
    let mut lines: Vec<String> = Vec::new();
    for (line, live) in macro_lines(text) {
        let bare = line.trim_end_matches(['\n', '\r']);
        let Some(c) = live.then(|| macro_re().captures(bare)).flatten() else {
            lines.push(line.to_string());
            continue;
        };
        let (pos, named) = parse_attrs(&c[2]);
        let html = match figure(&c[1], &pos, &named, env, out.count == 0) {
            Ok(h) => {
                out.count += 1;
                h
            }
            Err(e) => {
                out.problems.push(format!("model::{}: {e}", &c[1]));
                format!("<p class=\"model-missing\">3-D model unavailable: {}</p>", esc(&c[1]))
            }
        };
        lines.push(format!("++++\n{html}\n++++\n"));
    }
    out.text = lines.concat();
    out
}

// ------------------------------------------------------------------ editing a post's models

/// One `model::` line in a post, as the editor and `gw models in` list them.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Embed {
    /// 0-based among the post's models, in order.
    pub index: usize,
    /// 1-based line number in the file.
    pub line: usize,
    /// `media:<key>`, a path or a URL.
    pub target: String,
    /// The media key, for `media:<key>` targets.
    pub key: Option<String>,
    pub caption: String,
    /// `download=` as written (absent: the entry's default).
    pub download: Option<String>,
    /// The attribute list as written (between the brackets).
    pub attrs: String,
}

/// The post's `model::` lines, in order (verbatim blocks skipped, as [`expand`] does).
pub fn embeds(text: &str) -> Vec<Embed> {
    let mut out = vec![];
    for (n, (line, live)) in macro_lines(text).into_iter().enumerate() {
        let bare = line.trim_end_matches(['\n', '\r']);
        let Some(c) = live.then(|| macro_re().captures(bare)).flatten() else { continue };
        let (pos, named) = parse_attrs(&c[2]);
        out.push(Embed {
            index: out.len(),
            line: n + 1,
            target: c[1].to_string(),
            key: c[1].strip_prefix("media:").map(str::to_string),
            caption: pos.first().cloned().or_else(|| named.get("caption").cloned()).unwrap_or_default(),
            download: named.get("download").cloned(),
            attrs: c[2].to_string(),
        });
    }
    out
}

/// A caption as a positional attribute: quoted when it holds a comma, a quote or `=`, and never
/// with a `]` (which would end the macro).
fn caption_attr(caption: &str) -> String {
    let c = caption.trim().replace(']', ")").replace(['\n', '\r'], " ");
    if c.contains([',', '=', '"']) || c.starts_with('\'') {
        format!("\"{}\"", c.replace('"', "'"))
    } else {
        c
    }
}

/// `model::media:<key>[Caption, download=false]`. `download`: `None` leaves the entry's default.
pub fn macro_line(key: &str, caption: &str, download: Option<bool>) -> String {
    let mut attrs = vec![];
    if !caption.trim().is_empty() {
        attrs.push(caption_attr(caption));
    }
    if let Some(d) = download {
        attrs.push(format!("download={d}"));
    }
    format!("model::media:{key}[{}]", attrs.join(", "))
}

/// Where [`insert`] puts a new model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Place {
    /// At the end of the post.
    End,
    /// After this 1-based line (the header's lines move it past the header).
    After(usize),
}

/// The 1-based line of the first body line: after `= Title` and the attribute lines.
fn body_start(lines: &[&str]) -> usize {
    let mut i = 0;
    while i < lines.len() && lines[i].trim().is_empty() {
        i += 1;
    }
    if i < lines.len() && lines[i].starts_with("= ") {
        i += 1;
        while i < lines.len() && !lines[i].trim().is_empty() {
            i += 1;
        }
    }
    i
}

/// Put `macro_text` (a `model::…[]` line) in the post as its own block: a blank line before and
/// after it. After a line inside a verbatim block, it goes after that block instead.
pub fn insert(text: &str, macro_text: &str, place: Place) -> String {
    let ml = macro_lines(text);
    let lines: Vec<&str> = ml.iter().map(|(l, _)| l.trim_end_matches(['\n', '\r'])).collect();
    let start = body_start(&lines);
    let mut at = match place {
        Place::End => lines.len(),
        Place::After(n) => n.clamp(start, lines.len()),
    };
    // Not inside a verbatim block (or between its delimiters).
    while at < lines.len() && at > 0 && !ml[at].1 {
        at += 1;
    }
    // Not in the middle of a paragraph: move to the end of it.
    while at < lines.len() && at > start && !lines[at - 1].trim().is_empty() && !lines[at].trim().is_empty() {
        at += 1;
    }
    let mut out: Vec<String> = lines[..at].iter().map(|s| s.to_string()).collect();
    while out.len() > start && out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    out.push(String::new());
    out.push(macro_text.trim().to_string());
    let rest: Vec<&str> = lines[at..].iter().copied().skip_while(|l| l.trim().is_empty()).collect();
    if !rest.is_empty() {
        out.push(String::new());
        out.extend(rest.iter().map(|s| s.to_string()));
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

/// Take out the model at `index` (see [`embeds`]) and the blank line it leaves behind.
pub fn remove(text: &str, index: usize) -> anyhow::Result<String> {
    let e = embeds(text).into_iter().find(|e| e.index == index).ok_or_else(|| anyhow::anyhow!("the post has no model #{}", index + 1))?;
    let mut lines: Vec<&str> = text.split_inclusive('\n').collect();
    let i = e.line - 1;
    lines.remove(i);
    // Two blank lines meet (or the file now ends in one): drop one.
    let blank = |l: Option<&&str>| l.is_some_and(|l| l.trim().is_empty());
    if i > 0 && blank(lines.get(i - 1)) && (blank(lines.get(i)) || i == lines.len()) {
        lines.remove(i - 1);
    }
    Ok(lines.concat())
}

/// Rewrite the model at `index` with a new caption and/or download setting (other attributes
/// are kept). `download`: `Some(None)` removes `download=`.
pub fn update(text: &str, index: usize, caption: Option<&str>, download: Option<Option<bool>>) -> anyhow::Result<String> {
    let e = embeds(text).into_iter().find(|e| e.index == index).ok_or_else(|| anyhow::anyhow!("the post has no model #{}", index + 1))?;
    let (pos, mut named) = parse_attrs(&e.attrs);
    let caption = caption.map(str::to_string).unwrap_or_else(|| pos.first().cloned().unwrap_or_default());
    named.remove("caption");
    match download {
        Some(Some(d)) => {
            named.insert("download".into(), d.to_string());
        }
        Some(None) => {
            named.remove("download");
        }
        None => {}
    }
    let mut attrs = vec![];
    if !caption.trim().is_empty() {
        attrs.push(caption_attr(&caption));
    }
    // Further positional attributes are not used by the macro; named ones are kept as written.
    for (k, v) in &named {
        let v = if v.contains([',', ' ', '"']) { format!("\"{}\"", v.replace('"', "'")) } else { v.clone() };
        attrs.push(format!("{k}={v}"));
    }
    let new = format!("model::{}[{}]", e.target, attrs.join(", "));
    let mut lines: Vec<String> = text.split_inclusive('\n').map(str::to_string).collect();
    let eol = if lines[e.line - 1].ends_with("\r\n") { "\r\n" } else if lines[e.line - 1].ends_with('\n') { "\n" } else { "" };
    lines[e.line - 1] = format!("{new}{eol}");
    Ok(lines.concat())
}

/// The `<figure class="model-figure">` with a `<model-viewer>` for library model `key`, without
/// the component's `<script>` (the page loads it): what the models gallery shows. `reveal=auto`;
/// a download only when the entry offers one.
pub fn viewer_html(lib: &Library, key: &str, caption: &str, static_dir: Option<&Path>) -> anyhow::Result<String> {
    let mut named = BTreeMap::new();
    named.insert("reveal".to_string(), "auto".to_string());
    let env = Env { lib: Some(lib), bundle_dir: None, static_dir, static_html: false };
    figure(&format!("media:{key}"), &[caption.to_string()], &named, &env, false)
}

/// AsciiDoc-style attribute list: comma-separated, `name=value` or positional, values may be
/// quoted (`"a, b"`).
pub fn parse_attrs(s: &str) -> (Vec<String>, BTreeMap<String, String>) {
    let mut parts = vec![];
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for ch in s.chars() {
        match (ch, quote) {
            ('"' | '\'', None) if cur.trim().is_empty() || cur.ends_with('=') => {
                quote = Some(ch);
                cur.push(ch);
            }
            (q, Some(open)) if q == open => {
                quote = None;
                cur.push(ch);
            }
            (',', None) => parts.push(std::mem::take(&mut cur)),
            _ => cur.push(ch),
        }
    }
    if !cur.trim().is_empty() || !parts.is_empty() {
        parts.push(cur);
    }
    let unquote = |v: &str| {
        let v = v.trim();
        for q in ['"', '\''] {
            if v.len() >= 2 && v.starts_with(q) && v.ends_with(q) {
                return v[1..v.len() - 1].to_string();
            }
        }
        v.to_string()
    };
    static NAME: OnceLock<Regex> = OnceLock::new();
    let name = NAME.get_or_init(|| Regex::new(r"^\s*([A-Za-z][A-Za-z0-9_-]*)\s*=(.*)$").unwrap());
    let (mut pos, mut named) = (vec![], BTreeMap::new());
    for p in parts {
        if let Some(c) = name.captures(&p) {
            named.insert(c[1].to_ascii_lowercase(), unquote(&c[2]));
        } else {
            pos.push(unquote(&p));
        }
    }
    (pos, named)
}

/// HTML-escape text and attribute values. `media:` and `story:` are broken up so the media and
/// story expansions that run after this never touch a caption.
pub fn esc(s: &str) -> String {
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
    o.replace("media:", "media&#58;").replace("story:", "story&#58;")
}

/// Is `p` a URL or path a page may point at: `https://`/`http://`, a site path (`/x/y.glb`), or
/// a file beside the post (`y.glb`, `sub/y.glb`) — never `..`, a backslash, control characters,
/// or another scheme (`javascript:`, `data:`, `file:`).
pub fn safe_ref(p: &str) -> bool {
    if p.is_empty() || p.chars().any(|c| c.is_control() || c == '\\' || c == '"' || c == '<' || c == '>' || c == '`' || c.is_whitespace()) {
        return false;
    }
    if let Some(rest) = p.strip_prefix("https://").or_else(|| p.strip_prefix("http://")) {
        return !rest.is_empty() && !rest.starts_with('/');
    }
    if p.starts_with("//") {
        return false;
    }
    let path = p.split(['?', '#']).next().unwrap_or("");
    if path.contains(':') {
        return false;
    }
    !path.split('/').any(|seg| seg == ".." || seg == ".")
}

/// The local file behind a site path or a bundle-relative path, if it can be found.
fn local_file(r: &str, env: &Env) -> Option<std::path::PathBuf> {
    if r.starts_with("http://") || r.starts_with("https://") {
        return None;
    }
    let path = r.split(['?', '#']).next().unwrap_or(r);
    let p = if let Some(site) = path.strip_prefix('/') {
        env.static_dir?.join(site)
    } else {
        env.bundle_dir?.join(path)
    };
    p.is_file().then_some(p)
}

/// Human file size: 950 KB, 31.2 MB.
pub fn human_bytes(n: u64) -> String {
    let n = n as f64;
    if n >= 1e9 {
        format!("{:.1} GB", n / 1e9)
    } else if n >= 1e7 {
        format!("{:.0} MB", n / 1e6)
    } else if n >= 1e6 {
        format!("{:.1} MB", n / 1e6)
    } else {
        format!("{:.0} KB", (n / 1e3).max(1.0))
    }
}

fn truthy(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "" | "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" | "none" => Some(false),
        _ => None,
    }
}

/// `480` → `480px`; `480px`, `30rem`, `60vh`, `40em` as given.
fn css_height(v: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"^(\d{1,4}(?:\.\d+)?)(px|rem|em|vh|svh|dvh)?$").unwrap());
    let c = re.captures(v.trim())?;
    Some(format!("{}{}", &c[1], c.get(2).map(|m| m.as_str()).unwrap_or("px")))
}

/// model-viewer `camera-orbit`: e.g. `30deg 75deg 105%`.
fn safe_orbit(v: &str) -> bool {
    !v.is_empty() && v.len() < 80 && v.chars().all(|c| c.is_ascii_alphanumeric() || " .%-+".contains(c))
}

/// Swap a path's extension: `a/b.glb` → `a/b.usdz`.
fn sibling(r: &str, ext: &str) -> String {
    match r.rsplit_once('.') {
        Some((stem, _)) if !stem.ends_with('/') => format!("{stem}.{ext}"),
        _ => format!("{r}.{ext}"),
    }
}

fn figure(target: &str, pos: &[String], named: &BTreeMap<String, String>, env: &Env, first: bool) -> anyhow::Result<String> {
    use anyhow::bail;
    let m: Media = if let Some(key) = target.strip_prefix("media:") {
        let lib = env.lib.ok_or_else(|| anyhow::anyhow!("no media library"))?;
        let Some(m) = lib.resolve(key).cloned() else { bail!("unknown media key {key:?} (see `gw media list`; add one with `gw media add-model`)") };
        if m.kind != "model" {
            bail!("media:{key} is not a model (kind: {})", m.kind);
        }
        m
    } else if let Some(id) = target.strip_prefix("story:") {
        bail!("story:{id} is resolved when the story's draft is written (`gw story draft`)");
    } else {
        if !safe_ref(target) {
            bail!("not a usable path or URL");
        }
        let lower = target.split(['?', '#']).next().unwrap_or("").to_ascii_lowercase();
        if !(lower.ends_with(".glb") || lower.ends_with(".gltf")) {
            bail!("a model file must be .glb or .gltf");
        }
        let mut m = Media { url: target.into(), kind: "model".into(), provider: "local".into(), ..Default::default() };
        // A USDZ and poster with the same name beside a local file are picked up.
        if local_file(target, env).is_some() {
            for ext in ["usdz"] {
                let s = sibling(target, ext);
                if local_file(&s, env).is_some() {
                    m.ios = s;
                }
            }
            for ext in ["webp", "jpg", "png"] {
                let s = sibling(target, ext);
                if m.poster.is_empty() && local_file(&s, env).is_some() {
                    m.poster = s;
                }
            }
        }
        m
    };
    let get = |k: &str| named.get(k).map(|s| s.as_str());
    let flag = |k: &str, default: bool| -> anyhow::Result<bool> {
        match get(k) {
            None => Ok(default),
            Some(v) => truthy(v).ok_or_else(|| anyhow::anyhow!("{k}={v:?}: use true or false")),
        }
    };
    let path_attr = |k: &str, fallback: &str| -> anyhow::Result<String> {
        match get(k) {
            Some(v) if !v.is_empty() => {
                if !safe_ref(v) {
                    bail!("{k}={v:?} is not a usable path or URL");
                }
                Ok(v.to_string())
            }
            _ => Ok(fallback.to_string()),
        }
    };
    let src = m.url.clone();
    if !safe_ref(&src) {
        bail!("the model's URL {src:?} is not usable");
    }
    let poster = path_attr("poster", &m.poster)?;
    let poster = if poster.is_empty() { m.thumb.clone() } else { poster };
    let ios = path_attr("ios", &m.ios)?;
    for (what, v) in [("poster", &poster), ("ios", &ios)] {
        if !v.is_empty() && !safe_ref(v) {
            bail!("the model's {what} {v:?} is not usable");
        }
    }
    // download: true|false|<path>; default on when the entry offers one.
    let download = match get("download") {
        None => m.download.clone(),
        Some(v) => match truthy(v) {
            Some(true) => {
                if m.download.is_empty() {
                    bail!("download=true, but this model has no download file (gw media add-model --download stl)");
                }
                m.download.clone()
            }
            Some(false) => String::new(),
            None => {
                if !safe_ref(v) {
                    bail!("download={v:?} is not a usable path or URL");
                }
                v.to_string()
            }
        },
    };
    let ar = flag("ar", true)?;
    let autorotate = flag("autorotate", true)?;
    let height = match get("height") {
        Some(h) => Some(css_height(h).ok_or_else(|| anyhow::anyhow!("height={h:?}: use e.g. 480, 480px, 30rem or 60vh"))?),
        None => None,
    };
    let reveal = match get("reveal").unwrap_or("auto") {
        r @ ("auto" | "interaction" | "manual") => r.to_string(),
        r => bail!("reveal={r:?}: use auto or interaction"),
    };
    let orbit = match get("orbit") {
        Some(o) if safe_orbit(o) => Some(o.to_string()),
        Some(o) => bail!("orbit={o:?} is not a camera orbit (e.g. \"30deg 75deg 105%\")"),
        None => None,
    };
    let caption = pos.first().cloned().filter(|c| !c.trim().is_empty()).or_else(|| get("caption").map(str::to_string)).unwrap_or_else(|| m.caption.clone());
    let alt = get("alt").map(str::to_string).filter(|a| !a.is_empty())
        .or_else(|| (!m.alt.is_empty()).then(|| m.alt.clone()))
        .unwrap_or_else(|| if caption.is_empty() { "3-D model".into() } else { format!("3-D model: {caption}") });

    // Caption facts: printed size and parts; the download with its size.
    let mut facts = vec![];
    if m.dimensions_mm.len() == 3 {
        let d: Vec<String> = m.dimensions_mm.iter().map(|x| format!("{x:.0}")).collect();
        facts.push(format!("printed {} mm", d.join(" × ")));
    }
    if m.parts > 1 {
        facts.push(format!("{} parts", m.parts));
    }
    let dl_html = if download.is_empty() {
        String::new()
    } else {
        let bytes = local_file(&download, env).and_then(|p| std::fs::metadata(p).ok()).map(|md| md.len())
            .or_else(|| (download == m.download && m.download_bytes > 0).then_some(m.download_bytes));
        let ext = download.split(['?', '#']).next().unwrap_or("").rsplit('.').next().unwrap_or("").to_ascii_uppercase();
        let label = if ext.len() <= 4 && !ext.is_empty() { ext } else { "file".into() };
        let name = download.split(['?', '#']).next().unwrap_or("").rsplit('/').next().unwrap_or("");
        format!(
            "<a class=\"model-download\" href=\"{}\" download=\"{}\">Download {}{}</a>",
            esc(&download),
            esc(name),
            esc(&label),
            bytes.map(|b| format!(" <span class=\"model-bytes\">({})</span>", human_bytes(b))).unwrap_or_default()
        )
    };
    let mut cap = String::new();
    if !caption.is_empty() {
        cap.push_str(&format!("<span class=\"model-title\">{}</span>", esc(&caption)));
    }
    if !facts.is_empty() {
        cap.push_str(&format!("<span class=\"model-facts\">{}</span>", esc(&facts.join(" · "))));
    }
    cap.push_str(&dl_html);

    if env.static_html {
        let img = if poster.is_empty() { esc(&alt) } else { format!("<img src=\"{}\" alt=\"{}\" loading=\"lazy\">", esc(&poster), esc(&alt)) };
        return Ok(format!(
            "<figure class=\"model-figure model-static\"><a href=\"{}\">{img}</a><figcaption>{cap}</figcaption></figure>",
            esc(if download.is_empty() { &src } else { &download })
        ));
    }

    let mut attrs = vec![
        format!("src=\"{}\"", esc(&src)),
        format!("alt=\"{}\"", esc(&alt)),
        "camera-controls".into(),
        "touch-action=\"pan-y\"".into(),
        "interaction-prompt=\"auto\"".into(),
        "shadow-intensity=\"0.8\"".into(),
        "exposure=\"1\"".into(),
        "loading=\"lazy\"".into(),
        format!("reveal=\"{reveal}\""),
    ];
    if !poster.is_empty() {
        attrs.push(format!("poster=\"{}\"", esc(&poster)));
    }
    if autorotate {
        attrs.push("auto-rotate".into());
        attrs.push("auto-rotate-delay=\"1500\"".into());
        attrs.push("rotation-per-second=\"18deg\"".into());
    }
    if ar {
        attrs.push("ar".into());
        attrs.push("ar-modes=\"webxr scene-viewer quick-look\"".into());
        attrs.push("ar-scale=\"auto\"".into());
        if !ios.is_empty() {
            attrs.push(format!("ios-src=\"{}\"", esc(&ios)));
        }
    }
    if let Some(o) = &orbit {
        attrs.push(format!("camera-orbit=\"{}\"", esc(o)));
    }
    if let Some(h) = &height {
        attrs.push(format!("style=\"--model-h: {h}\""));
    }
    // Without JavaScript (or before the component loads) only the light-DOM children show:
    // the poster as an image, and a link to the file.
    let fallback = if poster.is_empty() {
        String::new()
    } else {
        format!("<img class=\"model-poster\" slot=\"poster\" src=\"{}\" alt=\"{}\" loading=\"lazy\" decoding=\"async\">", esc(&poster), esc(&alt))
    };
    let script = if first { format!("<script type=\"module\" src=\"{VIEWER_JS}\"></script>\n") } else { String::new() };
    let hint = if ar { "Drag to turn · pinch to zoom · on a phone, view it in your room" } else { "Drag to turn · pinch or scroll to zoom" };
    Ok(format!(
        "{script}<figure class=\"model-figure\">\n<model-viewer {}>{fallback}</model-viewer>\n<figcaption>{cap}<span class=\"model-hint\">{hint}</span><noscript><a class=\"model-file\" href=\"{}\">3-D file (GLB)</a></noscript></figcaption>\n</figure>",
        attrs.join(" "),
        esc(&src)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lib() -> Library {
        let mut l = Library::default();
        l.media.insert(
            "wiz".into(),
            Media {
                url: "/models/wiz/wiz.glb".into(),
                kind: "model".into(),
                provider: "local".into(),
                ios: "/models/wiz/wiz.usdz".into(),
                poster: "/models/wiz/poster.webp".into(),
                download: "/models/wiz/wizard-lemming-plate.stl".into(),
                download_bytes: 32_361_984,
                caption: "Wizard <Lemming> & co".into(),
                dimensions_mm: vec![82.7, 60.2, 100.0],
                parts: 2,
                ..Default::default()
            },
        );
        l.media.insert("plain".into(), Media { url: "/models/p/p.glb".into(), kind: "model".into(), ..Default::default() });
        l.media.insert("photo".into(), Media { url: "/x.jpg".into(), kind: "image".into(), ..Default::default() });
        l
    }

    fn run(text: &str) -> Expanded {
        let l = lib();
        expand(text, &Env { lib: Some(&l), ..Default::default() })
    }

    #[test]
    fn renders_viewer_with_attributes() {
        let e = run("Intro.\n\nmodel::media:wiz[]\n\nmodel::media:plain[\"A, quoted\", ar=false, autorotate=false, height=30rem, reveal=interaction]\n");
        assert!(e.problems.is_empty(), "{:?}", e.problems);
        assert_eq!(e.count, 2);
        let t = &e.text;
        assert_eq!(t.matches(VIEWER_JS).count(), 1, "the component is loaded once");
        assert!(t.contains("++++\n<script type=\"module\""));
        assert!(t.contains("src=\"/models/wiz/wiz.glb\""));
        assert!(t.contains("ios-src=\"/models/wiz/wiz.usdz\""));
        assert!(t.contains("poster=\"/models/wiz/poster.webp\""));
        assert!(t.contains("camera-controls") && t.contains("touch-action=\"pan-y\"") && t.contains("loading=\"lazy\""));
        assert!(t.contains("ar-modes=\"webxr scene-viewer quick-look\""));
        // Caption: entry caption escaped, printed size, parts, download with size.
        assert!(t.contains("Wizard &lt;Lemming&gt; &amp; co"));
        assert!(t.contains("printed 83 × 60 × 100 mm · 2 parts"));
        assert!(t.contains("href=\"/models/wiz/wizard-lemming-plate.stl\" download=\"wizard-lemming-plate.stl\">Download STL <span class=\"model-bytes\">(32 MB)</span>"));
        // Second figure: quoted caption with a comma, no AR, no rotation, height, reveal.
        let second = &t[t.find("/models/p/p.glb").unwrap() - 200..];
        assert!(second.contains("A, quoted"));
        assert!(!second.contains(" ar ") && !second.contains("auto-rotate"));
        assert!(second.contains("style=\"--model-h: 30rem\"") && second.contains("reveal=\"interaction\""));
        assert!(!second.contains("model-download"), "no download file → no link");
        assert!(t.starts_with("Intro.\n\n"));
    }

    #[test]
    fn download_switch_and_paths() {
        let e = run("model::media:wiz[download=false]\n");
        assert!(!e.text.contains("model-download"));
        let e = run("model::media:wiz[Wiz, download=files/other.3mf, height=480]\n");
        assert!(e.text.contains("href=\"files/other.3mf\"") && e.text.contains("Download 3MF<"));
        assert!(e.text.contains("--model-h: 480px"));
        let e = run("model::media:plain[download=true]\n");
        assert_eq!(e.problems.len(), 1);
        assert!(e.problems[0].contains("no download file"));
        assert!(e.text.contains("model-missing"));
    }

    #[test]
    fn escaping_and_bad_attributes() {
        let e = run("model::media:wiz[\"<script>alert(1)</script>\" , alt=\"say <b>hi</b> media:photo\"]\n");
        assert!(!e.text.contains("<script>alert"));
        assert!(e.text.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(!e.text.contains("media:photo"), "media: in text is broken up so the media expansion leaves it");
        for bad in ["height=100%;color:red", "ar=maybe", "reveal=never", "orbit=\"1deg;x\"", "download=javascript:alert(1)", "poster=../../etc/passwd"] {
            let e = run(&format!("model::media:wiz[{bad}]\n"));
            assert_eq!(e.problems.len(), 1, "{bad}: {:?}", e.problems);
            assert!(!e.text.contains("<model-viewer"), "{bad}");
        }
    }

    #[test]
    fn unknown_and_wrong_kind_keys() {
        let e = run("model::media:nope[]\nmodel::media:photo[]\nmodel::story:img3[]\n");
        assert_eq!(e.count, 0);
        assert_eq!(e.problems.len(), 3);
        assert!(e.problems[0].contains("unknown media key \"nope\""));
        assert!(e.problems[1].contains("is not a model (kind: image)"));
        assert!(e.problems[2].contains("gw story draft"));
    }

    #[test]
    fn verbatim_blocks_and_inline_mentions_are_left_alone() {
        let src = "----\nmodel::media:wiz[]\n----\n\n....\nmodel::media:nope[]\n....\n\nSay `model::media:wiz[]` inline.\n// model::media:nope[]\n";
        let e = run(src);
        assert_eq!(e.text, src);
        assert!(e.problems.is_empty());
    }

    #[test]
    fn direct_paths_and_safety() {
        let dir = std::env::temp_dir().join(format!("gw-model-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["robot.glb", "robot.usdz", "robot.webp", "robot.stl"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        let l = lib();
        let env = Env { lib: Some(&l), bundle_dir: Some(&dir), ..Default::default() };
        let e = expand("model::robot.glb[Robot, download=robot.stl]\n", &env);
        assert!(e.problems.is_empty(), "{:?}", e.problems);
        assert!(e.text.contains("ios-src=\"robot.usdz\"") && e.text.contains("poster=\"robot.webp\""));
        assert!(e.text.contains("Download STL <span class=\"model-bytes\">(1 KB)</span>"));
        let e = expand("model::https://example.org/m/robot.glb[]\n", &env);
        assert!(e.problems.is_empty() && e.text.contains("src=\"https://example.org/m/robot.glb\""));
        for bad in ["../secret.glb", "/models/../../x.glb", "javascript:alert(1).glb", "data:x.glb", "robot.stl", "//evil.org/x.glb", "a\\b.glb"] {
            let e = expand(&format!("model::{bad}[]\n"), &env);
            assert_eq!(e.problems.len(), 1, "{bad}");
            assert!(!e.text.contains("<model-viewer"), "{bad}");
        }
        std::fs::remove_dir_all(&dir).ok();
        assert!(safe_ref("/models/a/b.glb") && safe_ref("a/b.glb") && safe_ref("https://x.org/a.glb?v=1"));
        assert!(!safe_ref("") && !safe_ref("a b.glb") && !safe_ref("x\"onload=\"y.glb") && !safe_ref("./a.glb"));
    }

    #[test]
    fn static_html_for_feeds() {
        let l = lib();
        let e = expand("model::media:wiz[]\n", &Env { lib: Some(&l), static_html: true, ..Default::default() });
        assert!(!e.text.contains("<model-viewer") && !e.text.contains("<script"));
        assert!(e.text.contains("<a href=\"/models/wiz/wizard-lemming-plate.stl\"><img src=\"/models/wiz/poster.webp\""));
    }

    const POST: &str = "= Lemmings\n:type: project\n:date: 2026-10-01\n\nIntro paragraph\nstill intro.\n\nmodel::media:wiz[Wizard, download=false]\n\n----\nmodel::media:not-this[]\n----\n\n== More\n\nmodel::media:plain[]\n";

    #[test]
    fn lists_embeds() {
        let e = embeds(POST);
        assert_eq!(e.len(), 2, "the one in the listing block is not a model");
        assert_eq!((e[0].index, e[0].line, e[0].key.as_deref(), e[0].caption.as_str(), e[0].download.as_deref()), (0, 8, Some("wiz"), "Wizard", Some("false")));
        assert_eq!((e[1].index, e[1].line, e[1].key.as_deref(), e[1].caption.as_str()), (1, 16, Some("plain"), ""));
    }

    #[test]
    fn inserts_as_own_block() {
        let line = macro_line("gino", "Gino, the first", Some(true));
        assert_eq!(line, "model::media:gino[\"Gino, the first\", download=true]");
        assert_eq!(macro_line("x", "a]b", None), "model::media:x[a)b]");
        // At the end.
        let t = insert(POST, &line, Place::End);
        assert!(t.ends_with("model::media:plain[]\n\nmodel::media:gino[\"Gino, the first\", download=true]\n"), "{t}");
        assert_eq!(embeds(&t).len(), 3);
        // After a line in the middle of a paragraph → after the paragraph.
        let t = insert(POST, "model::media:gino[]", Place::After(5));
        assert!(t.contains("still intro.\n\nmodel::media:gino[]\n\nmodel::media:wiz["), "{t}");
        // Inside the header → first thing in the body.
        let t = insert(POST, "model::media:gino[]", Place::After(1));
        assert!(t.starts_with("= Lemmings\n:type: project\n:date: 2026-10-01\n\nmodel::media:gino[]\n\nIntro paragraph"), "{t}");
        // Inside a listing block → after it.
        let t = insert(POST, "model::media:gino[]", Place::After(11));
        assert!(t.contains("----\nmodel::media:not-this[]\n----\n\nmodel::media:gino[]\n\n== More"), "{t}");
        assert_eq!(embeds(&t).iter().map(|e| e.key.clone().unwrap()).collect::<Vec<_>>(), ["wiz", "gino", "plain"]);
    }

    #[test]
    fn removes_and_updates() {
        let t = remove(POST, 0).unwrap();
        assert!(t.contains("still intro.\n\n----\n"), "{t}");
        assert_eq!(embeds(&t).len(), 1);
        let t = remove(POST, 1).unwrap();
        assert!(t.ends_with("== More\n"), "{t:?}");
        assert!(remove(POST, 5).is_err());
        let t = update(POST, 0, Some("The wizard, again"), Some(None)).unwrap();
        assert!(t.contains("\nmodel::media:wiz[\"The wizard, again\"]\n"), "{t}");
        let t = update(POST, 1, None, Some(Some(true))).unwrap();
        assert!(t.contains("\nmodel::media:plain[download=true]\n"), "{t}");
        let t = update("model::media:wiz[Wiz, height=480, orbit=\"30deg 75deg 105%\"]\n", 0, Some("W"), None).unwrap();
        assert_eq!(t, "model::media:wiz[W, height=480, orbit=\"30deg 75deg 105%\"]\n");
        assert!(run(&t).problems.is_empty());
    }

    #[test]
    fn attribute_parsing() {
        let (p, n) = parse_attrs("Wizard, \"with, comma\", download = false, height='40rem'");
        assert_eq!(p, vec!["Wizard", "with, comma"]);
        assert_eq!(n["download"], "false");
        assert_eq!(n["height"], "40rem");
        let (p, n) = parse_attrs("");
        assert!(p.is_empty() && n.is_empty());
        assert_eq!(human_bytes(32_361_984), "32 MB");
        assert_eq!(human_bytes(2_097_008), "2.1 MB");
    }
}
