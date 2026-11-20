//! GOG's public games database (gamesdb.gog.com) knows releases from GOG, Epic, Steam and others:
//! vertical covers, genres and series. Used for GOG and Epic games, whose stores don't give us genres.

use crate::net;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct Meta {
    pub cover: Option<String>,
    pub genres: Vec<String>,
    pub series: Option<String>,
}

/// `platform` is "gog" (product id) or "epic" (legendary app_name).
pub async fn lookup(platform: &str, id: &str) -> Option<Meta> {
    let v = net::get_json(&format!("https://gamesdb.gog.com/platforms/{platform}/external_releases/{}", urlencoding::encode(id)), &[]).await.ok()?;
    let g = &v["game"];
    let raw: Vec<String> = g["genres"].as_array().into_iter().flatten().filter_map(|x| x["name"]["*"].as_str().map(String::from)).collect();
    Some(Meta {
        cover: g["vertical_cover"]["url_format"].as_str().map(|f| f.replace("{formatter}", "").replace("{ext}", "jpg")),
        genres: crate::model::normalize_genres(raw.iter().map(String::as_str)),
        series: g["series"]["name"].as_str().filter(|s| !s.is_empty()).map(String::from),
    })
}

/// Look up many ids, 12 at a time. Missing entries come back as None.
pub async fn lookup_many(platform: &'static str, ids: Vec<String>) -> Vec<(String, Option<Meta>)> {
    let mut out = Vec::new();
    for chunk in ids.chunks(12) {
        let handles: Vec<_> = chunk
            .iter()
            .map(|id| {
                let id = id.clone();
                tauri::async_runtime::spawn(async move {
                    let m = lookup(platform, &id).await;
                    (id, m)
                })
            })
            .collect();
        for h in handles {
            if let Ok(r) = h.await {
                out.push(r);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    /// `cargo test live_genres -- --ignored --nocapture`: refresh Epic/GOG metadata and report coverage.
    #[test]
    #[ignore]
    fn live_genres() {
        tauri::async_runtime::block_on(async {
            if crate::stores::epic::account().is_some() {
                println!("epic refresh: {:?}", crate::stores::epic::refresh().await);
            }
            if crate::stores::gog::account().is_some() {
                println!("gog refresh: {:?}", crate::stores::gog::refresh().await);
            }
        });
        let all = crate::stores::scan_all();
        let mut by: std::collections::BTreeMap<String, (usize, usize, usize)> = Default::default();
        for g in &all {
            let e = by.entry(g.store.clone()).or_default();
            e.0 += 1;
            e.1 += (!g.genres.is_empty()) as usize;
            e.2 += g.series.is_some() as usize;
        }
        for (s, (n, gn, sr)) in by {
            println!("{s:<10} {n:>4} games  {gn:>4} with genres  {sr:>4} with series");
        }
        let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
        for g in &all {
            for x in &g.genres {
                *counts.entry(x.as_str()).or_default() += 1;
            }
        }
        println!("{counts:?}");
        for id in ["steam:2909400", "steam:292030", "battlenet:fen", "battlenet:d3"] {
            if let Some(g) = all.iter().find(|g| g.id == id) {
                println!("{} -> {:?} / {:?}", g.title, g.genres, g.series);
            }
        }
    }
}
