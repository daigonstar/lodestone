//! Windows-only launchers and games that Lodestone installs into its own Proton prefixes:
//! Battle.net, the EA app and Ubisoft Connect (and the games they install), and Guild Wars 2.

use crate::model::*;
use crate::proton::{self, Run};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    /// A store client that installs other games (games are detected inside its prefix).
    Client,
    /// A single game with its own updater.
    Game,
}

pub struct WinApp {
    pub id: &'static str,
    pub name: &'static str,
    pub store: &'static str,
    pub installer_url: &'static str,
    pub installer_file: &'static str,
    pub installer_args: &'static [&'static str],
    /// Executable path relative to drive_c.
    pub exe: &'static str,
    pub kind: Kind,
    pub proton_store: Option<&'static str>,
    pub game_id: &'static str,
}

pub const APPS: &[WinApp] = &[
    WinApp {
        id: "battlenet",
        name: "Battle.net",
        store: "battlenet",
        installer_url: "https://downloader.battle.net/download/getInstaller?os=win&installer=Battle.net-Setup.exe",
        installer_file: "Battle.net-Setup.exe",
        installer_args: &["--lang=enUS", "--installpath=C:\\Program Files (x86)\\Battle.net"],
        exe: "Program Files (x86)/Battle.net/Battle.net.exe",
        kind: Kind::Client,
        proton_store: Some("battlenet"),
        game_id: "umu-default",
    },
    WinApp {
        id: "ea",
        name: "EA app",
        store: "ea",
        installer_url: "https://origin-a.akamaihd.net/EA-Desktop-Client-Download/installer-releases/EAappInstaller.exe",
        installer_file: "EAappInstaller.exe",
        installer_args: &[],
        exe: "Program Files/Electronic Arts/EA Desktop/EA Desktop/EADesktop.exe",
        kind: Kind::Client,
        proton_store: Some("ea"),
        game_id: "umu-default",
    },
    WinApp {
        id: "ubisoft",
        name: "Ubisoft Connect",
        store: "ubisoft",
        installer_url: "https://static3.cdn.ubi.com/orbit/launcher_installer/UbisoftConnectInstaller.exe",
        installer_file: "UbisoftConnectInstaller.exe",
        installer_args: &["/S"],
        exe: "Program Files (x86)/Ubisoft/Ubisoft Game Launcher/UbisoftConnect.exe",
        kind: Kind::Client,
        proton_store: Some("ubisoft"),
        game_id: "umu-default",
    },
    WinApp {
        id: "gw2",
        name: "Guild Wars 2",
        store: "arenanet",
        installer_url: "https://s3.amazonaws.com/gw2cdn/client/branches/Gw2Setup-64.exe",
        installer_file: "Gw2Setup-64.exe",
        installer_args: &[],
        exe: "Program Files/Guild Wars 2/Gw2-64.exe",
        kind: Kind::Game,
        proton_store: None,
        game_id: "umu-default",
    },
];

pub fn find(id: &str) -> Result<&'static WinApp, String> {
    APPS.iter().find(|a| a.id == id).ok_or_else(|| format!("unknown app {id}"))
}

impl WinApp {
    pub fn prefix(&self) -> PathBuf {
        proton::prefix(self.id)
    }
    pub fn exe_path(&self) -> PathBuf {
        proton::drive_c(&self.prefix()).join(self.exe)
    }
    pub fn installed(&self) -> bool {
        if self.exe_path().is_file() {
            return true;
        }
        if self.id == "ea" {
            ea_fix_link(&proton::drive_c(&self.prefix()));
        }
        self.exe_path().is_file()
    }
    pub fn run(&self, exe: &Path, args: Vec<String>) -> Result<LaunchSpec, String> {
        if self.id == "battlenet" {
            bnet_keep_signed_in(&proton::drive_c(&self.prefix()));
        }
        let mut spec = proton::spec(Run { prefix: &self.prefix(), exe, args, store: self.proton_store, game_id: self.game_id })?;
        // umu's default waits for everything already running in the prefix to exit first. The store app
        // stays running (often in its tray), so Play/Install/Open would wait forever; join its session.
        spec.env.insert("PROTON_VERB".into(), "run".into());
        Ok(spec)
    }
    /// Open the client/game normally (sign in, install games, settings).
    pub fn open(&self) -> Result<LaunchSpec, String> {
        self.run(&self.exe_path(), vec![])
    }
}

#[derive(Serialize)]
pub struct Status {
    pub id: &'static str,
    pub name: &'static str,
    pub store: &'static str,
    pub installed: bool,
    pub prefix: String,
}

pub fn status() -> Vec<Status> {
    APPS.iter()
        .map(|a| Status { id: a.id, name: a.name, store: a.store, installed: a.installed(), prefix: a.prefix().to_string_lossy().into_owned() })
        .collect()
}

/// Download the official installer and start it inside the app's prefix. The installer window appears.
pub async fn setup(app: &WinApp) -> Result<LaunchSpec, String> {
    if crate::tools::umu_path().is_none() {
        crate::tools::install_umu().await?;
    }
    let dl = crate::state::data_dir().join("downloads");
    fs::create_dir_all(&dl).map_err(|e| e.to_string())?;
    let file = dl.join(app.installer_file);
    let r = crate::net::client().get(app.installer_url).send().await.map_err(crate::net::err)?.error_for_status().map_err(crate::net::err)?;
    let bytes = r.bytes().await.map_err(crate::net::err)?;
    // Windows executables start with "MZ"; refuse anything else (e.g. an HTML error page).
    if !bytes.starts_with(b"MZ") {
        return Err(format!("{} download wasn't a Windows installer", app.name));
    }
    fs::write(&file, &bytes).map_err(|e| e.to_string())?;
    app.run(&file, app.installer_args.iter().map(|s| s.to_string()).collect())
}

// ---------------- Battle.net games ----------------

/// Start a game through Battle.net without its window: "--autostarted" is Battle.net's tray start
/// (with AutoStartMinimized on). If Battle.net is already running, the command goes to it.
fn bnet_play(app: &WinApp, client: &Path, exec: String) -> Result<LaunchSpec, String> {
    let spec = app.run(client, vec![])?;
    Ok(super::bnet_agent::play_spec(spec, client, &exec))
}

/// Battle.net settings for Play from Lodestone: start in the tray, and close once the game is running
/// (GameLaunchWindowBehavior 2; 1 is "minimize", which some desktops can't do), without coming back
/// when the game ends.
pub fn bnet_quiet_launches(drive_c: &Path) {
    bnet_config_set(drive_c, "AutoStartMinimized", "true");
    bnet_config_set(drive_c, "GameLaunchWindowBehavior", "2");
    bnet_config_set(drive_c, "RestoreWindowOnGameEnd", "false");
}

/// Make Battle.net's tray start ("--autostarted", what Windows uses at login) stay in the tray.
pub fn bnet_start_minimized(drive_c: &Path) {
    bnet_config_set(drive_c, "AutoStartMinimized", "true");
}

/// Launch code ("OSI") and download product ("osi") of a catalog game, for background installs.
pub fn bnet_product(code: &str) -> Option<(String, String)> {
    let app = find("battlenet").ok()?;
    let cat = bnet_catalog::catalog(&proton::drive_c(&app.prefix()));
    let p = cat.values().find(|p| p.code.eq_ignore_ascii_case(code))?;
    let lower = p.code.to_ascii_lowercase();
    let tact = p.tact.iter().find(|t| **t == lower).or_else(|| p.tact.iter().min_by_key(|t| (t.len(), (*t).clone())))?;
    Some((p.code.clone(), tact.clone()))
}

/// Battle.net deletes its saved sign-in on exit when "Keep me logged in" was unticked (Client.AutoLogin
/// false in Battle.net.config), so every start asks for the password again. Turn it back on before
/// starting Battle.net; only that one setting is touched.
fn bnet_keep_signed_in(drive_c: &Path) {
    bnet_config_set(drive_c, "AutoLogin", "true");
}

/// Set a Client setting in Battle.net.config, leaving everything else as it is.
fn bnet_config_set(drive_c: &Path, key: &str, value: &str) {
    let Ok(users) = fs::read_dir(drive_c.join("users")) else { return };
    for u in users.flatten() {
        let f = u.path().join("AppData/Roaming/Battle.net/Battle.net.config");
        let Ok(text) = fs::read_to_string(&f) else { continue };
        let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
        let Some(client) = v.get_mut("Client").and_then(|c| c.as_object_mut()) else { continue };
        if client.get(key).and_then(|a| a.as_str()) == Some(value) {
            continue;
        }
        client.insert(key.into(), serde_json::Value::String(value.into()));
        if let Ok(out) = serde_json::to_string_pretty(&v) {
            let tmp = f.with_extension("config.tmp");
            if fs::write(&tmp, out).is_ok() {
                let _ = fs::rename(&tmp, &f);
            }
        }
    }
}

use super::bnet_catalog::{self, FREE_TO_PLAY};

/// Installed Blizzard games (found by their `.build.info`) plus everything the signed-in account owns,
/// named and illustrated from Battle.net's own cached catalog.
fn bnet_games(app: &WinApp) -> Vec<Game> {
    let c = proton::drive_c(&app.prefix());
    let client = app.exe_path();
    let catalog = bnet_catalog::catalog(&c);
    let by_tact: std::collections::HashMap<&str, &bnet_catalog::Product> =
        catalog.values().flat_map(|p| p.tact.iter().map(move |t| (t.as_str(), p))).collect();
    let art = |p: &Option<PathBuf>| p.as_ref().map(|x| path_str(x));
    let mut out = Vec::new();
    let mut installed_codes = std::collections::HashSet::new();
    let aggregate = bnet_catalog::aggregate_products(&c);

    for base in ["Program Files (x86)", "Program Files"] {
        let Ok(rd) = fs::read_dir(c.join(base)) else { continue };
        for e in rd.flatten() {
            let dir = e.path();
            let folder = e.file_name().to_string_lossy().into_owned();
            if folder == "Battle.net" {
                continue;
            }
            if !dir.join(".build.info").is_file() {
                continue;
            }
            let Some(tact) = bnet_catalog::build_info_product(&dir.join(".build.info")).or_else(|| aggregate.get(&folder.to_ascii_lowercase()).cloned()) else { continue };
            let prod = by_tact.get(tact.to_ascii_lowercase().as_str()).copied();
            // Unknown to the catalog (shouldn't happen): fall back to the install id.
            let exec = match prod {
                Some(p) => format!("--exec=launch {}", p.code),
                None => format!("--exec=launch_uid {tact}"),
            };
            let Ok(launch) = bnet_play(app, &client, exec) else { continue };
            let code = prod.map(|p| p.code.clone()).unwrap_or_else(|| tact.clone());
            installed_codes.insert(code.clone());
            out.push(Game {
                id: format!("battlenet:{}", code.to_ascii_lowercase()),
                store: "battlenet".into(),
                via: "Battle.net · Proton".into(),
                title: prod.map(|p| p.name.clone()).unwrap_or_else(|| folder.clone()),
                installed: true,
                cover: prod.and_then(|p| art(&p.cover)),
                hero: prod.and_then(|p| art(&p.hero)),
                launch: Some(launch),
                detect: Some(Detect::Contains(folder)),
                prefix: Some(path_str(&app.prefix())),
                genres: prod.map(|p| p.genres.clone()).unwrap_or_default(),
                ..Default::default()
            });
        }
    }

    // Owned but not installed. Only when someone is signed in (otherwise we can't know what's owned).
    if let Some(licenses) = bnet_catalog::account_licenses(&c) {
        for p in catalog.values() {
            if installed_codes.contains(&p.code) {
                continue;
            }
            let owned = !p.licenses.is_disjoint(&licenses);
            if !owned && !FREE_TO_PLAY.contains(&p.code.as_str()) {
                continue;
            }
            // Install happens in Battle.net: open it on the game's page.
            let install = app.run(&client, vec![format!("battlenet://{}", p.code)]).ok();
            out.push(Game {
                id: format!("battlenet:{}", p.code.to_ascii_lowercase()),
                store: "battlenet".into(),
                via: if owned { "Battle.net · Proton".into() } else { "Battle.net · Free to play".into() },
                title: p.name.clone(),
                installed: false,
                cover: art(&p.cover),
                hero: art(&p.hero),
                launch: bnet_play(app, &client, format!("--exec=launch {}", p.code)).ok(),
                can_install: install.is_some(),
                install,
                genres: p.genres.clone(),
                ..Default::default()
            });
        }
    }
    out
}

// ---------------- registry (Wine keeps it as text in the prefix) ----------------

/// Values of every key under `prefix` in a Wine .reg file: (key path after the prefix, name -> value).
fn reg_keys(reg: &str, prefix: &str) -> Vec<(String, std::collections::HashMap<String, String>)> {
    let want = prefix.to_ascii_lowercase().replace('\\', "\\\\");
    let mut out = Vec::new();
    let mut cur: Option<(String, std::collections::HashMap<String, String>)> = None;
    for line in reg.lines() {
        if let Some(rest) = line.strip_prefix('[') {
            out.extend(cur.take());
            let key = rest.split(']').next().unwrap_or("");
            if key.to_ascii_lowercase().starts_with(&want) {
                cur = Some((key[want.len()..].trim_start_matches('\\').replace("\\\\", "\\"), Default::default()));
            }
        } else if let Some((_, vals)) = cur.as_mut() {
            if let Some((k, v)) = line.split_once("\"=\"") {
                let k = k.trim_start_matches('"').to_string();
                let v = v.trim_end_matches('"').replace("\\\\", "\\").replace("\\\"", "\"");
                vals.insert(k, v);
            }
        }
    }
    out.extend(cur);
    out
}

/// "C:/Program Files/X/" or "C:\\Program Files\\X" inside a prefix -> a Linux path.
fn win_to_unix(drive_c: &Path, p: &str) -> Option<PathBuf> {
    let rest = p.strip_prefix("C:").or_else(|| p.strip_prefix("c:"))?;
    let rel = rest.replace('\\', "/");
    Some(drive_c.join(rel.trim_matches('/')))
}

// ---------------- Ubisoft Connect games ----------------

/// Installed games, from the launcher's registry entries (Installs\<id>\InstallDir). Launch goes
/// through Ubisoft Connect (`uplay://launch/<id>/0`), which handles sign-in and updates.
fn ubisoft_games(app: &WinApp) -> Vec<Game> {
    let c = proton::drive_c(&app.prefix());
    let reg = fs::read_to_string(app.prefix().join("system.reg")).unwrap_or_default();
    let client = app.exe_path();
    let owned = super::ubisoft_api::owned();
    let mut out = Vec::new();
    for (id, vals) in reg_keys(&reg, "Software\\Wow6432Node\\Ubisoft\\Launcher\\Installs\\") {
        if id.is_empty() || id.contains('\\') {
            continue;
        }
        let Some(dir) = vals.get("InstallDir").and_then(|d| win_to_unix(&c, d)).filter(|d| d.is_dir()) else { continue };
        let folder = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let Ok(launch) = app.run(&client, vec![format!("uplay://launch/{id}/0")]) else { continue };
        // The owned-games list has the proper name and art; match it by name.
        let o = owned.iter().find(|o| same_title(&o.name, &folder));
        out.push(Game {
            id: format!("ubisoft:{id}"),
            store: "ubisoft".into(),
            via: "Ubisoft Connect · Proton".into(),
            title: o.map(|o| o.name.clone()).unwrap_or_else(|| folder.clone()),
            cover: o.and_then(|o| o.cover.clone()),
            hero: o.and_then(|o| o.hero.clone()),
            installed: true,
            launch: Some(launch),
            detect: Some(Detect::Contains(folder)),
            prefix: Some(path_str(&app.prefix())),
            ..Default::default()
        });
    }
    // Owned but not installed: Install opens Ubisoft Connect on the game.
    for o in owned {
        if out.iter().any(|g| same_title(&g.title, &o.name)) {
            continue;
        }
        // Ubisoft's web API doesn't give the id Ubisoft Connect's install links use, so open the app
        // and let the player pick the game in its library.
        let install = app.run(&client, vec![]).ok();
        out.push(Game {
            id: format!("ubisoft:{}", o.space_id),
            store: "ubisoft".into(),
            via: "Ubisoft Connect · Proton".into(),
            title: o.name,
            cover: o.cover,
            hero: o.hero,
            installed: false,
            can_install: install.is_some(),
            install,
            ..Default::default()
        });
    }
    out
}

/// Titles equal ignoring case, punctuation and trademark signs ("Far Cry® 5" = "Far Cry 5").
fn same_title(a: &str, b: &str) -> bool {
    let k = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect::<String>();
    let (a, b) = (k(a), k(b));
    !a.is_empty() && a == b
}

// ---------------- EA app games ----------------

/// The EA app installs into `EA Desktop\\<version>\\EA Desktop` and points `EA Desktop\\EA Desktop` at it with
/// a Windows junction, which Wine leaves as a broken folder. Everything (Lodestone, the EA app's own links,
/// games starting it for sign-in) uses the junction path, so make it a real link to the newest version.
fn ea_fix_link(drive_c: &Path) {
    let base = drive_c.join("Program Files/Electronic Arts/EA Desktop");
    let Ok(rd) = fs::read_dir(&base) else { return };
    let ver = |n: &str| n.split('.').map(|p| p.parse::<u32>().unwrap_or(0)).collect::<Vec<_>>();
    let newest = rd
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            (n.chars().next().is_some_and(|c| c.is_ascii_digit()) && e.path().join("EA Desktop/EADesktop.exe").is_file()).then_some(n)
        })
        .max_by_key(|n| ver(n));
    let Some(newest) = newest else { return };
    let link = base.join("EA Desktop");
    let target = PathBuf::from(&newest).join("EA Desktop");
    if fs::read_link(&link).ok().as_deref() == Some(target.as_path()) {
        return;
    }
    // Only replace an old link or an empty leftover folder, never real files.
    match fs::symlink_metadata(&link) {
        Ok(m) if m.file_type().is_symlink() => {
            let _ = fs::remove_file(&link);
        }
        Ok(m) if m.is_dir() => {
            if fs::remove_dir(&link).is_err() {
                return;
            }
        }
        Ok(_) => return,
        Err(_) => {}
    }
    let _ = std::os::unix::fs::symlink(&target, &link);
}

#[derive(Debug, PartialEq)]
struct EaGame {
    id: String,
    title: String,
    /// Game executable, relative to the game folder.
    exe: String,
}

/// `__Installer/installerdata.xml`, which every EA app (and Origin) install has.
fn ea_installerdata(xml: &str) -> Option<EaGame> {
    let tag = |s: &str, open: &str, close: &str| -> Vec<String> {
        let mut v = Vec::new();
        let mut rest = s;
        while let Some(i) = rest.find(open) {
            let after = &rest[i + open.len()..];
            // "<contentID" must not match "<contentIDs>".
            if !after.starts_with('>') && !after.starts_with(' ') {
                rest = after;
                continue;
            }
            let Some(gt) = after.find('>') else { break };
            let body = &after[gt + 1..];
            let Some(end) = body.find(close) else { break };
            v.push(body[..end].trim().to_string());
            rest = &body[end..];
        }
        v
    };
    let id = tag(xml, "<contentID", "</contentID>").into_iter().next()?;
    // English title when there's a choice.
    let title = xml
        .split("<gameTitle")
        .skip(1)
        .find(|t| t.starts_with(" locale=\"en_US\"") || t.starts_with(" locale=\"en_GB\""))
        .or_else(|| xml.split("<gameTitle").nth(1))
        .and_then(|t| t.split_once('>').map(|x| x.1))
        .and_then(|t| t.split("</gameTitle>").next())
        .map(|t| t.trim().to_string())?;
    // "[HKEY_LOCAL_MACHINE\\SOFTWARE\\...\\Install Dir]bin\\game.exe": the part after ] is relative to the game folder.
    let exe = tag(xml, "<filePath", "</filePath>")
        .into_iter()
        .map(|f| f.rsplit(']').next().unwrap_or(&f).replace('\\', "/"))
        .find(|f| f.to_ascii_lowercase().ends_with(".exe"))?;
    Some(EaGame { id, title, exe })
}

/// Installed games, found by their installerdata.xml in the usual library folders.
/// `origin2://` links go to EALauncher.exe, which hands them to the running EA app.
fn ea_link(app: &WinApp, action: &str, content_id: &str) -> Result<LaunchSpec, String> {
    let launcher = app.exe_path().with_file_name("EALauncher.exe");
    let extra = if action == "install" { "&autoDownload=1" } else { "" };
    app.run(&launcher, vec![format!("origin2://game/launch?offerIds={content_id}{extra}")])
}

fn ea_games(app: &WinApp) -> Vec<Game> {
    let c = proton::drive_c(&app.prefix());
    let owned = super::ea_api::owned();
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut dirs = Vec::new();
    for base in ["Program Files/EA Games", "Program Files (x86)/Origin Games", "Program Files", "Program Files (x86)"] {
        if let Ok(rd) = fs::read_dir(c.join(base)) {
            dirs.extend(rd.flatten().map(|e| e.path()));
        }
    }
    for dir in dirs {
        let Ok(xml) = fs::read_to_string(dir.join("__Installer/installerdata.xml")) else { continue };
        let Some(g) = ea_installerdata(&xml) else { continue };
        let exe = dir.join(&g.exe);
        if !exe.is_file() || !seen.insert(g.id.clone()) {
            continue;
        }
        // Through the EA app, which signs in, checks for updates and starts the game.
        let Ok(launch) = ea_link(app, "launch", &g.id) else { continue };
        let exe_name = exe.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let o = owned.iter().find(|o| o.content_id == g.id || same_title(&o.name, &g.title));
        out.push(Game {
            id: format!("ea:{}", g.id.to_ascii_lowercase()),
            store: "ea".into(),
            via: "EA app · Proton".into(),
            title: o.map(|o| o.name.clone()).unwrap_or(g.title),
            cover: o.and_then(|o| o.cover.clone()),
            hero: o.and_then(|o| o.hero.clone()),
            installed: true,
            launch: Some(launch),
            detect: Some(Detect::Contains(exe_name)),
            prefix: Some(path_str(&app.prefix())),
            ..Default::default()
        });
    }
    // Owned but not installed: Install opens the EA app and starts the download.
    for o in owned {
        if seen.contains(&o.content_id) || out.iter().any(|g| same_title(&g.title, &o.name)) {
            continue;
        }
        let install = ea_link(app, "install", &o.content_id).ok();
        out.push(Game {
            id: format!("ea:{}", o.content_id.to_ascii_lowercase()),
            store: "ea".into(),
            via: "EA app · Proton".into(),
            title: o.name,
            cover: o.cover,
            hero: o.hero,
            installed: false,
            can_install: install.is_some(),
            install,
            ..Default::default()
        });
    }
    out
}

pub fn scan() -> Vec<Game> {
    let mut out = Vec::new();
    for app in APPS {
        if !app.installed() {
            continue;
        }
        match app.kind {
            Kind::Client => out.extend(match app.id {
                "battlenet" => bnet_games(app),
                "ea" => ea_games(app),
                "ubisoft" => ubisoft_games(app),
                _ => vec![],
            }),
            Kind::Game => {
                let Ok(launch) = app.open() else { continue };
                let exe_name = Path::new(app.exe).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                out.push(Game {
                    id: format!("winapp:{}", app.id),
                    store: app.store.into(),
                    via: "Proton".into(),
                    title: app.name.into(),
                    installed: true,
                    launch: Some(launch),
                    detect: Some(Detect::Contains(exe_name)),
                    prefix: Some(path_str(&app.prefix())),
                    genres: if app.id == "gw2" { vec!["MMO".into(), "RPG".into()] } else { vec![] },
                    series: (app.id == "gw2").then(|| "Guild Wars".into()),
                    ..Default::default()
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::bnet_catalog::build_info_product;
    use std::fs;
    use std::path::Path;

    #[test]
    #[ignore]
    fn live_clients_installed() {
        for a in super::APPS {
            println!("{} installed={} games={:?}", a.id, a.installed(), if a.installed() && a.kind == super::Kind::Client && a.id != "battlenet" { super::scan().into_iter().filter(|g| g.store == a.store).map(|g| g.title).collect::<Vec<_>>() } else { vec![] });
        }
    }

    #[test]
    fn keeps_battlenet_signed_in() {
        let c = std::env::temp_dir().join(format!("lodestone-bn-{}", std::process::id()));
        let d = c.join("users/steamuser/AppData/Roaming/Battle.net");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("Battle.net.config"), r#"{"Client":{"AutoLogin":"false","SavedAccountNames":"x"},"Other":{"A":"1"}}"#).unwrap();
        super::bnet_keep_signed_in(&c);
        let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(d.join("Battle.net.config")).unwrap()).unwrap();
        assert_eq!(v["Client"]["AutoLogin"], "true");
        assert_eq!((v["Client"]["SavedAccountNames"].as_str(), v["Other"]["A"].as_str()), (Some("x"), Some("1")));
        let _ = fs::remove_dir_all(c);
    }

    #[test]
    #[ignore]
    fn live_bnet_scan() {
        let app = super::find("battlenet").unwrap();
        let c = crate::proton::drive_c(&app.prefix());
        let d = c.join("Program Files/Diablo II Resurrected/.build.info");
        println!("build.info product: {:?}", super::bnet_catalog::build_info_product(&d));
        let cat = super::bnet_catalog::catalog(&c);
        for p in cat.values().filter(|p| p.code.eq_ignore_ascii_case("osi")) { println!("catalog OSI tact {:?}", p.tact); }
        for g in super::bnet_games(app) { println!("{} installed={} {}", g.id, g.installed, g.title); }
    }

    #[test]
    fn links_ea_app_version_folder() {
        let c = std::env::temp_dir().join(format!("lodestone-ea-{}", std::process::id()));
        let base = c.join("Program Files/Electronic Arts/EA Desktop");
        for v in ["13.1.0.1", "13.832.0.6345"] {
            fs::create_dir_all(base.join(v).join("EA Desktop")).unwrap();
            fs::write(base.join(v).join("EA Desktop/EADesktop.exe"), "MZ").unwrap();
        }
        fs::create_dir_all(base.join("EA Desktop")).unwrap();
        super::ea_fix_link(&c);
        assert!(base.join("EA Desktop/EADesktop.exe").is_file());
        assert_eq!(fs::read_link(base.join("EA Desktop")).unwrap(), Path::new("13.832.0.6345/EA Desktop"));
        let _ = fs::remove_dir_all(c);
    }

    #[test]
    fn reads_wine_registry() {
        let reg = "WINE REGISTRY Version 2\n\n[Software\\\\Wow6432Node\\\\Ubisoft\\\\Launcher\\\\Installs\\\\635] 1795\n#time=1\n\"InstallDir\"=\"C:/Program Files (x86)/Ubisoft/Ubisoft Game Launcher/games/Far Cry 5/\"\n\n[Software\\\\Other] 1\n\"X\"=\"y\"\n";
        let k = super::reg_keys(reg, "Software\\Wow6432Node\\Ubisoft\\Launcher\\Installs\\");
        assert_eq!(k.len(), 1);
        assert_eq!(k[0].0, "635");
        let p = super::win_to_unix(Path::new("/pfx/drive_c"), &k[0].1["InstallDir"]).unwrap();
        assert_eq!(p, Path::new("/pfx/drive_c/Program Files (x86)/Ubisoft/Ubisoft Game Launcher/games/Far Cry 5"));
    }

    #[test]
    fn reads_ea_installerdata() {
        let xml = r#"<DiPManifest version="4.0"><contentIDs><contentID>1035052</contentID><contentID>1035053</contentID></contentIDs>
            <gameTitles><gameTitle locale="de_DE">Spiel</gameTitle><gameTitle locale="en_US">Mass Effect Legendary Edition</gameTitle></gameTitles>
            <runtime><launcher><filePath>[HKEY_LOCAL_MACHINE\SOFTWARE\BioWare\Mass Effect Legendary Edition\Install Dir]Game\Launcher\MassEffectLauncher.exe</filePath></launcher></runtime></DiPManifest>"#;
        let g = super::ea_installerdata(xml).unwrap();
        assert_eq!(g, super::EaGame { id: "1035052".into(), title: "Mass Effect Legendary Edition".into(), exe: "Game/Launcher/MassEffectLauncher.exe".into() });
    }

    #[test]
    fn reads_build_info_product() {
        let dir = std::env::temp_dir().join(format!("lodestone-bi-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let f = dir.join(".build.info");
        fs::write(&f, "Branch!STRING:0|Active!DEC:1|Build Key!HEX:16|Product!STRING:0|Version!STRING:0\nus|1|abc|pro|2.13.0\n").unwrap();
        assert_eq!(build_info_product(&f).as_deref(), Some("pro"));
        let _ = fs::remove_dir_all(dir);
    }
}
