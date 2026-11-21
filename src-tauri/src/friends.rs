//! Friends from your stores, read-only:
//! - Steam: your friends list, online status and the game each friend is playing (Steam Web API with
//!   your key), and which friends own a game (their owned-games lists, cached for a day).
//! - Epic: your friends' names through Lodestone's Epic sign-in. Epic only shares online status over
//!   its chat connection, so Epic friends have no status here.

use crate::{net, settings, state};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Serialize, Clone, Debug)]
pub struct Friend {
    pub store: &'static str,
    pub id: String,
    pub name: String,
    pub avatar: Option<String>,
    /// ingame, online, away, offline, unknown
    pub status: &'static str,
    pub game: Option<String>,
    /// Lodestone game id of what they're playing ("steam:570").
    pub game_id: Option<String>,
    pub last_online: u64,
    pub profile: Option<String>,
}

static CACHE: Mutex<Option<(Instant, Vec<Friend>)>> = Mutex::new(None);

fn steam_key() -> Option<String> {
    let k = settings::load().steam_api_key.trim().to_string();
    (!k.is_empty()).then_some(k)
}

fn steam_friend_ids(v: &Value) -> Vec<String> {
    v["friendslist"]["friends"].as_array().into_iter().flatten().filter_map(|f| f["steamid"].as_str().map(String::from)).collect()
}

fn steam_status(p: &Value) -> &'static str {
    if p["gameid"].as_str().is_some() || p["gameextrainfo"].as_str().is_some() {
        return "ingame";
    }
    match p["personastate"].as_u64().unwrap_or(0) {
        0 => "offline",
        2 | 3 | 4 => "away",
        _ => "online",
    }
}

fn steam_friend(p: &Value) -> Option<Friend> {
    let id = p["steamid"].as_str()?.to_string();
    // Non-Steam games added to Steam get huge ids; only real app ids map to a library game.
    let game_id = p["gameid"].as_str().and_then(|g| g.parse::<u64>().ok()).filter(|g| *g < u32::MAX as u64).map(|g| format!("steam:{g}"));
    Some(Friend {
        store: "steam",
        name: p["personaname"].as_str().unwrap_or("Steam user").to_string(),
        avatar: p["avatarmedium"].as_str().map(String::from),
        status: steam_status(p),
        game: p["gameextrainfo"].as_str().map(String::from),
        game_id,
        last_online: p["lastlogoff"].as_u64().unwrap_or(0),
        profile: p["profileurl"].as_str().map(String::from),
        id,
    })
}

async fn steam() -> Result<Vec<Friend>, String> {
    let Some(key) = steam_key() else { return Ok(vec![]) };
    let me = crate::stores::steam::steam_id64().ok_or("No Steam account found on this PC")?;
    let k = urlencoding::encode(&key).into_owned();
    let list = net::get_json(&format!("https://api.steampowered.com/ISteamUser/GetFriendList/v1/?key={k}&steamid={me}&relationship=friend"), &[])
        .await
        .map_err(|e| if e.starts_with("401") { "Steam won't share your friends list: set Friends List to Public in your Steam privacy settings".to_string() } else { e })?;
    let ids = steam_friend_ids(&list);
    let mut out = Vec::new();
    for chunk in ids.chunks(100) {
        let v = net::get_json(&format!("https://api.steampowered.com/ISteamUser/GetPlayerSummaries/v2/?key={k}&steamids={}", chunk.join(",")), &[]).await?;
        out.extend(v["response"]["players"].as_array().into_iter().flatten().filter_map(steam_friend));
    }
    Ok(out)
}

#[derive(Deserialize)]
struct EpicUser {
    access_token: String,
    account_id: String,
}

fn epic_user() -> Option<EpicUser> {
    serde_json::from_str(&fs::read_to_string(crate::stores::epic::config_dir().join("user.json")).ok()?).ok()
}

async fn epic_get(url: &str, refresh: &mut bool) -> Result<Value, String> {
    loop {
        let u = epic_user().ok_or("not signed in to Epic")?;
        let auth = format!("bearer {}", u.access_token);
        match net::get_json(url, &[("authorization", auth.as_str())]).await {
            // Sessions expire after a few hours; legendary renews it when it runs.
            Err(e) if e.starts_with("401") && !*refresh => {
                *refresh = true;
                crate::stores::epic::run(vec!["status".into()]).await?;
            }
            r => return r,
        }
    }
}

async fn epic() -> Result<Vec<Friend>, String> {
    let Some(u) = epic_user() else { return Ok(vec![]) };
    let mut refreshed = false;
    let sum = epic_get(&format!("https://friends-public-service-prod.ol.epicgames.com/friends/api/v1/{}/summary", u.account_id), &mut refreshed).await?;
    let ids: Vec<String> = sum["friends"].as_array().into_iter().flatten().filter_map(|f| f["accountId"].as_str().map(String::from)).collect();
    let mut names: HashMap<String, String> = HashMap::new();
    for chunk in ids.chunks(100) {
        let q = chunk.iter().map(|i| format!("accountId={i}")).collect::<Vec<_>>().join("&");
        let v = epic_get(&format!("https://account-public-service-prod.ol.epicgames.com/account/api/public/account?{q}"), &mut refreshed).await?;
        for a in v.as_array().into_iter().flatten() {
            if let (Some(id), Some(n)) = (a["id"].as_str(), a["displayName"].as_str()) {
                names.insert(id.into(), n.into());
            }
        }
    }
    Ok(ids
        .into_iter()
        .map(|id| Friend {
            store: "epic",
            name: names.get(&id).cloned().unwrap_or_else(|| "Epic user".into()),
            avatar: None,
            status: "unknown",
            game: None,
            game_id: None,
            last_online: 0,
            profile: None,
            id,
        })
        .collect())
}

fn rank(s: &str) -> u8 {
    match s {
        "ingame" => 0,
        "online" => 1,
        "away" => 2,
        "offline" => 3,
        _ => 4,
    }
}

/// Everyone, in-game first. Cached for a minute. Errors per store come back as notes.
pub async fn list(force: bool) -> (Vec<Friend>, Vec<String>) {
    if !force {
        if let Some((at, f)) = CACHE.lock().unwrap().as_ref() {
            if at.elapsed() < Duration::from_secs(60) {
                return (f.clone(), vec![]);
            }
        }
    }
    let mut notes = Vec::new();
    let (s, e) = (steam().await, epic().await);
    let mut all = Vec::new();
    match s {
        Ok(v) => all.extend(v),
        Err(e) => notes.push(format!("Steam: {e}")),
    }
    match e {
        Ok(v) => all.extend(v),
        Err(e) => notes.push(format!("Epic: {e}")),
    }
    all.sort_by(|a, b| rank(a.status).cmp(&rank(b.status)).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    *CACHE.lock().unwrap() = Some((Instant::now(), all.clone()));
    (all, notes)
}

// ---------------- who owns a game ----------------

#[derive(Serialize, Deserialize, Default)]
struct Owned {
    at: u64,
    /// app id -> minutes played (all time)
    games: HashMap<u32, u64>,
    /// Their game details are private.
    private: bool,
}

fn owned_path(steamid: &str) -> std::path::PathBuf {
    state::data_dir().join("cache/friends_owned").join(format!("{steamid}.json"))
}

async fn owned(steamid: String, key: String) -> Owned {
    if let Some(o) = fs::read_to_string(owned_path(&steamid)).ok().and_then(|t| serde_json::from_str::<Owned>(&t).ok()) {
        if state::now().saturating_sub(o.at) < 86400 {
            return o;
        }
    }
    let url = format!("https://api.steampowered.com/IPlayerService/GetOwnedGames/v1/?key={}&steamid={steamid}&include_played_free_games=1", urlencoding::encode(&key));
    let o = match net::get_json(&url, &[]).await {
        Ok(v) => {
            let games: HashMap<u32, u64> = v["response"]["games"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|g| Some((g["appid"].as_u64()? as u32, g["playtime_forever"].as_u64().unwrap_or(0))))
                .collect();
            Owned { at: state::now(), private: games.is_empty(), games }
        }
        // Don't cache network trouble.
        Err(_) => return Owned { private: true, ..Default::default() },
    };
    let p = owned_path(&steamid);
    let _ = fs::create_dir_all(p.parent().unwrap());
    let _ = fs::write(p, serde_json::to_string(&o).unwrap_or_default());
    o
}

#[derive(Serialize, Clone)]
pub struct Owner {
    pub friend: Friend,
    /// Hours played, all time.
    pub hours: f32,
}

/// Steam friends who own this Steam app, most played first.
pub async fn owners(appid: u32) -> Vec<Owner> {
    let Some(key) = steam_key() else { return vec![] };
    let friends: Vec<Friend> = list(false).await.0.into_iter().filter(|f| f.store == "steam").collect();
    let mut handles = Vec::new();
    // A handful at a time: the first look fetches every friend's library once (then it's cached a day).
    for chunk in friends.chunks(6) {
        let mut hs = Vec::new();
        for f in chunk {
            let (id, key) = (f.id.clone(), key.clone());
            hs.push(tauri::async_runtime::spawn(async move { owned(id, key).await }));
        }
        for (f, h) in chunk.iter().zip(hs) {
            handles.push((f.clone(), h.await.unwrap_or_default()));
        }
    }
    let mut out: Vec<Owner> = handles
        .into_iter()
        .filter_map(|(f, o)| o.games.get(&appid).map(|m| Owner { friend: f, hours: *m as f32 / 60.0 }))
        .collect();
    out.sort_by(|a, b| rank(a.friend.status).cmp(&rank(b.friend.status)).then(b.hours.partial_cmp(&a.hours).unwrap_or(std::cmp::Ordering::Equal)));
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn reads_steam_presence() {
        let p = json!({"steamid": "1", "personaname": "A", "personastate": 1, "gameid": "570", "gameextrainfo": "Dota 2"});
        let f = super::steam_friend(&p).unwrap();
        assert_eq!((f.status, f.game_id.as_deref()), ("ingame", Some("steam:570")));
        let p = json!({"steamid": "2", "personaname": "B", "personastate": 3});
        assert_eq!(super::steam_friend(&p).unwrap().status, "away");
        let p = json!({"steamid": "3", "personastate": 1, "gameid": "13048826281347178496", "gameextrainfo": "Shortcut"});
        assert_eq!(super::steam_friend(&p).unwrap().game_id, None);
    }

    #[test]
    #[ignore]
    fn live_friends() {
        tauri::async_runtime::block_on(async {
            let (f, notes) = super::list(true).await;
            println!("{} friends, notes {:?}", f.len(), notes);
            println!("steam {} epic {} online {}", f.iter().filter(|x| x.store == "steam").count(), f.iter().filter(|x| x.store == "epic").count(), f.iter().filter(|x| x.status != "offline" && x.status != "unknown").count());
            let o = super::owners(251570).await;
            println!("own 7DTD: {}", o.len());
        });
    }
}
