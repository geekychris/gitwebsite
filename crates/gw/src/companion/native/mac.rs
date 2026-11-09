//! Apple frameworks (Vision, PDFKit, Speech, AVFoundation, Foundation Models …) through the
//! Swift helper `gw-mac-helper` (source in app/mac-helper; built by the app's build.rs or
//! `make mac-helper`). Scope `native.mac`.
//!
//! Endpoints:
//! * `GET /v1/native/mac` → `{"ops":[{"op":"vision.ocr","about":"…","session":false}, …]}`
//! * `POST /v1/native/mac/{op}` — one call. Arguments as JSON in `X-Native-Args` (or `{}`);
//!   the body is the input bytes (an image, a PDF, audio …). The answer has the op's content
//!   type: JSON, bytes (with `X-Native-Meta`, small JSON), or `application/x-ndjson`, streamed
//!   line by line as the helper writes it (e.g. LLM tokens).
//! * `GET /v1/native/mac/session/{op}` — a WebSocket for long-running ops (live speech, screen
//!   recording): the first text message is the arguments (JSON); after that every text message
//!   is a line on the helper's stdin and every stdout line comes back as a text message. Closing
//!   the socket ends the helper.
//!
//! Helper protocol: `gw-mac-helper --list` prints the op list (above). `gw-mac-helper <op>
//! <args-json>` reads the input on stdin and writes one header line, `{"type": "<mime>",
//! "meta": {…}}`, then the body. `--session` after the args: JSON lines both ways. A non-zero
//! exit is a failure; the last stderr line (`{"error": "…"}` or plain text) is the message.

use super::super::{authorize, err, tagged, Gateway, Origin};
use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

/// The longest a one-shot call may run (a long transcription or an LLM answer).
pub const CALL_TIMEOUT: Duration = Duration::from_secs(600);
const MAX_INPUT: usize = 512 * 1024 * 1024;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Op {
    pub op: String,
    #[serde(default)]
    pub about: String,
    /// Runs as a session (WebSocket) rather than one call.
    #[serde(default)]
    pub session: bool,
}

/// Where the helper is: `$GW_MAC_HELPER`, next to this executable (the app bundle's
/// Contents/MacOS, or ~/.local/bin beside `gw`), then the workspace's target/ (development).
pub fn helper_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("GW_MAC_HELPER") {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    let mut cands = vec![];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            cands.push(d.join("gw-mac-helper"));
            cands.push(d.join("../Resources/gw-mac-helper"));
        }
        if let Ok(real) = exe.canonicalize() {
            if let Some(d) = real.parent() {
                cands.push(d.join("gw-mac-helper"));
            }
        }
    }
    cands.push(PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/gw-mac-helper")));
    cands.into_iter().find(|p| p.is_file())
}

static OPS: OnceLock<Vec<Op>> = OnceLock::new();

/// The helper's ops (asked once; empty without a helper, or off macOS).
pub fn ops() -> &'static [Op] {
    OPS.get_or_init(|| {
        if !cfg!(target_os = "macos") {
            return vec![];
        }
        let Some(p) = helper_path() else { return vec![] };
        let out = match std::process::Command::new(&p).arg("--list").stderr(Stdio::null()).output() {
            Ok(o) if o.status.success() => o.stdout,
            _ => return vec![],
        };
        serde_json::from_slice::<Value>(&out)
            .ok()
            .and_then(|v| serde_json::from_value::<Vec<Op>>(v["ops"].clone()).ok())
            .unwrap_or_default()
    })
}

fn find_op(name: &str) -> Option<&'static Op> {
    ops().iter().find(|o| o.op == name)
}

pub fn router() -> Router<Arc<Gateway>> {
    Router::new()
        .route("/v1/native/mac", get(list))
        .route("/v1/native/mac/session/{op}", get(session))
        .route("/v1/native/mac/{op}", post(call))
}

pub fn capabilities(_gw: &Gateway) -> Vec<String> {
    let ops = ops();
    if ops.is_empty() {
        return vec![];
    }
    let mut v = vec!["native.mac".to_string()];
    v.extend(ops.iter().map(|o| format!("native.mac.{}", o.op)));
    v
}

async fn list(State(gw): State<Arc<Gateway>>, axum::Extension(Origin(origin)): axum::Extension<Origin>, h: HeaderMap) -> Response {
    let (rec, _) = match authorize(&gw, &h, &origin, "native.mac") {
        Ok(x) => x,
        Err(r) => return r,
    };
    tagged(Json(json!({ "ops": ops() })).into_response(), &rec)
}

fn args_of(h: &HeaderMap) -> Result<String, Response> {
    let raw = h.get("x-native-args").and_then(|v| v.to_str().ok()).unwrap_or("{}");
    match serde_json::from_str::<Value>(raw) {
        Ok(v) if v.is_object() => Ok(v.to_string()),
        _ => Err(err(StatusCode::BAD_REQUEST, "X-Native-Args must be a JSON object")),
    }
}

/// The helper's error message: the last stderr line, `{"error": …}` or plain text.
fn helper_error(stderr: &[u8]) -> String {
    let s = String::from_utf8_lossy(stderr);
    let line = s.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("the helper failed").trim().to_string();
    serde_json::from_str::<Value>(&line).ok().and_then(|v| v["error"].as_str().map(str::to_string)).unwrap_or(line)
}

/// Run a one-shot op. Ok((content type, meta, body)); the body is the rest of stdout.
pub async fn run(op: &str, args: &str, input: &[u8]) -> anyhow::Result<(String, Value, Vec<u8>)> {
    let p = helper_path().ok_or_else(|| anyhow::anyhow!("the macOS helper (gw-mac-helper) is not installed"))?;
    let mut child = Command::new(p).arg(op).arg(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true).spawn()?;
    let mut stdin = child.stdin.take().unwrap();
    let data = input.to_vec();
    let writer = tokio::spawn(async move {
        let _ = stdin.write_all(&data).await;
    });
    let out = tokio::time::timeout(CALL_TIMEOUT, child.wait_with_output()).await.map_err(|_| anyhow::anyhow!("{op} took too long"))??;
    let _ = writer.await;
    if !out.status.success() {
        anyhow::bail!("{}", helper_error(&out.stderr));
    }
    let (head, body) = match out.stdout.iter().position(|b| *b == b'\n') {
        Some(i) => (&out.stdout[..i], out.stdout[i + 1..].to_vec()),
        None => (&out.stdout[..], vec![]),
    };
    let head: Value = serde_json::from_slice(head).map_err(|_| anyhow::anyhow!("the helper sent no header for {op}"))?;
    Ok((head["type"].as_str().unwrap_or("application/octet-stream").to_string(), head["meta"].clone(), body))
}

async fn call(State(gw): State<Arc<Gateway>>, axum::Extension(Origin(origin)): axum::Extension<Origin>, Path(op): Path<String>, h: HeaderMap, body: Body) -> Response {
    let (rec, _) = match authorize(&gw, &h, &origin, "native.mac") {
        Ok(x) => x,
        Err(r) => return r,
    };
    let Some(o) = find_op(&op) else {
        return tagged(err(StatusCode::NOT_FOUND, format!("no macOS op {op:?} (see GET /v1/native/mac)")), &rec);
    };
    if o.session {
        return tagged(err(StatusCode::BAD_REQUEST, format!("{op} is a session: open GET /v1/native/mac/session/{op} as a WebSocket")), &rec);
    }
    let args = match args_of(&h) {
        Ok(a) => a,
        Err(r) => return tagged(r, &rec),
    };
    let input = match axum::body::to_bytes(body, MAX_INPUT).await {
        Ok(b) => b,
        Err(e) => return tagged(err(StatusCode::PAYLOAD_TOO_LARGE, e), &rec),
    };
    let r = match stream_or_run(&op, &args, &input).await {
        Ok(r) => r,
        Err(e) => err(StatusCode::UNPROCESSABLE_ENTITY, format!("{e:#}")),
    };
    tagged(r, &rec)
}

/// Run the helper; an `application/x-ndjson` answer is streamed as it comes.
async fn stream_or_run(op: &str, args: &str, input: &[u8]) -> anyhow::Result<Response> {
    let p = helper_path().ok_or_else(|| anyhow::anyhow!("the macOS helper (gw-mac-helper) is not installed"))?;
    let mut child = Command::new(p).arg(op).arg(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true).spawn()?;
    let mut stdin = child.stdin.take().unwrap();
    let data = input.to_vec();
    tokio::spawn(async move {
        let _ = stdin.write_all(&data).await;
    });
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut stderr = child.stderr.take().unwrap();
    let mut head = String::new();
    let n = tokio::time::timeout(CALL_TIMEOUT, stdout.read_line(&mut head)).await.map_err(|_| anyhow::anyhow!("{op} took too long"))??;
    if n == 0 {
        let mut e = vec![];
        let _ = stderr.read_to_end(&mut e).await;
        let _ = child.wait().await;
        anyhow::bail!("{}", helper_error(&e));
    }
    let head: Value = serde_json::from_str(head.trim()).map_err(|_| anyhow::anyhow!("the helper sent no header for {op}"))?;
    let ctype = head["type"].as_str().unwrap_or("application/octet-stream").to_string();
    let meta = head["meta"].clone();
    if ctype == "application/x-ndjson" {
        // Stream the lines; the child is kept alive by the stream and killed when it is dropped.
        let s = async_stream_lines(stdout, child);
        let mut r = Response::new(Body::from_stream(s));
        r.headers_mut().insert("content-type", HeaderValue::from_static("application/x-ndjson"));
        return Ok(r);
    }
    let mut body = vec![];
    tokio::time::timeout(CALL_TIMEOUT, stdout.read_to_end(&mut body)).await.map_err(|_| anyhow::anyhow!("{op} took too long"))??;
    let status = child.wait().await?;
    if !status.success() {
        let mut e = vec![];
        let _ = stderr.read_to_end(&mut e).await;
        anyhow::bail!("{}", helper_error(&e));
    }
    let mut r = Response::new(Body::from(body));
    if let Ok(v) = HeaderValue::from_str(&ctype) {
        r.headers_mut().insert("content-type", v);
    }
    if !meta.is_null() {
        if let Ok(v) = HeaderValue::from_str(&meta.to_string()) {
            r.headers_mut().insert("x-native-meta", v);
        }
    }
    Ok(r)
}

fn async_stream_lines(
    stdout: BufReader<tokio::process::ChildStdout>,
    child: tokio::process::Child,
) -> impl futures_util::Stream<Item = Result<bytes::Bytes, std::io::Error>> {
    futures_util::stream::unfold((stdout, child), |(mut out, child)| async move {
        let mut line = String::new();
        match out.read_line(&mut line).await {
            Ok(0) | Err(_) => None,
            Ok(_) => Some((Ok(bytes::Bytes::from(line)), (out, child))),
        }
    })
}

async fn session(
    State(gw): State<Arc<Gateway>>,
    axum::Extension(Origin(origin)): axum::Extension<Origin>,
    Path(op): Path<String>,
    h: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let (rec, p) = match super::super::check_token(&gw, &h, &origin, "native.mac") {
        Ok(x) => x,
        Err((code, msg)) => return super::super::ws_refuse(ws, super::super::ws_close_code(code), &msg),
    };
    if !find_op(&op).is_some_and(|o| o.session) {
        return super::super::ws_refuse(ws, 4400, &format!("no macOS session op {op:?}"));
    }
    let ws = match p.subprotocol {
        Some(_) => ws.protocols(["gw-companion.v1"]),
        None => ws,
    };
    tagged(ws.on_upgrade(move |sock| pipe(sock, op)).into_response(), &rec)
}

/// The WebSocket ↔ helper stdin/stdout lines.
async fn pipe(mut sock: WebSocket, op: String) {
    let args = loop {
        match sock.recv().await {
            Some(Ok(Message::Text(t))) => break t.to_string(),
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
            _ => return,
        }
    };
    let Some(path) = helper_path() else {
        let _ = sock.send(Message::Text(json!({"error": "the macOS helper is not installed"}).to_string().into())).await;
        return;
    };
    let Ok(mut child) = Command::new(path).arg(&op).arg(&args).arg("--session").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true).spawn() else {
        let _ = sock.send(Message::Text(json!({"error": "could not start the macOS helper"}).to_string().into())).await;
        return;
    };
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut stderr = child.stderr.take().unwrap();
    let (mut tx, mut rx) = sock.split();
    loop {
        tokio::select! {
            m = rx.next() => match m {
                Some(Ok(Message::Text(t))) => {
                    let mut l = t.to_string().replace('\n', " ");
                    l.push('\n');
                    if stdin.write_all(l.as_bytes()).await.is_err() { break; }
                }
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                _ => break,
            },
            l = lines.next_line() => match l {
                Ok(Some(l)) => { if tx.send(Message::Text(l.into())).await.is_err() { break; } }
                _ => {
                    let mut e = vec![];
                    let _ = stderr.read_to_end(&mut e).await;
                    let ok = child.wait().await.map(|s| s.success()).unwrap_or(false);
                    if !ok {
                        let _ = tx.send(Message::Text(json!({"error": helper_error(&e)}).to_string().into())).await;
                    }
                    break;
                }
            },
        }
    }
    drop(stdin);
    let _ = tx.close().await;
    let _ = child.kill().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_errors() {
        assert_eq!(helper_error(b"warming up\n{\"error\":\"no text found\"}\n"), "no text found");
        assert_eq!(helper_error(b"plain failure\n\n"), "plain failure");
        assert_eq!(helper_error(b""), "the helper failed");
    }
}
