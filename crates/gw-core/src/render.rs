//! AsciiDoc → HTML via Asciidoctor.
//!
//! Asciidoctor is Ruby; starting it per file costs ~0.3 s, so a build renders every changed
//! document in one Ruby process (documents in, JSON out) and caches the HTML by a hash of
//! the source and attributes in `.gw-cache/render/`.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const SCRIPT: &str = r#"
require 'json'
begin
  require 'asciidoctor'
rescue LoadError
  STDERR.puts 'asciidoctor gem not found: gem install asciidoctor'
  exit 3
end
jobs = JSON.parse(STDIN.read)
out = {}
jobs.each do |j|
  attrs = { 'showtitle!' => '', 'icons' => 'font', 'source-highlighter' => 'highlight.js',
            'sectanchors' => '', 'idprefix' => '', 'idseparator' => '-', 'experimental' => '' }.merge(j['attrs'] || {})
  begin
    out[j['id']] = Asciidoctor.convert(j['text'], safe: :safe, standalone: false,
                                       base_dir: j['base_dir'], attributes: attrs)
  rescue => e
    out[j['id']] = { 'error' => e.message }
  end
end
STDOUT.write(JSON.generate(out))
"#;

#[derive(Serialize)]
struct Job<'a> {
    id: &'a str,
    text: &'a str,
    base_dir: String,
    attrs: &'a BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Out {
    Html(String),
    Err { error: String },
}

pub struct Doc {
    pub id: String,
    pub text: String,
    pub base_dir: PathBuf,
    pub attrs: BTreeMap<String, String>,
}

pub struct Renderer {
    pub ruby: String,
    pub cache_dir: PathBuf,
}

impl Renderer {
    fn key(d: &Doc) -> String {
        let mut h = Sha256::new();
        h.update(b"gw-render-v1");
        h.update(d.text.as_bytes());
        for (k, v) in &d.attrs {
            h.update(k.as_bytes());
            h.update(b"=");
            h.update(v.as_bytes());
        }
        hex::encode(&h.finalize()[..12])
    }

    /// Render many documents; returns id → HTML. Uses the cache where it can.
    pub fn render_all(&self, docs: &[Doc]) -> Result<BTreeMap<String, String>> {
        std::fs::create_dir_all(&self.cache_dir)?;
        let mut out = BTreeMap::new();
        let mut todo = vec![];
        for d in docs {
            let f = self.cache_dir.join(format!("{}.html", Self::key(d)));
            match std::fs::read_to_string(&f) {
                Ok(html) => {
                    out.insert(d.id.clone(), html);
                }
                Err(_) => todo.push(d),
            }
        }
        if todo.is_empty() {
            return Ok(out);
        }
        let jobs: Vec<Job> = todo
            .iter()
            .map(|d| Job { id: &d.id, text: &d.text, base_dir: d.base_dir.display().to_string(), attrs: &d.attrs })
            .collect();
        let mut child = Command::new(&self.ruby)
            .args(["-e", SCRIPT])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("starting {} (Ruby with the asciidoctor gem is required)", self.ruby))?;
        child.stdin.take().unwrap().write_all(serde_json::to_string(&jobs)?.as_bytes())?;
        let res = child.wait_with_output()?;
        if !res.status.success() {
            bail!("asciidoctor failed: {}", String::from_utf8_lossy(&res.stderr));
        }
        let rendered: BTreeMap<String, Out> = serde_json::from_slice(&res.stdout).context("reading asciidoctor output")?;
        let mut errors = vec![];
        for d in todo {
            match rendered.get(&d.id) {
                Some(Out::Html(h)) => {
                    std::fs::write(self.cache_dir.join(format!("{}.html", Self::key(d))), h)?;
                    out.insert(d.id.clone(), h.clone());
                }
                Some(Out::Err { error }) => errors.push(format!("{}: {error}", d.id)),
                None => errors.push(format!("{}: not rendered", d.id)),
            }
        }
        if !errors.is_empty() {
            bail!("rendering failed:\n  {}", errors.join("\n  "));
        }
        Ok(out)
    }
}

pub fn render_one(ruby: &str, cache: &Path, doc: Doc) -> Result<String> {
    let id = doc.id.clone();
    let r = Renderer { ruby: ruby.into(), cache_dir: cache.into() };
    Ok(r.render_all(&[doc])?.remove(&id).unwrap_or_default())
}
