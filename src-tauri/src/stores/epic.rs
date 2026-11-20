//! Epic Games Store, standalone: Lodestone drives its own copy of legendary with its own config
//! (~/.local/share/lodestone/legendary). Games run through Proton via umu.

use crate::model::*;
use crate::{proton, settings, state, tools};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

pub const LOGIN_URL: &str = "https://www.epicgames.com/id/login?redirectUrl=https%3A%2F%2Fwww.epicgames.com%2Fid%2Fapi%2Fredirect%3FclientId%3D34a02cf8f4414e29b15921876da36f9a%26responseType%3Dcode";

pub fn config_dir() -> PathBuf {
    state::data_dir().join("legendary")
}

fn library_cache() -> PathBuf {
    config_dir().join("lodestone_library.json")
}

fn meta_cache() -> PathBuf {
    config_dir().join("lodestone_meta.json")
}

fn read_meta() -> HashMap<String, crate::gamesdb::Meta> {
    fs::read_to_string(meta_cache()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// A legendary command using Lodestone's config directory.
pub fn cmd(args: &[&str]) -> Result<LaunchSpec, String> {
    let bin = tools::LEGENDARY.path().ok_or("legendary isn't installed yet")?;
    let mut s = LaunchSpec::new(bin.to_string_lossy(), args);
    s.env.insert("LEGENDARY_CONFIG_PATH".into(), config_dir().to_string_lossy().into_owned());
    Ok(s)
}

/// Run legendary to completion and return stdout.
pub async fn run(args: Vec<String>) -> Result<String, String> {
    tools::LEGENDARY.ensure().await?;
    let spec = cmd(&args.iter().map(String::as_str).collect::<Vec<_>>())?;
    tauri::async_runtime::spawn_blocking(move || {
        let out = std::process::Command::new(&spec.program).args(&spec.args).envs(&spec.env).stdin(std::process::Stdio::null()).output().map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(err.lines().filter(|l| l.contains("ERROR") || l.contains("CRITICAL")).last().unwrap_or(err.lines().last().unwrap_or("legendary failed")).to_string())
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

pub fn account() -> Option<String> {
    let v: Value = serde_json::from_str(&fs::read_to_string(config_dir().join("user.json")).ok()?).ok()?;
    v["displayName"].as_str().map(String::from)
}

pub async fn login(code: &str) -> Result<String, String> {
    // legendary exits 0 even when Epic rejects the code, so check that a session was actually saved.
    let _ = fs::remove_file(config_dir().join("user.json"));
    run(vec!["auth".into(), "--code".into(), code.into()]).await?;
    if account().is_none() {
        return Err("Epic didn't accept that sign-in code (codes expire after a few minutes). Try signing in again.".into());
    }
    refresh().await?;
    account().ok_or_else(|| "Epic sign-in didn't complete".into())
}

pub async fn logout() -> Result<(), String> {
    run(vec!["auth".into(), "--delete".into()]).await?;
    let _ = fs::remove_file(library_cache());
    Ok(())
}

/// Fetch the owned library from Epic and cache it for scans.
pub async fn refresh() -> Result<usize, String> {
    let out = run(vec!["list".into(), "--json".into()]).await?;
    let v: Value = serde_json::from_str(&out).map_err(|e| format!("unexpected legendary output: {e}"))?;
    let n = v.as_array().map(|a| a.len()).unwrap_or(0);
    fs::create_dir_all(config_dir()).map_err(|e| e.to_string())?;
    fs::write(library_cache(), out).map_err(|e| e.to_string())?;
    // Genres and series from GOG's games database (Epic's own metadata has none). Once per game.
    let mut meta = read_meta();
    let missing: Vec<String> = v.as_array().into_iter().flatten().filter_map(|g| g["app_name"].as_str()).filter(|a| !meta.contains_key(*a)).map(String::from).collect();
    for (app, m) in crate::gamesdb::lookup_many("epic", missing).await {
        meta.insert(app, m.unwrap_or_default());
    }
    let _ = fs::write(meta_cache(), serde_json::to_string(&meta).unwrap_or_default());
    Ok(n)
}

fn key_image(meta: &Value, kinds: &[&str]) -> Option<String> {
    let imgs = meta["keyImages"].as_array()?;
    kinds.iter().find_map(|k| imgs.iter().find(|i| i["type"] == *k).and_then(|i| i["url"].as_str()).map(String::from))
}

fn installed() -> HashMap<String, Value> {
    fs::read_to_string(config_dir().join("installed.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .map(|m| m.into_iter().collect())
        .unwrap_or_default()
}

pub fn install_dir() -> PathBuf {
    settings::games_dir().join("Epic")
}

pub fn prefix(app: &str) -> PathBuf {
    proton::prefix(&format!("epic-{app}"))
}

/// legendary resolves the game's exe and launch args (and online auth) and hands the command to umu.
fn launch_spec(app: &str) -> Option<LaunchSpec> {
    let umu = tools::umu_path()?;
    let mut s = cmd(&["launch", app, "--no-wine", "--wrapper", &umu.to_string_lossy()]).ok()?;
    let pfx = prefix(app);
    let _ = fs::create_dir_all(&pfx);
    s.env.insert("WINEPREFIX".into(), pfx.to_string_lossy().into_owned());
    s.env.insert("GAMEID".into(), "umu-default".into());
    s.env.insert("PROTONPATH".into(), tools::proton_path());
    s.env.insert("STORE".into(), "egs".into());
    Some(s)
}

pub fn scan() -> Vec<Game> {
    if account().is_none() {
        return vec![];
    }
    let Some(lib) = fs::read_to_string(library_cache()).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { return vec![] };
    let inst = installed();
    let dbmeta = read_meta();
    lib.as_array()
        .into_iter()
        .flatten()
        .filter_map(|g| {
            let app = g["app_name"].as_str()?;
            let meta = &g["metadata"];
            let i = inst.get(app);
            let path = i.and_then(|i| i["install_path"].as_str());
            Some(Game {
                id: format!("epic:{app}"),
                store: "epic".into(),
                via: "Lodestone · Proton".into(),
                title: g["app_title"].as_str().or(meta["title"].as_str()).unwrap_or(app).to_string(),
                installed: i.is_some(),
                cover: key_image(meta, &["DieselGameBoxTall", "OfferImageTall", "Thumbnail"]),
                hero: key_image(meta, &["DieselGameBox", "OfferImageWide", "DieselGameBoxTall"]),
                size: i.and_then(|i| i["install_size"].as_u64()),
                launch: launch_spec(app),
                detect: path.and_then(|p| std::path::Path::new(p).file_name()).map(|n| Detect::Contains(n.to_string_lossy().into_owned())),
                can_install: i.is_none(),
                can_uninstall: i.is_some(),
                prefix: Some(path_str(&prefix(app))),
                genres: dbmeta.get(app).map(|m| m.genres.clone()).unwrap_or_default(),
                series: dbmeta.get(app).and_then(|m| m.series.clone()),
                ..Default::default()
            })
        })
        .collect()
}

/// Download command for the downloads manager (progress lines go to the log).
pub fn install_cmd(app: &str) -> Result<LaunchSpec, String> {
    let dir = install_dir();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    cmd(&["install", app, "-y", "--base-path", &dir.to_string_lossy(), "--platform", "Windows", "--skip-sdl", "--with-dlcs"])
}

pub async fn uninstall(app: &str) -> Result<(), String> {
    run(vec!["uninstall".into(), app.into(), "-y".into()]).await.map(|_| ())
}

/// Use the Epic sign-in from an installed Heroic (moves it: Heroic will need to sign in again).
pub fn import_heroic() -> Result<String, String> {
    let h = home();
    let src = [
        h.join(".var/app/com.heroicgameslauncher.hgl/config/heroic/legendaryConfig/legendary/user.json"),
        h.join(".config/heroic/legendaryConfig/legendary/user.json"),
    ]
    .into_iter()
    .find(|p| p.is_file())
    .ok_or("Heroic isn't signed in to Epic")?;
    fs::create_dir_all(config_dir()).map_err(|e| e.to_string())?;
    fs::copy(&src, config_dir().join("user.json")).map_err(|e| e.to_string())?;
    account().ok_or_else(|| "Couldn't read Heroic's Epic sign-in".into())
}
