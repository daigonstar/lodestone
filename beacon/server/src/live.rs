//! Live connections: one WebSocket per signed-in device, used to push events (new messages, welcomes,
//! members joining or leaving). Who gets a community's events is checked against the database on every send,
//! so someone removed from a community stops receiving its events immediately.

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

pub const SUBPROTOCOL: &str = "lodestone-beacon.v1";

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
    /// Add a connection; true if it's the account's first (they just came online).
    fn add(&self, account: i64, device: String, tx: mpsc::UnboundedSender<String>) -> (u64, bool) {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let mut map = self.by_account.lock().unwrap();
        let list = map.entry(account).or_default();
        let first = list.is_empty();
        list.push(Conn { id, device, tx });
        (id, first)
    }

    /// Remove a connection; true if it was the account's last (they went offline).
    fn remove(&self, account: i64, id: u64) -> bool {
        let mut map = self.by_account.lock().unwrap();
        if let Some(v) = map.get_mut(&account) {
            v.retain(|c| c.id != id);
            if v.is_empty() {
                map.remove(&account);
                return true;
            }
        }
        false
    }

    /// Someone's presence or status changed: tell the members of their communities, so their lists update at
    /// once. Not for invisible accounts (that would show when they come and go). The event carries no
    /// details: clients fetch them with their own requests.
    pub fn presence(&self, app: &Shared, account: i64) {
        self.announce(app, account, false);
    }

    /// As `presence`; `even_invisible` is for the moment someone switches to invisible (they appear
    /// offline, which others should see).
    pub fn announce(&self, app: &Shared, account: i64, even_invisible: bool) {
        let found: Option<(String, String)> = app.db.with(|c| {
            use rusqlite::OptionalExtension;
            c.query_row("SELECT username, status FROM accounts WHERE id = ?1", [account], |r| Ok((r.get(0)?, r.get(1)?))).optional()
        }).ok().flatten();
        let Some((username, status)) = found else { return };
        if status == "invisible" && !even_invisible {
            return;
        }
        let communities: Vec<String> = app.db.with(|c| c.prepare("SELECT community FROM members WHERE account = ?1")?.query_map([account], |r| r.get(0))?.collect()).unwrap_or_default();
        for s in communities {
            self.to_community(app, &s, json!({ "type": "presence", "community": s, "username": username }));
        }
    }

    /// Send an event to every connected device of every current member of a community.
    pub fn to_community(&self, app: &Shared, community: &str, event: Value) {
        let members: Vec<i64> = app
            .db
            .with(|c| c.prepare("SELECT account FROM members WHERE community = ?1")?.query_map([community], |r| r.get(0))?.collect())
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

    pub fn member_joined(&self, app: &Shared, community: &str, username: &str) {
        self.to_community(app, community, json!({ "type": "member_joined", "community": community, "username": username }));
    }
}

/// `GET /v1/live`: the token comes in the Sec-WebSocket-Protocol header ("lodestone-beacon.v1, <token>"),
/// never in the URL, so it can't end up in proxy logs.
pub async fn connect(State(app): State<Shared>, headers: HeaderMap, ws: WebSocketUpgrade) -> Result<Response, ApiError> {
    let offered = headers.get("sec-websocket-protocol").and_then(|v| v.to_str().ok()).unwrap_or_default();
    let mut parts = offered.split(',').map(str::trim);
    if parts.next() != Some(SUBPROTOCOL) {
        return Err(ApiError::bad("Use the lodestone-beacon.v1 protocol"));
    }
    let token = parts.next().ok_or_else(|| ApiError::unauthorized("Sign in first"))?;
    let me = session(&app, token)?;
    Ok(ws.protocols([SUBPROTOCOL]).on_upgrade(move |socket| run(app, me, socket)))
}

async fn run(app: Shared, me: Auth, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let (id, first) = app.live.add(me.account, me.device.clone(), tx);
    if first {
        app.live.presence(&app, me.account);
    }
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
    if app.live.remove(me.account, id) {
        app.live.presence(&app, me.account);
    }
}
