//! Play time the stores themselves recorded, so games played before Lodestone (or outside it) show
//! their real hours:
//! - Steam: localconfig.vdf (minutes and last played per game), and the Web API owned list when a key is set;
//! - Epic: Epic's play-time service, through Lodestone's Epic sign-in (cached, refreshed with Rescan);
//! - Lutris: its game database; Heroic: its own play-time file (Epic, GOG and Amazon games it ran).
//! - Battle.net: only when each game was last played (Battle.net.config). Battle.net keeps no play time on
//!   disk (its own "last played" list is in an encrypted database) and Blizzard has no play-time service.
//! EA, Ubisoft and GOG Galaxy don't keep play time anywhere Lodestone can read.
//!
//! Lodestone shows the larger of the store's figure and its own, so time isn't counted twice when
//! the store also saw the game run.

#[cfg(not(windows))]
use crate::model::home;
use crate::{state, vdf};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
#[cfg(not(windows))]
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Played {
    /// Seconds played, all time.
    pub seconds: u64,
    /// Unix time last played (0 = unknown).
    pub last: u64,
}

fn merge(out: &mut HashMap<String, Played>, id: String, p: Played) {
    if p.seconds == 0 && p.last == 0 {
        return;
    }
    let e = out.entry(id).or_default();
    e.seconds = e.seconds.max(p.seconds);
    e.last = e.last.max(p.last);
}

/// Every store's play time by Lodestone game id. Read at most every 30 seconds.
pub fn all() -> HashMap<String, Played> {
    static CACHE: Mutex<Option<(Instant, HashMap<String, Played>)>> = Mutex::new(None);
    if let Some((at, m)) = CACHE.lock().unwrap().as_ref() {
        if at.elapsed() < Duration::from_secs(30) {
            return m.clone();
        }
    }
    let mut out = HashMap::new();
    steam(&mut out);
    epic_cached(&mut out);
    #[cfg(not(windows))]
    {
        lutris(&mut out);
        heroic(&mut out);
    }
    battlenet(&mut out);
    *CACHE.lock().unwrap() = Some((Instant::now(), out.clone()));
    out
}

// ---------------- Steam ----------------

fn steam(out: &mut HashMap<String, Played>) {
    let Some(id64) = crate::stores::steam::steam_id64().and_then(|s| s.parse::<u64>().ok()) else { return };
    let account = id64.saturating_sub(76561197960265728);
    if let Some((root, _)) = crate::stores::steam::primary_root() {
        if let Ok(txt) = fs::read_to_string(root.join(format!("userdata/{account}/config/localconfig.vdf"))) {
            steam_localconfig(&txt, out);
        }
    }
    // The Web API's owned list (when you added a key) has Steam's server-side totals.
    let owned = state::data_dir().join("steam_owned.json");
    if let Some(v) = fs::read_to_string(owned).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
        for g in v["games"].as_array().into_iter().flatten() {
            let Some(app) = g["appid"].as_u64() else { continue };
            let p = Played { seconds: g["playtime_forever"].as_u64().unwrap_or(0) * 60, last: g["rtime_last_played"].as_u64().unwrap_or(0) };
            merge(out, format!("steam:{app}"), p);
        }
    }
}

fn steam_localconfig(txt: &str, out: &mut HashMap<String, Played>) {
    let v = vdf::parse(txt);
    let Some(apps) = v.get("UserLocalConfigStore").and_then(|s| s.get("Software")).and_then(|s| s.get("Valve")).and_then(|s| s.get("Steam")).and_then(|s| s.get("apps")) else { return };
    for (app, a) in apps.entries() {
        let num = |k: &str| a.str(k).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
        merge(out, format!("steam:{app}"), Played { seconds: num("Playtime") * 60, last: num("LastPlayed") });
    }
}

// ---------------- Epic ----------------

fn epic_file() -> PathBuf {
    state::data_dir().join("epic_playtime.json")
}

fn epic_cached(out: &mut HashMap<String, Played>) {
    let m: HashMap<String, Played> = fs::read_to_string(epic_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    for (app, p) in m {
        merge(out, format!("epic:{app}"), p);
    }
}

/// Ask Epic for your play time (with Rescan). Epic counts time played through its own launcher.
pub async fn refresh_epic() -> Result<usize, String> {
    let Some(u) = crate::friends::epic_user() else { return Ok(0) };
    let mut refreshed = false;
    let url = format!("https://library-service.live.use1a.on.epicgames.com/library/api/public/playtime/account/{}/all", u.account_id);
    let v = crate::friends::epic_get(&url, &mut refreshed).await?;
    let m = parse_epic(&v);
    let n = m.len();
    let _ = fs::create_dir_all(state::data_dir());
    fs::write(epic_file(), serde_json::to_string(&m).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    Ok(n)
}

fn parse_epic(v: &Value) -> HashMap<String, Played> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|g| Some((g["artifactId"].as_str()?.to_string(), Played { seconds: g["totalTime"].as_u64().unwrap_or(0), last: 0 })))
        .collect()
}

// ---------------- Battle.net ----------------

/// Windows: Battle.net keeps its settings in %APPDATA%\\Battle.net.
#[cfg(windows)]
fn battlenet(out: &mut HashMap<String, Played>) {
    let Some(appdata) = std::env::var_os("APPDATA") else { return };
    if let Some(v) = fs::read_to_string(std::path::Path::new(&appdata).join("Battle.net").join("Battle.net.config")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
        bnet_config(&v, out);
    }
}

/// Linux: inside Battle.net's Proton prefix.
#[cfg(not(windows))]
fn battlenet(out: &mut HashMap<String, Played>) {
    let Ok(app) = crate::stores::winapps::find("battlenet") else { return };
    let users = crate::proton::drive_c(&app.prefix()).join("users");
    for u in fs::read_dir(users).into_iter().flatten().flatten() {
        if let Some(v) = fs::read_to_string(u.path().join("AppData/Roaming/Battle.net/Battle.net.config")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
            bnet_config(&v, out);
        }
    }
}

/// Battle.net.config: Games.<install uid>.LastPlayed (unix time, as text).
fn bnet_config(v: &Value, out: &mut HashMap<String, Played>) {
    for (uid, g) in v["Games"].as_object().into_iter().flatten() {
        let Some(last) = g["LastPlayed"].as_str().and_then(|s| s.parse::<u64>().ok()) else { continue };
        let code = crate::stores::bnet_catalog::code_for_uid(uid);
        merge(out, format!("battlenet:{}", code.to_ascii_lowercase()), Played { seconds: 0, last });
    }
}

// ---------------- Lutris and Heroic ----------------

#[cfg(not(windows))]
fn lutris(out: &mut HashMap<String, Played>) {
    let h = home();
    for db in [h.join(".var/app/net.lutris.Lutris/data/lutris/pga.db"), h.join(".local/share/lutris/pga.db")] {
        lutris_db(&db, out);
    }
}

#[cfg(not(windows))]
fn lutris_db(db: &Path, out: &mut HashMap<String, Played>) {
    use rusqlite::{Connection, OpenFlags};
    let Ok(conn) = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX) else { return };
    // Lutris keeps hours as a decimal number.
    let Ok(mut stmt) = conn.prepare("SELECT id, COALESCE(playtime, 0), COALESCE(lastplayed, 0) FROM games") else { return };
    let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1).unwrap_or(0.0), r.get::<_, i64>(2).unwrap_or(0))));
    for (id, hours, last) in rows.into_iter().flatten().flatten() {
        merge(out, format!("lutris:{id}"), Played { seconds: (hours * 3600.0) as u64, last: last.max(0) as u64 });
    }
}

#[cfg(not(windows))]
fn heroic(out: &mut HashMap<String, Played>) {
    let h = home();
    for f in [h.join(".var/app/com.heroicgameslauncher.hgl/config/heroic/store/timestamp.json"), h.join(".config/heroic/store/timestamp.json")] {
        if let Some(v) = fs::read_to_string(f).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
            heroic_timestamps(&v, out);
        }
    }
}

#[cfg(not(windows))]
/// Heroic's timestamp.json: { appName: { totalPlayed (minutes), lastPlayed (ISO date) } }. App names are
/// unique across its stores, so the same figure is offered for each store's id; only the real one exists.
fn heroic_timestamps(v: &Value, out: &mut HashMap<String, Played>) {
    for (app, t) in v.as_object().into_iter().flatten() {
        let p = Played { seconds: t["totalPlayed"].as_u64().unwrap_or(0) * 60, last: t["lastPlayed"].as_str().map(iso_to_unix).unwrap_or(0) };
        for store in ["epic", "gog", "amazon"] {
            merge(out, format!("{store}:{app}"), p);
        }
    }
}

#[cfg(not(windows))]
/// "2026-11-21T10:26:36.522Z" -> unix seconds (UTC).
fn iso_to_unix(s: &str) -> u64 {
    let n = |a: usize, b: usize| s.get(a..b).and_then(|x| x.parse::<i64>().ok());
    let (Some(y), Some(m), Some(d), Some(hh), Some(mm), Some(ss)) = (n(0, 4), n(5, 7), n(8, 10), n(11, 13), n(14, 16), n(17, 19)) else { return 0 };
    // Days from civil date (Howard Hinnant's algorithm).
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = y2 - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    (days * 86400 + hh * 3600 + mm * 60 + ss).max(0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_store_play_time() {
        let mut out = HashMap::new();
        let cfg = r#""UserLocalConfigStore" { "Software" { "Valve" { "Steam" { "apps" {
            "570" { "LastPlayed" "1700000000" "Playtime" "125" }
            "10" { "cloud" { "x" "y" } } } } } } }"#;
        steam_localconfig(cfg, &mut out);
        assert_eq!(out["steam:570"], Played { seconds: 125 * 60, last: 1700000000 });
        assert!(!out.contains_key("steam:10"));

        let epic = serde_json::json!([{ "accountId": "a", "artifactId": "Fortnite", "totalTime": 7200 }]);
        assert_eq!(parse_epic(&epic)["Fortnite"].seconds, 7200);

        let cfg = serde_json::json!({ "Games": { "battle_net": {}, "osi": { "LastPlayed": "1795292815" }, "prometheus": { "LastPlayed": "1700000000" } } });
        bnet_config(&cfg, &mut out);
        assert_eq!(out["battlenet:osi"].last, 1795292815);
        assert_eq!(out["battlenet:pro"].last, 1700000000);

        #[cfg(not(windows))]
        {
            let h = serde_json::json!({ "1207664593": { "lastPlayed": "2026-11-21T10:26:36.522Z", "totalPlayed": 90 } });
            heroic_timestamps(&h, &mut out);
            assert_eq!(out["gog:1207664593"].seconds, 5400);
            assert_eq!(iso_to_unix("1970-01-02T00:00:00Z"), 86400);
            assert_eq!(iso_to_unix("2023-11-14T22:13:20Z"), 1700000000);
        }
    }

    /// `cargo test live_playtime -- --ignored --nocapture`: totals from this PC's stores.
    #[test]
    #[ignore]
    fn live_playtime() {
        let n = tauri::async_runtime::block_on(refresh_epic());
        println!("epic refresh: {n:?}");
        let all = all();
        let mut v: Vec<_> = all.iter().collect();
        v.sort_by(|a, b| b.1.seconds.cmp(&a.1.seconds));
        println!("{} games with play time", v.len());
        for (id, p) in v.iter().take(8) {
            println!("{id:24} {:>6.1} h  last {}", p.seconds as f64 / 3600.0, p.last);
        }
    }
}
