//! A minimal XML-RPC client (enough for the WordPress API): JSON values in, JSON values out.

use anyhow::{bail, Context, Result};
use quick_xml::events::Event;
use quick_xml::Reader;
use serde_json::{Map, Value};

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn value_xml(v: &Value, out: &mut String) {
    out.push_str("<value>");
    match v {
        Value::Null => out.push_str("<nil/>"),
        Value::Bool(b) => out.push_str(&format!("<boolean>{}</boolean>", if *b { 1 } else { 0 })),
        Value::Number(n) if n.is_i64() => out.push_str(&format!("<int>{n}</int>")),
        Value::Number(n) => out.push_str(&format!("<double>{n}</double>")),
        Value::String(s) => out.push_str(&format!("<string>{}</string>", esc(s))),
        Value::Array(a) => {
            out.push_str("<array><data>");
            for x in a {
                value_xml(x, out);
            }
            out.push_str("</data></array>");
        }
        Value::Object(o) => {
            out.push_str("<struct>");
            for (k, x) in o {
                out.push_str(&format!("<member><name>{}</name>", esc(k)));
                value_xml(x, out);
                out.push_str("</member>");
            }
            out.push_str("</struct>");
        }
    }
    out.push_str("</value>");
}

pub fn request_xml(method: &str, params: &[Value]) -> String {
    let mut s = format!("<?xml version=\"1.0\"?>\n<methodCall><methodName>{}</methodName><params>", esc(method));
    for p in params {
        s.push_str("<param>");
        value_xml(p, &mut s);
        s.push_str("</param>");
    }
    s.push_str("</params></methodCall>");
    s
}

/// A simple element tree, enough to walk XML-RPC responses.
#[derive(Debug, Default)]
struct Node {
    name: String,
    text: String,
    children: Vec<Node>,
}

fn parse_tree(xml: &str) -> Result<Node> {
    let mut r = Reader::from_str(xml);
    let mut stack: Vec<Node> = vec![Node { name: "#root".into(), ..Default::default() }];
    loop {
        match r.read_event()? {
            Event::Start(e) => stack.push(Node { name: String::from_utf8_lossy(e.name().as_ref()).into(), ..Default::default() }),
            Event::Empty(e) => {
                let n = Node { name: String::from_utf8_lossy(e.name().as_ref()).into(), ..Default::default() };
                stack.last_mut().unwrap().children.push(n);
            }
            Event::Text(t) => stack.last_mut().unwrap().text.push_str(&t.unescape()?.into_owned()),
            Event::CData(t) => stack.last_mut().unwrap().text.push_str(&String::from_utf8_lossy(&t)),
            Event::End(_) => {
                let n = stack.pop().unwrap();
                stack.last_mut().context("malformed XML")?.children.push(n);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(stack.pop().unwrap())
}

fn to_value(n: &Node) -> Value {
    // n is a <value> node.
    let Some(t) = n.children.first() else { return Value::String(n.text.clone()) };
    match t.name.as_str() {
        "string" => Value::String(t.text.clone()),
        "int" | "i4" | "i8" => t.text.trim().parse::<i64>().map(Value::from).unwrap_or(Value::String(t.text.clone())),
        "double" => t.text.trim().parse::<f64>().map(Value::from).unwrap_or(Value::Null),
        "boolean" => Value::Bool(t.text.trim() == "1"),
        "nil" => Value::Null,
        "dateTime.iso8601" | "base64" => Value::String(t.text.clone()),
        "array" => Value::Array(
            t.children.iter().find(|c| c.name == "data").map(|d| d.children.iter().map(to_value).collect()).unwrap_or_default(),
        ),
        "struct" => {
            let mut m = Map::new();
            for mem in &t.children {
                let name = mem.children.iter().find(|c| c.name == "name").map(|c| c.text.clone()).unwrap_or_default();
                if let Some(v) = mem.children.iter().find(|c| c.name == "value") {
                    m.insert(name, to_value(v));
                }
            }
            Value::Object(m)
        }
        _ => Value::String(t.text.clone()),
    }
}

pub fn parse_response(xml: &str) -> Result<Value> {
    let root = parse_tree(xml)?;
    let resp = root.children.iter().find(|c| c.name == "methodResponse").context("not an XML-RPC response")?;
    if let Some(f) = resp.children.iter().find(|c| c.name == "fault") {
        let v = f.children.iter().find(|c| c.name == "value").map(to_value).unwrap_or_default();
        bail!(
            "XML-RPC fault {}: {}",
            v.get("faultCode").cloned().unwrap_or_default(),
            v.get("faultString").and_then(|s| s.as_str()).unwrap_or("")
        );
    }
    let v = resp
        .children
        .iter()
        .find(|c| c.name == "params")
        .and_then(|p| p.children.first())
        .and_then(|p| p.children.iter().find(|c| c.name == "value"))
        .map(to_value)
        .unwrap_or(Value::Null);
    Ok(v)
}

pub fn call(url: &str, method: &str, params: &[Value]) -> Result<Value> {
    let body = request_xml(method, params);
    let resp = reqwest::blocking::Client::builder()
        .user_agent("gitwebsite/0.1")
        .build()?
        .post(url)
        .header("Content-Type", "text/xml")
        .body(body)
        .send()
        .with_context(|| format!("calling {url}"))?;
    let status = resp.status();
    let text = resp.text()?;
    if !status.is_success() {
        bail!("{url}: HTTP {status}");
    }
    parse_response(&text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn round_trip_shapes() {
        let x = request_xml("wp.newPost", &[json!(1), json!("u"), json!({"post_title": "A & B", "terms_names": {"post_tag": ["x"]}})]);
        assert!(x.contains("<int>1</int>") && x.contains("A &amp; B") && x.contains("<array><data>"));
        let resp = r#"<?xml version="1.0"?><methodResponse><params><param><value><string>42</string></value></param></params></methodResponse>"#;
        assert_eq!(parse_response(resp).unwrap(), json!("42"));
        let fault = r#"<methodResponse><fault><value><struct><member><name>faultCode</name><value><int>403</int></value></member><member><name>faultString</name><value><string>Incorrect username or password.</string></value></member></struct></value></fault></methodResponse>"#;
        assert!(parse_response(fault).unwrap_err().to_string().contains("Incorrect username"));
        let arr = r#"<methodResponse><params><param><value><array><data><value><struct><member><name>blogid</name><value><string>7</string></value></member></struct></value></data></array></value></param></params></methodResponse>"#;
        assert_eq!(parse_response(arr).unwrap(), json!([{"blogid": "7"}]));
    }
}
