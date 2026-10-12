//! HTML → AsciiDoc via pandoc, with clean-up for WordPress markup.

use anyhow::{bail, Context, Result};
use regex::Regex;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

fn re(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).unwrap())
}

fn pandoc() -> &'static str {
    if std::path::Path::new("/opt/homebrew/bin/pandoc").exists() {
        "/opt/homebrew/bin/pandoc"
    } else {
        "pandoc"
    }
}

/// Convert an HTML fragment to AsciiDoc.
pub fn convert(html: &str) -> Result<String> {
    static WPCOMMENT: OnceLock<Regex> = OnceLock::new();
    static DATAIMG: OnceLock<Regex> = OnceLock::new();
    static BUTTON: OnceLock<Regex> = OnceLock::new();
    let cleaned = re(&WPCOMMENT, r"(?s)<!--.*?-->").replace_all(html, "");
    // Inline data: images are UI chrome (lightbox icons), and <button>s are controls, not content.
    let cleaned = re(&BUTTON, r"(?is)<button\b.*?</button>").replace_all(&cleaned, "");
    let cleaned = re(&DATAIMG, r#"(?i)<img[^>]+src=["']data:[^"']*["'][^>]*>"#).replace_all(&cleaned, "");
    let mut child = Command::new(pandoc())
        .args(["-f", "html", "-t", "asciidoc", "--wrap=none"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("running pandoc (brew install pandoc)")?;
    child.stdin.take().unwrap().write_all(cleaned.as_bytes())?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!("pandoc failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(tidy(&String::from_utf8_lossy(&out.stdout)))
}

fn tidy(s: &str) -> String {
    static URLALT: OnceLock<Regex> = OnceLock::new();
    // pandoc fills an empty alt with the image URL; an alt that is a URL helps nobody.
    let s = re(&URLALT, r"(image::?[^\[\s]+)\[https?://[^,\]]*,?").replace_all(s, "$1[");
    let s = s.as_ref();
    static BLANKS: OnceLock<Regex> = OnceLock::new();
    static EMPTYROLE: OnceLock<Regex> = OnceLock::new();
    // pandoc emits `[]` / `[.wp-block-x]` attribute lines for divs; they carry nothing here.
    let s = re(&EMPTYROLE, r"(?m)^\[(\.[\w-]+)*\]\s*$\n?").replace_all(s, "");
    let s = re(&BLANKS, r"\n{3,}").replace_all(&s, "\n\n");
    // A level-0 heading (= x) inside a body would be taken as a document title.
    let s = s.lines().map(|l| if l.starts_with("= ") { format!("={l}") } else { l.to_string() }).collect::<Vec<_>>().join("\n");
    s.trim().to_string() + "\n"
}

/// Plain text from HTML (for summaries): tags stripped, common entities decoded.
pub fn plain(html: &str) -> String {
    static TAGS: OnceLock<Regex> = OnceLock::new();
    static WS: OnceLock<Regex> = OnceLock::new();
    let t = re(&TAGS, r"(?s)<[^>]*>").replace_all(html, " ");
    let t = decode_entities(&t);
    let t = re(&WS, r"\s+").replace_all(&t, " ");
    t.trim().trim_end_matches("[…]").trim_end_matches("[&hellip;]").trim().to_string()
}

pub fn decode_entities(s: &str) -> String {
    static NUM: OnceLock<Regex> = OnceLock::new();
    let s = s
        .replace("&nbsp;", " ")
        .replace("&hellip;", "…")
        .replace("&#8217;", "’")
        .replace("&#8216;", "‘")
        .replace("&#8220;", "“")
        .replace("&#8221;", "”")
        .replace("&#8211;", "–")
        .replace("&#8212;", "—")
        .replace("&quot;", "\"")
        .replace("&#039;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">");
    let s = re(&NUM, r"&#(\d+);").replace_all(&s, |c: &regex::Captures| {
        c[1].parse::<u32>().ok().and_then(char::from_u32).map(|c| c.to_string()).unwrap_or_default()
    });
    let s = s.replace("&amp;", "&");
    // WordPress excerpts are sometimes double-encoded (&amp;mdash;): decode named entities again.
    s.replace("&mdash;", "—")
        .replace("&ndash;", "–")
        .replace("&rsquo;", "’")
        .replace("&lsquo;", "‘")
        .replace("&ldquo;", "“")
        .replace("&rdquo;", "”")
        .replace("&hellip;", "…")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
}

/// Every image URL referenced by `<img src>` in the HTML.
pub fn image_urls(html: &str) -> Vec<String> {
    static IMG: OnceLock<Regex> = OnceLock::new();
    let mut v: Vec<String> = re(&IMG, r#"(?i)<img[^>]+src=["']([^"']+)["']"#)
        .captures_iter(html)
        .map(|c| decode_entities(&c[1]))
        .filter(|u| !u.starts_with("data:"))
        .collect();
    v.dedup();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text() {
        assert_eq!(plain("<p>It&#8217;s <b>bold</b>&nbsp;here [&hellip;]</p>"), "It’s bold here");
    }

    #[test]
    fn url_alt_removed() {
        assert_eq!(tidy("image::x.png[https://a.b/x,width=10]\n"), "image::x.png[width=10]\n");
        assert_eq!(tidy("image::x.png[https://a.b/x]\n"), "image::x.png[]\n");
        assert_eq!(tidy("image::x.png[A bench]\n"), "image::x.png[A bench]\n");
    }

    #[test]
    fn images() {
        let v = image_urls(r#"<p><img class="x" src="https://a.b/c.jpg?w=1024&amp;h=7" alt=""></p>"#);
        assert_eq!(v, vec!["https://a.b/c.jpg?w=1024&h=7".to_string()]);
    }
}
