//! `/v1/relay/tcp?target=host:port` (WebSocket) — copies bytes, unchanged, between the WebSocket
//! and one new TCP connection to a target on the user's allow-list (as postgres-relay.py, the
//! Redis bridge, and aviation-relay.py's Beast/raw feeds). Binary frames both ways; with
//! `&mode=text` the server sends text frames (Mode S AVR). The token comes in the
//! `Sec-WebSocket-Protocol` header as `bearer.<token>` — never in the URL.

use super::{authorize, err, settings::parse_target, tagged, Gateway, Origin};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::{HeaderMap, StatusCode},
    response::Response,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[derive(Deserialize)]
pub struct TcpQuery {
    #[serde(default)]
    target: String,
    #[serde(default)]
    mode: String,
}

fn canon(h: &str) -> &str {
    if h == "localhost" || h == "::1" {
        "127.0.0.1"
    } else {
        h
    }
}

/// Is `target` on the allow-list? `localhost`, `127.0.0.1` and `::1` count as the same host.
pub fn allowed(target: &str, allow: &[String]) -> Option<(String, u16)> {
    let (h, p) = parse_target(target)?;
    allow
        .iter()
        .filter_map(|a| parse_target(a))
        .any(|(ah, ap)| ap == p && canon(&ah) == canon(&h))
        .then_some((h, p))
}

pub async fn handle(
    State(gw): State<Arc<Gateway>>,
    axum::Extension(Origin(origin)): axum::Extension<Origin>,
    h: HeaderMap,
    Query(q): Query<TcpQuery>,
    ws: Result<WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>,
) -> Response {
    let s = gw.settings();
    if !s.relays.tcp {
        return err(StatusCode::SERVICE_UNAVAILABLE, "the TCP relay is turned off in the companion settings");
    }
    let (rec, presented) = match authorize(&gw, &h, &origin, "relay.tcp") {
        Ok(x) => x,
        Err(r) => return r,
    };
    let Ok(ws) = ws else {
        return tagged(err(StatusCode::BAD_REQUEST, "this endpoint is a WebSocket"), &rec);
    };
    let Some((host, port)) = allowed(&q.target, &s.tcp_allow) else {
        return tagged(
            err(StatusCode::FORBIDDEN, format!("target {:?} is not on the TCP allow-list (add it in the app's Companion settings)", q.target)),
            &rec,
        );
    };
    let up = match tokio::time::timeout(Duration::from_secs(10), TcpStream::connect((host.as_str(), port))).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return tagged(err(StatusCode::BAD_GATEWAY, format!("cannot connect to {host}:{port}: {e}")), &rec),
        Err(_) => return tagged(err(StatusCode::GATEWAY_TIMEOUT, format!("timed out connecting to {host}:{port}")), &rec),
    };
    let _ = up.set_nodelay(true);
    let text = q.mode == "text";
    let mut protos = vec!["gw-companion.v1".to_string()];
    protos.extend(presented.subprotocol);
    let r = ws.protocols(protos).max_message_size(16 << 20).on_upgrade(move |sock| pump(sock, up, text));
    tagged(r, &rec)
}

async fn pump(sock: WebSocket, up: TcpStream, text: bool) {
    let (mut tx, mut rx) = sock.split();
    let (mut r, mut w) = up.into_split();
    let to_tcp = async {
        while let Some(Ok(m)) = rx.next().await {
            let data = match m {
                Message::Binary(b) => b.to_vec(),
                Message::Text(t) => t.as_str().as_bytes().to_vec(),
                Message::Close(_) => break,
                _ => continue, // pings are answered by the library
            };
            if w.write_all(&data).await.is_err() {
                break;
            }
        }
        let _ = w.shutdown().await;
    };
    let to_ws = async {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match r.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let m = if text {
                        Message::Text(String::from_utf8_lossy(&buf[..n]).into_owned().into())
                    } else {
                        Message::Binary(buf[..n].to_vec().into())
                    };
                    if tx.send(m).await.is_err() {
                        break;
                    }
                }
            }
        }
        let _ = tx.send(Message::Close(None)).await;
    };
    tokio::select! {
        _ = to_tcp => {},
        _ = to_ws => {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn allow_list() {
        let a = vec!["127.0.0.1:5432".to_string(), "pi.local:30005".to_string()];
        assert!(allowed("127.0.0.1:5432", &a).is_some());
        assert!(allowed("localhost:5432", &a).is_some());
        assert!(allowed("PI.local:30005", &a).is_some());
        assert!(allowed("127.0.0.1:5433", &a).is_none());
        assert!(allowed("10.0.0.1:5432", &a).is_none());
        assert!(allowed("pi.local", &a).is_none());
        assert!(allowed("", &a).is_none());
    }
}
