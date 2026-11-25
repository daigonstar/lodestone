//! Store achievements for the details panel. Steam for now, through the Steam Web API with your own key
//! (the same key as friends and play time). ROMs use RetroAchievements instead (stores/retroachievements.rs).
//!
//! Sent to Steam: your key, your Steam ID (read from the Steam client on this PC) and the game's app ID.
//! The game's achievement list is cached for a week in `<data>/achievements/`; your progress for 5 minutes.

use crate::{net, settings, state};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const STEAM_API: &str = "https://api.steampowered.com";
const WEEK: u64 = 7 * 24 * 3600;

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Achievement {
    pub id: String,
    pub title: String,
    pub description: String,
    pub icon: String,
    pub icon_locked: String,
    pub hidden: bool,
    /// Unix time you earned it, 0 if not.
    pub earned_at: u64,
    /// Share of all players who have it, if the store says.
    pub percent: Option<f64>,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Achievements {
    pub source: String,
    pub url: String,
    pub achievements: Vec<Achievement>,
    pub earned: usize,
}

/// What the details panel can say when there's nothing to show.
pub const NEED_KEY: &str = "need-key";

#[derive(Serialize, Deserialize, Default)]
struct Schema {
    fetched: u64,
    achievements: Vec<Achievement>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn schema_file(appid: u32) -> PathBuf {
    state::data_dir().join("achievements").join(format!("steam-{appid}.json"))
}

fn steam_key() -> Option<String> {
    let k = settings::load().steam_api_key.trim().to_string();
    (!k.is_empty()).then_some(k)
}

/// The game's achievements (names, descriptions, icons) from GetSchemaForGame.
fn parse_schema(v: &Value) -> Vec<Achievement> {
    v["game"]["availableGameStats"]["achievements"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|a| {
                    Some(Achievement {
                        id: a["name"].as_str()?.to_string(),
                        title: a["displayName"].as_str().unwrap_or_default().to_string(),
                        description: a["description"].as_str().unwrap_or_default().to_string(),
                        icon: a["icon"].as_str().unwrap_or_default().to_string(),
                        icon_locked: a["icongray"].as_str().unwrap_or_default().to_string(),
                        hidden: a["hidden"].as_i64().unwrap_or(0) == 1,
                        ..Default::default()
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Your unlocks from GetPlayerAchievements: api name -> unlock time.
fn parse_player(v: &Value) -> HashMap<String, u64> {
    v["playerstats"]["achievements"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|a| a["achieved"].as_i64().unwrap_or(0) == 1)
        .filter_map(|a| Some((a["apiname"].as_str()?.to_string(), a["unlocktime"].as_u64().unwrap_or(1).max(1))))
        .collect()
}

/// Rarity from GetGlobalAchievementPercentagesForApp (percent comes as a number or a string).
fn parse_percentages(v: &Value) -> HashMap<String, f64> {
    v["achievementpercentages"]["achievements"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| {
            let p = a["percent"].as_f64().or_else(|| a["percent"].as_str()?.parse().ok())?;
            Some((a["name"].as_str()?.to_string(), p))
        })
        .collect()
}

async fn steam_schema(appid: u32, key: &str) -> Result<Vec<Achievement>, String> {
    let file = schema_file(appid);
    if let Some(s) = fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str::<Schema>(&t).ok()) {
        if now().saturating_sub(s.fetched) < WEEK {
            return Ok(s.achievements);
        }
    }
    let k = urlencoding::encode(key);
    let v = net::get_json(&format!("{STEAM_API}/ISteamUserStats/GetSchemaForGame/v2/?key={k}&appid={appid}&l=english"), &[])
        .await
        .map_err(|e| if e.starts_with("403") { "Steam didn't accept your Web API key".to_string() } else { e })?;
    let mut achievements = parse_schema(&v);
    // Rarity is public (no key) and changes slowly, so it's cached with the list.
    if !achievements.is_empty() {
        if let Ok(p) = net::get_json(&format!("{STEAM_API}/ISteamUserStats/GetGlobalAchievementPercentagesForApp/v2/?gameid={appid}"), &[]).await {
            let pct = parse_percentages(&p);
            for a in &mut achievements {
                a.percent = pct.get(&a.id).copied();
            }
        }
    }
    let _ = fs::create_dir_all(file.parent().unwrap());
    if let Ok(t) = serde_json::to_string(&Schema { fetched: now(), achievements: achievements.clone() }) {
        let _ = fs::write(&file, t);
    }
    Ok(achievements)
}

static RECENT: Mutex<Option<HashMap<u32, (Instant, Achievements)>>> = Mutex::new(None);

/// A Steam game's achievements with your progress. Err(NEED_KEY) without a Steam Web API key.
pub async fn steam(appid: u32) -> Result<Achievements, String> {
    if let Some((at, a)) = RECENT.lock().unwrap().as_ref().and_then(|m| m.get(&appid).cloned()) {
        if at.elapsed() < Duration::from_secs(300) {
            return Ok(a);
        }
    }
    let key = steam_key().ok_or(NEED_KEY)?;
    let mut list = steam_schema(appid, &key).await?;
    let url = format!("https://steamcommunity.com/stats/{appid}/achievements");
    if list.is_empty() {
        return Ok(Achievements { source: "Steam".into(), url, ..Default::default() });
    }
    let me = crate::stores::steam::steam_id64().ok_or("No Steam account found on this PC")?;
    let k = urlencoding::encode(&key);
    let mine = net::get_json(&format!("{STEAM_API}/ISteamUserStats/GetPlayerAchievements/v1/?key={k}&steamid={me}&appid={appid}"), &[])
        .await
        .map_err(|e| {
            if e.starts_with("403") || e.contains("not public") {
                "Steam won't share your achievements: set Game details to Public in your Steam privacy settings".to_string()
            } else {
                e
            }
        })?;
    let got = parse_player(&mine);
    for a in &mut list {
        a.earned_at = got.get(&a.id).copied().unwrap_or(0);
    }
    let out = Achievements { source: "Steam".into(), url, earned: list.iter().filter(|a| a.earned_at > 0).count(), achievements: list };
    RECENT.lock().unwrap().get_or_insert_with(HashMap::new).insert(appid, (Instant::now(), out.clone()));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_steam_achievements() {
        let schema = json!({"game": {"availableGameStats": {"achievements": [
            {"name": "WIN", "displayName": "Winner", "description": "Win once", "icon": "https://x.invalid/a.jpg", "icongray": "https://x.invalid/b.jpg", "hidden": 0},
            {"name": "SECRET", "displayName": "Secret", "hidden": 1}
        ]}}});
        let list = parse_schema(&schema);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].title, "Winner");
        assert!(list[1].hidden);
        let player = json!({"playerstats": {"achievements": [
            {"apiname": "WIN", "achieved": 1, "unlocktime": 1700000000},
            {"apiname": "SECRET", "achieved": 0, "unlocktime": 0}
        ]}});
        let got = parse_player(&player);
        assert_eq!(got.get("WIN"), Some(&1700000000));
        assert!(!got.contains_key("SECRET"));
        let pct = json!({"achievementpercentages": {"achievements": [{"name": "WIN", "percent": "41.5"}, {"name": "SECRET", "percent": 2.25}]}});
        let p = parse_percentages(&pct);
        assert_eq!(p["WIN"], 41.5);
        assert_eq!(p["SECRET"], 2.25);
    }

    /// Real achievements for one of your Steam games: `cargo test --release live_steam_ach -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_steam_ach() {
        let appid = std::env::var("APPID").ok().and_then(|a| a.parse().ok()).unwrap_or(219990);
        match tauri::async_runtime::block_on(steam(appid)) {
            Ok(a) => {
                let rare = a.achievements.iter().filter(|x| x.percent.is_some()).count();
                println!("app {appid}: {} achievements, {} earned, {rare} with rarity", a.achievements.len(), a.earned);
            }
            Err(e) => println!("app {appid}: {e}"),
        }
    }
}
