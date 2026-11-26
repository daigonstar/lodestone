//! Windows-only launchers and games that Lodestone installs into its own Proton prefixes:
//! Battle.net, the EA app and Ubisoft Connect (and the games they install), and Guild Wars 2.
// Parts of this file are Linux-only (Proton prefixes, Flatpak); on Windows they're simply unused.

use crate::model::*;
#[allow(unused_imports)]
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

#[cfg_attr(windows, allow(dead_code))]
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

/// Windows only: the stores' own launchers, which on Linux come from elsewhere (Steam natively, Epic
/// and GOG through Heroic). Paths are their installers' defaults.
#[cfg(windows)]
pub const WINDOWS_LAUNCHERS: &[WinApp] = &[
    WinApp { id: "steam", name: "Steam", store: "steam", installer_url: "", installer_file: "", installer_args: &[], exe: "Program Files (x86)/Steam/steam.exe", kind: Kind::Client, proton_store: None, game_id: "" },
    WinApp { id: "epic", name: "Epic Games Launcher", store: "epic", installer_url: "", installer_file: "", installer_args: &[], exe: "Program Files (x86)/Epic Games/Launcher/Portal/Binaries/Win64/EpicGamesLauncher.exe", kind: Kind::Client, proton_store: None, game_id: "" },
    WinApp { id: "gog", name: "GOG GALAXY", store: "gog", installer_url: "", installer_file: "", installer_args: &[], exe: "Program Files (x86)/GOG Galaxy/GalaxyClient.exe", kind: Kind::Client, proton_store: None, game_id: "" },
];

/// Every app Lodestone can set up: on Windows also the stores' own launchers.
pub fn apps() -> Vec<&'static WinApp> {
    #[allow(unused_mut)]
    let mut v: Vec<&'static WinApp> = APPS.iter().collect();
    #[cfg(windows)]
    v.extend(WINDOWS_LAUNCHERS.iter());
    v
}

/// Windows: the winget package for an app (installed from its publisher's official download).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn winget_id(id: &str) -> Option<&'static str> {
    match id {
        "battlenet" => Some("Blizzard.BattleNet"),
        "ea" => Some("ElectronicArts.EADesktop"),
        "ubisoft" => Some("Ubisoft.Connect"),
        "steam" => Some("Valve.Steam"),
        "epic" => Some("EpicGames.EpicGamesLauncher"),
        "gog" => Some("GOG.Galaxy"),
        _ => None,
    }
}

pub fn find(id: &str) -> Result<&'static WinApp, String> {
    apps().into_iter().find(|a| a.id == id).ok_or_else(|| format!("unknown app {id}"))
}

#[cfg(not(windows))]
/// Wine prefixes other launchers made, where a store app may already be installed:
/// Lutris games' prefixes, Lutris's usual ~/Games folders, and Steam's Proton prefixes (non-Steam games).
fn foreign_prefixes() -> Vec<(PathBuf, &'static str)> {
    let h = home();
    let mut out: Vec<(PathBuf, &'static str)> = Vec::new();
    let lutris_cfg = [
        h.join(".config/lutris/games"),
        h.join(".local/share/lutris/games"),
        h.join(".var/app/net.lutris.Lutris/config/lutris/games"),
        h.join(".var/app/net.lutris.Lutris/data/lutris/games"),
    ];
    for dir in lutris_cfg {
        for e in fs::read_dir(dir).into_iter().flatten().flatten() {
            let Ok(text) = fs::read_to_string(e.path()) else { continue };
            for line in text.lines() {
                let Some(v) = line.trim().strip_prefix("prefix:") else { continue };
                let v = v.trim().trim_matches(|c| c == '\'' || c == '"');
                let p = match v.strip_prefix("~/") {
                    Some(rest) => h.join(rest),
                    None => PathBuf::from(v),
                };
                if p.is_absolute() {
                    out.push((p, "Lutris"));
                }
            }
        }
    }
    for e in fs::read_dir(h.join("Games")).into_iter().flatten().flatten() {
        out.push((e.path(), "Lutris"));
    }
    for lib in super::steam::library_dirs() {
        for e in fs::read_dir(lib.join("steamapps/compatdata")).into_iter().flatten().flatten() {
            out.push((e.path(), "Steam"));
        }
    }
    out
}

#[cfg(not(windows))]
/// Where another launcher already installed this app, if Lodestone hasn't set it up itself.
/// Checked at most every 30 seconds.
fn foreign(app: &WinApp) -> Option<(PathBuf, &'static str)> {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    type Found = Option<(PathBuf, &'static str)>;
    static CACHE: Mutex<Option<std::collections::HashMap<&'static str, (Instant, Found)>>> = Mutex::new(None);
    if let Some((at, f)) = CACHE.lock().unwrap().get_or_insert_with(Default::default).get(app.id) {
        if at.elapsed() < Duration::from_secs(30) {
            return f.clone();
        }
    }
    let found = foreign_prefixes().into_iter().find(|(p, _)| app_in(app, &proton::drive_c(p)));
    CACHE.lock().unwrap().get_or_insert_with(Default::default).insert(app.id, (Instant::now(), found.clone()));
    found
}

#[cfg(not(windows))]
/// The app's executable is in this drive_c (the EA app after fixing its version link).
fn app_in(app: &WinApp, drive_c: &Path) -> bool {
    if drive_c.join(app.exe).is_file() {
        return true;
    }
    if app.id == "ea" && drive_c.join("Program Files/Electronic Arts/EA Desktop").is_dir() {
        ea_fix_link(drive_c);
        return drive_c.join(app.exe).is_file();
    }
    false
}

impl WinApp {
    /// On Windows, the system drive itself.
    #[cfg(windows)]
    pub fn prefix(&self) -> PathBuf {
        PathBuf::from("C:\\")
    }
    #[cfg(windows)]
    pub fn found_in(&self) -> Option<&'static str> {
        None
    }
    /// Windows: the program's real path ("Program Files" folders where Windows says they are).
    #[cfg(windows)]
    pub fn exe_path(&self) -> PathBuf {
        let env = |k: &str, d: &str| std::env::var_os(k).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(d));
        let rel = self.exe.replace('/', "\\");
        if let Some(r) = rel.strip_prefix("Program Files (x86)\\") {
            env("ProgramFiles(x86)", "C:\\Program Files (x86)").join(r)
        } else if let Some(r) = rel.strip_prefix("Program Files\\") {
            env("ProgramFiles", "C:\\Program Files").join(r)
        } else {
            PathBuf::from("C:\\").join(rel)
        }
    }
    #[cfg(windows)]
    pub fn installed(&self) -> bool {
        self.exe_path().is_file()
    }
    /// Windows: programs run directly.
    #[cfg(windows)]
    pub fn run(&self, exe: &Path, args: Vec<String>) -> Result<LaunchSpec, String> {
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        let mut spec = LaunchSpec::new(exe.to_string_lossy().into_owned(), &a);
        spec.cwd = exe.parent().map(Path::to_path_buf);
        Ok(spec)
    }
    #[cfg(windows)]
    pub fn open(&self) -> Result<LaunchSpec, String> {
        self.run(&self.exe_path(), vec![])
    }
}

#[cfg(not(windows))]
impl WinApp {
    /// Lodestone's own prefix, or one where Lutris or Steam already has this app installed.
    pub fn prefix(&self) -> PathBuf {
        let own = proton::prefix(self.id);
        if proton::drive_c(&own).join(self.exe).is_file() {
            return own;
        }
        foreign(self).map(|f| f.0).unwrap_or(own)
    }
    /// "Lutris" or "Steam" when the app is used from another launcher's prefix.
    pub fn found_in(&self) -> Option<&'static str> {
        let own = proton::prefix(self.id);
        if proton::drive_c(&own).join(self.exe).is_file() {
            return None;
        }
        foreign(self).map(|f| f.1)
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
    /// Installed by another launcher ("Lutris", "Steam") and used from its prefix.
    pub found_in: Option<&'static str>,
}

pub fn status() -> Vec<Status> {
    apps()
        .into_iter()
        .map(|a| Status { id: a.id, name: a.name, store: a.store, installed: a.installed(), prefix: a.prefix().to_string_lossy().into_owned(), found_in: a.found_in() })
        .collect()
}

/// Download the official installer and start it inside the app's prefix. The installer window appears.
pub async fn setup(app: &WinApp) -> Result<LaunchSpec, String> {
    // Windows: through winget where the app has a package; otherwise its official installer, run
    // directly (no Proton).
    #[cfg(windows)]
    {
        if let Some(id) = winget_id(app.id) {
            return Ok(crate::winget::install_spec(id));
        }
        let dl = crate::state::data_dir().join("downloads");
        fs::create_dir_all(&dl).map_err(|e| e.to_string())?;
        let file = dl.join(app.installer_file);
        let bytes = crate::net::client().get(app.installer_url).send().await.map_err(crate::net::err)?.error_for_status().map_err(crate::net::err)?.bytes().await.map_err(crate::net::err)?;
        if !bytes.starts_with(b"MZ") {
            return Err(format!("{} download wasn't a Windows installer", app.name));
        }
        fs::write(&file, &bytes).map_err(|e| e.to_string())?;
        return app.run(&file, app.installer_args.iter().map(|s| s.to_string()).collect());
    }
    #[cfg(not(windows))]
    {
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
}

// ---------------- Battle.net games ----------------

/// Start a game through Battle.net without its window: "--autostarted" is Battle.net's tray start
/// (with AutoStartMinimized on). If Battle.net is already running, the command goes to it.
fn bnet_play(app: &WinApp, client: &Path, exec: String) -> Result<LaunchSpec, String> {
    // Windows: Battle.net takes the command itself; no helper program is needed.
    #[cfg(windows)]
    return app.run(client, vec!["--autostarted".to_string(), exec]);
    #[cfg(not(windows))]
    {
        let spec = app.run(client, vec![])?;
        Ok(super::bnet_agent::play_spec(spec, client, &exec))
    }
}

#[cfg(not(windows))]
/// Battle.net settings for Play from Lodestone: start in the tray, and close once the game is running
/// (GameLaunchWindowBehavior 2; 1 is "minimize", which some desktops can't do), without coming back
/// when the game ends.
pub fn bnet_quiet_launches(drive_c: &Path) {
    bnet_config_set(drive_c, "AutoStartMinimized", "true");
    bnet_config_set(drive_c, "GameLaunchWindowBehavior", "2");
    bnet_config_set(drive_c, "RestoreWindowOnGameEnd", "false");
}

#[cfg(not(windows))]
/// Make Battle.net's tray start ("--autostarted", what Windows uses at login) stay in the tray.
pub fn bnet_start_minimized(drive_c: &Path) {
    bnet_config_set(drive_c, "AutoStartMinimized", "true");
}

#[cfg(not(windows))]
/// Launch code ("OSI") and download product ("osi") of a catalog game, for background installs.
pub fn bnet_product(code: &str) -> Option<(String, String)> {
    if let Some((uid, _, tact, _)) = bnet_catalog::WOW.iter().find(|w| w.0.eq_ignore_ascii_case(code)) {
        return Some((uid.to_string(), tact.to_string()));
    }
    let app = find("battlenet").ok()?;
    let cat = bnet_catalog::catalog(&proton::drive_c(&app.prefix()));
    let p = cat.values().find(|p| p.code.eq_ignore_ascii_case(code))?;
    let lower = p.code.to_ascii_lowercase();
    let tact = p.tact.iter().find(|t| **t == lower).or_else(|| p.tact.iter().min_by_key(|t| (t.len(), (*t).clone())))?;
    Some((p.code.clone(), tact.clone()))
}

#[cfg(not(windows))]
/// Battle.net deletes its saved sign-in on exit when "Keep me logged in" was unticked (Client.AutoLogin
/// false in Battle.net.config), so every start asks for the password again. Turn it back on before
/// starting Battle.net; only that one setting is touched.
fn bnet_keep_signed_in(drive_c: &Path) {
    bnet_config_set(drive_c, "AutoLogin", "true");
}

#[cfg(not(windows))]
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
    let wow = bnet_catalog::wow_versions(&c);
    let mut wow_installed = std::collections::HashSet::new();
    // Retail by its product code (Battle.net's usual Play); the others by install id.
    let wow_exec = |v: &bnet_catalog::WowVersion| match v.uid.as_str() {
        "wow" => "--exec=launch WoW".to_string(),
        "wowf" => "--exec=launch WoWF".to_string(),
        uid => format!("--exec=launch_uid {uid}"),
    };
    let wow_game = |v: &bnet_catalog::WowVersion, installed: bool, launch: Option<LaunchSpec>| Game {
        id: format!("battlenet:{}", v.uid),
        store: "battlenet".into(),
        via: "Battle.net · Proton".into(),
        title: v.name.clone(),
        installed,
        cover: art(&v.cover),
        hero: art(&v.hero),
        launch,
        detect: installed.then(|| Detect::Contains(v.folder.clone())),
        prefix: installed.then(|| path_str(&app.prefix())),
        genres: vec!["RPG".into(), "MMO".into()],
        series: Some("World of Warcraft".into()),
        ..Default::default()
    };

    // Where games are: every install Battle.net's Agent records (any folder or drive, including ones
    // started in an earlier Lodestone session), plus a look through Program Files for older installs.
    let agent = bnet_catalog::installs(&c);
    let mut dirs: Vec<PathBuf> = Vec::new();
    for i in agent.iter().filter(|i| i.installed || i.playable) {
        if let Some(d) = win_path_in(&app.prefix(), &i.path).filter(|d| d.is_dir()) {
            if !dirs.contains(&d) {
                dirs.push(d);
            }
        }
    }
    for base in ["Program Files (x86)", "Program Files"] {
        for e in fs::read_dir(c.join(base)).into_iter().flatten().flatten() {
            let d = e.path();
            if d.join(".build.info").is_file() && !dirs.contains(&d) {
                dirs.push(d);
            }
        }
    }
    // Agent record for a folder: (install id, product code), when it's installed there.
    let agent_for = |dir: &Path| -> Vec<&bnet_catalog::Install> {
        agent.iter().filter(|i| (i.installed || i.playable) && win_path_in(&app.prefix(), &i.path).is_some_and(|p| p == dir)).collect()
    };
    {
        for dir in dirs {
            let folder = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if folder == "Battle.net" {
                continue;
            }
            let recs = agent_for(&dir);
            // World of Warcraft: one folder for every version. A version is installed when the Agent
            // says so, .build.info lists it, or its folder has the game in it (installs the Agent made
            // leave .build.info's product column empty).
            let rows = bnet_catalog::build_info_products(&dir.join(".build.info"));
            let known: Vec<&str> = wow.iter().map(|v| v.folder.as_str()).collect();
            let is_wow = recs.iter().any(|r| r.code.starts_with("wow") || r.uid.starts_with("wow"))
                || rows.iter().any(|r| r.starts_with("wow"))
                || wow.iter().any(|v| wow_exe(&dir.join(&v.folder)).is_some());
            let versions: Vec<(&bnet_catalog::WowVersion, Option<PathBuf>)> = if is_wow {
                wow.iter()
                    .filter_map(|v| {
                        let recorded = recs.iter().any(|r| r.uid.eq_ignore_ascii_case(&v.uid) || r.code.eq_ignore_ascii_case(&v.tact))
                            || rows.iter().any(|r| r.eq_ignore_ascii_case(&v.tact));
                        let vdir = wow_version_dir(&dir, &v.folder, &known, recorded);
                        // When Battle.net has records for this folder, they decide; the files alone only
                        // count for installs it has no record of.
                        let has_records = !recs.is_empty() || !rows.is_empty();
                        (recorded || (!has_records && vdir.is_some())).then_some((v, vdir))
                    })
                    .collect()
            } else {
                vec![]
            };
            if !versions.is_empty() {
                for (v, vdir) in versions {
                    wow_installed.insert(v.uid.clone());
                    let through_bnet = bnet_play(app, &client, wow_exec(v)).ok();
                    // Straight to the game: no waiting for Battle.net to start and hand it over. WoW asks
                    // you to sign in itself the first time (it can remember you).
                    let direct = vdir.as_deref().and_then(wow_exe).and_then(|exe| {
                        let mut s = app.run(&exe, vec![]).ok()?;
                        s.cwd = exe.parent().map(Path::to_path_buf);
                        Some(s)
                    });
                    let mut g = match direct {
                        Some(d) => {
                            let mut g = wow_game(v, true, Some(d));
                            g.via = "Direct · Proton".into();
                            g.alt_launch = through_bnet;
                            g.alt_label = Some("Play through Battle.net".into());
                            g.pick_realm = true;
                            g
                        }
                        None => wow_game(v, true, through_bnet),
                    };
                    // Track it by the folder it really runs from.
                    if let Some(n) = vdir.as_ref().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().into_owned()) {
                        g.detect = Some(Detect::Contains(n));
                    }
                    out.push(g);
                }
                installed_codes.extend(["WoW".to_string(), "WoWC".to_string()]);
                continue;
            }
            if is_wow {
                continue;
            }
            let Some(tact) = recs
                .first()
                .map(|r| r.code.clone())
                .filter(|c| !c.is_empty())
                .or_else(|| bnet_catalog::build_info_product(&dir.join(".build.info")))
                .or_else(|| aggregate.get(&folder.to_ascii_lowercase()).cloned())
            else {
                continue;
            };
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
        let owned_wow = bnet_catalog::wow_owned(&c);
        for v in wow.iter().filter(|v| !wow_installed.contains(&v.uid) && owned_wow.contains(bnet_catalog::WOW[v.order].1)) {
            let install = app.run(&client, vec!["battlenet://WoW".into()]).ok();
            let mut g = wow_game(v, false, bnet_play(app, &client, wow_exec(v)).ok());
            g.can_install = install.is_some();
            g.install = install;
            out.push(g);
        }
        for p in catalog.values() {
            // Listed per version above.
            if installed_codes.contains(&p.code) || p.code.starts_with("WoW") {
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

/// Before starting a WoW version directly with "choose a realm": forget the last realm in
/// WTF/Config.wtf (`SET realmName`), so WoW shows the realm list.
pub fn wow_prepare(version_dir: &Path) -> Result<(), String> {
    let f = version_dir.join("WTF/Config.wtf");
    let Ok(text) = fs::read_to_string(&f) else { return Ok(()) };
    let out = wow_forget_realm(&text);
    if out != text {
        fs::write(&f, out).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn wow_forget_realm(text: &str) -> String {
    let mut s: String = text.lines().filter(|l| !l.trim_start().starts_with("SET realmName ")).collect::<Vec<_>>().join("\n");
    if !s.is_empty() {
        s.push('\n');
    }
    s
}

/// Where a WoW version's files are inside the install: its usual folder (_retail_, _classic_era_...),
/// or, for a version Battle.net says is installed there but whose folder name Lodestone doesn't know
/// (WoW: Forever), the one "_name_" folder with a WoW program that no other version uses. An install
/// with the program right at the top counts too.
fn wow_version_dir(dir: &Path, folder: &str, known: &[&str], recorded: bool) -> Option<PathBuf> {
    let usual = dir.join(folder);
    if wow_exe(&usual).is_some() {
        return Some(usual);
    }
    if !recorded {
        return None;
    }
    let mut others: Vec<PathBuf> = fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            p.is_dir() && n.starts_with('_') && n.ends_with('_') && !known.contains(&n.as_str()) && !n.contains("ptr") && !n.contains("beta")
        })
        .filter(|p| wow_exe(p).is_some())
        .collect();
    others.sort();
    others.into_iter().next().or_else(|| wow_exe(dir).map(|_| dir.to_path_buf()))
}

/// The game's own program in a WoW version folder (_retail_/Wow.exe, _classic_*/WowClassic.exe).
fn wow_exe(dir: &Path) -> Option<PathBuf> {
    for n in ["Wow.exe", "WowClassic.exe"] {
        if dir.join(n).is_file() {
            return Some(dir.join(n));
        }
    }
    // Versions named differently: the one Wow*.exe that isn't a crash reporter or a test build.
    fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| {
        let n = p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        n.starts_with("wow") && n.ends_with(".exe") && !n.contains("error") && !n.contains("crash") && !n.ends_with("t.exe") && !n.ends_with("b.exe")
    })
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
/// A Windows path in a prefix ("D:/Games/World of Warcraft") as a Linux path: C: is drive_c, other
/// drives follow the prefix's dosdevices links (Z: is usually /).
fn win_path_in(prefix: &Path, p: &str) -> Option<PathBuf> {
    let p = p.replace('\\', "/");
    let (drive, rest) = p.split_once(':')?;
    if drive.len() != 1 {
        return None;
    }
    let rest = rest.trim_start_matches('/');
    let d = drive.to_ascii_lowercase();
    if d == "c" {
        return Some(proton::drive_c(prefix).join(rest));
    }
    let pfx = if prefix.join("pfx/dosdevices").is_dir() { prefix.join("pfx") } else { prefix.to_path_buf() };
    let link = pfx.join("dosdevices").join(format!("{d}:"));
    let root = fs::canonicalize(&link).ok()?;
    Some(root.join(rest))
}

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
#[cfg(not(windows))]
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
    let _ = crate::tools::link_dir(&target, &link);
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

    #[cfg(not(windows))]
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

    #[cfg(unix)] // Linux behaviour (sh, Linux paths, symlinks)

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

#[cfg(test)]
mod foreign_tests {
    /// A Battle.net installed by Lutris (plain prefix) or Steam (compatdata with pfx/) is found.
    #[cfg(unix)] // Linux behaviour (sh, Linux paths, symlinks)
    #[test]
    fn finds_store_apps_in_other_launchers_prefixes() {
        let root = std::env::temp_dir().join(format!("lodestone-foreign-{}", std::process::id()));
        let app = super::find("battlenet").unwrap();
        let lutris = root.join("battlenet");
        let steam = root.join("compatdata/123");
        for (p, dc) in [(&lutris, lutris.join("drive_c")), (&steam, steam.join("pfx/drive_c"))] {
            let exe = dc.join(app.exe);
            std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
            std::fs::write(&exe, b"MZ").unwrap();
            assert!(super::app_in(app, &crate::proton::drive_c(p)), "{}", p.display());
        }
        assert!(!super::app_in(super::find("ubisoft").unwrap(), &crate::proton::drive_c(&lutris)));
        let _ = std::fs::remove_dir_all(root);
    }
}

#[cfg(test)]
mod wow_dir_tests {
    #[test]
    fn finds_forevers_folder_by_elimination() {
        let root = std::env::temp_dir().join(format!("lodestone-wowdir-{}", std::process::id()));
        for (sub, exe) in [("_retail_", "Wow.exe"), ("_forever_", "WowClassic.exe"), ("_ptr_", "WowT.exe")] {
            std::fs::create_dir_all(root.join(sub)).unwrap();
            std::fs::write(root.join(sub).join(exe), b"MZ").unwrap();
        }
        let known = ["_retail_", "_wowf_", "_classic_", "_classic_era_", "_anniversary_"];
        assert!(super::wow_version_dir(&root, "_retail_", &known, true).unwrap().ends_with("_retail_"));
        assert!(super::wow_version_dir(&root, "_wowf_", &known, true).unwrap().ends_with("_forever_"));
        // Not recorded as installed: no guessing.
        assert!(super::wow_version_dir(&root, "_wowf_", &known, false).is_none());
        let _ = std::fs::remove_dir_all(root);
    }
}

#[cfg(test)]
mod wow_config_tests {
    #[test]
    fn forgets_the_last_realm() {
        let cfg = "SET portal \"EU\"\nSET realmName \"Draenor\"\nSET gxApi \"D3D12\"\n";
        assert_eq!(super::wow_forget_realm(cfg), "SET portal \"EU\"\nSET gxApi \"D3D12\"\n");
    }
}

#[cfg(test)]
mod wow_tests {
    #[test]
    fn finds_each_wow_versions_program() {
        let root = std::env::temp_dir().join(format!("lodestone-wow-exe-{}", std::process::id()));
        let mk = |sub: &str, files: &[&str]| {
            let d = root.join(sub);
            std::fs::create_dir_all(&d).unwrap();
            for f in files {
                std::fs::write(d.join(f), b"MZ").unwrap();
            }
            d
        };
        let retail = mk("_retail_", &["Wow.exe", "WowErrorReporter.exe"]);
        let classic = mk("_classic_era_", &["WowClassic.exe"]);
        let other = mk("_wowf_", &["BlizzardError.exe", "WowCrashHandler.exe", "WowForever.exe"]);
        assert!(super::wow_exe(&retail).unwrap().ends_with("Wow.exe"));
        assert!(super::wow_exe(&classic).unwrap().ends_with("WowClassic.exe"));
        assert!(super::wow_exe(&other).unwrap().ends_with("WowForever.exe"));
        assert!(super::wow_exe(&mk("_empty_", &[])).is_none());
        let _ = std::fs::remove_dir_all(root);
    }
}
