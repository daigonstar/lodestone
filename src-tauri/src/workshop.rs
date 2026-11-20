//! Steam Workshop: what's installed per game (from Steam's own files) plus item details from Steam's public API.

use crate::net;
use crate::stores::steam;
use crate::vdf;
use serde::Serialize;
use std::fs;

#[derive(Serialize, Clone, Default)]
pub struct WorkshopItem {
    pub id: String,
    pub title: String,
    pub preview: Option<String>,
    pub size: u64,
    pub updated: u64,
    pub subscriptions: u64,
    pub path: Option<String>,
}

fn installed(appid: u32) -> Vec<WorkshopItem> {
    let mut out = Vec::new();
    for lib in steam::library_dirs() {
        let ws = lib.join("steamapps/workshop");
        let Ok(txt) = fs::read_to_string(ws.join(format!("appworkshop_{appid}.acf"))) else { continue };
        let v = vdf::parse(&txt);
        let Some(items) = v.get("AppWorkshop").and_then(|a| a.get("WorkshopItemsInstalled")) else { continue };
        for (id, it) in items.entries() {
            let dir = ws.join("content").join(appid.to_string()).join(id);
            out.push(WorkshopItem {
                id: id.clone(),
                title: format!("Item {id}"),
                size: it.str("size").and_then(|s| s.parse().ok()).unwrap_or(0),
                updated: it.str("timeupdated").and_then(|s| s.parse().ok()).unwrap_or(0),
                path: dir.is_dir().then(|| dir.to_string_lossy().into_owned()),
                ..Default::default()
            });
        }
    }
    out
}

/// Installed Workshop items for a game, with titles and preview images when Steam's API is reachable.
pub async fn list(appid: u32) -> Result<Vec<WorkshopItem>, String> {
    let mut items = installed(appid);
    if items.is_empty() {
        return Ok(items);
    }
    let mut form: Vec<(String, String)> = vec![("itemcount".into(), items.len().to_string())];
    for (i, it) in items.iter().enumerate() {
        form.push((format!("publishedfileids[{i}]"), it.id.clone()));
    }
    let resp = net::client()
        .post("https://api.steampowered.com/ISteamRemoteStorage/GetPublishedFileDetails/v1/")
        .form(&form)
        .send()
        .await;
    if let Ok(r) = resp {
        if let Ok(v) = r.json::<serde_json::Value>().await {
            let details = v["response"]["publishedfiledetails"].as_array().cloned().unwrap_or_default();
            for d in details {
                let Some(id) = d["publishedfileid"].as_str() else { continue };
                if let Some(it) = items.iter_mut().find(|i| i.id == id) {
                    if let Some(t) = d["title"].as_str() {
                        it.title = t.to_string();
                    }
                    it.preview = d["preview_url"].as_str().filter(|s| !s.is_empty()).map(String::from);
                    it.subscriptions = d["lifetime_subscriptions"].as_u64().unwrap_or(0);
                }
            }
        }
    }
    items.sort_by_key(|i| i.title.to_lowercase());
    Ok(items)
}
