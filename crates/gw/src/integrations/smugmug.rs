//! SmugMug: host images and video and register them in the site's media library, so pages
//! can use `image::media:<key>[]` and the browser loads them from SmugMug's CDN.
//!
//! API v2 with OAuth 1.0a. `gw smugmug auth` stores the API key and secret and the access
//! token and secret in the Keychain (service `gitwebsite-smugmug`).

use super::keychain;
use super::oauth1::{self, Creds};
use super::SmugmugCmd;
use anyhow::{bail, Context, Result};
use gw_core::content::slugify;
use gw_core::media::Media;
use gw_core::Site;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;

const SERVICE: &str = "gitwebsite-smugmug";
const API: &str = "https://api.smugmug.com";
const OAUTH: &str = "https://api.smugmug.com/services/oauth/1.0a";
const UPLOAD: &str = "https://upload.smugmug.com/";

pub struct Auth {
    key: String,
    secret: String,
    token: String,
    token_secret: String,
}

impl Auth {
    pub fn load() -> Result<Self> {
        let get = |a: &str| keychain::get(SERVICE, a);
        match (get("api-key"), get("api-secret"), get("access-token"), get("access-token-secret")) {
            (Some(key), Some(secret), Some(token), Some(token_secret)) => Ok(Auth { key, secret, token, token_secret }),
            _ => bail!("SmugMug is not authorised. Run `gw smugmug auth` in a terminal (you will need an API key from https://api.smugmug.com/api/developer/apply)."),
        }
    }
    fn creds(&self) -> Creds<'_> {
        Creds { consumer_key: &self.key, consumer_secret: &self.secret, token: Some(&self.token), token_secret: Some(&self.token_secret) }
    }
}

fn client() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .user_agent("gitwebsite/0.1")
        .timeout(std::time::Duration::from_secs(600))
        .build()?)
}

/// Signed GET of an API path (e.g. `/api/v2!authuser`), returning `Response`.
fn get(auth: &Auth, path: &str) -> Result<Value> {
    let url = if path.starts_with("http") { path.to_string() } else { format!("{API}{path}") };
    let h = oauth1::header("GET", &url, &auth.creds(), &[], &[]);
    let r = client()?.get(&url).header("Authorization", h).header("Accept", "application/json").send()?;
    let status = r.status();
    let v: Value = r.json().with_context(|| format!("{url}: not JSON (HTTP {status})"))?;
    if !status.is_success() {
        bail!("{url}: HTTP {status}: {}", v.get("Message").and_then(|m| m.as_str()).unwrap_or(""));
    }
    Ok(v.get("Response").cloned().unwrap_or(v))
}

/// Signed POST of a JSON body to an API path, returning `Response`. With a JSON body the OAuth
/// signature covers no body parameters.
fn post_json(auth: &Auth, path: &str, body: &Value) -> Result<Value> {
    let url = format!("{API}{path}");
    let h = oauth1::header("POST", &url, &auth.creds(), &[], &[]);
    let r = client()?
        .post(&url)
        .header("Authorization", h)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()?;
    let status = r.status();
    let v: Value = r.json().with_context(|| format!("{url}: not JSON (HTTP {status})"))?;
    if !status.is_success() {
        bail!("{url}: HTTP {status}: {}", v.get("Message").and_then(|m| m.as_str()).unwrap_or(""));
    }
    Ok(v.get("Response").cloned().unwrap_or(v))
}

/// SmugMug URL names: start with a capital letter, letters, digits and hyphens only.
fn url_name(name: &str) -> String {
    let mut out: String = name
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| { let mut c = w.chars(); c.next().map(|f| f.to_ascii_uppercase().to_string() + c.as_str()).unwrap_or_default() })
        .collect::<Vec<_>>()
        .join("-");
    if !out.chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false) {
        out = format!("A-{out}");
    }
    out.chars().take(60).collect()
}

/// The album settings for a privacy level. Unlisted: viewable by link, hidden from your pages and
/// from SmugMug and web search.
fn album_settings(privacy: &str) -> Result<Value> {
    Ok(match privacy {
        "unlisted" => json!({ "Privacy": "Unlisted", "SmugSearchable": "No", "WorldSearchable": false }),
        "public" => json!({ "Privacy": "Public", "SmugSearchable": "Inherit from User", "WorldSearchable": true }),
        "private" => json!({ "Privacy": "Private", "SmugSearchable": "No", "WorldSearchable": false }),
        other => bail!("--privacy must be unlisted, public or private (got {other})"),
    })
}

fn create_album(site: &Site, auth: &Auth, name: &str, url: Option<&str>, privacy: &str, default: bool) -> Result<Value> {
    let nick = nickname(auth)?;
    let mut body = album_settings(privacy)?;
    body["Name"] = json!(name);
    body["UrlName"] = json!(url.map(|u| u.to_string()).unwrap_or_else(|| url_name(name)));
    let r = post_json(auth, &format!("/api/v2/folder/user/{nick}!albums"), &body)?;
    let a = r.get("Album").context("no Album in the response")?;
    let key = a.get("AlbumKey").and_then(|x| x.as_str()).context("no AlbumKey in the response")?.to_string();
    if default {
        set_default_album(site, &key)?;
    }
    Ok(json!({ "album_key": key, "name": name, "privacy": a.get("Privacy"), "web": a.get("WebUri"), "default": default }))
}

/// The site's media album, from `smugmug: { album: KEY }` in site.yaml.
fn default_album(site: &Site) -> Option<String> {
    let text = std::fs::read_to_string(site.root.join("site.yaml")).ok()?;
    let y: serde_yaml::Value = serde_yaml::from_str(&text).ok()?;
    y.get("smugmug")?.get("album")?.as_str().map(|s| s.to_string())
}

/// The site's media album for the app: its key and web address (`{}` when none is set). Looks
/// the album up on SmugMug when authorised; otherwise returns just the key.
pub fn site_album(site: &Site) -> Result<Value> {
    let Some(key) = default_album(site) else { return Ok(json!({})) };
    let web = Auth::load().ok().and_then(|a| get(&a, &format!("/api/v2/album/{key}")).ok())
        .and_then(|r| r.pointer("/Album/WebUri").and_then(|x| x.as_str()).map(|s| s.to_string()));
    Ok(json!({ "album_key": key, "web": web }))
}

/// Write `smugmug: { album: KEY }` into site.yaml, replacing an existing block, keeping the rest
/// of the file (and its comments) as it is.
fn set_default_album(site: &Site, key: &str) -> Result<()> {
    let path = site.root.join("site.yaml");
    let text = std::fs::read_to_string(&path)?;
    let line = format!("smugmug: {{ album: {key} }}");
    let mut out = vec![];
    let mut lines = text.lines().peekable();
    let mut done = false;
    while let Some(l) = lines.next() {
        if l.starts_with("smugmug:") {
            // drop the old block: the key line and any indented lines under it
            while lines.peek().map(|n| n.starts_with(' ') || n.starts_with('\t')).unwrap_or(false) {
                lines.next();
            }
            out.push(line.clone());
            done = true;
        } else {
            out.push(l.to_string());
        }
    }
    if !done {
        out.push(String::new());
        out.push("# SmugMug album that `gw smugmug upload` uses when no --album is given.".into());
        out.push(line);
    }
    std::fs::write(&path, out.join("\n") + "\n")?;
    Ok(())
}

fn form(s: &str) -> BTreeMap<String, String> {
    s.split('&')
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (k.to_string(), urlencoding::decode(v).map(|x| x.into_owned()).unwrap_or_else(|_| v.into())))
        .collect()
}

/// Read "API Key: ..." / "API Secret: ..." (or key=/secret=) lines from a file.
fn read_credentials_file(path: &std::path::Path) -> Result<(String, String)> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let (mut key, mut secret) = (None, None);
    for line in text.lines() {
        let Some((k, v)) = line.split_once(':').or_else(|| line.split_once('=')) else { continue };
        let k = k.trim().to_ascii_lowercase().replace(['_', '-'], " ");
        let v = v.trim().trim_matches('"').to_string();
        if k.contains("secret") {
            secret = Some(v);
        } else if k.contains("key") {
            key = Some(v);
        }
    }
    match (key, secret) {
        (Some(k), Some(s)) if !k.is_empty() && !s.is_empty() => Ok((k, s)),
        _ => bail!("{} must contain an API key line and an API secret line", path.display()),
    }
}

fn exchange(key: &str, secret: &str, tok: &str, tok_secret: &str, verifier: &str) -> Result<Value> {
    let c = Creds { consumer_key: key, consumer_secret: secret, token: Some(tok), token_secret: Some(tok_secret) };
    let url = format!("{OAUTH}/getAccessToken");
    let h = oauth1::header("GET", &url, &c, &[("oauth_verifier", verifier)], &[]);
    let r = client()?.get(&url).header("Authorization", h).send()?;
    let status = r.status();
    let body = r.text()?;
    if !status.is_success() {
        bail!("access token failed (HTTP {status}): {body}");
    }
    let at = form(&body);
    keychain::set(SERVICE, "api-key", key)?;
    keychain::set(SERVICE, "api-secret", secret)?;
    keychain::set(SERVICE, "access-token", at.get("oauth_token").context("no access token")?)?;
    keychain::set(SERVICE, "access-token-secret", at.get("oauth_token_secret").context("no access token secret")?)?;
    keychain::delete(SERVICE, "pending-token");
    keychain::delete(SERVICE, "pending-token-secret");
    let me = whoami(&Auth::load()?)?;
    Ok(json!({ "authorised": true, "user": me }))
}

fn request_token(key: &str, secret: &str) -> Result<(String, String, String)> {
    let c = Creds { consumer_key: key, consumer_secret: secret, token: None, token_secret: None };
    let url = format!("{OAUTH}/getRequestToken");
    let h = oauth1::header("GET", &url, &c, &[("oauth_callback", "oob")], &[]);
    let r = client()?.get(&url).header("Authorization", h).send()?;
    let status = r.status();
    let body = r.text()?;
    if !status.is_success() {
        bail!("request token failed (HTTP {status}): {body}");
    }
    let rt = form(&body);
    let tok = rt.get("oauth_token").context("no oauth_token")?.clone();
    let tok_secret = rt.get("oauth_token_secret").context("no oauth_token_secret")?.clone();
    let authorize = format!("{OAUTH}/authorize?mode=auth_only&Access=Full&Permissions=Modify&oauth_token={tok}");
    Ok((tok, tok_secret, authorize))
}

fn auth_cmd(file: Option<&std::path::Path>, start: bool, verifier: Option<&str>) -> Result<Value> {
    if let Some(f) = file {
        let (k, s) = read_credentials_file(f)?;
        keychain::set(SERVICE, "api-key", &k)?;
        keychain::set(SERVICE, "api-secret", &s)?;
    }
    let stored = || -> Result<(String, String)> {
        match (keychain::get(SERVICE, "api-key"), keychain::get(SERVICE, "api-secret")) {
            (Some(k), Some(s)) => Ok((k, s)),
            _ => bail!("no SmugMug API key stored: pass --credentials-file, or run `gw smugmug auth` in a terminal"),
        }
    };
    if start {
        let (k, s) = stored()?;
        let (tok, tok_secret, url) = request_token(&k, &s)?;
        keychain::set(SERVICE, "pending-token", &tok)?;
        keychain::set(SERVICE, "pending-token-secret", &tok_secret)?;
        let _ = std::process::Command::new("open").arg(&url).status();
        return Ok(json!({
            "step": "approve",
            "authorize_url": url,
            "next": "Approve access in the browser, then run: gw smugmug auth --verifier <six-digit code>",
        }));
    }
    if let Some(code) = verifier {
        let (k, s) = stored()?;
        let (tok, tok_secret) = match (keychain::get(SERVICE, "pending-token"), keychain::get(SERVICE, "pending-token-secret")) {
            (Some(t), Some(ts)) => (t, ts),
            _ => bail!("no pending authorisation: run `gw smugmug auth --start` first"),
        };
        return exchange(&k, &s, &tok, &tok_secret, code.trim());
    }
    if file.is_some() {
        let (k, s) = stored()?;
        let (tok, tok_secret, url) = request_token(&k, &s)?;
        eprintln!("\nOpen this URL, approve access, and copy the six-digit code:\n\n  {url}\n");
        let _ = std::process::Command::new("open").arg(&url).status();
        let code = keychain::prompt("Verification code")?;
        return exchange(&k, &s, &tok, &tok_secret, code.trim());
    }
    auth_flow()
}

fn auth_flow() -> Result<Value> {
    eprintln!("SmugMug authorisation. You need an API key: https://api.smugmug.com/api/developer/apply");
    let key = keychain::prompt("API key")?;
    let secret = keychain::prompt_secret("API secret")?;
    if key.is_empty() || secret.is_empty() {
        bail!("API key and secret are required");
    }
    let c = Creds { consumer_key: &key, consumer_secret: &secret, token: None, token_secret: None };
    let url = format!("{OAUTH}/getRequestToken");
    let h = oauth1::header("GET", &url, &c, &[("oauth_callback", "oob")], &[]);
    let r = client()?.get(&url).header("Authorization", h).send()?;
    let status = r.status();
    let body = r.text()?;
    if !status.is_success() {
        bail!("request token failed (HTTP {status}): {body}");
    }
    let rt = form(&body);
    let (tok, tok_secret) = (rt.get("oauth_token").context("no oauth_token")?, rt.get("oauth_token_secret").context("no oauth_token_secret")?);
    let authorize = format!("{OAUTH}/authorize?mode=auth_only&Access=Full&Permissions=Modify&oauth_token={tok}");
    eprintln!("\nOpen this URL, approve access, and copy the six-digit code:\n\n  {authorize}\n");
    let _ = std::process::Command::new("open").arg(&authorize).status();
    let verifier = keychain::prompt("Verification code")?;
    let c = Creds { consumer_key: &key, consumer_secret: &secret, token: Some(tok), token_secret: Some(tok_secret) };
    let url = format!("{OAUTH}/getAccessToken");
    let h = oauth1::header("GET", &url, &c, &[("oauth_verifier", &verifier)], &[]);
    let r = client()?.get(&url).header("Authorization", h).send()?;
    let status = r.status();
    let body = r.text()?;
    if !status.is_success() {
        bail!("access token failed (HTTP {status}): {body}");
    }
    let at = form(&body);
    keychain::set(SERVICE, "api-key", &key)?;
    keychain::set(SERVICE, "api-secret", &secret)?;
    keychain::set(SERVICE, "access-token", at.get("oauth_token").context("no access token")?)?;
    keychain::set(SERVICE, "access-token-secret", at.get("oauth_token_secret").context("no access token secret")?)?;
    let me = whoami(&Auth::load()?)?;
    Ok(json!({ "authorised": true, "user": me }))
}

fn whoami(auth: &Auth) -> Result<Value> {
    let r = get(auth, "/api/v2!authuser")?;
    let u = r.get("User").context("no User in !authuser response")?;
    Ok(json!({ "nickname": u.get("NickName"), "name": u.get("Name"), "web": u.get("WebUri") }))
}

fn nickname(auth: &Auth) -> Result<String> {
    Ok(whoami(auth)?["nickname"].as_str().context("no nickname")?.to_string())
}

/// Follow `Pages.NextPage` to collect every item of a list endpoint.
fn paged(auth: &Auth, path: &str, field: &str) -> Result<Vec<Value>> {
    let mut out = vec![];
    let sep = if path.contains('?') { '&' } else { '?' };
    let mut next = Some(format!("{path}{sep}count=100"));
    while let Some(p) = next {
        let r = get(auth, &p)?;
        if let Some(a) = r.get(field).and_then(|x| x.as_array()) {
            out.extend(a.iter().cloned());
        }
        next = r.pointer("/Pages/NextPage").and_then(|x| x.as_str()).map(|s| s.to_string());
    }
    Ok(out)
}

pub fn albums(auth: &Auth) -> Result<Value> {
    let nick = nickname(auth)?;
    let list = paged(auth, &format!("/api/v2/user/{nick}!albums"), "Album")?;
    Ok(json!(list
        .iter()
        .map(|a| json!({ "name": a.get("Name"), "album_key": a.get("AlbumKey"), "web": a.get("WebUri"), "images": a.get("ImageCount") }))
        .collect::<Vec<_>>()))
}

fn album_uri(album: &str) -> String {
    if album.starts_with("/api/v2/album/") {
        album.to_string()
    } else {
        format!("/api/v2/album/{}", album.trim_matches('/').rsplit('/').next().unwrap_or(album))
    }
}

/// `ImageKey` from a key, `/api/v2/image/KEY-0` URI, or a gallery URL ending `/i-KEY`.
fn image_key(s: &str) -> String {
    let s = s.trim().trim_end_matches('/');
    if let Some(i) = s.rfind("/i-") {
        return s[i + 3..].split(['/', '?', '#']).next().unwrap_or("").to_string();
    }
    if let Some(rest) = s.strip_prefix("/api/v2/image/") {
        return rest.split(['-', '!', '?']).next().unwrap_or("").to_string();
    }
    s.to_string()
}

fn first_url(details: &Value, names: &[&str]) -> Option<String> {
    names.iter().find_map(|n| details.get(*n).and_then(|d| d.get("Url")).and_then(|u| u.as_str()).map(|s| s.to_string()))
}

/// Look up an image by key and build its media-library record.
fn media_for(auth: &Auth, key: &str, album_key: Option<&str>, alt: Option<&str>, caption: Option<&str>) -> Result<Media> {
    Ok(media_and_image(auth, key, album_key, alt, caption)?.0)
}

/// [`media_for`], also returning SmugMug's Image object (file name, size, caption, dates).
pub fn media_and_image(auth: &Auth, key: &str, album_key: Option<&str>, alt: Option<&str>, caption: Option<&str>) -> Result<(Media, Value)> {
    let resp = get(auth, &format!("/api/v2/image/{key}-0"))?;
    let img = resp.get("Image").context("no Image in response")?;
    let is_video = img.get("IsVideo").and_then(|x| x.as_bool()).unwrap_or(false);
    let web = img.get("WebUri").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let sizes = get(auth, &format!("/api/v2/image/{key}-0!sizedetails"))?;
    let d = sizes.get("ImageSizeDetails").cloned().unwrap_or(Value::Null);
    let still = first_url(&d, &["ImageSizeX3Large", "ImageSizeX2Large", "ImageSizeXLarge", "ImageSizeLarge", "ImageSizeOriginal"])
        .or_else(|| get(auth, &format!("/api/v2/image/{key}-0!largestimage")).ok()
            .and_then(|r| r.pointer("/LargestImage/Url").and_then(|u| u.as_str()).map(|s| s.to_string())))
        .unwrap_or_default();
    let thumb = first_url(&d, &["ImageSizeMedium", "ImageSizeSmall", "ImageSizeThumb"]).unwrap_or_default();
    let mut ids = BTreeMap::new();
    ids.insert("image_key".to_string(), key.to_string());
    ids.insert("web_uri".to_string(), web.clone());
    if let Some(a) = album_key {
        ids.insert("album_key".to_string(), a.to_string());
    }
    let (kind, url) = if is_video {
        match get(auth, &format!("/api/v2/image/{key}-0!largestvideo")).ok()
            .and_then(|r| r.pointer("/LargestVideo/Url").and_then(|u| u.as_str()).map(|s| s.to_string()))
        {
            Some(v) => ("video", v),
            None => {
                ids.insert("note".into(), "no playable video URL from the API; url is the gallery page".into());
                ("video", web.clone())
            }
        }
    } else {
        ("image", still.clone())
    };
    if url.is_empty() {
        bail!("SmugMug returned no usable URL for image {key}");
    }
    let field = |k: &str| img.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    Ok((Media {
        url,
        kind: kind.into(),
        provider: "smugmug".into(),
        thumb: if is_video { still } else { thumb },
        alt: alt.map(|s| s.to_string()).unwrap_or_else(|| field("Title")),
        caption: caption.map(|s| s.to_string()).unwrap_or_else(|| field("Caption")),
        ids,
        ..Default::default()
    }, img.clone()))
}

fn register(site: &Site, key: &str, m: Media) -> Result<Value> {
    if !key.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) || key.is_empty() {
        bail!("media keys may use letters, digits, '-', '_', '.'");
    }
    let mut lib = site.media()?;
    let macro_ = if m.kind == "video" { "video" } else { "image" };
    lib.media.insert(key.to_string(), m.clone());
    lib.save(&site.media_path())?;
    Ok(json!({ "key": key, "media": m, "use": format!("{macro_}::media:{key}[]") }))
}

fn mime_for(p: &Path) -> &'static str {
    match p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("heic") => "image/heic",
        Some("webp") => "image/webp",
        Some("tif" | "tiff") => "image/tiff",
        Some("mp4" | "m4v") => "video/mp4",
        Some("mov") => "video/quicktime",
        Some("avi") => "video/x-msvideo",
        _ => "application/octet-stream",
    }
}

fn upload(site: &Site, auth: &Auth, file: &Path, album: &str, key: Option<String>, title: Option<String>, caption: Option<String>, alt: Option<String>) -> Result<Value> {
    let bytes = std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    let name = file.file_name().context("no file name")?.to_string_lossy().to_string();
    let key = key.unwrap_or_else(|| slugify(&file.file_stem().unwrap().to_string_lossy()));
    if site.media()?.media.contains_key(&key) {
        bail!("media key {key} already exists; pass --key");
    }
    let auri = album_uri(album);
    let h = oauth1::header("POST", UPLOAD, &auth.creds(), &[], &[]);
    let mut req = client()?
        .post(UPLOAD)
        .header("Authorization", h)
        .header("Content-Type", mime_for(file))
        .header("X-Smug-AlbumUri", &auri)
        .header("X-Smug-ResponseType", "JSON")
        .header("X-Smug-Version", "v2")
        .header("X-Smug-FileName", &name);
    if let Some(t) = &title {
        req = req.header("X-Smug-Title", t);
    }
    if let Some(c) = &caption {
        req = req.header("X-Smug-Caption", c);
    }
    eprintln!("uploading {} ({:.1} MB) to {auri}…", name, bytes.len() as f64 / 1e6);
    let r = req.body(bytes).send()?;
    let status = r.status();
    let v: Value = r.json().with_context(|| format!("upload response not JSON (HTTP {status})"))?;
    if v.get("stat").and_then(|s| s.as_str()) != Some("ok") {
        bail!("upload failed: {}", v.get("message").and_then(|m| m.as_str()).unwrap_or(&v.to_string()));
    }
    let image_uri = v.pointer("/Image/ImageUri").and_then(|x| x.as_str()).context("no ImageUri in upload response")?;
    let ik = image_key(image_uri);
    let album_key = auri.rsplit('/').next().map(|s| s.to_string());
    let m = media_for(auth, &ik, album_key.as_deref(), alt.as_deref(), caption.as_deref())?;
    register(site, &key, m)
}

// ------------------------------------------------------------------ browsing (for stories)

/// A size of a SmugMug image derived from its thumbnail URL
/// (`…/Th/name-Th.jpg` → `…/M/name-M.jpg`). Sizes: Ti Th S M L XL X2 X3.
pub fn sized(thumb: &str, size: &str) -> String {
    if thumb.contains("/Th/") && thumb.contains("-Th.") {
        thumb.replacen("/Th/", &format!("/{size}/"), 1).replacen("-Th.", &format!("-{size}."), 1)
    } else {
        thumb.to_string()
    }
}

/// One image as the picker shows it.
pub fn browse_image(img: &Value) -> Value {
    let s = |k: &str| img.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let thumb = s("ThumbnailUrl");
    json!({
        "image_key": s("ImageKey"), "album_key": s("AlbumKey"), "filename": s("FileName"),
        "title": s("Title"), "caption": s("Caption"),
        "date": img.get("DateTimeOriginal").or_else(|| img.get("Date")).and_then(|x| x.as_str()),
        "width": img.get("OriginalWidth"), "height": img.get("OriginalHeight"),
        "is_video": img.get("IsVideo").and_then(|x| x.as_bool()).unwrap_or(false),
        "thumb": thumb, "small": sized(&thumb, "S"), "medium": sized(&thumb, "M"),
        "large": sized(&thumb, "L"), "xlarge": sized(&thumb, "X2"), "web": s("WebUri"),
    })
}

fn album_summary(a: &Value) -> Value {
    json!({
        "album_key": a.get("AlbumKey"), "name": a.get("Name"), "web": a.get("WebUri"),
        "images": a.get("ImageCount"), "url_path": a.get("UrlPath"),
    })
}

/// One page of an album's images.
fn album_page(auth: &Auth, album_key: &str, start: usize, count: usize) -> Result<Value> {
    let a = get(auth, &format!("/api/v2/album/{album_key}"))?;
    let album = a.get("Album").cloned().unwrap_or(Value::Null);
    let r = get(auth, &format!("/api/v2/album/{album_key}!images?start={}&count={count}", start.max(1)))?;
    let images: Vec<Value> = r.get("AlbumImage").and_then(|x| x.as_array()).map(|a| a.iter().map(browse_image).collect()).unwrap_or_default();
    let total = r.pointer("/Pages/Total").and_then(|x| x.as_u64()).unwrap_or(images.len() as u64);
    let next = r.pointer("/Pages/NextPage").and_then(|x| x.as_str()).map(|_| start.max(1) + images.len());
    Ok(json!({ "kind": "album", "album": album_summary(&album), "total": total, "start": start.max(1), "next_start": next, "images": images }))
}

/// Resolve a SmugMug page URL, album key, or image reference and list what is there: an
/// album's images (paged), a folder's albums, or a single image. Read-only.
pub fn browse(auth: &Auth, target: &str, start: usize, count: usize) -> Result<Value> {
    use gw_core::story::{parse_smugmug, SmugRef};
    let count = count.clamp(1, 200);
    let t = target.trim();
    let r = parse_smugmug(t).or_else(|| (!t.contains('/') && !t.contains(':') && !t.is_empty()).then(|| SmugRef::Album(t.to_string())));
    match r {
        Some(SmugRef::Album(k)) => album_page(auth, &k, start, count),
        Some(SmugRef::Image(k)) => {
            let v = get(auth, &format!("/api/v2/image/{k}-0"))?;
            let img = v.get("Image").context("no Image in response")?;
            Ok(json!({ "kind": "image", "total": 1, "start": 1, "next_start": null, "images": [browse_image(img)] }))
        }
        Some(SmugRef::Page(url)) => {
            let v = get(auth, &format!("/api/v2!weburilookup?WebUri={}", urlencoding::encode(&url)))?;
            let loc = v.get("Locator").and_then(|x| x.as_str()).unwrap_or("");
            match loc {
                "Album" => {
                    let k = v.pointer("/Album/AlbumKey").and_then(|x| x.as_str()).context("no AlbumKey")?;
                    album_page(auth, k, start, count)
                }
                "AlbumImage" | "Image" => {
                    let img = v.get(loc).context("no image")?;
                    let k = img.get("ImageKey").and_then(|x| x.as_str()).context("no ImageKey")?;
                    browse(auth, &format!("smugmug:{k}"), 1, 1)
                }
                "Folder" => {
                    let f = v.get("Folder").cloned().unwrap_or(Value::Null);
                    let uri = |name: &str| f.pointer(&format!("/Uris/{name}/Uri")).and_then(|x| x.as_str()).map(|s| s.to_string());
                    let albums = match uri("FolderAlbums") { Some(u) => paged(auth, &u, "Album")?, None => vec![] };
                    let folders = match uri("Folders") { Some(u) => paged(auth, &u, "Folder")?, None => vec![] };
                    Ok(json!({
                        "kind": "folder",
                        "folder": { "name": f.get("Name"), "web": f.get("WebUri").or_else(|| f.get("UrlPath")) },
                        "albums": albums.iter().map(album_summary).collect::<Vec<_>>(),
                        "folders": folders.iter().map(|x| json!({ "name": x.get("Name"), "web": x.get("WebUri") })).collect::<Vec<_>>(),
                    }))
                }
                "" => bail!("SmugMug could not find a page at {url}"),
                other => bail!("{url} is a SmugMug {other}, not an album, folder, or image page"),
            }
        }
        None => bail!("{t} is not a SmugMug page URL, AlbumKey, or image reference"),
    }
}

pub fn run(site: &Site, cmd: SmugmugCmd) -> Result<Value> {
    match cmd {
        SmugmugCmd::Auth { credentials_file, start, verifier } => auth_cmd(credentials_file.as_deref(), start, verifier.as_deref()),
        SmugmugCmd::Logout => {
            for a in ["api-key", "api-secret", "access-token", "access-token-secret"] {
                keychain::delete(SERVICE, a);
            }
            Ok(json!({ "logged_out": true }))
        }
        SmugmugCmd::Whoami => whoami(&Auth::load()?),
        SmugmugCmd::Get { path } => get(&Auth::load()?, &path),
        SmugmugCmd::Albums => albums(&Auth::load()?),
        SmugmugCmd::CreateAlbum { name, url_name, privacy, default } => create_album(site, &Auth::load()?, &name, url_name.as_deref(), &privacy, default),
        SmugmugCmd::Upload { file, album, key, title, caption, alt } => {
            let auth = Auth::load()?;
            let album = match album.or_else(|| default_album(site)) {
                Some(a) => a,
                None => bail!("no --album, and site.yaml has no SmugMug media album: run `gw smugmug create-album \"<name>\" --default` or pass --album"),
            };
            upload(site, &auth, &file, &album, key, title, caption, alt)
        }
        SmugmugCmd::Import { image, key, alt, caption } => {
            let auth = Auth::load()?;
            if site.media()?.media.contains_key(&key) {
                bail!("media key {key} already exists");
            }
            let m = media_for(&auth, &image_key(&image), None, alt.as_deref(), caption.as_deref())?;
            register(site, &key, m)
        }
        SmugmugCmd::Refresh { key } => {
            let auth = Auth::load()?;
            let mut lib = site.media()?;
            let old = lib.media.get(&key).with_context(|| format!("no media key {key}"))?.clone();
            if old.provider != "smugmug" {
                bail!("{key} is not SmugMug media (provider {})", old.provider);
            }
            let ik = old.ids.get("image_key").map(|s| s.to_string()).with_context(|| format!("{key} has no image_key"))?;
            let ak = old.ids.get("album_key").map(|s| s.to_string());
            let alt = (!old.alt.is_empty()).then(|| old.alt.clone());
            let cap = (!old.caption.is_empty()).then(|| old.caption.clone());
            let m = media_for(&auth, &ik, ak.as_deref(), alt.as_deref(), cap.as_deref())?;
            let playable = !m.ids.contains_key("note");
            lib.media.insert(key.clone(), m.clone());
            lib.save(&site.media_path())?;
            Ok(json!({ "key": key, "url": m.url, "playable": playable }))
        }
        SmugmugCmd::SyncAlbum { album, prefix } => {
            let auth = Auth::load()?;
            let auri = album_uri(&album);
            let ak = auri.rsplit('/').next().unwrap_or("").to_string();
            let images = paged(&auth, &format!("{auri}!images"), "AlbumImage")?;
            let mut lib = site.media()?;
            let mut added = vec![];
            for (n, img) in images.iter().enumerate() {
                let key = format!("{prefix}-{}", n + 1);
                let ik = img.get("ImageKey").and_then(|x| x.as_str()).unwrap_or("");
                if ik.is_empty() {
                    continue;
                }
                let m = media_for(&auth, ik, Some(&ak), None, None)?;
                lib.media.insert(key.clone(), m);
                added.push(key);
            }
            lib.save(&site.media_path())?;
            Ok(json!({ "album": ak, "registered": added }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_from_thumbnail() {
        let t = "https://photos.smugmug.com/Album/i-AbC12dE/0/HASH/Th/IMG_1-Th.jpg";
        assert_eq!(sized(t, "M"), "https://photos.smugmug.com/Album/i-AbC12dE/0/HASH/M/IMG_1-M.jpg");
        assert_eq!(sized(t, "X2"), "https://photos.smugmug.com/Album/i-AbC12dE/0/HASH/X2/IMG_1-X2.jpg");
        assert_eq!(sized("https://example.org/odd.jpg", "M"), "https://example.org/odd.jpg");
    }

    #[test]
    fn picker_image_from_album_image() {
        // Shape of an AlbumImage from GET /api/v2/album/<key>!images (values invented).
        let v = serde_json::json!({
            "ImageKey": "AbC12dE", "AlbumKey": "Zz9", "FileName": "IMG_0001.JPG", "Title": "", "Caption": "Setting up",
            "Date": "2026-10-05T14:03:22+00:00", "OriginalWidth": 6000, "OriginalHeight": 4000, "IsVideo": false,
            "ThumbnailUrl": "https://photos.smugmug.com/A/i-AbC12dE/0/H/Th/IMG_0001-Th.jpg", "WebUri": "https://me.smugmug.com/A/i-AbC12dE"
        });
        let b = browse_image(&v);
        assert_eq!(b["image_key"], "AbC12dE");
        assert_eq!(b["caption"], "Setting up");
        assert_eq!(b["medium"], "https://photos.smugmug.com/A/i-AbC12dE/0/H/M/IMG_0001-M.jpg");
        assert_eq!(b["width"], 6000);
        assert_eq!(b["is_video"], false);
    }

    #[test]
    fn keys_from_urls() {
        assert_eq!(image_key("https://me.smugmug.com/Workshop/i-AbC12dE"), "AbC12dE");
        assert_eq!(image_key("https://me.smugmug.com/Workshop/n-x/i-AbC12dE/A"), "AbC12dE");
        assert_eq!(image_key("/api/v2/image/AbC12dE-0"), "AbC12dE");
        assert_eq!(image_key("AbC12dE"), "AbC12dE");
        assert_eq!(album_uri("ZbcdEf"), "/api/v2/album/ZbcdEf");
        assert_eq!(album_uri("/api/v2/album/ZbcdEf"), "/api/v2/album/ZbcdEf");
    }
}

#[cfg(test)]
mod album_tests {
    use super::*;

    #[test]
    fn url_names_follow_smugmug_rules() {
        assert_eq!(url_name("geekychris.github.io media"), "Geekychris-Github-Io-Media");
        assert_eq!(url_name("2026 videos"), "A-2026-Videos");
    }

    #[test]
    fn unlisted_albums_are_hidden_from_search() {
        let s = album_settings("unlisted").unwrap();
        assert_eq!(s["Privacy"], "Unlisted");
        assert_eq!(s["WorldSearchable"], false);
        assert!(album_settings("secret").is_err());
    }
}
