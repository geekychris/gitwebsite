//! `/v1/relay/tcp?target=host:port` (WebSocket) — copies bytes, unchanged, between the WebSocket
//! and one new TCP connection to a target on the user's allow-list (as postgres-relay.py, the
//! Redis bridge, and aviation-relay.py's Beast/raw feeds). Binary frames both ways; with
//! `&mode=text` the server sends text frames (Mode S AVR). The token comes in the
//! `Sec-WebSocket-Protocol` header as `bearer.<token>` — never in the URL. A client that also
//! offers `gw-companion.codes` gets refusals as close codes after the upgrade (4400, 4401,
//! 4403, 4502) instead of an HTTP error a browser cannot see. A paired page's first connection
//! to a new LAN target (v1.2) waits while the app asks the user; approving adds it.

use super::{check_token, err, pairing::TokenRecord, settings::parse_target, tagged, wants_close_codes, ws_close_code, ws_refuse, Gateway, Origin};
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
    let Ok(ws) = ws else {
        return err(StatusCode::BAD_REQUEST, "this endpoint is a WebSocket");
    };
    let codes = wants_close_codes(&h);
    // A refusal: an HTTP error before the upgrade, or (when the client offered
    // `gw-companion.codes`) the upgrade and then a close with an application code.
    let refuse = |ws: WebSocketUpgrade, status: StatusCode, msg: String, rec: Option<&TokenRecord>| {
        let r = if codes { ws_refuse(ws, ws_close_code(status), &msg) } else { err(status, msg) };
        match rec {
            Some(rec) => tagged(r, rec),
            None => r,
        }
    };
    let s = gw.settings();
    if !s.relays.tcp {
        return refuse(ws, StatusCode::SERVICE_UNAVAILABLE, "the TCP relay is turned off in the companion settings".into(), None);
    }
    let (rec, presented) = match check_token(&gw, &h, &origin, "relay.tcp") {
        Ok(x) => x,
        Err((status, msg)) => return refuse(ws, status, msg, None),
    };
    if q.target.trim().is_empty() {
        return refuse(ws, StatusCode::BAD_REQUEST, "missing ?target=host:port".into(), Some(&rec));
    }
    let (host, port) = match allowed(&q.target, &s.tcp_allow) {
        Some(x) => x,
        None => {
            // v1.2: a new LAN target asks the user (and approving adds it); others are refused.
            let Some((h, p)) = parse_target(&q.target) else {
                return refuse(ws, StatusCode::BAD_REQUEST, format!("target {:?} must be host:port", q.target), Some(&rec));
            };
            let lan_addrs = super::lan::resolve(&h, &gw.dns).await;
            let refused = match &lan_addrs {
                Ok(a) if super::lan::own_port_refused(a, p, &gw.own_ports()) => Some(format!("{h}:{p} is the companion's or the app's own port")),
                Ok(_) => None,
                Err(e) => Some(format!("target {:?} is not on the TCP allow-list (add it in the app's Companion settings; only LAN targets are offered for approval: {e})", q.target)),
            };
            if let Some(msg) = refused {
                return refuse(ws, StatusCode::FORBIDDEN, msg, Some(&rec));
            }
            let t = if h.contains(':') { format!("[{h}]:{p}") } else { format!("{h}:{p}") };
            if let Err((status, msg)) = gw.ask_approval("tcp", &t, &origin, &rec.tool, &format!("TCP {t}")).await {
                return refuse(ws, status, msg, Some(&rec));
            }
            (h, p)
        }
    };
    let up = match tokio::time::timeout(Duration::from_secs(10), TcpStream::connect((host.as_str(), port))).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return refuse(ws, StatusCode::BAD_GATEWAY, format!("cannot connect to {host}:{port}: {e}"), Some(&rec)),
        Err(_) => return refuse(ws, StatusCode::GATEWAY_TIMEOUT, format!("timed out connecting to {host}:{port}"), Some(&rec)),
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
