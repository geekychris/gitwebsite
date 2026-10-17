//! AWS Signature Version 4 (header and query-string/presigned), for the S3 relay. Pure
//! functions, checked against AWS's published SigV4 test suite (aws-c-auth
//! `aws-signing-test-suite/v4`) and the S3 examples in the S3 API reference.

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

pub const ALGO: &str = "AWS4-HMAC-SHA256";
/// SHA-256 of the empty string.
pub const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
pub const UNSIGNED: &str = "UNSIGNED-PAYLOAD";

#[derive(Clone)]
pub struct Creds {
    pub access_key: String,
    pub secret_key: String,
}

/// RFC 3986 encoding as SigV4 wants it: everything but `A-Za-z0-9-._~` is %XX (upper case);
/// `/` is kept when `keep_slash` (S3 object paths).
pub fn uri_encode(s: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) || (keep_slash && b == b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The canonical query string from decoded (name, value) pairs: each encoded, sorted by name
/// then value, joined with `&`.
pub fn canonical_query(pairs: &[(String, String)]) -> String {
    let mut v: Vec<(String, String)> = pairs.iter().map(|(k, v)| (uri_encode(k, false), uri_encode(v, false))).collect();
    v.sort();
    v.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&")
}

/// Parse a raw query string (`a=1&b=%20`) into decoded pairs.
pub fn parse_query(q: &str) -> Vec<(String, String)> {
    q.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            let d = |s: &str| urlencoding::decode(&s.replace('+', "%20")).map(|c| c.into_owned()).unwrap_or_else(|_| s.to_string());
            (d(k), d(v))
        })
        .collect()
}

fn hmac(key: &[u8], data: &str) -> Vec<u8> {
    let mut m = Hmac::<Sha256>::new_from_slice(key).expect("hmac key");
    m.update(data.as_bytes());
    m.finalize().into_bytes().to_vec()
}

pub fn sha256_hex(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

pub fn signing_key(secret: &str, date: &str, region: &str, service: &str) -> Vec<u8> {
    let k = hmac(format!("AWS4{secret}").as_bytes(), date);
    let k = hmac(&k, region);
    let k = hmac(&k, service);
    hmac(&k, "aws4_request")
}

pub fn amz_date(t: &DateTime<Utc>) -> String {
    t.format("%Y%m%dT%H%M%SZ").to_string()
}

/// One request to sign. `path` is the canonical (already URI-encoded) path; `query` holds
/// decoded pairs; `headers` are (name, value) — names in any case, all of them signed.
pub struct Request<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub payload_hash: &'a str,
    pub region: &'a str,
    pub service: &'a str,
    pub time: DateTime<Utc>,
}

fn canon_headers(h: &[(String, String)]) -> (String, String) {
    let mut v: Vec<(String, String)> = h
        .iter()
        .map(|(k, v)| (k.to_ascii_lowercase(), v.split_whitespace().collect::<Vec<_>>().join(" ")))
        .collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    // Repeated names are joined with commas.
    let mut merged: Vec<(String, String)> = vec![];
    for (k, val) in v {
        match merged.last_mut() {
            Some((lk, lv)) if *lk == k => {
                lv.push(',');
                lv.push_str(&val);
            }
            _ => merged.push((k, val)),
        }
    }
    let canon = merged.iter().map(|(k, v)| format!("{k}:{v}\n")).collect::<String>();
    let signed = merged.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(";");
    (canon, signed)
}

pub fn canonical_request(r: &Request) -> (String, String) {
    let (ch, signed) = canon_headers(&r.headers);
    (format!("{}\n{}\n{}\n{}\n{}\n{}", r.method, r.path, canonical_query(&r.query), ch, signed, r.payload_hash), signed)
}

fn scope(r: &Request) -> String {
    format!("{}/{}/{}/aws4_request", r.time.format("%Y%m%d"), r.region, r.service)
}

pub fn string_to_sign(r: &Request, creq: &str) -> String {
    format!("{ALGO}\n{}\n{}\n{}", amz_date(&r.time), scope(r), sha256_hex(creq.as_bytes()))
}

fn signature(r: &Request, creds: &Creds, creq: &str) -> String {
    let key = signing_key(&creds.secret_key, &r.time.format("%Y%m%d").to_string(), r.region, r.service);
    hex::encode(hmac(&key, &string_to_sign(r, creq)))
}

/// The `Authorization` header value. The caller must already have put `host` and
/// `x-amz-date` (and for S3 `x-amz-content-sha256`) in `r.headers`.
pub fn authorization(r: &Request, creds: &Creds) -> String {
    let (creq, signed) = canonical_request(r);
    format!("{ALGO} Credential={}/{}, SignedHeaders={signed}, Signature={}", creds.access_key, scope(r), signature(r, creds, &creq))
}

/// Presign: returns the full query (the request's own pairs plus the X-Amz-* ones and the
/// signature), encoded, to append after `?`. Only `host` should be in `r.headers`.
pub fn presign_query(r: &Request, creds: &Creds, expires: u64) -> String {
    let (_, signed) = canon_headers(&r.headers);
    let mut q = r.query.clone();
    q.push(("X-Amz-Algorithm".into(), ALGO.into()));
    q.push(("X-Amz-Credential".into(), format!("{}/{}", creds.access_key, scope(r))));
    q.push(("X-Amz-Date".into(), amz_date(&r.time)));
    q.push(("X-Amz-Expires".into(), expires.to_string()));
    q.push(("X-Amz-SignedHeaders".into(), signed));
    let signed_req = Request { query: q.clone(), method: r.method, path: r.path, headers: r.headers.clone(), payload_hash: r.payload_hash, region: r.region, service: r.service, time: r.time };
    let (creq, _) = canonical_request(&signed_req);
    let sig = signature(&signed_req, creds, &creq);
    format!("{}&X-Amz-Signature={sig}", canonical_query(&q))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn suite() -> Creds {
        Creds { access_key: "AKIDEXAMPLE".into(), secret_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into() }
    }
    fn t2015() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2015, 8, 30, 12, 36, 0).unwrap()
    }
    fn suite_req<'a>(method: &'a str, path: &'a str, query: &str) -> Request<'a> {
        Request {
            method,
            path,
            query: parse_query(query),
            headers: vec![("Host".into(), "example.amazonaws.com".into()), ("X-Amz-Date".into(), "20150830T123600Z".into())],
            payload_hash: EMPTY_SHA256,
            region: "us-east-1",
            service: "service",
            time: t2015(),
        }
    }
    fn sig_of(auth: &str) -> &str {
        auth.rsplit("Signature=").next().unwrap()
    }

    /// aws-c-auth tests/aws-signing-test-suite/v4/*/header-signed-request.txt
    #[test]
    fn aws_sigv4_test_suite_header() {
        let c = suite();
        let r = suite_req("GET", "/", "");
        let (creq, _) = canonical_request(&r);
        assert_eq!(creq, "GET\n/\n\nhost:example.amazonaws.com\nx-amz-date:20150830T123600Z\n\nhost;x-amz-date\ne3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(string_to_sign(&r, &creq).rsplit('\n').next().unwrap(), "bb579772317eb040ac9ed261061d46c1f17a8133879d6129b6e1c25292927e63");
        assert_eq!(
            authorization(&r, &c),
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, SignedHeaders=host;x-amz-date, Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
        for (name, method, path, query, want) in [
            ("get-vanilla-query-order-key-case", "GET", "/", "Param2=value2&Param1=value1", "b97d918cfa904a5beff61c982a1b6f458b799221646efd99d3219ec94cdf2500"),
            ("get-unreserved", "GET", "/-._~0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz", "", "07ef7494c76fa4850883e2b006601f940f8a34d404d0cfa977f52a65bbf5f24f"),
            ("post-vanilla-query", "POST", "/", "Param1=value1", "28038455d6de14eafc1f9222cf5aa6f1a96197d7deb8263271d420d138af7f11"),
            ("get-vanilla-utf8-query", "GET", "/", "%E1%88%B4=bar", "2cdec8eed098649ff3a119c94853b13c643bcf08f8b0a1d91e12c9027818dd04"),
        ] {
            assert_eq!(sig_of(&authorization(&suite_req(method, path, query), &c)), want, "{name}");
        }
    }

    /// aws-c-auth tests/aws-signing-test-suite/v4/get-vanilla/query-signed-request.txt
    #[test]
    fn aws_sigv4_test_suite_query() {
        let mut r = suite_req("GET", "/", "");
        r.headers.truncate(1);
        let q = presign_query(&r, &suite(), 3600);
        assert!(q.ends_with("X-Amz-Signature=e93c787ed7f371d5c6b165c1b38ede9550f4dce4144713e844b25b7192d3865d"), "{q}");
        assert!(q.contains("X-Amz-Credential=AKIDEXAMPLE%2F20150830%2Fus-east-1%2Fservice%2Faws4_request"));
    }

    /// The S3 API reference's examples (examplebucket, 2013-05-24).
    #[test]
    fn s3_reference_examples() {
        let c = Creds { access_key: "AKIAIOSFODNN7EXAMPLE".into(), secret_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".into() };
        let t = Utc.with_ymd_and_hms(2013, 5, 24, 0, 0, 0).unwrap();
        let h = |extra: &[(&str, &str)]| {
            let mut v: Vec<(String, String)> = vec![("host".into(), "examplebucket.s3.amazonaws.com".into())];
            v.extend(extra.iter().map(|(a, b)| (a.to_string(), b.to_string())));
            v.push(("x-amz-content-sha256".into(), EMPTY_SHA256.into()));
            v.push(("x-amz-date".into(), "20130524T000000Z".into()));
            v
        };
        let get = Request { method: "GET", path: "/test.txt", query: vec![], headers: h(&[("range", "bytes=0-9")]), payload_hash: EMPTY_SHA256, region: "us-east-1", service: "s3", time: t };
        assert_eq!(sig_of(&authorization(&get, &c)), "f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41");
        let list = Request { method: "GET", path: "/", query: parse_query("max-keys=2&prefix=J"), headers: h(&[]), payload_hash: EMPTY_SHA256, region: "us-east-1", service: "s3", time: t };
        assert_eq!(sig_of(&authorization(&list, &c)), "34b48302e7b5fa45bde8084f4b7868a86f0a534bc59db6670ed5711ef69dc6f7");
        let pre = Request { method: "GET", path: "/test.txt", query: vec![], headers: vec![("host".into(), "examplebucket.s3.amazonaws.com".into())], payload_hash: UNSIGNED, region: "us-east-1", service: "s3", time: t };
        let q = presign_query(&pre, &c, 86400);
        assert!(q.ends_with("X-Amz-Signature=aeeed9bbccd4d02ee5c0109b86d86835f995330da4c265957d157751f604d404"), "{q}");
    }

    #[test]
    fn encoding() {
        assert_eq!(uri_encode("a b/c+d~", true), "a%20b/c%2Bd~");
        assert_eq!(uri_encode("a/b", false), "a%2Fb");
        assert_eq!(canonical_query(&parse_query("b=2&a=1&a=0")), "a=0&a=1&b=2");
    }
}
