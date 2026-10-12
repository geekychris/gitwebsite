//! OAuth 1.0a request signing (HMAC-SHA1), RFC 5849.

use base64::Engine;
use hmac::{Hmac, Mac};
use rand::Rng;
use sha1::Sha1;

/// RFC 3986 percent-encoding as OAuth requires (unreserved: A-Z a-z 0-9 - . _ ~).
pub fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub struct Creds<'a> {
    pub consumer_key: &'a str,
    pub consumer_secret: &'a str,
    pub token: Option<&'a str>,
    pub token_secret: Option<&'a str>,
}

/// Signature over method, base URL (no query), and all parameters (query + oauth + form).
pub fn signature(method: &str, base_url: &str, params: &[(String, String)], consumer_secret: &str, token_secret: &str) -> String {
    let mut p: Vec<(String, String)> = params.iter().map(|(k, v)| (enc(k), enc(v))).collect();
    p.sort();
    let norm = p.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&");
    let base = format!("{}&{}&{}", method.to_uppercase(), enc(base_url), enc(&norm));
    let key = format!("{}&{}", enc(consumer_secret), enc(token_secret));
    let mut mac = Hmac::<Sha1>::new_from_slice(key.as_bytes()).unwrap();
    mac.update(base.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}

/// Build the `Authorization: OAuth ...` header for a request. `url` may carry a query
/// string; its parameters are included in the signature. `extra_oauth` adds parameters
/// such as `oauth_callback` or `oauth_verifier`.
pub fn header(method: &str, url: &str, c: &Creds, extra_oauth: &[(&str, &str)], form: &[(String, String)]) -> String {
    let nonce: String = rand::thread_rng().sample_iter(&rand::distributions::Alphanumeric).take(24).map(char::from).collect();
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs().to_string();
    header_with(method, url, c, extra_oauth, form, &nonce, &ts)
}

pub fn header_with(method: &str, url: &str, c: &Creds, extra_oauth: &[(&str, &str)], form: &[(String, String)], nonce: &str, ts: &str) -> String {
    let (base, query) = url.split_once('?').unwrap_or((url, ""));
    let mut oauth: Vec<(String, String)> = vec![
        ("oauth_consumer_key".into(), c.consumer_key.into()),
        ("oauth_nonce".into(), nonce.into()),
        ("oauth_signature_method".into(), "HMAC-SHA1".into()),
        ("oauth_timestamp".into(), ts.into()),
        ("oauth_version".into(), "1.0".into()),
    ];
    if let Some(t) = c.token {
        oauth.push(("oauth_token".into(), t.into()));
    }
    for (k, v) in extra_oauth {
        oauth.push((k.to_string(), v.to_string()));
    }
    let mut all = oauth.clone();
    for pair in query.split('&').filter(|s| !s.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let dk = urlencoding::decode(k).map(|x| x.into_owned()).unwrap_or_else(|_| k.into());
        let dv = urlencoding::decode(&v.replace('+', " ")).map(|x| x.into_owned()).unwrap_or_else(|_| v.into());
        all.push((dk, dv));
    }
    all.extend(form.iter().cloned());
    let sig = signature(method, base, &all, c.consumer_secret, c.token_secret.unwrap_or(""));
    oauth.push(("oauth_signature".into(), sig));
    oauth.sort();
    format!(
        "OAuth {}",
        oauth.iter().map(|(k, v)| format!("{}=\"{}\"", enc(k), enc(v))).collect::<Vec<_>>().join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Twitter's documented example ("Creating a signature").
    #[test]
    fn twitter_example_signature() {
        let params: Vec<(String, String)> = vec![
            ("status", "Hello Ladies + Gentlemen, a signed OAuth request!"),
            ("include_entities", "true"),
            ("oauth_consumer_key", "xvz1evFS4wEEPTGEFPHBog"),
            ("oauth_nonce", "kYjzVBB8Y0ZFabxSWbWovY3uYSQ2pTgmZeNu2VS4cg"),
            ("oauth_signature_method", "HMAC-SHA1"),
            ("oauth_timestamp", "1318622958"),
            ("oauth_token", "370773112-GmHxMAgYyLbNEtIKZeRNFsMKPR9EyMZeS9weJAEb"),
            ("oauth_version", "1.0"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let sig = signature(
            "post",
            "https://api.twitter.com/1.1/statuses/update.json",
            &params,
            "kAcSOqF21Fu85e7zjz7ZN2U4ZRhfV3WpwPAoE3Z7kBw",
            "LswwdoUaIvS8ltyTt5jkRh4J50vUPVVHtR2YPi5kE",
        );
        assert_eq!(sig, "hCtSmYh+iHYCEqBWrE7C7hYmtUk=");
    }

    /// The same request through header_with, with the status in the form body and
    /// include_entities in the query string.
    #[test]
    fn header_includes_query_and_form() {
        let c = Creds {
            consumer_key: "xvz1evFS4wEEPTGEFPHBog",
            consumer_secret: "kAcSOqF21Fu85e7zjz7ZN2U4ZRhfV3WpwPAoE3Z7kBw",
            token: Some("370773112-GmHxMAgYyLbNEtIKZeRNFsMKPR9EyMZeS9weJAEb"),
            token_secret: Some("LswwdoUaIvS8ltyTt5jkRh4J50vUPVVHtR2YPi5kE"),
        };
        let h = header_with(
            "POST",
            "https://api.twitter.com/1.1/statuses/update.json?include_entities=true",
            &c,
            &[],
            &[("status".into(), "Hello Ladies + Gentlemen, a signed OAuth request!".into())],
            "kYjzVBB8Y0ZFabxSWbWovY3uYSQ2pTgmZeNu2VS4cg",
            "1318622958",
        );
        assert!(h.contains("oauth_signature=\"hCtSmYh%2BiHYCEqBWrE7C7hYmtUk%3D\""), "{h}");
    }

    #[test]
    fn percent_encoding() {
        assert_eq!(enc("Ladies + Gentlemen"), "Ladies%20%2B%20Gentlemen");
        assert_eq!(enc("An encoded string!"), "An%20encoded%20string%21");
        assert_eq!(enc("Dogs, Cats & Mice"), "Dogs%2C%20Cats%20%26%20Mice");
        assert_eq!(enc("☃"), "%E2%98%83");
    }
}
