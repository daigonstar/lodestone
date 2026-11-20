//! Epic, GOG, Amazon and Heroic "sideloaded" apps, all read from Heroic's config and run through it headless.

use crate::model::*;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

const FLATPAK_ID: &str = "com.heroicgameslauncher.hgl";

fn read_json(p: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(p).ok()?).ok()
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(|x| x.as_str()).filter(|x| !x.is_empty()).map(String::from)
}

struct Install {
    /// Directory containing Heroic's config ("heroic" folder lives here).
    base: PathBuf,
    flatpak: bool,
}

fn installs() -> Vec<Install> {
    let h = home();
    let mut v = Vec::new();
    let fp = h.join(".var/app").join(FLATPAK_ID).join("config");
    if fp.join("heroic").is_dir() {
        v.push(Install { base: fp, flatpak: true });
    }
    let native = h.join(".config");
    if native.join("heroic").is_dir() && which("heroic").is_some() {
        v.push(Install { base: native, flatpak: false });
    }
    v
}

fn launcher(inst: &Install, app: &str, runner: &str) -> LaunchSpec {
    let url = format!("heroic://launch?appName={app}&runner={runner}");
    // --no-gui: Heroic launches the game and never shows its window.
    if inst.flatpak {
        LaunchSpec::new("flatpak", &["run", FLATPAK_ID, "--no-gui", &url])
    } else {
        LaunchSpec::new("heroic", &["--no-gui", &url])
    }
}

fn detect_for(path: Option<&str>) -> Option<Detect> {
    let p = path?;
    let name = Path::new(p).file_name()?.to_string_lossy().into_owned();
    // Wine shows paths as Z:\home\...\Game, native shows /home/.../Game; the folder name matches both.
    (name.len() >= 4).then_some(Detect::Contains(name))
}

pub fn scan() -> Vec<Game> {
    let mut out = Vec::new();
    for inst in installs() {
        let hdir = inst.base.join("heroic");
        let cache = hdir.join("store_cache");

        // Epic (legendary): installed.json is authoritative for what's installed.
        let epic_installed: HashMap<String, Value> = first_existing(&[
            hdir.join("legendaryConfig/legendary/installed.json"),
            inst.base.join("legendary/installed.json"),
        ])
        .and_then(|p| read_json(&p))
        .and_then(|v| v.as_object().cloned())
        .map(|m| m.into_iter().collect())
        .unwrap_or_default();

        // GOG: list of { appName, install_path, platform }.
        let gog_installed: HashMap<String, Value> = read_json(&hdir.join("gog_store/installed.json"))
            .and_then(|v| v.get("installed").and_then(|x| x.as_array()).cloned())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|g| Some((s(&g, "appName")?, g)))
            .collect();

        for (store, runner, file, key) in [
            ("epic", "legendary", "legendary_library.json", "library"),
            ("gog", "gog", "gog_library.json", "games"),
            ("amazon", "nile", "nile_library.json", "library"),
        ] {
            let Some(lib) = read_json(&cache.join(file)) else { continue };
            let Some(items) = lib.get(key).and_then(|x| x.as_array()) else { continue };
            let mut seen = HashSet::new();
            for g in items {
                let Some(app) = s(g, "app_name") else { continue };
                if !seen.insert(app.clone()) || g.get("is_dlc").and_then(|x| x.as_bool()) == Some(true) {
                    continue;
                }
                let title = s(g, "title").unwrap_or_else(|| app.clone());
                let (installed, path) = match store {
                    "epic" => match epic_installed.get(&app) {
                        Some(i) => (true, s(i, "install_path")),
                        None => (false, None),
                    },
                    "gog" => match gog_installed.get(&app) {
                        Some(i) => (true, s(i, "install_path")),
                        None => (false, None),
                    },
                    _ => (
                        g.get("is_installed").and_then(|x| x.as_bool()).unwrap_or(false),
                        g.get("install").and_then(|i| s(i, "install_path")),
                    ),
                };
                out.push(Game {
                    id: format!("{store}:{app}"),
                    store: store.into(),
                    via: "Heroic".into(),
                    title,
                    installed,
                    cover: s(g, "art_square").or_else(|| s(g, "art_cover")),
                    hero: s(g, "art_background").or_else(|| s(g, "art_cover")),
                    size: None,
                    launch: Some(launcher(&inst, &app, runner)),
                    detect: detect_for(path.as_deref()),
                    // Heroic has no install-by-URL, so installing opens its window.
                    install: (!installed).then(|| client_spec(&inst)),
                    can_install: !installed,
                    ..Default::default()
                });
            }
        }

        // Apps added manually inside Heroic ("Add game").
        if let Some(lib) = read_json(&hdir.join("sideload_apps/library.json")) {
            for g in lib.get("games").and_then(|x| x.as_array()).into_iter().flatten() {
                let Some(app) = s(g, "app_name") else { continue };
                let exe = g.get("install").and_then(|i| s(i, "executable"));
                out.push(Game {
                    id: format!("heroic-sideload:{app}"),
                    store: "custom".into(),
                    via: "Heroic".into(),
                    title: s(g, "title").unwrap_or_else(|| app.clone()),
                    installed: true,
                    cover: s(g, "art_square").or_else(|| s(g, "art_cover")),
                    hero: s(g, "art_cover"),
                    size: None,
                    launch: Some(launcher(&inst, &app, "sideload")),
                    detect: detect_for(exe.as_deref().and_then(|e| Path::new(e).parent()).map(|p| p.to_str().unwrap_or(""))),
                    ..Default::default()
                });
            }
        }
    }
    out
}

/// Open Heroic's normal window (used for installing games).
pub fn open_client() -> Option<LaunchSpec> {
    installs().first().map(client_spec)
}

fn client_spec(inst: &Install) -> LaunchSpec {
    if inst.flatpak {
        LaunchSpec::new("flatpak", &["run", FLATPAK_ID])
    } else {
        LaunchSpec::new("heroic", &[])
    }
}
