//! The hosts you've joined: one encrypted client per host, each with its own folder, kept in sync in the
//! background.

use crate::Shared;
use lodestone_comms_client::Client;
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct Hosts {
    dir: PathBuf,
    clients: Mutex<HashMap<String, Arc<Client>>>,
}

/// A folder name for a host URL ("https://comms.example.com" -> "https_comms.example.com").
fn folder(url: &str) -> String {
    url.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect()
}

impl Hosts {
    pub fn load(dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
        let urls: Vec<String> = std::fs::read_to_string(dir.join("hosts.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let mut clients = HashMap::new();
        for u in urls {
            match Client::open(&dir.join("hosts").join(folder(&u)), &u) {
                Ok(c) => {
                    clients.insert(u, Arc::new(c));
                }
                Err(e) => eprintln!("comms: can't open the data for a host: {e}"),
            }
        }
        Self { dir, clients: Mutex::new(clients) }
    }

    pub fn all(&self) -> Vec<(String, Arc<Client>)> {
        let mut v: Vec<_> = self.clients.lock().unwrap().iter().map(|(u, c)| (u.clone(), c.clone())).collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    pub fn get(&self, url: &str) -> Option<Arc<Client>> {
        self.clients.lock().unwrap().get(url.trim_end_matches('/')).cloned()
    }

    /// The client for a host, opening (and remembering) it if it's new.
    pub fn open(&self, url: &str) -> Result<Arc<Client>, String> {
        let url = url.trim_end_matches('/').to_string();
        if !(url.starts_with("https://") || url.starts_with("http://127.0.0.1") || url.starts_with("http://localhost")) {
            return Err("Hosts must use https:// (plain http only for this PC)".into());
        }
        if let Some(c) = self.get(&url) {
            return Ok(c);
        }
        let c = Arc::new(Client::open(&self.dir.join("hosts").join(folder(&url)), &url).map_err(|e| e.to_string())?);
        let mut map = self.clients.lock().unwrap();
        map.insert(url, c.clone());
        let urls: Vec<&String> = map.keys().collect();
        let _ = std::fs::write(self.dir.join("hosts.json"), serde_json::to_string(&urls).unwrap_or_default());
        Ok(c)
    }
}

/// Every few seconds: sign in where needed, sync each host, and tell the interface what changed.
pub fn start_sync(app: Shared) {
    tokio::spawn(async move {
        let mut known: HashMap<String, usize> = HashMap::new();
        loop {
            for (url, c) in app.hosts.all() {
                if c.username().is_none() {
                    continue;
                }
                if !c.host.signed_in() && c.sign_in().await.is_err() {
                    continue;
                }
                match c.sync().await {
                    Ok(new) => {
                        let mut chans: Vec<&String> = new.iter().map(|m| &m.channel).collect();
                        chans.dedup();
                        if !chans.is_empty() {
                            let _ = app.events.send(json!({ "type": "messages", "host": url, "channels": chans }).to_string());
                        }
                        let count = c.channels().map(|v| v.len()).unwrap_or(0);
                        if known.insert(url.clone(), count) != Some(count) {
                            let _ = app.events.send(json!({ "type": "state" }).to_string());
                        }
                    }
                    Err(lodestone_comms_client::Error::Host(e)) if e.status == 401 => c.host.set_token(None),
                    Err(_) => {}
                }
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    });
}
