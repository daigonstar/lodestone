//! Game series from Wikidata, looked up by Steam app id (P1733). Epic/GOG games are first matched to
//! their Steam app by exact title via Steam's public store search (Wikidata rarely has their store ids).
//! Preference: the game's media franchise (P8345, e.g. "Final Fantasy"), else its top-level
//! "part of the series" (P179). Results are cached in ~/.local/share/lodestone/series.json.

use crate::net;
use crate::state;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;

const UA: &str = concat!("lodestone-launcher/", env!("CARGO_PKG_VERSION"), " (game library series lookup)");
/// Wikidata's query service slows sharply with bigger batches; 25 ids ≈ 6 s.
const BATCH: usize = 25;
/// Concurrent queries (Wikidata allows a handful per client).
const PARALLEL: usize = 4;
/// Games with no series are re-checked after this long (Wikidata gets edited).
const RECHECK_SECS: u64 = 30 * 24 * 3600;

#[derive(Serialize, Deserialize, Clone, Default)]
struct Entry {
    series: Option<String>,
    checked: u64,
    /// Non-Steam games: Steam app ids with the same title (None = not resolved yet).
    #[serde(default)]
    steam: Option<Vec<String>>,
    /// Non-Steam games: whether the matched Steam app is tagged like a Diablo-style ARPG.
    #[serde(default)]
    arpg: Option<bool>,
}

#[derive(Serialize, Deserialize, Default)]
struct Cache {
    games: HashMap<String, Entry>,
}

fn file() -> std::path::PathBuf {
    state::data_dir().join("series.json")
}

fn load() -> Cache {
    fs::read_to_string(file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save(c: &Cache) {
    let _ = fs::create_dir_all(state::data_dir());
    let _ = fs::write(file(), serde_json::to_string(c).unwrap_or_default());
}

/// Non-Steam games whose matched Steam app counts as an isometric ARPG.
pub fn cached_arpg() -> std::collections::HashSet<String> {
    load().games.into_iter().filter(|(_, e)| e.arpg == Some(true)).map(|(k, _)| k).collect()
}

/// Steam store tags for many apps (public API, no key), 50 per request.
async fn steam_tags(appids: Vec<String>) -> HashMap<String, std::collections::HashSet<u32>> {
    let mut out = HashMap::new();
    for chunk in appids.chunks(50) {
        let ids: Vec<serde_json::Value> = chunk.iter().filter_map(|a| a.parse::<u64>().ok()).map(|a| serde_json::json!({ "appid": a })).collect();
        let input = serde_json::json!({ "ids": ids, "context": { "language": "english", "country_code": "US" }, "data_request": { "include_tag_count": 20 } });
        let url = format!("https://api.steampowered.com/IStoreBrowseService/GetItems/v1/?input_json={}", urlencoding::encode(&input.to_string()));
        let Ok(v) = net::get_json(&url, &[]).await else { continue };
        for it in v["response"]["store_items"].as_array().into_iter().flatten() {
            if let Some(a) = it["appid"].as_u64() {
                let tags = it["tags"].as_array().into_iter().flatten().filter_map(|t| t["tagid"].as_u64().map(|x| x as u32)).collect();
                out.insert(a.to_string(), tags);
            }
        }
    }
    out
}

/// Non-Steam games matched to a Steam app by title ("epic:abc" -> "1174180").
pub fn cached_steam() -> HashMap<String, String> {
    load().games.into_iter().filter_map(|(k, e)| e.steam.and_then(|v| v.into_iter().next()).map(|a| (k, a))).collect()
}

/// Cached series by Lodestone game id ("steam:220" -> "Half-Life").
pub fn cached() -> HashMap<String, String> {
    load().games.into_iter().filter_map(|(k, e)| e.series.map(|s| (k, s))).collect()
}

fn qid(v: &serde_json::Value) -> Option<String> {
    v["value"].as_str().and_then(|u| u.rsplit('/').next()).map(String::from)
}

async fn sparql(query: String) -> Result<serde_json::Value, String> {
    let r = net::client()
        .post("https://query.wikidata.org/sparql")
        .header("user-agent", UA)
        .header("accept", "application/sparql-results+json")
        .form(&[("query", query.as_str()), ("format", "json")])
        .send()
        .await
        .map_err(net::err)?;
    if !r.status().is_success() {
        return Err(format!("Wikidata returned {}", r.status()));
    }
    r.json().await.map_err(net::err)
}

/// store ids -> Wikidata item id of the franchise or top-level series.
async fn lookup(prop: &str, ids: &[String]) -> Result<HashMap<String, String>, String> {
    let values = ids.iter().map(|i| format!("\"{}\"", i.replace('"', ""))).collect::<Vec<_>>().join(" ");
    let q = format!(
        "SELECT ?id ?f0 ?of ?os1 ?s1 ?f1 ?s2 ?f2 WHERE {{
          VALUES ?id {{ {values} }}
          ?game wdt:{prop} ?id .
          OPTIONAL {{ ?game wdt:P8345 ?f0 }}
          OPTIONAL {{ ?game wdt:P629 ?orig . OPTIONAL {{ ?orig wdt:P8345 ?of }} OPTIONAL {{ ?orig wdt:P179 ?os1 }} }}
          OPTIONAL {{ ?game wdt:P179 ?s1 . OPTIONAL {{ ?s1 wdt:P8345 ?f1 }} OPTIONAL {{ ?s1 wdt:P179 ?s2 . OPTIONAL {{ ?s2 wdt:P8345 ?f2 }} }} }}
        }}"
    );
    let v = sparql(q).await?;
    // (item, rank): franchise beats series; first franchise found wins.
    let mut pick: HashMap<String, (String, u8)> = HashMap::new();
    for r in v["results"]["bindings"].as_array().into_iter().flatten() {
        let Some(id) = r["id"]["value"].as_str() else { continue };
        let g = |k: &str| qid(&r[k]);
        let franchise = g("f0").or_else(|| g("of")).or_else(|| g("f2")).or_else(|| g("f1"));
        let (cand, rank) = match franchise {
            Some(f) => (Some(f), 0),
            None => (g("s2").or_else(|| g("s1")).or_else(|| g("os1")), 1),
        };
        if let Some(c) = cand {
            if pick.get(id).is_none_or(|cur| rank < cur.1) {
                pick.insert(id.to_string(), (c, rank));
            }
        }
    }
    Ok(pick.into_iter().map(|(k, v)| (k, v.0)).collect())
}

/// English (or language-neutral) names for Wikidata items.
async fn labels(qids: Vec<String>) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut q = qids;
    q.sort();
    q.dedup();
    for chunk in q.chunks(50) {
        let url = format!("https://www.wikidata.org/w/api.php?action=wbgetentities&format=json&props=labels&languages=en|mul&ids={}", chunk.join("|"));
        let Ok(r) = net::client().get(&url).header("user-agent", UA).send().await else { continue };
        let Ok(v) = r.json::<serde_json::Value>().await else { continue };
        if let Some(ents) = v["entities"].as_object() {
            for (k, e) in ents {
                let l = &e["labels"];
                if let Some(name) = l["en"]["value"].as_str().or(l["mul"]["value"].as_str()) {
                    out.insert(k.clone(), name.to_string());
                }
            }
        }
    }
    out
}

/// Same normalization as the UI's merge key: case, accents, ™/®, punctuation and edition words ignored.
pub fn title_key(t: &str) -> String {
    const SKIP: &[&str] = &["the", "edition", "goty", "definitive", "complete", "deluxe", "enhanced", "ultimate", "anniversary", "remastered", "special", "standard", "gold", "digital"];
    let lower = t.to_lowercase().replace('&', " and ").replace(['\'', '’'], "");
    let cleaned: String = lower.chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect();
    let words: Vec<&str> = cleaned.split_whitespace().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < words.len() {
        // multi-word edition phrases
        if words[i..].starts_with(&["game", "of", "the", "year"]) {
            i += 4;
            continue;
        }
        if words[i..].starts_with(&["directors", "cut"]) {
            i += 2;
            continue;
        }
        if !SKIP.contains(&words[i]) {
            out.push(words[i]);
        }
        i += 1;
    }
    out.join(" ")
}

/// Steam app ids whose store name matches this title exactly (after normalization).
pub async fn steam_ids_for(title: &str) -> Option<Vec<String>> {
    let url = format!("https://store.steampowered.com/api/storesearch/?term={}&cc=us&l=en", urlencoding::encode(title));
    let v = net::get_json(&url, &[]).await.ok()?;
    let want = title_key(title);
    Some(
        v["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|i| i["name"].as_str().map(title_key).as_deref() == Some(want.as_str()))
            .filter_map(|i| i["id"].as_u64().map(|n| n.to_string()))
            .take(4)
            .collect(),
    )
}

async fn parallel<T: Send + 'static, R: Send + 'static, F>(items: Vec<T>, f: F) -> Vec<R>
where
    F: Fn(T) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<R>> + Send>> + Send + Sync + 'static,
{
    let queue = Arc::new(AsyncMutex::new(items));
    let out: Arc<AsyncMutex<Vec<R>>> = Arc::default();
    let f = Arc::new(f);
    let workers: Vec<_> = (0..PARALLEL)
        .map(|_| {
            let (queue, out, f) = (queue.clone(), out.clone(), f.clone());
            tauri::async_runtime::spawn(async move {
                loop {
                    let Some(item) = queue.lock().await.pop() else { break };
                    if let Some(r) = f(item).await {
                        out.lock().await.push(r);
                    }
                }
            })
        })
        .collect();
    for w in workers {
        let _ = w.await;
    }
    Arc::try_unwrap(out).map(|m| m.into_inner()).unwrap_or_default()
}

static RUNNING: AsyncMutex<()> = AsyncMutex::const_new(());

/// Look up series for games not yet (or not recently) checked. `games` = (Lodestone id, title).
/// Returns how many games got a series.
pub async fn refresh(games: Vec<(String, String)>) -> Result<usize, String> {
    let _guard = RUNNING.try_lock().map_err(|_| "already updating series".to_string())?;
    let now = state::now();
    let mut cache = load();
    let due = |id: &String, c: &Cache| match c.games.get(id) {
        None => true,
        Some(e) if !id.starts_with("steam:") && e.steam.is_none() => true,
        Some(e) => e.series.is_none() && now.saturating_sub(e.checked) > RECHECK_SECS,
    };
    let due_games: Vec<(String, String)> = games.into_iter().filter(|(id, _)| (id.starts_with("steam:") || id.starts_with("epic:") || id.starts_with("gog:")) && due(id, &cache)).collect();
    if due_games.is_empty() {
        tag_missing(&mut cache).await;
        save(&cache);
        return Ok(0);
    }

    // 1. Non-Steam games -> Steam app ids by exact title.
    let others: Vec<(String, String)> = due_games.iter().filter(|(id, _)| !id.starts_with("steam:")).cloned().collect();
    let resolved = parallel(others, |(id, title)| Box::pin(async move { steam_ids_for(&title).await.map(|ids| (id, ids)) })).await;
    let mut candidates: Vec<(String, Vec<String>)> = Vec::new();
    for (id, ids) in resolved {
        cache.games.entry(id.clone()).or_default().steam = Some(ids.clone());
        candidates.push((id, ids));
    }
    for (id, _) in due_games.iter().filter(|(id, _)| id.starts_with("steam:")) {
        candidates.push((id.clone(), vec![id["steam:".len()..].to_string()]));
    }

    // 2. Steam app ids -> Wikidata franchise/series.
    let mut appids: Vec<String> = candidates.iter().flat_map(|(_, v)| v.clone()).collect();
    appids.sort();
    appids.dedup();
    let batches: Vec<Vec<String>> = appids.chunks(BATCH).map(|c| c.to_vec()).collect();
    let found: HashMap<String, String> = parallel(batches, |ids| {
        Box::pin(async move {
            // One retry: the query service occasionally times out.
            match lookup("P1733", &ids).await {
                Ok(f) => Some(f),
                Err(_) => lookup("P1733", &ids).await.ok(),
            }
        })
    })
    .await
    .into_iter()
    .flatten()
    .collect();
    let names = labels(found.values().cloned().collect()).await;

    let mut n = 0;
    for (id, ids) in candidates {
        let series = ids.iter().find_map(|a| found.get(a)).and_then(|q| names.get(q).cloned());
        n += series.is_some() as usize;
        let e = cache.games.entry(id).or_default();
        e.series = series;
        e.checked = now;
    }
    tag_missing(&mut cache).await;
    save(&cache);
    Ok(n)
}

/// Fill `arpg` for non-Steam games matched to Steam apps but not yet tagged.
async fn tag_missing(cache: &mut Cache) {
    let todo: Vec<(String, Vec<String>)> = cache
        .games
        .iter()
        .filter(|(id, e)| !id.starts_with("steam:") && e.arpg.is_none() && e.steam.as_ref().is_some_and(|s| !s.is_empty()))
        .map(|(id, e)| (id.clone(), e.steam.clone().unwrap_or_default()))
        .collect();
    if todo.is_empty() {
        return;
    }
    let mut appids: Vec<String> = todo.iter().flat_map(|(_, v)| v.clone()).collect();
    appids.sort();
    appids.dedup();
    let tags = steam_tags(appids).await;
    for (id, apps) in todo {
        let looked_up = apps.iter().any(|a| tags.contains_key(a));
        if looked_up {
            let arpg = apps.iter().filter_map(|a| tags.get(a)).any(crate::model::is_isometric_arpg);
            if let Some(e) = cache.games.get_mut(&id) {
                e.arpg = Some(arpg);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    /// `cargo test live_series -- --ignored --nocapture`: look up the real library on Wikidata.
    #[test]
    #[ignore]
    fn live_series() {
        let mut games = crate::stores::scan_all();
        let ids: Vec<(String, String)> = games.iter().map(|g| (g.id.clone(), g.title.clone())).collect();
        let t = std::time::Instant::now();
        let n = tauri::async_runtime::block_on(super::refresh(ids)).unwrap();
        println!("found {n} new series in {:?}", t.elapsed());
        crate::apply_series(&mut games);
        let mut groups: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        for g in &games {
            if let Some(s) = &g.series {
                groups.entry(s.to_lowercase()).or_default().push(g.title.clone());
            }
        }
        groups.retain(|_, v| {
            v.sort();
            v.dedup();
            v.len() >= 2
        });
        println!("{} series with 2+ games, {} games in them", groups.len(), groups.values().map(|v| v.len()).sum::<usize>());
        for (k, v) in &groups {
            println!("  {k}: {}", v.join(" | "));
        }
    }
}

#[cfg(test)]
mod key_tests {
    #[test]
    fn title_keys_match_store_variants() {
        use super::title_key as k;
        assert_eq!(k("BioShock® 2"), k("BioShock 2 Remastered"));
        assert_eq!(k("BioShock™ 2 Remastered"), "bioshock 2");
        assert_eq!(k("The Witcher 3: Wild Hunt — Game of the Year Edition"), k("The Witcher 3: Wild Hunt"));
        assert_ne!(k("BioShock 2"), k("BioShock"));
        assert_ne!(k("Half-Life 2"), k("Half-Life"));
    }
}

#[cfg(test)]
mod arpg_tests {
    /// `cargo test live_arpg -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_arpg() {
        let mut games = crate::stores::scan_all();
        let ids: Vec<(String, String)> = games.iter().map(|g| (g.id.clone(), g.title.clone())).collect();
        let _ = tauri::async_runtime::block_on(super::refresh(ids));
        crate::apply_series(&mut games);
        let mut hits: Vec<String> = games.iter().filter(|g| g.genres.iter().any(|x| x == crate::model::ISO_ARPG)).map(|g| format!("{} [{}]", g.title, g.store)).collect();
        hits.sort();
        println!("{} games:\n  {}", hits.len(), hits.join("\n  "));
    }
}
