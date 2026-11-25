//! Live connections: one WebSocket per signed-in device, used to push events (new messages, welcomes,
//! members joining or leaving). Who gets a space's events is checked against the database on every send,
//! so someone removed from a space stops receiving its events immediately.

use crate::auth::{session, Auth};
use crate::util::ApiError;
use crate::Shared;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use tokio::sync::mpsc;

pub const SUBPROTOCOL: &str = "lodestone-comms.v1";

struct Conn {
    id: u64,
    device: String,
    tx: mpsc::UnboundedSender<String>,
}

#[derive(Default)]
pub struct Hub {
    next: AtomicU64,
    by_account: Mutex<HashMap<i64, Vec<Conn>>>,
}

impl Hub {
    fn add(&self, account: i64, device: String, tx: mpsc::UnboundedSender<String>) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.by_account.lock().unwrap().entry(account).or_default().push(Conn { id, device, tx });
        id
    }

    fn remove(&self, account: i64, id: u64) {
        let mut map = self.by_account.lock().unwrap();
        if let Some(v) = map.get_mut(&account) {
            v.retain(|c| c.id != id);
            if v.is_empty() {
                map.remove(&account);
            }
        }
    }

    /// Send an event to every connected device of every current member of a space.
    pub fn to_space(&self, app: &Shared, space: &str, event: Value) {
        let members: Vec<i64> = app
            .db
            .with(|c| c.prepare("SELECT account FROM members WHERE space = ?1")?.query_map([space], |r| r.get(0))?.collect())
            .unwrap_or_default();
        let text = event.to_string();
        let map = self.by_account.lock().unwrap();
        for m in members {
            for c in map.get(&m).into_iter().flatten() {
                let _ = c.tx.send(text.clone());
            }
        }
    }

    /// Send an event to one device, wherever it's connected.
    pub fn to_device(&self, device: &str, event: Value) {
        let text = event.to_string();
        for c in self.by_account.lock().unwrap().values().flatten().filter(|c| c.device == device) {
            let _ = c.tx.send(text.clone());
        }
    }

    pub fn member_joined(&self, app: &Shared, space: &str, username: &str) {
        self.to_space(app, space, json!({ "type": "member_joined", "space": space, "username": username }));
    }
}

/// `GET /v1/live`: the token comes in the Sec-WebSocket-Protocol header ("lodestone-comms.v1, <token>"),
/// never in the URL, so it can't end up in proxy logs.
pub async fn connect(State(app): State<Shared>, headers: HeaderMap, ws: WebSocketUpgrade) -> Result<Response, ApiError> {
    let offered = headers.get("sec-websocket-protocol").and_then(|v| v.to_str().ok()).unwrap_or_default();
    let mut parts = offered.split(',').map(str::trim);
    if parts.next() != Some(SUBPROTOCOL) {
        return Err(ApiError::bad("Use the lodestone-comms.v1 protocol"));
    }
    let token = parts.next().ok_or_else(|| ApiError::unauthorized("Sign in first"))?;
    let me = session(&app, token)?;
    Ok(ws.protocols([SUBPROTOCOL]).on_upgrade(move |socket| run(app, me, socket)))
}

async fn run(app: Shared, me: Auth, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let id = app.live.add(me.account, me.device.clone(), tx);
    let _ = sink.send(Message::Text(json!({ "type": "hello", "username": me.username }).to_string().into())).await;
    loop {
        tokio::select! {
            out = rx.recv() => match out {
                Some(text) => if sink.send(Message::Text(text.into())).await.is_err() { break },
                None => break,
            },
            incoming = stream.next() => match incoming {
                // Clients only listen on this connection; pings are answered by the library.
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
        }
    }
    app.live.remove(me.account, id);
}
