//! Windows-only launchers and games that Lodestone installs into its own Proton prefixes:
//! Battle.net (and the Blizzard games it installs) and Guild Wars 2.

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
        self.exe_path().is_file()
    }
    pub fn run(&self, exe: &Path, args: Vec<String>) -> Result<LaunchSpec, String> {
        proton::spec(Run { prefix: &self.prefix(), exe, args, store: self.proton_store, game_id: self.game_id })
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

    for base in ["Program Files (x86)", "Program Files"] {
        let Ok(rd) = fs::read_dir(c.join(base)) else { continue };
        for e in rd.flatten() {
            let dir = e.path();
            let folder = e.file_name().to_string_lossy().into_owned();
            if folder == "Battle.net" {
                continue;
            }
            let Some(tact) = bnet_catalog::build_info_product(&dir.join(".build.info")) else { continue };
            let prod = by_tact.get(tact.to_ascii_lowercase().as_str()).copied();
            // Unknown to the catalog (shouldn't happen): fall back to the install id.
            let exec = match prod {
                Some(p) => format!("--exec=launch {}", p.code),
                None => format!("--exec=launch_uid {tact}"),
            };
            let Ok(launch) = app.run(&client, vec![exec]) else { continue };
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
                launch: app.run(&client, vec![format!("--exec=launch {}", p.code)]).ok(),
                can_install: install.is_some(),
                install,
                genres: p.genres.clone(),
                ..Default::default()
            });
        }
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
            Kind::Client => out.extend(bnet_games(app)),
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
