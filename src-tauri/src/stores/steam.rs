use crate::model::*;
use crate::steam_appinfo;
use crate::vdf;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const FLATPAK_ID: &str = "com.valvesoftware.Steam";

/// Tools and runtimes that show up as "installed apps" but aren't games.
fn is_tool(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("proton")
        || n.starts_with("steam linux runtime")
        || n.starts_with("steamworks common")
        || n.starts_with("steamworks shared")
        || n.starts_with("steamvr")
}

struct Root {
    path: PathBuf,
    flatpak: bool,
}

fn roots() -> Vec<Root> {
    let h = home();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (p, flatpak) in [
        (h.join(".local/share/Steam"), false),
        (h.join(".steam/steam"), false),
        (h.join(".steam/root"), false),
        (h.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"), true),
    ] {
        if let Ok(c) = fs::canonicalize(&p) {
            if c.join("steamapps").is_dir() && seen.insert(c.clone()) {
                out.push(Root { path: c, flatpak });
            }
        }
    }
    out
}

fn find_art(dir: &Path, names: &[&str]) -> Option<PathBuf> {
    // Newer Steam clients nest art in hash-named subfolders; check top level first, then one level down.
    for n in names {
        let p = dir.join(n);
        if p.is_file() {
            return Some(p);
        }
        if let Ok(rd) = fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path().join(n);
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    None
}

fn client(root: &Root, args: &[&str]) -> LaunchSpec {
    if root.flatpak {
        let mut a = vec!["run", FLATPAK_ID];
        a.extend(args);
        LaunchSpec::new("flatpak", &a)
    } else {
        LaunchSpec::new("steam", args)
    }
}

fn cover_for(cache: &Path, appid: &str) -> (Option<String>, Option<String>) {
    let art_dir = cache.join(appid);
    let cover = find_art(&art_dir, &["library_600x900.jpg", "library_capsule.jpg"]).map(|p| path_str(&p)).or_else(|| {
        Some(format!("https://shared.steamstatic.com/store_item_assets/steam/apps/{appid}/library_600x900.jpg"))
    });
    let hero = find_art(&art_dir, &["library_hero.jpg", "library_header.jpg", "header.jpg"]).map(|p| path_str(&p)).or_else(|| {
        Some(format!("https://shared.steamstatic.com/store_item_assets/steam/apps/{appid}/library_hero.jpg"))
    });
    (cover, hero)
}

/// Steam library folders (each contains steamapps/) for every Steam install found.
pub fn library_dirs() -> Vec<PathBuf> {
    let mut libs = Vec::new();
    for root in roots() {
        libs.push(root.path.clone());
        if let Ok(txt) = fs::read_to_string(root.path.join("steamapps/libraryfolders.vdf")) {
            let v = vdf::parse(&txt);
            if let Some(lf) = v.get("libraryfolders") {
                for (_, lib) in lf.entries() {
                    if let Some(p) = lib.str("path") {
                        libs.push(PathBuf::from(p));
                    }
                }
            }
        }
    }
    let mut seen = HashSet::new();
    libs.retain(|l| seen.insert(fs::canonicalize(l).unwrap_or(l.clone())));
    libs
}

/// Steam's AppState flags: 2 = update required; 256/1024/1048576 = update running/started/downloading.
/// TargetBuildID differing from buildid also means Steam has a newer build queued.
fn steam_update(app: &vdf::Vdf, flags: u32) -> Option<(&'static str, Option<u64>)> {
    let num = |k: &str| app.str(k).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    let (build, target) = (num("buildid"), num("TargetBuildID"));
    let pending = flags & 2 != 0 || (target != 0 && target != build);
    if flags & 4 == 0 && flags & 2 == 0 {
        return None; // not installed (or installing for the first time)
    }
    let left = num("BytesToDownload").saturating_sub(num("BytesDownloaded"));
    let size = (pending && left > 0).then_some(left);
    if flags & (256 | 1024 | 1_048_576) != 0 {
        Some(("updating", size))
    } else if pending {
        Some(("available", size))
    } else {
        None
    }
}

fn genres_of(i: Option<&steam_appinfo::AppInfo>) -> Vec<String> {
    let Some(i) = i else { return vec![] };
    let mut v: Vec<String> = i.genres.iter().filter_map(|g| steam_genre(*g)).map(String::from).collect();
    if is_isometric_arpg(&i.tags) {
        v.push(ISO_ARPG.into());
    }
    v
}

pub fn scan() -> Vec<Game> {
    let mut games = Vec::new();
    let mut seen = HashSet::new();
    for root in roots() {
        let info = steam_appinfo::load(&root.path);
        let mut libs = vec![root.path.clone()];
        if let Ok(txt) = fs::read_to_string(root.path.join("steamapps/libraryfolders.vdf")) {
            let v = vdf::parse(&txt);
            if let Some(lf) = v.get("libraryfolders") {
                for (_, lib) in lf.entries() {
                    if let Some(p) = lib.str("path") {
                        libs.push(PathBuf::from(p));
                    }
                }
            }
        }
        let cache = root.path.join("appcache/librarycache");
        for lib in libs {
            let Ok(rd) = fs::read_dir(lib.join("steamapps")) else { continue };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if !(name.starts_with("appmanifest_") && name.ends_with(".acf")) {
                    continue;
                }
                let Ok(txt) = fs::read_to_string(e.path()) else { continue };
                let v = vdf::parse(&txt);
                let Some(app) = v.get("AppState") else { continue };
                let (Some(appid), Some(title)) = (app.str("appid"), app.str("name")) else { continue };
                if is_tool(title) || !seen.insert(appid.to_string()) {
                    continue;
                }
                let flags: u32 = app.str("StateFlags").and_then(|s| s.parse().ok()).unwrap_or(0);
                let update = steam_update(app, flags);
                let (cover, hero) = cover_for(&cache, appid);
                let id_num: u32 = appid.parse().unwrap_or(0);
                // -silent: if Steam isn't running it starts minimized to tray instead of opening its window.
                let launch = client(&root, &["-silent", &format!("steam://rungameid/{appid}")]);
                games.push(Game {
                    id: format!("steam:{appid}"),
                    store: "steam".into(),
                    via: if root.flatpak { "Steam (Flatpak)".into() } else { "Steam".into() },
                    title: title.to_string(),
                    installed: flags & 4 != 0,
                    cover,
                    hero,
                    size: app.str("SizeOnDisk").and_then(|s| s.parse().ok()),
                    launch: Some(launch),
                    detect: Some(Detect::Arg(format!("AppId={appid}"))),
                    install: Some(client(&root, &[&format!("steam://install/{appid}")])),
                    uninstall: Some(client(&root, &[&format!("steam://uninstall/{appid}")])),
                    can_install: true,
                    can_uninstall: true,
                    workshop: info.get(&id_num).filter(|i| i.workshop).map(|_| id_num),
                    genres: genres_of(info.get(&id_num)),
                    series: info.get(&id_num).and_then(|i| i.franchise.clone()),
                    update_via: update.as_ref().map(|_| "steam".into()),
                    update_size: update.as_ref().and_then(|u| u.1),
                    update: update.map(|u| u.0.to_string()),
                    ..Default::default()
                });
            }
        }

        // Owned but not installed. With a Steam Web API key: Steam's exact owned list. Without one: games
        // Steam has local traces of (localconfig.vdf + library art cache), which also keeps refunded games.
        let api = owned();
        let candidates: Vec<(u32, Option<String>)> = match &api {
            Some(list) => list.iter().map(|(a, n)| (*a, Some(n.clone()))).collect(),
            None => steam_appinfo::library_ids(&root.path).into_iter().map(|a| (a, None)).collect(),
        };
        for (appid, api_name) in candidates {
            let key = appid.to_string();
            let fallback;
            let i = match (info.get(&appid), &api_name) {
                (Some(i), _) => i,
                // Owned per Steam but not in the local app cache: still list it with the API's name.
                (None, Some(n)) => {
                    fallback = steam_appinfo::AppInfo { name: n.clone(), kind: "game".into(), ..Default::default() };
                    &fallback
                }
                (None, None) => continue,
            };
            if i.kind != "game" || is_tool(&i.name) || seen.contains(&key) {
                continue;
            }
            seen.insert(key.clone());
            let (cover, hero) = cover_for(&cache, &key);
            games.push(Game {
                id: format!("steam:{appid}"),
                store: "steam".into(),
                via: if root.flatpak { "Steam (Flatpak)".into() } else { "Steam".into() },
                title: i.name.clone(),
                installed: false,
                cover,
                hero,
                launch: Some(client(&root, &["-silent", &format!("steam://rungameid/{appid}")])),
                detect: Some(Detect::Arg(format!("AppId={appid}"))),
                install: Some(client(&root, &[&format!("steam://install/{appid}")])),
                can_install: true,
                workshop: i.workshop.then_some(appid),
                genres: genres_of(Some(i)),
                series: i.franchise.clone(),
                ..Default::default()
            });
        }
    }
    games
}

fn owned_file() -> PathBuf {
    crate::state::data_dir().join("steam_owned.json")
}

/// Steam's owned-games list cached from the Web API (None when no key is set or never fetched).
pub fn owned() -> Option<Vec<(u32, String)>> {
    if crate::settings::load().steam_api_key.trim().is_empty() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(owned_file()).ok()?).ok()?;
    Some(v["games"].as_array()?.iter().filter_map(|g| Some((g["appid"].as_u64()? as u32, g["name"].as_str().unwrap_or("").to_string()))).collect())
}

/// The signed-in Steam account's 64-bit id (most recent login on this PC).
pub fn steam_id64() -> Option<String> {
    for root in roots() {
        let Ok(txt) = fs::read_to_string(root.path.join("config/loginusers.vdf")) else { continue };
        let v = vdf::parse(&txt);
        let users = v.get("users")?;
        let pick = users.entries().iter().find(|(_, u)| u.str("MostRecent") == Some("1")).or(users.entries().first());
        if let Some((id, _)) = pick {
            return Some(id.clone());
        }
    }
    None
}

/// Fetch the owned-games list with the user's Web API key and cache it.
pub async fn refresh_owned() -> Result<usize, String> {
    let key = crate::settings::load().steam_api_key.trim().to_string();
    if key.is_empty() {
        return Ok(0);
    }
    let id = steam_id64().ok_or("No Steam account found on this PC")?;
    let url = format!("https://api.steampowered.com/IPlayerService/GetOwnedGames/v1/?key={}&steamid={id}&include_appinfo=1&include_played_free_games=1", urlencoding::encode(&key));
    let r = crate::net::client().get(&url).send().await.map_err(crate::net::err)?;
    if r.status().as_u16() == 401 || r.status().as_u16() == 403 {
        return Err("Steam rejected the Web API key".into());
    }
    let v: serde_json::Value = r.error_for_status().map_err(crate::net::err)?.json().await.map_err(crate::net::err)?;
    let games = v["response"]["games"].as_array().cloned().unwrap_or_default();
    if games.is_empty() {
        return Err("Steam returned no games. In Steam → your profile → Edit Profile → Privacy Settings, set Game details to Public.".into());
    }
    let n = games.len();
    fs::write(owned_file(), serde_json::json!({ "fetched": crate::state::now(), "games": games }).to_string()).map_err(|e| e.to_string())?;
    Ok(n)
}

/// Open a Steam URL (store page, workshop...) in the Steam client.
pub fn open_url(url: &str) -> Option<LaunchSpec> {
    roots().first().map(|r| client(r, &[url]))
}
