//! The bridge to Lodestone over standard input and output (see PLUGIN.md). Lodestone starts the plug-in
//! with `--lodestone-bridge`; without it there's no bridge.

use crate::Shared;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{oneshot, Notify};

pub const PERMISSIONS: &[&str] = &["friends:read", "chat:open", "game:read"];

#[derive(Default)]
pub struct Bridge {
    connected: AtomicBool,
    next: AtomicU64,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>,
    out: tokio::sync::Mutex<Option<tokio::io::Stdout>>,
    gone: Notify,
    pub theme: Mutex<Option<Value>>,
}

impl Bridge {
    pub fn connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    async fn write(&self, v: &Value) -> bool {
        let mut out = self.out.lock().await;
        let Some(o) = out.as_mut() else { return false };
        let line = format!("{v}\n");
        o.write_all(line.as_bytes()).await.is_ok() && o.flush().await.is_ok()
    }

    /// Ask Lodestone for something; Lodestone refuses methods you haven't approved.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        if !self.connected() {
            return Err("Open Comms from Lodestone to use this".into());
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        if !self.write(&json!({ "type": "request", "id": id, "method": method, "params": params })).await {
            self.pending.lock().unwrap().remove(&id);
            return Err("Lost the connection to Lodestone".into());
        }
        match tokio::time::timeout(Duration::from_secs(30), rx).await {
            Ok(Ok(r)) => r,
            _ => {
                self.pending.lock().unwrap().remove(&id);
                Err("Lodestone didn't answer".into())
            }
        }
    }

    /// Resolves when Lodestone closes the bridge (never, when there's no bridge).
    pub async fn closed(&self) {
        if !self.connected() {
            std::future::pending::<()>().await;
        }
        self.gone.notified().await;
    }
}

/// Start the bridge if Lodestone launched us: send the hello, then read Lodestone's lines.
pub fn start(app: Shared, ui: &str) -> bool {
    if !std::env::args().any(|a| a == "--lodestone-bridge") {
        return false;
    }
    app.bridge.connected.store(true, Ordering::Relaxed);
    let hello = json!({ "type": "hello", "name": "Lodestone Comms", "version": env!("CARGO_PKG_VERSION"), "ui": ui, "permissions": PERMISSIONS });
    let app2 = app.clone();
    tokio::spawn(async move {
        *app2.bridge.out.lock().await = Some(tokio::io::stdout());
        app2.bridge.write(&hello).await;
        let mut lines = BufReader::new(tokio::io::stdin()).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
            match msg["type"].as_str() {
                Some("response") => {
                    let id = msg["id"].as_u64().unwrap_or(0);
                    if let Some(tx) = app2.bridge.pending.lock().unwrap().remove(&id) {
                        let r = if msg["error"].is_string() { Err(msg["error"].as_str().unwrap_or_default().to_string()) } else { Ok(msg["ok"].clone()) };
                        let _ = tx.send(r);
                    }
                }
                Some("event") if msg["name"] == "theme" => {
                    *app2.bridge.theme.lock().unwrap() = Some(msg["data"].clone());
                    let _ = app2.events.send(json!({ "type": "theme", "data": msg["data"] }).to_string());
                }
                _ => {}
            }
        }
        app2.bridge.connected.store(false, Ordering::Relaxed);
        app2.bridge.gone.notify_waiters();
        app2.bridge.gone.notify_one();
    });
    true
}
