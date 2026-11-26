//! The servers you've joined: one encrypted client per server, each with its own folder, kept in sync in the
//! background.

use crate::Shared;
use lodestone_beacon_client::Client;
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct Servers {
    dir: PathBuf,
    clients: Mutex<HashMap<String, Arc<Client>>>,
}

/// A folder name for a server URL ("https://beacon.yourdomain.example" -> "https_beacon.yourdomain.example").
fn folder(url: &str) -> String {
    url.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect()
}

impl Servers {
    pub fn load(dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
        let urls: Vec<String> = std::fs::read_to_string(dir.join("servers.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let mut clients = HashMap::new();
        for u in urls {
            match Client::open(&dir.join("servers").join(folder(&u)), &u) {
                Ok(c) => {
                    clients.insert(u, Arc::new(c));
                }
                Err(e) => eprintln!("beacon: can't open the data for a server: {e}"),
            }
        }
        Self { dir, clients: Mutex::new(clients) }
    }

    /// This app's own preferences (theme, accent), kept beside the servers list.
    pub fn prefs_file(&self) -> PathBuf {
        self.dir.join("prefs.json")
    }

    pub fn all(&self) -> Vec<(String, Arc<Client>)> {
        let mut v: Vec<_> = self.clients.lock().unwrap().iter().map(|(u, c)| (u.clone(), c.clone())).collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    pub fn get(&self, url: &str) -> Option<Arc<Client>> {
        self.clients.lock().unwrap().get(url.trim_end_matches('/')).cloned()
    }

    /// The client for a server, opening (and remembering) it if it's new.
    pub fn open(&self, url: &str) -> Result<Arc<Client>, String> {
        let url = url.trim_end_matches('/').to_string();
        if !(url.starts_with("https://") || url.starts_with("http://127.0.0.1") || url.starts_with("http://localhost")) {
            return Err("Servers must use https:// (plain http only for this PC)".into());
        }
        if let Some(c) = self.get(&url) {
            return Ok(c);
        }
        let c = Arc::new(Client::open(&self.dir.join("servers").join(folder(&url)), &url).map_err(|e| e.to_string())?);
        let mut map = self.clients.lock().unwrap();
        map.insert(url, c.clone());
        let urls: Vec<&String> = map.keys().collect();
        let _ = std::fs::write(self.dir.join("servers.json"), serde_json::to_string(&urls).unwrap_or_default());
        Ok(c)
    }
}

/// Every few seconds: sign in where needed, sync each server, and tell the interface what changed.
pub fn start_sync(app: Shared) {
    tokio::spawn(async move {
        let mut known: HashMap<String, usize> = HashMap::new();
        loop {
            for (url, c) in app.servers.all() {
                if c.username().is_none() {
                    continue;
                }
                if !c.server.signed_in() && c.sign_in().await.is_err() {
                    continue;
                }
                match c.sync().await {
                    Ok(new) => {
                        // Mentions of you in new messages from others.
                        if let Some(me) = c.username() {
                            for m in new.iter().filter(|m| m.sender != me && mentions(&m.body, &me)) {
                                *app.mentions.lock().unwrap().entry(format!("{url}|{}", m.channel)).or_default() += 1;
                                let _ = app.events.send(json!({ "type": "mention", "server": url, "channel": m.channel, "from": m.sender }).to_string());
                            }
                        }
                        let mut touched = c.take_touched();
                        if let Some(i) = touched.iter().position(|t| t == "profiles") {
                            touched.remove(i);
                            let _ = app.events.send(json!({ "type": "profiles", "server": url }).to_string());
                        }
                        let mut chans: Vec<&String> = new.iter().map(|m| &m.channel).chain(touched.iter()).collect();
                        chans.sort();
                        chans.dedup();
                        if !chans.is_empty() {
                            let _ = app.events.send(json!({ "type": "messages", "server": url, "channels": chans }).to_string());
                        }
                        let count = c.channels().map(|v| v.len()).unwrap_or(0);
                        if known.insert(url.clone(), count) != Some(count) {
                            let _ = app.events.send(json!({ "type": "state" }).to_string());
                        }
                    }
                    Err(lodestone_beacon_client::Error::Server(e)) if e.status == 401 => c.server.set_token(None),
                    Err(_) => {}
                }
            }
            // Sync again in a few seconds, or at once when a server's live connection says so.
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                _ = app.wake.notified() => {}
            }
        }
    });
}

/// Does `body` mention `@username` (as a whole name, any case)?
pub fn mentions(body: &str, username: &str) -> bool {
    let lower = body.to_lowercase();
    let want = format!("@{}", username.to_lowercase());
    let mut from = 0;
    while let Some(i) = lower[from..].find(&want) {
        let at = from + i;
        let end = at + want.len();
        let before_ok = at == 0 || !lower[..at].chars().last().is_some_and(|c| c.is_alphanumeric() || "_.-".contains(c));
        let after_ok = !lower[end..].chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '-');
        if before_ok && after_ok {
            return true;
        }
        from = end;
    }
    false
}

#[cfg(test)]
mod tests {
    #[test]
    fn finds_whole_mentions_only() {
        assert!(super::mentions("hey @Robb, look", "robb"));
        assert!(super::mentions("@robb", "robb"));
        assert!(super::mentions("cc @robb.", "robb"));
        assert!(!super::mentions("hey @robbie", "robb"));
        assert!(!super::mentions("mail me at x@robb", "robb"));
        assert!(!super::mentions("robb without at", "robb"));
    }
}

/// Keep a live connection to each server you've joined, so changes show up as they happen: new
/// messages wake the sync, and people joining or leaving, statuses and calls refresh the page at once.
/// Reconnects after a pause if it drops; the regular sync carries on either way.
pub fn start_live(app: Shared) {
    tokio::spawn(async move {
        let mut running: std::collections::HashSet<String> = Default::default();
        loop {
            for (url, c) in app.servers.all() {
                if c.username().is_none() || !running.insert(url.clone()) {
                    continue;
                }
                let app = app.clone();
                tokio::spawn(async move {
                    loop {
                        if c.server.signed_in() {
                            let a = app.clone();
                            let server = url.clone();
                            let _ = c
                                .server
                                .listen(move |ev| {
                                    let kind = ev["type"].as_str().unwrap_or_default();
                                    match kind {
                                        "message" | "welcome" => a.wake.notify_one(),
                                        "voice" => {
                                            let _ = a.events.send(json!({ "type": "state" }).to_string());
                                            let _ = a.events.send(json!({ "type": "voice", "server": server, "channel": ev["channel"] }).to_string());
                                            crate::voice::refresh_soon(&a, ev["channel"].as_str().unwrap_or_default());
                                        }
                                        "hello" => {}
                                        _ => {
                                            // Members, statuses, channels, community settings.
                                            a.wake.notify_one();
                                            let _ = a.events.send(json!({ "type": "state" }).to_string());
                                        }
                                    }
                                })
                                .await;
                        }
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                });
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
}

/// Show your communities what you're playing: ask Lodestone every 10 s (only with your `game:read`
/// permission; it gives just the title of the game you launched from it), and tell each server when it
/// changes, then every 5 minutes while you play (so people who open Beacon later see it too). Nothing
/// is sent while you're invisible.
pub fn start_playing(app: Shared) {
    tokio::spawn(async move {
        let mut current: Option<String> = None;
        let mut sent_at: Option<std::time::Instant> = None;
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            if !app.bridge.connected() {
                continue;
            }
            let now = match app.bridge.request("playing", json!({})).await {
                Ok(v) => v["title"].as_str().map(|t| t.chars().take(100).collect::<String>()),
                // Not allowed (or an older Lodestone): ask again later.
                Err(_) => {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    continue;
                }
            };
            let changed = now != current;
            // A game just started: the page offers to share it if you're in a call (or starts sharing,
            // as you chose in settings).
            if changed {
                if let Some(title) = &now {
                    let _ = app.events.send(json!({ "type": "game-started", "title": title }).to_string());
                }
            }
            let renew = now.is_some() && sent_at.is_none_or(|t| t.elapsed() >= Duration::from_secs(5 * 60));
            if !changed && !renew {
                continue;
            }
            for (url, c) in app.servers.all() {
                if c.username().is_none() || !c.server.signed_in() {
                    continue;
                }
                let invisible = c.server.get("/v1/me").await.ok().is_some_and(|me| me["status"] == "invisible");
                if invisible && now.is_some() {
                    continue;
                }
                if c.set_playing(now.as_deref()).await.is_ok() {
                    let _ = app.events.send(json!({ "type": "profiles", "server": url }).to_string());
                }
            }
            current = now;
            sent_at = Some(std::time::Instant::now());
        }
    });
}
