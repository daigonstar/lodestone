#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod achievements;
mod beacon_install;
mod plugin_sources;
mod plugins;
mod program_updates;
mod downloads;
mod friends;
#[cfg(not(windows))]
mod gog_setup;
#[cfg(not(windows))]
mod host;
#[cfg(windows)]
#[path = "host_windows.rs"]
mod host;
#[cfg(not(windows))]
mod mangohud;
mod playtime;
mod gamesdb;
mod kiln;
mod launcher;
mod meta;
mod mc;
mod model;
mod net;
mod os;
mod proton;
mod series;
mod settings;
mod shop;
mod state;
mod steam_appinfo;
mod steam_bg;
mod tools;
mod stores;
mod vdf;
mod winget;
mod workshop;

use launcher::{Running, Session, Sessions, Tick};
use model::{Game, LaunchSpec};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use mc::instance::{self as mci, Kind};
use mc::platforms;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

#[derive(Default)]
struct Inner {
    games: Mutex<HashMap<String, Game>>,
    sessions: Mutex<Sessions>,
    store: Mutex<state::Store>,
}
type Shared = Arc<Inner>;

#[derive(Serialize)]
struct GameDto {
    #[serde(flatten)]
    game: Game,
    playtime: u64,
    last_played: u64,
    favorite: bool,
    hidden: bool,
    /// No cover, or its cover image doesn't exist.
    art_missing: bool,
    /// You edited this game's details.
    edited: bool,
    /// MangoHud for this game: on/off, or null to follow Settings.
    mangohud: Option<bool>,
    /// You set the play time yourself.
    playtime_edited: bool,
}

fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> impl std::future::Future<Output = Result<T, String>> {
    async move { tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string()) }
}

#[tauri::command]
async fn list_games(inner: State<'_, Shared>) -> Result<Vec<GameDto>, String> {
    let inner = inner.inner().clone();
    blocking(move || {
        // Sign-ins may have been refreshed since (legendary and gogdl rewrite their token files).
        state::lock_down_secrets();
        let mut games = stores::scan_all();
        apply_series(&mut games);
        apply_kiln(&mut games);
        meta::apply(&mut games);
        let store = inner.store.lock().unwrap();
        // The store's own record when it's bigger (time played before Lodestone, or outside it).
        let played = playtime::all();
        let out = games
            .iter()
            .map(|g| {
                let e = store.games.get(&g.id).cloned().unwrap_or_default();
                let s = played.get(&g.id).copied().unwrap_or_default();
                GameDto { game: g.clone(), playtime: if e.manual_playtime { e.playtime } else { e.playtime.max(s.seconds) }, playtime_edited: e.manual_playtime, last_played: e.last_played.max(s.last), favorite: e.favorite, hidden: e.hidden, art_missing: meta::art_missing(g), edited: meta::edited(&g.id), mangohud: e.mangohud }
            })
            .collect();
        *inner.games.lock().unwrap() = games.into_iter().map(|g| (g.id.clone(), g)).collect();
        out
    })
    .await
}

/// Mark Steam games Kiln can mod (and those already set up in it).
fn apply_kiln(games: &mut [Game]) {
    let map = kiln::by_appid();
    if map.is_empty() {
        return;
    }
    for g in games.iter_mut() {
        let Some(appid) = g.id.strip_prefix("steam:").and_then(|a| a.parse::<u32>().ok()) else { continue };
        if let Some(k) = map.get(&appid) {
            g.kiln = Some(k.id.clone());
            // Opening a game in Kiln once isn't modding it: only count games with mods.
            g.kiln_managed = k.managed && k.mods.unwrap_or(1) > 0;
        }
    }
}

/// Series: Wikidata (by store id) wins; otherwise GOG's database; otherwise Steam's franchise, but only
/// when its name appears in the title (Steam sometimes lists the publisher there).
fn apply_series(games: &mut [Game]) {
    let arpg = series::cached_arpg();
    for g in games.iter_mut().filter(|g| arpg.contains(&g.id)) {
        if !g.genres.iter().any(|x| x == model::ISO_ARPG) {
            g.genres.push(model::ISO_ARPG.into());
        }
    }
    let wd = series::cached();
    let norm = |s: &str| {
        let t: String = s.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect();
        format!(" {} ", t.split_whitespace().filter(|w| *w != "the").collect::<Vec<_>>().join(" "))
    };
    for g in games.iter_mut() {
        if let Some(s) = wd.get(&g.id) {
            g.series = Some(s.clone());
        } else if g.store == "steam" {
            if let Some(s) = &g.series {
                if !norm(&g.title).contains(&norm(s)) {
                    g.series = None;
                }
            }
        }
    }
    // Copies of the same game on other stores (or its remaster) share a series: e.g. Epic's
    // "BioShock 2 Remastered" gets BioShock from GOG's "BioShock® 2".
    let mut by_key: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for g in games.iter() {
        if let Some(s) = &g.series {
            by_key.entry(series::title_key(&g.title)).or_insert_with(|| s.clone());
        }
    }
    for g in games.iter_mut().filter(|g| g.series.is_none()) {
        g.series = by_key.get(&series::title_key(&g.title)).cloned();
    }
}

/// Fetch series from Wikidata for games not looked up yet (background; emits "series-updated").
#[tauri::command]
async fn refresh_series(app: AppHandle, inner: State<'_, Shared>) -> Result<usize, String> {
    let games: Vec<(String, String)> = inner.games.lock().unwrap().values().map(|g| (g.id.clone(), g.title.clone())).collect();
    let n = series::refresh(games).await?;
    if n > 0 {
        let _ = app.emit("series-updated", n);
    }
    Ok(n)
}

#[tauri::command]
fn launch_game(id: String, mode: Option<String>, inner: State<'_, Shared>, app: AppHandle) -> Result<(), String> {
    let game = inner.games.lock().unwrap().get(&id).cloned().ok_or("Unknown game")?;
    if !game.installed {
        return Err(format!("{} isn't installed", game.title));
    }
    if inner.sessions.lock().unwrap().contains_key(&id) {
        return Err(format!("{} is already running", game.title));
    }
    if game.needs_emulator {
        return Err(format!("No emulator that can run {} is installed yet ({}).", game.title, game.via.split(" · ").next().unwrap_or("this system")));
    }
    // Kiln-managed games go through Kiln's Guarded Play so their mods are deployed and loaders set up.
    // mode: "modded" / "vanilla" from the Play menu; None = the default from Settings.
    let mode = mode.unwrap_or_else(|| if settings::load().kiln_play { "modded".into() } else { "plain".into() });
    let kiln_spec = match (game.kiln_managed, game.kiln.as_deref(), mode.as_str()) {
        (true, Some(k), "modded") => kiln::play(k),
        (true, Some(k), "vanilla") => kiln::play_vanilla(k),
        _ => None,
    };
    // "alt": the game's second way to start (e.g. through Battle.net instead of directly).
    let alt = if mode == "alt" { game.alt_launch.as_ref() } else { None };
    // WoW started directly with "realm": show the realm list instead of joining the last realm.
    if mode == "realm" && game.pick_realm {
        if let Some(dir) = game.launch.as_ref().and_then(|l| l.cwd.as_ref()) {
            stores::winapps::wow_prepare(dir)?;
        }
    }
    let spec = alt.or(kiln_spec.as_ref()).or(game.launch.as_ref()).ok_or("No launch method for this game")?;
    #[cfg(not(windows))]
    let with_hud;
    #[cfg(not(windows))]
    let spec = if inner.store.lock().unwrap().games.get(&id).and_then(|e| e.mangohud).unwrap_or_else(|| settings::load().mangohud) {
        with_hud = mangohud::apply(spec.clone());
        &with_hud
    } else {
        spec
    };
    let child = launcher::spawn(spec, launcher::log_path(&id)).map_err(|e| format!("Couldn't start {}: {e}", spec.program))?;
    // Kiln's window can stay open after the game closes, so track the game by its own process only.
    let child = if kiln_spec.is_some() { None } else { Some(child) };
    {
        let mut s = inner.store.lock().unwrap();
        s.entry(&id).last_played = state::now();
        s.save();
    }
    inner.sessions.lock().unwrap().insert(id.clone(), Session::new(&game, child));
    let _ = app.emit("running", running_now(&inner));
    Ok(())
}

/// Stop tracking a launch (e.g. it failed and is stuck on "launching"). Doesn't kill anything.
#[tauri::command]
fn forget_session(id: String, inner: State<'_, Shared>, app: AppHandle) {
    inner.sessions.lock().unwrap().remove(&id);
    let _ = app.emit("running", running_now(&inner));
}

#[tauri::command]
fn set_flag(id: String, flag: String, value: bool, inner: State<'_, Shared>) -> Result<(), String> {
    let mut s = inner.store.lock().unwrap();
    let e = s.entry(&id);
    match flag.as_str() {
        "favorite" => e.favorite = value,
        "hidden" => e.hidden = value,
        "mangohud" => e.mangohud = Some(value),
        _ => return Err("unknown flag".into()),
    }
    s.save();
    Ok(())
}

/// Set a game's play time (seconds); Lodestone adds what you play from now on. None goes back to
/// counting automatically (the store's figure or Lodestone's own, whichever is bigger).
#[tauri::command]
fn set_playtime(id: String, seconds: Option<u64>, inner: State<'_, Shared>) -> Result<(), String> {
    let mut s = inner.store.lock().unwrap();
    let e = s.entry(&id);
    match seconds {
        Some(n) => {
            let own = match e.playtime_before_edit {
                Some((own, set)) if e.manual_playtime => own + e.playtime.saturating_sub(set),
                _ => e.playtime,
            };
            e.playtime_before_edit = Some((own, n));
            e.playtime = n;
            e.manual_playtime = true;
        }
        None => {
            if let Some((own, set)) = e.playtime_before_edit.take().filter(|_| e.manual_playtime) {
                e.playtime = own + e.playtime.saturating_sub(set);
            }
            e.manual_playtime = false;
        }
    }
    s.save();
    Ok(())
}

#[tauri::command]
fn add_custom_game(title: String, command: String, store: String, cover: Option<String>, proton: Option<bool>) -> Result<(), String> {
    let proton = proton.unwrap_or(false);
    if proton {
        let exe = command.trim().trim_matches('"');
        if !exe.to_ascii_lowercase().ends_with(".exe") || !std::path::Path::new(exe).is_file() {
            return Err("For Proton, enter the full path to the game's .exe".into());
        }
    }
    let title = title.trim().to_string();
    if title.is_empty() || command.trim().is_empty() {
        return Err("Title and command are required".into());
    }
    let mut list = stores::custom::load();
    let base: String = title.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let mut id = base.clone();
    let mut n = 2;
    while list.iter().any(|c| c.id == id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    list.push(stores::custom::CustomGame {
        id,
        title,
        command: command.trim().to_string(),
        store: if store.is_empty() { "custom".into() } else { store },
        cover: cover.filter(|c| !c.trim().is_empty()),
        env: HashMap::new(),
        detect: None,
        runner: proton.then(|| "proton".to_string()),
    });
    stores::custom::save(&list).map_err(|e| e.to_string())
}

#[tauri::command]
fn remove_custom_game(id: String) -> Result<(), String> {
    let short = id.strip_prefix("custom:").ok_or("Only games you added can be removed here")?;
    let mut list = stores::custom::load();
    list.retain(|c| c.id != short);
    stores::custom::save(&list).map_err(|e| e.to_string())
}

/// Open a store's own app (for installing, logging in, settings).
#[tauri::command]
fn open_client(store: String) -> Result<(), String> {
    if let Some(app) = stores::winapps::apps().into_iter().find(|a| a.store == store && a.installed()) {
        let spec = app.open()?;
        return launcher::spawn(&spec, launcher::log_path(&format!("client-{}", app.id))).map(|_| ()).map_err(|e| e.to_string());
    }
    // Windows: every launcher is one of the apps above (installed through winget).
    #[cfg(windows)]
    let spec: Option<LaunchSpec> = None;
    #[cfg(not(windows))]
    let spec = match store.as_str() {
        "steam" => Some(LaunchSpec::new("steam", &[])),
        "epic" | "gog" | "amazon" => stores::heroic::open_client(),
        "minecraft" => Some(LaunchSpec::new("flatpak", &["run", "com.atlauncher.ATLauncher"])),
        _ => Some(LaunchSpec::new("flatpak", &["run", "net.lutris.Lutris"])),
    };
    let spec = spec.ok_or("That launcher isn't installed")?;
    launcher::spawn(&spec, launcher::log_path(&format!("client-{store}"))).map(|_| ()).map_err(|e| e.to_string())
}

#[tauri::command]
fn read_log(id: String) -> String {
    let p = launcher::log_path(&id);
    let txt = std::fs::read_to_string(&p).unwrap_or_else(|_| "(no log yet)".into());
    let tail: Vec<&str> = txt.lines().rev().take(200).collect();
    format!("{}\n\n{}", p.display(), tail.into_iter().rev().collect::<Vec<_>>().join("\n"))
}

#[tauri::command]
fn running(inner: State<'_, Shared>) -> Vec<Running> {
    running_now(&inner)
}

pub(crate) fn running_now(inner: &Inner) -> Vec<Running> {
    let procs = launcher::process_table();
    let mut sessions = inner.sessions.lock().unwrap();
    sessions
        .iter_mut()
        .filter_map(|(id, s)| match s.tick(id, &procs) {
            Tick::Alive(r) => Some(r),
            Tick::Stopped { .. } => None,
        })
        .collect()
}

// ---------------- install / uninstall via the store's client ----------------

fn store_action(inner: &Inner, id: &str, pick: fn(&Game) -> Option<&LaunchSpec>, what: &str) -> Result<(), String> {
    let game = inner.games.lock().unwrap().get(id).cloned().ok_or("Unknown game")?;
    let spec = pick(&game).ok_or_else(|| format!("{} can't be {what} from here", game.title))?.clone();
    launcher::spawn(&spec, launcher::log_path(&format!("{what}-{id}"))).map(|_| ()).map_err(|e| e.to_string())
}

#[tauri::command]
async fn install_game(id: String, inner: State<'_, Shared>) -> Result<String, String> {
    let game = inner.games.lock().unwrap().get(&id).cloned().ok_or("Unknown game")?;
    if let Some(app) = id.strip_prefix("epic:").filter(|_| stores::epic::account().is_some() && game.install.is_none()) {
        tools::LEGENDARY.ensure().await?;
        #[cfg(not(windows))]
        if tools::umu_path().is_none() {
            tools::install_umu().await?;
        }
        downloads::start(&id, &game.title, &stores::epic::install_cmd(app)?, None)?;
        return Ok("download".into());
    }
    if let Some(gid) = id.strip_prefix("gog:").filter(|_| stores::gog::account().is_some() && game.install.is_none()) {
        tools::GOGDL.ensure().await?;
        #[cfg(not(windows))]
        if tools::umu_path().is_none() {
            tools::install_umu().await?;
        }
        let (g2, t2) = (gid.to_string(), game.title.clone());
        downloads::start(&id, &game.title, &stores::gog::install_cmd(gid)?, Some(Box::new(move || stores::gog::finish_install(&g2, &t2))))?;
        return Ok("download".into());
    }
    let mut how = "client";
    // Blizzard games: download through Battle.net's own service with its window kept hidden.
    #[cfg(not(windows))]
    if let Some((code, tact)) = id.strip_prefix("battlenet:").filter(|_| !game.installed).and_then(stores::winapps::bnet_product) {
        let title = game.title.clone();
        let r = tauri::async_runtime::spawn_blocking(move || stores::bnet_agent::install(&code, &tact, &title)).await.map_err(|e| e.to_string())?;
        match r {
            Ok(()) => return Ok("download".into()),
            // Couldn't reach it: fall back to Battle.net's page for the game.
            Err(e) => {
                let _ = std::fs::write(launcher::log_path(&format!("bnet-install-{id}")), &e);
                how = "client-fallback";
            }
        }
    }
    if let Some(appid) = id.strip_prefix("steam:").and_then(|a| a.parse::<u32>().ok()).filter(|_| steam_bg::enabled()) {
        match steam_bg::install(appid, &game.title) {
            Ok(()) => return Ok("download".into()),
            Err(e) if e == steam_bg::BUSY => how = "client-busy",
            Err(e) => return Err(e),
        }
    }
    store_action(&inner, &id, |g| g.install.as_ref(), "installed")?;
    Ok(how.into())
}

/// Update a game: Epic/GOG games installed by Lodestone download here; Steam games go to Steam's
/// Downloads page (only the Steam client can update them).
#[tauri::command]
async fn update_game(id: String, inner: State<'_, Shared>) -> Result<String, String> {
    let game = inner.games.lock().unwrap().get(&id).cloned().ok_or("Unknown game")?;
    if let Some(app) = id.strip_prefix("epic:") {
        tools::LEGENDARY.ensure().await?;
        downloads::start(&format!("update-{id}"), &format!("Updating {}", game.title), &stores::epic::update_cmd(app)?, None)?;
        return Ok("download".into());
    }
    if let Some(gid) = id.strip_prefix("gog:") {
        tools::GOGDL.ensure().await?;
        downloads::start(&format!("update-{id}"), &format!("Updating {}", game.title), &stores::gog::update_cmd(gid)?, None)?;
        return Ok("download".into());
    }
    if let Some(appid) = id.strip_prefix("steam:").and_then(|a| a.parse::<u32>().ok()).filter(|_| steam_bg::enabled()) {
        match steam_bg::update(appid, &game.title) {
            Ok(()) => return Ok("download".into()),
            Err(e) if e == steam_bg::BUSY => {}
            Err(e) => return Err(e),
        }
    }
    if id.starts_with("steam:") {
        let spec = stores::steam::open_url("steam://open/downloads").ok_or("Steam isn't installed")?;
        launcher::spawn(&spec, launcher::log_path("steam-url")).map_err(|e| e.to_string())?;
        return Ok("steam".into());
    }
    Err(format!("{} updates through {}", game.title, game.via))
}

#[tauri::command]
async fn uninstall_game(id: String, inner: State<'_, Shared>) -> Result<String, String> {
    if let Some(app) = id.strip_prefix("epic:").filter(|_| stores::epic::account().is_some()) {
        let game = inner.games.lock().unwrap().get(&id).cloned();
        if game.is_some_and(|g| g.via.starts_with("Lodestone")) {
            stores::epic::uninstall(app).await?;
            return Ok("done".into());
        }
    }
    if let Some(gid) = id.strip_prefix("gog:").filter(|_| stores::gog::installed().iter().any(|i| format!("gog:{}", i.id) == id)) {
        stores::gog::uninstall(gid)?;
        return Ok("done".into());
    }
    let mut how = "client";
    if let Some(appid) = id.strip_prefix("steam:").and_then(|a| a.parse::<u32>().ok()).filter(|_| steam_bg::enabled()) {
        let title = inner.games.lock().unwrap().get(&id).map(|g| g.title.clone()).unwrap_or_default();
        match steam_bg::uninstall(appid, &title) {
            Ok(()) => return Ok("background".into()),
            Err(e) if e == steam_bg::BUSY => how = "client-busy",
            Err(e) => return Err(e),
        }
    }
    store_action(&inner, &id, |g| g.uninstall.as_ref(), "uninstalled")?;
    Ok(how.into())
}

// ---------------- accounts (Epic, GOG) ----------------

/// Epic shows the sign-in code as JSON on its redirect page; hand it to Lodestone by navigating
/// to a marker URL that on_navigation intercepts (the page never actually loads).
const EPIC_CODE_SCRIPT: &str = r#"(function(){
  if (!/epicgames\.com$/.test(location.hostname) || !location.pathname.startsWith('/id/api/redirect')) return;
  function go(){ try {
    var j = JSON.parse(document.body.innerText);
    var c = j.authorizationCode || (j.redirectUrl && new URL(j.redirectUrl).searchParams.get('code'));
    if (c) location.replace('https://lodestone.localhost/auth?code=' + encodeURIComponent(c));
  } catch (e) {} }
  if (document.readyState !== 'loading') go(); else document.addEventListener('DOMContentLoaded', go);
})();"#;

async fn store_login_finish(app: AppHandle, store: String, code: String) {
    if let Some(w) = app.get_webview_window(&format!("login-{store}")) {
        let _ = w.close();
    }
    let r = match store.as_str() {
        "epic" => stores::epic::login(&code).await,
        "gog" => stores::gog::login(&code).await,
        "ubisoft" => stores::ubisoft_api::login(&code).await,
        "ea" => {
            let (tok, exp) = code.split_once('|').unwrap_or((code.as_str(), "3600"));
            match stores::ea_api::login(tok, exp.parse().unwrap_or(3600)).await {
                Ok(n) => stores::ea_api::refresh().await.map(|_| n),
                Err(e) => Err(e),
            }
        }
        _ => Err("unknown store".into()),
    };
    let payload = match r {
        Ok(name) => serde_json::json!({ "store": store, "ok": true, "account": name }),
        Err(e) => serde_json::json!({ "store": store, "ok": false, "error": e }),
    };
    let _ = app.emit("account", payload);
}

/// Open the store's official sign-in page in a Lodestone window and catch the sign-in code.
#[tauri::command]
async fn store_login(app: AppHandle, store: String) -> Result<(), String> {
    let ubi_url = stores::ubisoft_api::login_url();
    let (url, title) = match store.as_str() {
        "epic" => (stores::epic::LOGIN_URL, "Sign in to Epic Games"),
        "gog" => (stores::gog::LOGIN_URL, "Sign in to GOG"),
        "ubisoft" => (ubi_url.as_str(), "Sign in to Ubisoft"),
        // Straight to the token page: it answers at once when the window still has EA's sign-in.
        "ea" => (stores::ea_api::TOKEN_URL, "Sign in to EA"),
        _ => return Err("unknown store".into()),
    };
    let label = format!("login-{store}");
    if let Some(w) = app.get_webview_window(&label) {
        let _ = w.set_focus();
        return Ok(());
    }
    let parsed: tauri::Url = url.parse().map_err(|_| "bad login url")?;
    let handle = app.clone();
    let st = store.clone();
    let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let script = match store.as_str() {
        "ubisoft" => stores::ubisoft_api::LOGIN_SCRIPT,
        "ea" => stores::ea_api::LOGIN_SCRIPT,
        _ => EPIC_CODE_SCRIPT,
    };
    let nav_app = app.clone();
    let ea_signing_in = Arc::new(std::sync::atomic::AtomicBool::new(false));
    // The last site this window was on: the token hand-off (a lodestone.localhost address) is only
    // accepted straight from the store's own sign-in pages, not from any other site the window visits.
    let last_host = Arc::new(Mutex::new(parsed.host_str().unwrap_or("").to_string()));
    let fired_close = fired.clone();
    let close_app = app.clone();
    let close_store = store.clone();
    if store == "ea" {
        let _ = std::fs::remove_file(launcher::log_path("login-ea"));
    }
    let win = WebviewWindowBuilder::new(&app, &label, WebviewUrl::External(parsed))
        .title(title)
        .inner_size(560.0, 780.0)
        .initialization_script(script)
        .on_navigation(move |u| {
            let host = u.host_str().unwrap_or("");
            let came_from = if host == "lodestone.localhost" { last_host.lock().unwrap().clone() } else { std::mem::replace(&mut *last_host.lock().unwrap(), host.to_string()) };
            if st == "ea" {
                // Where the sign-in goes (addresses only, no query strings: those can hold tokens).
                login_log(&format!("{}://{}{}", u.scheme(), host, u.path()));
                if host.ends_with("signin.ea.com") || (host == "accounts.ea.com" && !u.path().starts_with("/connect/auth")) {
                    ea_signing_in.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                // Back on ea.com after signing in: ask for the token again.
                if host.ends_with("ea.com") && !host.ends_with("signin.ea.com") && host != "accounts.ea.com" && !u.path().contains("/login")
                    && ea_signing_in.swap(false, std::sync::atomic::Ordering::SeqCst)
                {
                    let a = nav_app.clone();
                    tauri::async_runtime::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                        if let (Some(w), Ok(url)) = (a.get_webview_window("login-ea"), stores::ea_api::TOKEN_URL.parse()) {
                            let _ = w.navigate(url);
                        }
                    });
                }
            }
            if host == "lodestone.localhost" {
                let trusted = match st.as_str() {
                    "ea" => came_from == "accounts.ea.com" || came_from.ends_with(".ea.com") || came_from == "ea.com",
                    "ubisoft" => came_from.ends_with("ubisoft.com") || came_from.ends_with("ubi.com"),
                    "epic" => came_from.ends_with("epicgames.com"),
                    _ => false,
                };
                if !trusted {
                    if st == "ea" {
                        login_log(&format!("ignored a sign-in hand-off from {came_from}"));
                    }
                    return false;
                }
                let q: HashMap<String, String> = u.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
                // EA: not signed in yet, so show EA's sign-in page in this window.
                if q.contains_key("ea_login") {
                    let a = nav_app.clone();
                    tauri::async_runtime::spawn(async move {
                        if let (Some(w), Ok(url)) = (a.get_webview_window("login-ea"), stores::ea_api::LOGIN_URL.parse()) {
                            let _ = w.navigate(url);
                        }
                    });
                    return false;
                }
                let code = q.get("ubisoft").cloned().or_else(|| q.get("ea").map(|t| format!("{t}|{}", q.get("expires").map(String::as_str).unwrap_or("3600"))));
                if let Some(code) = code {
                    if !fired.swap(true, std::sync::atomic::Ordering::SeqCst) {
                        tauri::async_runtime::spawn(store_login_finish(handle.clone(), st.clone(), code));
                    }
                    return false;
                }
            }
            let is_code_url = host == "lodestone.localhost" || (host == "embed.gog.com" && u.path().starts_with("/on_login_success"));
            if !is_code_url {
                return true;
            }
            // GOG's code arrives on embed.gog.com: only straight from GOG's own sign-in pages.
            if st == "gog" && !came_from.ends_with("gog.com") {
                return false;
            }
            if let Some(code) = u.query_pairs().find(|(k, _)| k == "code").map(|(_, v)| v.into_owned()) {
                if !fired.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    tauri::async_runtime::spawn(store_login_finish(handle.clone(), st.clone(), code));
                }
            }
            false
        })
        .build()
        .map_err(|e| e.to_string())?;
    // Closed before finishing: stop showing "Waiting…".
    win.on_window_event(move |e| {
        if matches!(e, tauri::WindowEvent::Destroyed) && !fired_close.load(std::sync::atomic::Ordering::SeqCst) {
            let _ = close_app.emit("account", serde_json::json!({ "store": close_store, "ok": false, "error": "the sign-in window was closed before it finished" }));
        }
    });
    // Fetch the store's engine meanwhile so sign-in completes quickly.
    match store.as_str() {
        "epic" => drop(tools::LEGENDARY.ensure().await),
        "gog" => drop(tools::GOGDL.ensure().await),
        _ => {}
    }
    Ok(())
}

/// Fallback when the sign-in window can't be used: the user pastes the code shown by the store.
#[tauri::command]
async fn store_login_code(app: AppHandle, store: String, code: String) -> Result<(), String> {
    let code = code.trim().trim_matches('"').to_string();
    if code.is_empty() {
        return Err("Paste the code first".into());
    }
    store_login_finish(app, store, code).await;
    Ok(())
}

#[tauri::command]
async fn store_logout(store: String) -> Result<(), String> {
    match store.as_str() {
        "epic" => stores::epic::logout().await,
        "ubisoft" => {
            stores::ubisoft_api::logout();
            Ok(())
        }
        "ea" => {
            stores::ea_api::logout();
            Ok(())
        }
        "gog" => {
            stores::gog::logout();
            Ok(())
        }
        _ => Err("unknown store".into()),
    }
}

/// Reuse Heroic's sign-in instead of signing in again. Heroic will need to sign in afterwards.
#[tauri::command]
async fn store_import_heroic(store: String) -> Result<String, String> {
    match store.as_str() {
        "epic" => {
            tools::LEGENDARY.ensure().await?;
            stores::epic::import_heroic()?;
            stores::epic::refresh().await?;
            Ok(stores::epic::account().unwrap_or_default())
        }
        "gog" => {
            tools::GOGDL.ensure().await?;
            stores::gog::import_heroic()?;
            stores::gog::refresh().await?;
            Ok(stores::gog::account().unwrap_or_default())
        }
        _ => Err("unknown store".into()),
    }
}

/// Re-fetch owned libraries for signed-in stores.
#[tauri::command]
async fn store_refresh(app: AppHandle) -> Result<(), String> {
    let mut errs = Vec::new();
    if stores::ubisoft_api::account().is_some() {
        if let Err(e) = stores::ubisoft_api::refresh().await {
            errs.push(format!("Ubisoft: {e}"));
        }
    }
    if stores::ea_api::account().is_some() {
        let mut r = stores::ea_api::refresh().await;
        if matches!(&r, Err(e) if e == "expired") {
            r = match ea_renew(&app).await {
                Ok(()) => stores::ea_api::refresh().await,
                Err(e) => Err(e),
            };
        }
        if let Err(e) = r {
            errs.push(format!("EA: {e}"));
        }
    }
    if let Err(e) = stores::steam::refresh_owned().await {
        errs.push(format!("Steam: {e}"));
    }
    if stores::epic::account().is_some() {
        if let Err(e) = stores::epic::refresh().await {
            errs.push(format!("Epic: {e}"));
        }
        // Play time is extra: no error note if Epic's play-time service doesn't answer.
        let _ = playtime::refresh_epic().await;
    }
    if stores::gog::account().is_some() {
        if let Err(e) = stores::gog::refresh().await {
            errs.push(format!("GOG: {e}"));
        }
    }
    if errs.is_empty() { Ok(()) } else { Err(errs.join("; ")) }
}

fn login_log(line: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(launcher::log_path("login-ea")) {
        let _ = writeln!(f, "{} {line}", state::now());
    }
}

/// A new EA token from the sign-in cookies, in a hidden window (EA tokens last a few hours).
async fn ea_renew(app: &AppHandle) -> Result<(), String> {
    const LABEL: &str = "ea-renew";
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.close();
    }
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<(String, u64), String>>();
    let tx = Arc::new(Mutex::new(Some(tx)));
    let url: tauri::Url = stores::ea_api::TOKEN_URL.parse().map_err(|_| "bad url")?;
    let w = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::External(url))
        .visible(false)
        .initialization_script(stores::ea_api::LOGIN_SCRIPT)
        .on_navigation(move |u| {
            if u.host_str() != Some("lodestone.localhost") {
                return true;
            }
            let q: HashMap<String, String> = u.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
            let r = match q.get("ea") {
                Some(t) => Ok((t.clone(), q.get("expires").and_then(|e| e.parse().ok()).unwrap_or(3600))),
                None => Err("EA sign-in expired: sign in again in Accounts".to_string()),
            };
            if let Some(tx) = tx.lock().unwrap().take() {
                let _ = tx.send(r);
            }
            false
        })
        .build()
        .map_err(|e| e.to_string())?;
    let r = tokio::time::timeout(std::time::Duration::from_secs(30), rx).await;
    let _ = w.close();
    let (tok, exp) = r.map_err(|_| "EA didn't answer".to_string())?.map_err(|_| "EA didn't answer".to_string())??;
    stores::ea_api::login(&tok, exp).await.map(|_| ())
}

fn steam_account() -> Option<String> {
    let txt = std::fs::read_to_string(model::home().join(".local/share/Steam/config/loginusers.vdf")).ok()?;
    let v = vdf::parse(&txt);
    let users = v.get("users")?;
    let recent = users.entries().iter().find(|(_, u)| u.str("MostRecent") == Some("1")).or(users.entries().first())?;
    recent.1.str("PersonaName").map(String::from)
}

#[tauri::command]
fn accounts() -> serde_json::Value {
    let h = model::home();
    let heroic = |rel: &str| {
        [h.join(".var/app/com.heroicgameslauncher.hgl/config/heroic").join(rel), h.join(".config/heroic").join(rel)].iter().any(|p| p.is_file())
    };
    serde_json::json!({
        "epic": { "account": stores::epic::account(), "heroic": heroic("legendaryConfig/legendary/user.json") },
        "gog": { "account": stores::gog::account(), "heroic": heroic("gog_store/auth.json") },
        "steam": { "account": steam_account() },
        "battlenet": { "installed": stores::winapps::find("battlenet").map(|a| a.installed()).unwrap_or(false) },
        "ea": { "installed": stores::winapps::find("ea").map(|a| a.installed()).unwrap_or(false), "account": stores::ea_api::account() },
        "ubisoft": { "installed": stores::winapps::find("ubisoft").map(|a| a.installed()).unwrap_or(false), "account": stores::ubisoft_api::account() },
    })
}

/// Install DirectX/VC++ runtimes into a Proton game's prefix (shows as a download job).
#[tauri::command]
#[cfg(not(windows))]
fn install_runtimes(id: String, inner: State<'_, Shared>) -> Result<(), String> {
    let game = inner.games.lock().unwrap().get(&id).cloned().ok_or("Unknown game")?;
    let pfx = game.prefix.ok_or("This game doesn't run through Lodestone's Proton")?;
    let spec = proton::winetricks(std::path::Path::new(&pfx), proton::RUNTIMES)?;
    downloads::start(&format!("runtimes-{id}"), &format!("Runtimes for {}", game.title), &spec, None)
}

#[cfg(not(windows))]
fn have_flatpak(app_id: &str) -> bool {
    let h = model::home();
    [std::path::PathBuf::from("/var/lib/flatpak/app"), h.join(".local/share/flatpak/app")].iter().any(|b| b.join(app_id).exists())
}

/// What the first-run guide shows: launchers and tools already on this PC.
#[tauri::command]
fn setup_overview() -> serde_json::Value {
    let kiln = kiln::command().is_some().then(|| {
        let g = kiln::games();
        serde_json::json!({ "supported": g.iter().filter(|x| x.installed).count(), "managed": g.iter().filter(|x| x.managed).map(|x| x.name.clone()).collect::<Vec<_>>() })
    });
    // Windows: Steam, Epic and GOG are set up through winget, and games run natively.
    #[cfg(windows)]
    return serde_json::json!({
        "done": settings::load().onboarded,
        "steam": { "found": stores::steam::steam_id64().is_some(), "account": steam_account() },
        "games_dir": settings::games_dir().to_string_lossy(),
        "kiln": kiln,
    });
    #[cfg(not(windows))]
    {
        let w = |b: &str| model::which(b).is_some();
        serde_json::json!({
            "done": settings::load().onboarded,
            "steam": { "found": stores::steam::steam_id64().is_some() || w("steam") || have_flatpak("com.valvesoftware.Steam"), "account": steam_account() },
            "heroic": have_flatpak("com.heroicgameslauncher.hgl") || w("heroic"),
            "lutris": have_flatpak("net.lutris.Lutris") || w("lutris"),
            "atlauncher": have_flatpak("com.atlauncher.ATLauncher") || w("atlauncher"),
            "prism": have_flatpak("org.prismlauncher.PrismLauncher") || w("prismlauncher"),
            "protons": tools::protons().iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
            "umu": tools::umu_version(),
            "games_dir": settings::games_dir().to_string_lossy(),
            "kiln": kiln,
        })
    }
}

#[tauri::command]
fn set_onboarded(done: bool) -> Result<(), String> {
    let mut s = settings::load();
    s.onboarded = done;
    settings::save(&s)
}

#[tauri::command]
async fn kiln_status(game: String) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || kiln::status(&game))
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Kiln didn't answer".to_string())
}

/// Open Kiln on a game (to set up or manage its mods).
#[tauri::command]
fn kiln_open(game: String) -> Result<(), String> {
    let spec = kiln::open(&game).ok_or("Kiln isn't installed")?;
    launcher::spawn(&spec, launcher::log_path("kiln")).map(|_| ()).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_kiln_play(on: bool) -> Result<(), String> {
    let mut s = settings::load();
    s.kiln_play = on;
    settings::save(&s)
}

#[tauri::command]
fn shop_show(app: AppHandle, store: String, x: f64, y: f64, w: f64, h: f64) -> Result<(), String> {
    shop::show(&app, &store, x, y, w, h)
}

#[tauri::command]
fn shop_hide(app: AppHandle) {
    shop::hide(&app)
}

#[tauri::command]
fn shop_bounds(app: AppHandle, x: f64, y: f64, w: f64, h: f64) {
    shop::bounds(&app, x, y, w, h)
}

#[tauri::command]
fn shop_nav(app: AppHandle, store: String, action: String) -> Result<(), String> {
    shop::nav(&app, &store, &action)
}

#[tauri::command]
fn shop_open_external(app: AppHandle) -> Result<(), String> {
    let url = shop::current_url(&app).ok_or("store isn't open")?;
    open_url(url)
}

/// Async so checking Steam's log and Battle.net's download service never blocks the window.
#[tauri::command]
async fn downloads_status() -> Vec<downloads::Status> {
    tauri::async_runtime::spawn_blocking(|| {
        let mut v = downloads::statuses();
        v.extend(steam_bg::statuses());
        #[cfg(not(windows))]
        v.extend(stores::bnet_agent::statuses());
        downloads::finish(&mut v);
        downloads::record(&v);
        v
    })
    .await
    .unwrap_or_default()
}

#[tauri::command]
fn downloads_history() -> Vec<downloads::Past> {
    downloads::history()
}

#[tauri::command]
fn downloads_history_clear() {
    downloads::clear_history()
}

fn is_steam_job(id: &str) -> bool {
    id.starts_with("steam:") || id.starts_with("update-steam:") || id.starts_with("uninstall-steam:")
}

#[tauri::command]
fn download_cancel(id: String) -> Result<(), String> {
    #[cfg(not(windows))]
    if stores::bnet_agent::is_job(&id) {
        return stores::bnet_agent::cancel(&id);
    }
    if is_steam_job(&id) {
        return steam_bg::cancel(&id);
    }
    downloads::cancel(&id);
    Ok(())
}

#[tauri::command]
fn download_dismiss(id: String) {
    #[cfg(not(windows))]
    stores::bnet_agent::dismiss(&id);
    if is_steam_job(&id) {
        steam_bg::dismiss(&id);
    }
    downloads::dismiss(&id)
}

#[tauri::command]
fn set_steam_background(on: bool) -> Result<(), String> {
    let mut s = settings::load();
    s.steam_background = on;
    settings::save(&s)?;
    if on {
        std::thread::spawn(steam_bg::start_silent);
    }
    Ok(())
}

/// Open a web page in the user's default browser.
#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("only web links can be opened".into());
    }
    launcher::spawn(&crate::os::open_spec(&url), launcher::log_path("xdg-open")).map(|_| ()).map_err(|e| e.to_string())
}

// ---------------- metadata ----------------

fn game_title(inner: &Inner, id: &str) -> Result<String, String> {
    inner.games.lock().unwrap().get(id).map(|g| g.title.clone()).ok_or_else(|| "Unknown game".into())
}

#[tauri::command]
fn meta_get(id: String) -> meta::Override {
    meta::get(&id)
}

#[tauri::command]
async fn meta_save(id: String, edit: meta::Override) -> Result<(), String> {
    meta::save_edit(&id, edit).await
}

#[tauri::command]
fn meta_reset(id: String) {
    meta::reset(&id)
}

#[tauri::command]
async fn art_search(query: String) -> Result<Vec<meta::Art>, String> {
    meta::search(&query).await
}

/// Check covers, then fill in missing ones. Returns (missing before, fixed).
#[tauri::command]
async fn art_fix(inner: State<'_, Shared>) -> Result<(usize, usize), String> {
    let games: Vec<Game> = inner.games.lock().unwrap().values().cloned().collect();
    let missing = meta::check(games.clone()).await;
    let fixed = meta::autofix(games).await;
    Ok((missing, fixed))
}

/// Just check which covers are missing (fast after the first time).
#[tauri::command]
async fn art_check(inner: State<'_, Shared>) -> Result<usize, String> {
    let games: Vec<Game> = inner.games.lock().unwrap().values().cloned().collect();
    Ok(meta::check(games).await)
}

#[tauri::command]
async fn game_details(id: String, inner: State<'_, Shared>) -> Result<meta::Details, String> {
    let title = game_title(&inner, &id)?;
    let (ra_id, emulated) = inner.games.lock().unwrap().get(&id).map(|g| (g.ra_id, g.system.is_some())).unwrap_or((None, false));
    if !emulated {
        return meta::details(&id, &title).await;
    }
    // ROMs: RetroAchievements first (developer, publisher, genre, release, images, its page), with a
    // description from Wikipedia, which RA doesn't have.
    let wiki = meta::wikipedia_details(&title).await.unwrap_or_default();
    let ra = match ra_id {
        Some(r) => stores::retroachievements::game_info(r).await.ok(),
        None => None,
    };
    let Some(i) = ra else { return Ok(wiki) };
    let one = |s: &str| if s.is_empty() { vec![] } else { vec![s.to_string()] };
    let shots: Vec<(String, String)> = [&i.title_image, &i.ingame_image].into_iter().filter(|u| !u.is_empty()).map(|u| (u.clone(), u.clone())).collect();
    Ok(meta::Details {
        description: wiki.description,
        developers: if i.developer.is_empty() { wiki.developers } else { one(&i.developer) },
        publishers: if i.publisher.is_empty() { wiki.publishers } else { one(&i.publisher) },
        release: if i.released.is_empty() { wiki.release } else { i.released.split(' ').next().unwrap_or("").to_string() },
        features: one(&i.genre),
        screenshots: shots,
        store_url: i.url.clone(),
        website: wiki.website,
        source: "RetroAchievements".into(),
        ..Default::default()
    })
}

#[tauri::command]
async fn game_protondb(id: String, inner: State<'_, Shared>) -> Result<Option<meta::ProtonDb>, String> {
    let title = game_title(&inner, &id)?;
    Ok(meta::protondb(&id, &title).await)
}

#[tauri::command]
async fn games_compat(ids: Vec<String>) -> Vec<meta::Compat> {
    meta::compat(ids).await
}

#[tauri::command]
async fn game_news(ids: Vec<String>, inner: State<'_, Shared>) -> Result<Vec<meta::News>, String> {
    let games: Vec<(String, String)> = {
        let g = inner.games.lock().unwrap();
        // Emulated games have no store news; skip them rather than searching Steam for ROM titles.
        ids.into_iter().filter_map(|id| g.get(&id).filter(|x| x.system.is_none()).map(|x| (id.clone(), x.title.clone()))).collect()
    };
    Ok(meta::news(games).await)
}

/// Show a web page (patch notes, store page) in a Lodestone window.
#[tauri::command]
async fn open_page(app: AppHandle, url: String, title: Option<String>) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err("only https pages can be opened".into());
    }
    let parsed: tauri::Url = url.parse().map_err(|_| format!("bad url {url}"))?;
    let title = title.unwrap_or_else(|| "Lodestone".into());
    if let Some(w) = app.get_webview_window("page") {
        w.navigate(parsed).map_err(|e| e.to_string())?;
        let _ = w.set_title(&title);
        let _ = w.set_focus();
    } else {
        WebviewWindowBuilder::new(&app, "page", WebviewUrl::External(parsed)).title(&title).inner_size(1100.0, 860.0).build().map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ---------------- friends ----------------

#[derive(Serialize)]
struct FriendsDto {
    friends: Vec<friends::Friend>,
    notes: Vec<String>,
}

#[tauri::command]
async fn friends_list(force: Option<bool>) -> Result<FriendsDto, String> {
    let (friends, notes) = friends::list(force.unwrap_or(false)).await;
    Ok(FriendsDto { friends, notes })
}

/// Steam friends who own this game (non-Steam games use the matching Steam game).
#[tauri::command]
async fn friends_owning(id: String, inner: State<'_, Shared>) -> Result<Vec<friends::Owner>, String> {
    let title = game_title(&inner, &id)?;
    let Some(appid) = meta::steam_appid(&id, &title).await else { return Ok(vec![]) };
    Ok(friends::owners(appid).await)
}

// ---------------- Steam Workshop ----------------

#[tauri::command]
async fn workshop_items(appid: u32) -> Result<Vec<workshop::WorkshopItem>, String> {
    workshop::list(appid).await
}

/// Browse the Workshop in a Lodestone window (sign in to Steam Community once to subscribe),
/// or hand it to the Steam client.
#[tauri::command]
async fn open_workshop(app: AppHandle, appid: u32, item: Option<String>, in_steam: bool) -> Result<(), String> {
    if in_steam {
        let url = match &item {
            Some(i) => format!("steam://url/CommunityFilePage/{i}"),
            None => format!("steam://url/SteamWorkshopPage/{appid}"),
        };
        let spec = stores::steam::open_url(&url).ok_or("Steam isn't installed")?;
        return launcher::spawn(&spec, launcher::log_path("steam-url")).map(|_| ()).map_err(|e| e.to_string());
    }
    let url = match &item {
        Some(i) => format!("https://steamcommunity.com/sharedfiles/filedetails/?id={i}"),
        None => format!("https://steamcommunity.com/app/{appid}/workshop/"),
    };
    let parsed: tauri::Url = url.parse().map_err(|_| format!("bad url {url}"))?;
    if let Some(w) = app.get_webview_window("workshop") {
        w.navigate(parsed).map_err(|e| e.to_string())?;
        let _ = w.set_focus();
    } else {
        WebviewWindowBuilder::new(&app, "workshop", WebviewUrl::External(parsed))
            .title("Steam Workshop")
            .inner_size(1240.0, 880.0)
            .build()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ---------------- Minecraft ----------------

#[tauri::command]
fn mc_instances() -> Vec<mci::Instance> {
    mci::all()
}

#[tauri::command]
async fn mc_installed(instance: String, kind: String, updates: bool) -> Result<serde_json::Value, String> {
    let inst = mci::find(&instance)?;
    let kind = Kind::parse(&kind)?;
    let mut items = platforms::installed(&inst, kind).await;
    let mut update_error = None;
    if updates {
        if let Err(e) = platforms::check_updates(&inst, kind, &mut items).await {
            update_error = Some(e);
        }
    }
    Ok(serde_json::json!({ "items": items, "update_error": update_error }))
}

#[tauri::command]
async fn mc_search(source: String, instance: String, kind: String, query: String, offset: u32) -> Result<Vec<platforms::SearchHit>, String> {
    let inst = mci::find(&instance)?;
    platforms::search(&source, &inst, Kind::parse(&kind)?, &query, offset).await
}

#[tauri::command]
async fn mc_install(source: String, instance: String, kind: String, project: String) -> Result<Vec<String>, String> {
    let inst = mci::find(&instance)?;
    platforms::install(&source, &inst, Kind::parse(&kind)?, &project).await
}

#[tauri::command]
async fn mc_update(instance: String, kind: String, file: String) -> Result<String, String> {
    let inst = mci::find(&instance)?;
    platforms::update(&inst, Kind::parse(&kind)?, &file).await
}

#[tauri::command]
fn mc_toggle(instance: String, kind: String, file: String, enabled: bool) -> Result<(), String> {
    mci::find(&instance)?.set_enabled(Kind::parse(&kind)?, &file, enabled)
}

#[tauri::command]
fn mc_remove(instance: String, kind: String, file: String) -> Result<(), String> {
    mci::find(&instance)?.remove(Kind::parse(&kind)?, &file)
}

#[tauri::command]
fn mc_open_folder(instance: String, kind: String) -> Result<(), String> {
    let dir = mci::find(&instance)?.dir(Kind::parse(&kind)?);
    let _ = std::fs::create_dir_all(&dir);
    launcher::spawn(&crate::os::open_spec(&dir.to_string_lossy()), launcher::log_path("xdg-open")).map(|_| ()).map_err(|e| e.to_string())
}

// ---------------- Proton + Windows launchers ----------------

#[tauri::command]
#[cfg(not(windows))]
fn proton_info() -> serde_json::Value {
    serde_json::json!({
        "umu": tools::umu_version(),
        "protons": tools::protons(),
        "selected": settings::load().proton,
        "effective": tools::proton_path(),
        "auto": tools::AUTO_GE,
    })
}

#[tauri::command]
#[cfg(not(windows))]
async fn setup_proton() -> Result<String, String> {
    tools::install_umu().await
}

#[tauri::command]
#[cfg(not(windows))]
fn mangohud_info() -> mangohud::Info {
    mangohud::info()
}

#[tauri::command]
#[cfg(not(windows))]
async fn setup_mangohud() -> Result<String, String> {
    mangohud::install().await
}

#[tauri::command]
#[cfg(not(windows))]
fn mangohud_config_get() -> (String, String) {
    mangohud::config_get()
}

#[tauri::command]
#[cfg(not(windows))]
fn mangohud_config_save(text: String) -> Result<(), String> {
    mangohud::config_save(&text)
}

#[tauri::command]
#[cfg(not(windows))]
fn mangohud_preview() -> Result<(), String> {
    let spec = mangohud::preview()?;
    launcher::spawn(&spec, launcher::log_path("mangohud-preview")).map(|_| ()).map_err(|e| e.to_string())
}

#[tauri::command]
#[cfg(not(windows))]
fn set_mangohud(on: bool) -> Result<(), String> {
    let mut s = settings::load();
    s.mangohud = on;
    settings::save(&s)
}

#[tauri::command]
#[cfg(not(windows))]
fn set_proton(path: String) -> Result<(), String> {
    let mut s = settings::load();
    s.proton = path;
    settings::save(&s)
}

#[tauri::command]
fn winapp_status() -> Vec<stores::winapps::Status> {
    stores::winapps::status()
}

/// Download the official installer (Battle.net, GW2) and run it in Lodestone's own Proton prefix.
#[tauri::command]
async fn winapp_setup(id: String) -> Result<(), String> {
    let app = stores::winapps::find(&id)?;
    let spec = stores::winapps::setup(app).await?;
    launcher::spawn(&spec, launcher::log_path(&format!("setup-{id}"))).map(|_| ()).map_err(|e| e.to_string())
}

/// Open Battle.net / GW2 normally (sign in, install games, settings).
#[tauri::command]
fn winapp_open(id: String) -> Result<(), String> {
    let app = stores::winapps::find(&id)?;
    if !app.installed() {
        return Err(format!("{} isn't set up yet", app.name));
    }
    let spec = app.open()?;
    launcher::spawn(&spec, launcher::log_path(&format!("client-{id}"))).map(|_| ()).map_err(|e| e.to_string())
}

// ---------------- settings ----------------

/// `lodestone --view minecraft` opens straight into a section (e.g. a dedicated Minecraft launcher shortcut).
#[tauri::command]
fn startup_view() -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter().position(|a| a == "--view").and_then(|i| args.get(i + 1)).cloned()
}

/// Systems that have games (from the last library scan), for the sidebar.
#[tauri::command]
fn emu_systems() -> Vec<stores::emulation::System> {
    stores::emulation::systems()
}

/// RetroAchievements: refresh the game lists for your systems, fingerprint ROMs not seen yet (reads
/// them once), then fetch box art for matched games a few at a time. Emits "ra-updated" when the
/// library should reload. Does nothing until a username and key are set.
#[tauri::command]
async fn ra_sync(app: AppHandle, inner: State<'_, Shared>) -> Result<usize, String> {
    use stores::retroachievements as ra;
    if !ra::signed_in() {
        return Ok(0);
    }
    static RUNNING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let Ok(_guard) = RUNNING.try_lock() else { return Ok(0) };
    let mut consoles: Vec<u32> = stores::emulation::systems().iter().filter_map(|s| ra::console_for(&s.id)).collect();
    consoles.sort_unstable();
    consoles.dedup();
    let lists = ra::refresh_lists(&consoles).await?;
    let roms = stores::emulation::roms();
    let new = blocking(move || ra::fingerprint_all(&roms)).await?;
    if lists > 0 || new > 0 {
        let _ = app.emit("ra-updated", ());
    }
    // Box art for matched games, 40 at a time with a pause between requests (RA asks clients not to
    // hammer its API). The library refreshes after each batch; this stops when a batch gets nothing.
    let mut art = 0;
    loop {
        let ids: Vec<u64> = inner.games.lock().unwrap().values().filter_map(|g| g.ra_id).collect();
        let got = ra::fetch_missing_art(ids, 40).await;
        if got == 0 {
            break;
        }
        art += got;
        let _ = app.emit("ra-updated", ());
    }
    Ok(lists + new + art)
}

/// Refresh libretro's box art lists for the systems you have ROMs for (weekly), then reload the library.
#[tauri::command]
async fn thumbs_sync(app: AppHandle) -> Result<usize, String> {
    let systems: Vec<String> = stores::emulation::systems().iter().map(|s| s.id.clone()).collect();
    let n = stores::libretro_thumbs::refresh(&systems).await;
    if n > 0 {
        let _ = app.emit("ra-updated", ());
    }
    Ok(n)
}

/// A store game's achievements with your progress (Steam for now). "need-key" without a Steam Web API key.
#[tauri::command]
async fn store_achievements(id: String) -> Result<Option<achievements::Achievements>, String> {
    match id.strip_prefix("steam:").and_then(|a| a.parse::<u32>().ok()) {
        Some(appid) => achievements::steam(appid).await.map(Some),
        None => Ok(None),
    }
}

/// A game's RetroAchievements details: achievements and your progress.
#[tauri::command]
async fn ra_game(id: String, inner: State<'_, Shared>) -> Result<Option<stores::retroachievements::GameInfo>, String> {
    let ra_id = inner.games.lock().unwrap().get(&id).and_then(|g| g.ra_id);
    match ra_id {
        Some(r) => stores::retroachievements::game_info(r).await.map(Some),
        None => Ok(None),
    }
}

/// Choose how a system's games are played (the index of one of its emulator options).
#[tauri::command]
fn emu_set_choice(system: String, index: usize) -> Result<(), String> {
    let mut s = settings::load();
    s.emu_choice.insert(system, index);
    settings::save(&s)
}

/// Install what a system's chosen emulator needs ("installing", "done" or "already").
#[tauri::command]
async fn emu_install(system: String, index: usize, menu: bool) -> Result<&'static str, String> {
    let mut st = settings::load();
    if st.emu_menu_entry != menu {
        st.emu_menu_entry = menu;
        settings::save(&st)?;
    }
    stores::emulation::install(&system, index, menu).await
}

/// The ROM folder setting and every ROMs folder Lodestone found.
#[tauri::command]
async fn emu_folders() -> Result<stores::emulation::FolderInfo, String> {
    blocking(stores::emulation::folder_info).await
}

/// Use this folder for ROMs (an Emulation folder, a roms folder, or "" to only search automatically).
#[tauri::command]
fn emu_set_folder(path: String) -> Result<(), String> {
    let p = path.trim();
    if !p.is_empty() {
        let d = stores::emulation::expand(p).ok_or("Use a full folder path, like ~/Emulation")?;
        if !d.is_dir() {
            return Err(format!("{} doesn't exist; use Create folders to make it", d.display()));
        }
    }
    let mut s = settings::load();
    s.emulation_dir = p.to_string();
    settings::save(&s)
}

/// Create the ROM folder layout in this folder and use it.
#[tauri::command]
async fn emu_create_folders(path: String) -> Result<(String, usize), String> {
    let p = if path.trim().is_empty() { "~/Emulation".to_string() } else { path.trim().to_string() };
    let (roms, made) = blocking({ let p = p.clone(); move || stores::emulation::create_layout(&p) }).await??;
    let mut s = settings::load();
    s.emulation_dir = p;
    settings::save(&s)?;
    Ok((roms.to_string_lossy().into_owned(), made))
}

/// A folder picker from the desktop (zenity or kdialog).
#[tauri::command]
async fn emu_pick_folder(start: String) -> Result<Option<String>, String> {
    blocking(move || stores::emulation::pick_folder(&start)).await?
}

/// Lodestone Beacon: installed? running? its page, and the permissions it asks for.
#[tauri::command]
async fn beacon_status() -> Result<plugins::Status, String> {
    blocking(plugins::status).await
}

/// Which of the programs Lodestone installs (emulators; on Windows also store launchers) can be updated.
/// Only when you ask.
#[tauri::command]
async fn program_updates() -> Result<Vec<program_updates::Update>, String> {
    tauri::async_runtime::spawn_blocking(program_updates::check).await.map_err(|e| e.to_string())?
}

/// Update one of them (a background job in Downloads).
#[tauri::command]
fn program_update(id: String) -> Result<(), String> {
    let (spec, name) = program_updates::update_spec(&id)?;
    downloads::start(&format!("program-update:{}", id.to_ascii_lowercase()), &format!("Updating {name}"), &spec, None)
}

/// Where Lodestone keeps its data on this system (shown in the interface).
#[tauri::command]
fn data_folder() -> String {
    state::data_dir().to_string_lossy().into_owned()
}

/// A problem in Lodestone's own page, written to logs/ui.log on this PC (for bug reports; never sent
/// anywhere). Kept short: the file is cut back when it passes 256 KB.
#[tauri::command]
fn ui_error(message: String) {
    use std::io::Write;
    let p = launcher::log_path("ui");
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if std::fs::metadata(&p).is_ok_and(|m| m.len() > 256 * 1024) {
        let _ = std::fs::remove_file(&p);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let line: String = message.chars().filter(|c| *c != '\r').take(2000).collect();
        let _ = writeln!(f, "{} {}", state::now(), line.replace('\n', " | "));
    }
}

/// Is there a Beacon to install, or a newer version? (Asked when you open Plug-ins or Beacon.)
#[tauri::command]
async fn beacon_check_update() -> Result<serde_json::Value, String> {
    let running = blocking(plugins::status).await?.version;
    beacon_install::check(running).await
}

/// Install (or update to) the newest Beacon from its official releases, after checking its checksum.
#[tauri::command]
async fn beacon_install() -> Result<String, String> {
    beacon_install::install().await
}

/// Start Lodestone Beacon (if it isn't running) and wait for it to be ready.
#[tauri::command]
async fn beacon_start(app: AppHandle) -> Result<plugins::Status, String> {
    blocking(move || plugins::start(app)).await?
}

/// Approve the permissions Beacon asked for (only known ones are stored).
#[tauri::command]
fn beacon_approve(permissions: Vec<String>) -> Result<plugins::Status, String> {
    plugins::approve(permissions)
}

/// The plug-ins Lodestone knows, with their status and permissions.
#[tauri::command]
async fn plugins_list() -> Result<Vec<plugins::PluginInfo>, String> {
    blocking(plugins::list).await
}

/// Stop a running plug-in.
#[tauri::command]
async fn plugin_stop(app: AppHandle, id: String) -> Result<(), String> {
    blocking(move || plugins::stop_plugin(&id)).await??;
    let _ = app.emit("beacon-stopped", ());
    Ok(())
}

/// Is there a release to install, or a newer one? For an official plug-in or a repository you added.
#[tauri::command]
async fn plugin_release_check(id: String) -> Result<serde_json::Value, String> {
    let src = plugin_sources::find(&id).ok_or("Unknown plug-in")?;
    plugin_sources::check(&src).await
}

/// Download, check and install a plug-in's newest release. Returns the version installed.
#[tauri::command]
async fn plugin_install(id: String) -> Result<String, String> {
    if id == plugins::BEACON {
        return beacon_install::install().await;
    }
    let src = plugin_sources::find(&id).ok_or("Unknown plug-in")?;
    let v = plugin_sources::install(&src).await?;
    kiln::forget();
    Ok(v)
}

/// The GitHub repositories you added for third-party plug-ins.
#[tauri::command]
fn plugin_repos() -> Vec<String> {
    plugin_sources::user().into_iter().map(|s| s.repo).collect()
}

#[tauri::command]
fn plugin_repo_add(repo: String) -> Result<String, String> {
    plugin_sources::add_repo(&repo)
}

#[tauri::command]
fn plugin_repo_remove(repo: String) -> Result<(), String> {
    plugin_sources::remove_repo(&repo)
}

/// Whether pre-releases (test builds) of plug-ins are offered on the Plug-ins page.
#[tauri::command]
fn plugin_test_releases() -> bool {
    settings::load().plugin_test_releases
}

#[tauri::command]
fn plugin_set_test_releases(on: bool) -> Result<(), String> {
    let mut s = settings::load();
    s.plugin_test_releases = on;
    settings::save(&s)
}

/// Switch a companion plug-in (Kiln) on or off.
#[tauri::command]
fn plugin_set_enabled(id: String, enabled: bool) -> Result<(), String> {
    plugins::set_enabled(&id, enabled)
}

/// Pass Lodestone's colours to Beacon.
#[tauri::command]
fn beacon_theme(vars: serde_json::Value) {
    plugins::send_theme(vars)
}

/// Open the folder a ROM is in, in the file manager.
#[tauri::command]
fn rom_reveal(id: String) -> Result<(), String> {
    let file = stores::emulation::rom_file(&id).ok_or("Lodestone doesn't know this ROM's file; try Rescan")?;
    let dir = file.parent().ok_or("No folder")?.to_string_lossy().into_owned();
    launcher::spawn(&crate::os::open_spec(&dir), launcher::log_path("xdg-open")).map(|_| ()).map_err(|e| e.to_string())
}

/// Open the emulator a ROM plays with, without the game (to set up controllers, video and so on).
#[tauri::command]
fn emu_open(id: String, inner: State<'_, Shared>) -> Result<(), String> {
    let launch = inner.games.lock().unwrap().get(&id).and_then(|g| g.launch.clone()).ok_or("No emulator for this game yet")?;
    let spec = if launch.program == "flatpak" {
        // "flatpak run <app id> <its arguments and the ROM>": keep only "run <app id>".
        LaunchSpec { args: launch.args.iter().take(2).cloned().collect(), ..launch.clone() }
    } else {
        LaunchSpec { args: Vec::new(), ..launch.clone() }
    };
    launcher::spawn(&spec, launcher::log_path(&id)).map(|_| ()).map_err(|e| e.to_string())
}

/// Show or hide a system's emulator in the desktop's app menu.
#[tauri::command]
#[cfg(not(windows))]
async fn emu_menu_entry(system: String, index: usize, show: bool) -> Result<(), String> {
    blocking(move || stores::emulation::set_system_menu_entry(&system, index, show)).await?
}

/// Lodestone's version (from Cargo.toml), shown in the sidebar and Settings.
#[tauri::command]
fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[tauri::command]
fn get_settings() -> settings::Settings {
    settings::load()
}

#[tauri::command]
fn save_settings(curseforge_api_key: String, games_dir: Option<String>, steam_api_key: Option<String>, steamgriddb_key: Option<String>, ra_user: Option<String>, ra_key: Option<String>) -> Result<(), String> {
    let mut s = settings::load();
    if let Some(u) = ra_user {
        s.ra_user = u.trim().to_string();
    }
    if let Some(k) = ra_key {
        s.ra_key = k.trim().to_string();
    }
    if let Some(k) = steamgriddb_key {
        s.steamgriddb_key = k.trim().to_string();
    }
    if let Some(k) = steam_api_key {
        s.steam_api_key = k.trim().to_string();
    }
    s.curseforge_api_key = curseforge_api_key.trim().to_string();
    if let Some(d) = games_dir {
        s.games_dir = d.trim().to_string();
    }
    settings::save(&s)
}

/// Background loop: watch launched games, record playtime when they exit.
fn tracker(app: AppHandle, inner: Shared) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(2));
        if inner.sessions.lock().unwrap().is_empty() {
            continue;
        }
        let procs = launcher::process_table();
        let mut alive = Vec::new();
        let mut stopped = Vec::new();
        {
            let mut sessions = inner.sessions.lock().unwrap();
            sessions.retain(|id, s| match s.tick(id, &procs) {
                Tick::Alive(r) => {
                    alive.push(r);
                    true
                }
                Tick::Stopped { played } => {
                    stopped.push((id.clone(), s.title.clone(), played));
                    false
                }
            });
        }
        if !stopped.is_empty() {
            let mut st = inner.store.lock().unwrap();
            for (id, _, played) in &stopped {
                let e = st.entry(id);
                e.playtime += played;
                e.last_played = state::now();
            }
            st.save();
            for (id, title, played) in stopped {
                let _ = app.emit("stopped", serde_json::json!({ "id": id, "title": title, "played": played }));
            }
        }
        let _ = app.emit("running", &alive);
    });
}

fn main() {
    #[cfg(not(windows))]
    host::use_host_dirs();
    state::lock_down_secrets();
    #[cfg(not(windows))]
    gog_setup::main_hook();
    #[cfg(not(windows))]
    stores::bnet_agent::play_hook();
    launcher::ORIGINAL_ENV.get_or_init(|| std::env::vars().collect());
    // WebKitGTK's DMA-BUF renderer shows a blank window with NVIDIA's proprietary driver. Everywhere else
    // it's the fast GPU path, so only turn it off there.
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() && std::path::Path::new("/proc/driver/nvidia").exists() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
    let inner: Shared = Arc::new(Inner { store: Mutex::new(state::Store::load()), ..Default::default() });
    tauri::Builder::default()
        .manage(inner.clone())
        .setup(move |app| {
            tracker(app.handle().clone(), inner.clone());
            // Steam in the tray from the start, so playing or installing never pops its window up.
            if steam_bg::enabled() {
                std::thread::spawn(steam_bg::start_silent);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_games,
            launch_game,
            forget_session,
            set_flag,
            set_playtime,
            add_custom_game,
            remove_custom_game,
            open_client,
            read_log,
            running,
            install_game,
            update_game,
            uninstall_game,
            open_url,
            workshop_items,
            open_workshop,
            mc_instances,
            mc_installed,
            mc_search,
            mc_install,
            mc_update,
            mc_toggle,
            mc_remove,
            mc_open_folder,
            get_settings,
            startup_view,
            refresh_series,
            #[cfg(not(windows))]
            proton_info,
            app_version,
            emu_systems,
            emu_set_choice,
            emu_install,
            ra_sync,
            thumbs_sync,
            store_achievements,
            #[cfg(not(windows))]
            emu_menu_entry,
            emu_folders,
            beacon_status,
            beacon_check_update,
            ui_error,
            data_folder,
            program_updates,
            program_update,
            beacon_install,
            beacon_start,
            beacon_approve,
            beacon_theme,
            plugins_list,
            plugin_stop,
            plugin_set_enabled,
            plugin_release_check,
            plugin_install,
            plugin_repos,
            plugin_repo_add,
            plugin_test_releases,
            plugin_set_test_releases,
            plugin_repo_remove,
            rom_reveal,
            emu_open,
            emu_set_folder,
            emu_create_folders,
            emu_pick_folder,
            ra_game,
            downloads_history,
            downloads_history_clear,
            #[cfg(not(windows))]
            mangohud_info,
            #[cfg(not(windows))]
            setup_mangohud,
            #[cfg(not(windows))]
            set_mangohud,
            #[cfg(not(windows))]
            mangohud_config_get,
            #[cfg(not(windows))]
            mangohud_config_save,
            #[cfg(not(windows))]
            mangohud_preview,
            #[cfg(not(windows))]
            setup_proton,
            #[cfg(not(windows))]
            set_proton,
            winapp_status,
            winapp_setup,
            winapp_open,
            store_login,
            store_login_code,
            store_logout,
            store_import_heroic,
            store_refresh,
            accounts,
            downloads_status,
            shop_show,
            shop_hide,
            shop_bounds,
            shop_nav,
            shop_open_external,
            kiln_status,
            kiln_open,
            set_kiln_play,
            set_steam_background,
            meta_get,
            meta_save,
            meta_reset,
            art_search,
            art_fix,
            art_check,
            game_details,
            game_protondb,
            games_compat,
            game_news,
            open_page,
            friends_list,
            friends_owning,
            setup_overview,
            set_onboarded,
            #[cfg(not(windows))]
            install_runtimes,
            download_cancel,
            download_dismiss,
            save_settings
        ])
        .build(tauri::generate_context!())
        .expect("error while running Lodestone")
        .run(|_, event| {
            if let tauri::RunEvent::Exit = event {
                plugins::stop();
            }
        });
}

#[cfg(all(test, windows))]
mod windows_checks {
    /// Windows: what the Accounts dialog gets (real system, so only on purpose).
    #[test]
    #[ignore]
    fn accounts_on_windows() {
        println!("accounts: {}", super::accounts());
    }
}
