//! GOG, standalone: sign-in tokens and downloads via Lodestone's own gogdl, library via GOG's API.
//! Data in ~/.local/share/lodestone/gog/. Games run through Proton via umu.

use crate::model::*;
use crate::{net, proton, settings, state, tools};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const LOGIN_URL: &str = "https://auth.gog.com/auth?client_id=46899977096215655&redirect_uri=https%3A%2F%2Fembed.gog.com%2Fon_login_success%3Forigin%3Dclient&response_type=code&layout=client2";

fn dir() -> PathBuf {
    state::data_dir().join("gog")
}
fn auth_file() -> PathBuf {
    dir().join("auth.json")
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct Entry {
    id: String,
    title: String,
    cover: Option<String>,
    hero: Option<String>,
    /// None = not looked up yet.
    #[serde(default)]
    genres: Option<Vec<String>>,
    #[serde(default)]
    series: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Installed {
    pub id: String,
    pub title: String,
    pub path: String,
}

fn read<T: for<'a> Deserialize<'a> + Default>(name: &str) -> T {
    fs::read_to_string(dir().join(name)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn write<T: Serialize>(name: &str, v: &T) -> Result<(), String> {
    fs::create_dir_all(dir()).map_err(|e| e.to_string())?;
    fs::write(dir().join(name), serde_json::to_string_pretty(v).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

fn gogdl(args: &[&str]) -> Result<LaunchSpec, String> {
    let bin = tools::GOGDL.path().ok_or("gogdl isn't installed yet")?;
    let auth = auth_file().to_string_lossy().into_owned();
    let mut a = vec!["--auth-config-path", auth.as_str()];
    a.extend(args);
    Ok(LaunchSpec::new(bin.to_string_lossy(), &a))
}

async fn run(args: Vec<String>) -> Result<String, String> {
    tools::GOGDL.ensure().await?;
    fs::create_dir_all(dir()).map_err(|e| e.to_string())?;
    let spec = gogdl(&args.iter().map(String::as_str).collect::<Vec<_>>())?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut c = std::process::Command::new(&spec.program);
        c.args(&spec.args);
        let out = crate::host::wrap(c).stdin(std::process::Stdio::null()).output().map_err(|e| e.to_string())?;
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        if out.status.success() {
            Ok(stdout)
        } else {
            Err(String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("gogdl failed").to_string())
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// A fresh access token (gogdl refreshes it when needed).
async fn token() -> Result<String, String> {
    let out = run(vec!["auth".into()]).await?;
    let v: Value = serde_json::from_str(out.trim()).map_err(|_| "GOG sign-in expired; sign in again")?;
    v["access_token"].as_str().map(String::from).ok_or_else(|| "GOG sign-in expired; sign in again".into())
}

pub fn account() -> Option<String> {
    if !auth_file().is_file() {
        return None;
    }
    let v: Value = read("account.json");
    Some(v["username"].as_str().unwrap_or("signed in").to_string())
}

pub async fn login(code: &str) -> Result<String, String> {
    let out = run(vec!["auth".into(), "--code".into(), code.into()]).await?;
    if out.contains("\"error\"") {
        return Err("GOG rejected the sign-in code; try again".into());
    }
    refresh().await?;
    account().ok_or_else(|| "GOG sign-in didn't complete".into())
}

pub fn logout() {
    for f in ["auth.json", "account.json", "library.json"] {
        let _ = fs::remove_file(dir().join(f));
    }
}

/// Fetch owned games (and vertical covers from GOG's games database) and cache them for scans.
pub async fn refresh() -> Result<usize, String> {
    let tok = token().await?;
    let bearer = format!("Bearer {tok}");
    let h = [("authorization", bearer.as_str())];
    if let Ok(u) = net::get_json("https://embed.gog.com/userData.json", &h).await {
        let _ = write("account.json", &serde_json::json!({ "username": u["username"] }));
    }
    let old: HashMap<String, Entry> = read::<Vec<Entry>>("library.json").into_iter().map(|e| (e.id.clone(), e)).collect();
    let mut entries = Vec::new();
    let mut page = 1;
    loop {
        let v = net::get_json(&format!("https://embed.gog.com/account/getFilteredProducts?mediaType=1&sortBy=title&page={page}"), &h).await?;
        for p in v["products"].as_array().into_iter().flatten() {
            let Some(id) = p["id"].as_u64().map(|n| n.to_string()) else { continue };
            let img = p["image"].as_str().map(|s| format!("https:{s}"));
            let prev = old.get(&id);
            entries.push(Entry {
                id: id.clone(),
                title: p["title"].as_str().unwrap_or("").to_string(),
                cover: prev.and_then(|e| e.cover.clone()),
                hero: img.map(|i| format!("{i}_bg_crop_1366x655.jpg")),
                genres: prev.and_then(|e| e.genres.clone()),
                series: prev.and_then(|e| e.series.clone()),
            });
        }
        let total = v["totalPages"].as_u64().unwrap_or(1);
        if page as u64 >= total {
            break;
        }
        page += 1;
    }
    // Covers, genres and series from GOG's games database, only for games not looked up yet.
    let missing: Vec<String> = entries.iter().filter(|e| e.cover.is_none() || e.genres.is_none()).map(|e| e.id.clone()).collect();
    let found: HashMap<String, Option<crate::gamesdb::Meta>> = crate::gamesdb::lookup_many("gog", missing).await.into_iter().collect();
    for e in entries.iter_mut() {
        if let Some(m) = found.get(&e.id) {
            let m = m.clone().unwrap_or_default();
            e.cover = e.cover.take().or(m.cover);
            e.genres = Some(m.genres);
            e.series = m.series;
        }
    }
    let n = entries.len();
    write("library.json", &entries)?;
    refresh_builds().await;
    Ok(n)
}

/// Build installed on disk, from the game's goggame-<id>.info.
fn installed_build(i: &Installed) -> Option<String> {
    let v: Value = serde_json::from_str(&fs::read_to_string(Path::new(&i.path).join(format!("goggame-{}.info", i.id))).ok()?).ok()?;
    v["buildId"].as_str().map(String::from)
}

/// Newest Windows build per installed game, from GOG's public content-system API.
async fn refresh_builds() {
    let mut latest: HashMap<String, String> = HashMap::new();
    for i in installed() {
        let url = format!("https://content-system.gog.com/products/{}/os/windows/builds?generation=2", i.id);
        if let Ok(v) = net::get_json(&url, &[]).await {
            if let Some(b) = v["items"][0]["build_id"].as_str() {
                latest.insert(i.id.clone(), b.to_string());
            }
        }
    }
    let _ = write("builds.json", &latest);
}

pub fn update_cmd(id: &str) -> Result<LaunchSpec, String> {
    let i = installed().into_iter().find(|i| i.id == id).ok_or("not installed by Lodestone")?;
    gogdl(&["update", id, "--platform", "windows", "--path", &i.path, "--lang", "en-US"])
}

pub fn install_dir() -> PathBuf {
    settings::games_dir().join("GOG")
}

pub fn prefix(id: &str) -> PathBuf {
    proton::prefix(&format!("gog-{id}"))
}

pub fn installed() -> Vec<Installed> {
    read("installed.json")
}

fn launch_spec(i: &Installed) -> Option<LaunchSpec> {
    let umu = tools::umu_path()?;
    let mut s = gogdl(&["launch", &i.path, &i.id, "--platform", "windows", "--no-wine", "--wrapper", &umu.to_string_lossy()]).ok()?;
    let pfx = prefix(&i.id);
    let _ = fs::create_dir_all(&pfx);
    s.env.insert("WINEPREFIX".into(), pfx.to_string_lossy().into_owned());
    s.env.insert("GAMEID".into(), "umu-default".into());
    s.env.insert("PROTONPATH".into(), tools::proton_path());
    s.env.insert("STORE".into(), "gog".into());
    Some(crate::gog_setup::wrap(s, &pfx, Path::new(&i.path), &i.id))
}

pub fn scan() -> Vec<Game> {
    if account().is_none() {
        return vec![];
    }
    let inst: HashMap<String, Installed> = installed().into_iter().filter(|i| Path::new(&i.path).is_dir()).map(|i| (i.id.clone(), i)).collect();
    let latest: HashMap<String, String> = read("builds.json");
    read::<Vec<Entry>>("library.json")
        .into_iter()
        .map(|e| {
            let i = inst.get(&e.id);
            let update = i.is_some_and(|i| matches!((installed_build(i), latest.get(&i.id)), (Some(h), Some(l)) if &h != l));
            Game {
                id: format!("gog:{}", e.id),
                store: "gog".into(),
                via: "Lodestone · Proton".into(),
                title: e.title.clone(),
                installed: i.is_some(),
                cover: e.cover.clone(),
                hero: e.hero.clone(),
                launch: i.and_then(launch_spec),
                detect: i.and_then(|i| Path::new(&i.path).file_name()).map(|n| Detect::Contains(n.to_string_lossy().into_owned())),
                can_install: i.is_none(),
                can_uninstall: i.is_some(),
                prefix: Some(path_str(&prefix(&e.id))),
                update: update.then(|| "available".into()),
                update_via: update.then(|| "lodestone".into()),
                genres: e.genres.clone().unwrap_or_default(),
                series: e.series.clone(),
                ..Default::default()
            }
        })
        .collect()
}

pub fn install_cmd(id: &str) -> Result<LaunchSpec, String> {
    let dir = install_dir();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    gogdl(&["download", id, "--platform", "windows", "--path", &dir.to_string_lossy(), "--lang", "en-US", "--with-dlcs"])
}

/// After a download finishes: find the folder gogdl created (it holds goggame-<id>.info) and record it.
pub fn finish_install(id: &str, title: &str) -> Result<(), String> {
    let marker = format!("goggame-{id}.info");
    let found = fs::read_dir(install_dir())
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.join(&marker).is_file())
        .ok_or("download finished but the game folder wasn't found")?;
    let mut list = installed();
    list.retain(|i| i.id != id);
    list.push(Installed { id: id.into(), title: title.into(), path: found.to_string_lossy().into_owned() });
    write("installed.json", &list)
}

/// Use a game folder you picked (detection missed it, or you moved the game).
pub fn set_path(id: &str, path: &str) -> Result<(), String> {
    let title = read::<Vec<Entry>>("library.json").into_iter().find(|e| e.id == id).map(|e| e.title).unwrap_or_default();
    let mut list = installed();
    list.retain(|i| i.id != id);
    list.push(Installed { id: id.into(), title, path: path.into() });
    write("installed.json", &list)
}

/// Move the game folder to the trash and forget it.
pub fn uninstall(id: &str) -> Result<(), String> {
    let mut list = installed();
    if let Some(i) = list.iter().find(|i| i.id == id) {
        let ok = std::process::Command::new("gio").arg("trash").arg(&i.path).status().map(|s| s.success()).unwrap_or(false);
        if !ok {
            fs::remove_dir_all(&i.path).map_err(|e| e.to_string())?;
        }
    }
    list.retain(|i| i.id != id);
    write("installed.json", &list)
}

/// Use the GOG sign-in from an installed Heroic (it uses gogdl too, same token format).
pub fn import_heroic() -> Result<(), String> {
    let h = home();
    let src = [
        h.join(".var/app/com.heroicgameslauncher.hgl/config/heroic/gog_store/auth.json"),
        h.join(".config/heroic/gog_store/auth.json"),
    ]
    .into_iter()
    .find(|p| p.is_file())
    .ok_or("Heroic isn't signed in to GOG")?;
    fs::create_dir_all(dir()).map_err(|e| e.to_string())?;
    fs::copy(src, auth_file()).map_err(|e| e.to_string())?;
    Ok(())
}
