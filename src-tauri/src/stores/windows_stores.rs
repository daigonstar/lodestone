//! Windows: games installed through the stores' own launchers, which is how people on Windows have them
//! (on Linux, Lodestone reads Epic and GOG through Heroic or its own tools instead).
//!
//! - Epic Games Launcher: one manifest per installed game in
//!   C:\ProgramData\Epic\EpicGamesLauncher\Data\Manifests (*.item, JSON). Games start through the
//!   launcher's own link, com.epicgames.launcher://apps/…?action=launch, so its sign-in, cloud saves and
//!   overlay work as usual.
//! - GOG GALAXY: a registry key per installed game under SOFTWARE\GOG.com\Games, started with GALAXY's
//!   documented "run game" command (so its overlay and cloud saves work), or directly without GALAXY.
//!
//! Only reads these files and keys; nothing is changed.

use crate::model::{path_str, Detect, Game, LaunchSpec};
use std::fs;
use std::path::{Path, PathBuf};

/// Epic and GOG games installed on this Windows PC.
pub fn scan() -> Vec<Game> {
    let mut out = epic_games();
    out.extend(gog_games());
    out
}

fn epic_manifests_dir() -> PathBuf {
    let data = std::env::var_os("ProgramData").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\ProgramData"));
    data.join("Epic").join("EpicGamesLauncher").join("Data").join("Manifests")
}

/// The Epic link that starts (or installs) a game through the launcher.
fn epic_link(m: &serde_json::Value, action: &str) -> Option<String> {
    let (ns, item, app) = (m["CatalogNamespace"].as_str()?, m["CatalogItemId"].as_str()?, m["AppName"].as_str()?);
    let ok = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    (ok(ns) && ok(item) && ok(app)).then(|| format!("com.epicgames.launcher://apps/{ns}%3A{item}%3A{app}?action={action}&silent=true"))
}

fn epic_games() -> Vec<Game> {
    let mut out = Vec::new();
    for e in fs::read_dir(epic_manifests_dir()).into_iter().flatten().flatten() {
        if e.path().extension().is_none_or(|x| x != "item") {
            continue;
        }
        let Ok(text) = fs::read_to_string(e.path()) else { continue };
        let Ok(m) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
        // Unreal Engine and other non-game installs, and half-finished downloads.
        if m["bIsIncompleteInstall"] == serde_json::json!(true) || m["AppCategories"].as_array().is_some_and(|c| !c.iter().any(|x| x == "games")) {
            continue;
        }
        let (Some(title), Some(app), Some(dir)) = (m["DisplayName"].as_str(), m["AppName"].as_str(), m["InstallLocation"].as_str()) else { continue };
        let Some(link) = epic_link(&m, "launch") else { continue };
        out.push(Game {
            id: format!("epic:{app}"),
            store: "epic".into(),
            via: "Epic Games Launcher".into(),
            title: title.to_string(),
            installed: Path::new(dir).is_dir(),
            size: m["InstallSize"].as_u64(),
            launch: Some(crate::os::open_spec(&link)),
            detect: Some(Detect::Contains(dir.to_string())),
            ..Default::default()
        });
    }
    out
}

fn gog_games() -> Vec<Game> {
    const KEY: &str = "SOFTWARE\\WOW6432Node\\GOG.com\\Games";
    let galaxy = crate::os::registry_string(true, "SOFTWARE\\WOW6432Node\\GOG.com\\GalaxyClient\\paths", "client").map(PathBuf::from);
    let mut out = Vec::new();
    for id in crate::os::registry_subkeys(true, KEY) {
        if !id.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let k = format!("{KEY}\\{id}");
        let get = |v: &str| crate::os::registry_string(true, &k, v);
        let (Some(title), Some(dir)) = (get("gameName"), get("path")) else { continue };
        let exe = get("exe").map(PathBuf::from);
        // Through GALAXY when it's installed (overlay, cloud saves), otherwise the game's own program.
        let launch = match (&galaxy, &exe) {
            (Some(g), _) if g.join("GalaxyClient.exe").is_file() => {
                let gid = format!("/gameId={id}");
                let path = format!("/path={dir}");
                Some(LaunchSpec::new(path_str(&g.join("GalaxyClient.exe")), &["/command=runGame", &gid, &path]))
            }
            (_, Some(e)) if e.is_file() => {
                let mut s = LaunchSpec::new(path_str(e), &[]);
                s.cwd = e.parent().map(Path::to_path_buf);
                Some(s)
            }
            _ => None,
        };
        out.push(Game {
            id: format!("gog:{id}"),
            store: "gog".into(),
            via: "GOG GALAXY".into(),
            title,
            installed: Path::new(&dir).is_dir(),
            launch,
            detect: Some(Detect::Contains(dir)),
            ..Default::default()
        });
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn epic_links_only_from_plain_ids() {
        let m = serde_json::json!({ "CatalogNamespace": "fn", "CatalogItemId": "4fe75bbc5a674f4f9b356b5c90567da5", "AppName": "Fortnite" });
        assert_eq!(super::epic_link(&m, "launch").unwrap(), "com.epicgames.launcher://apps/fn%3A4fe75bbc5a674f4f9b356b5c90567da5%3AFortnite?action=launch&silent=true");
        let bad = serde_json::json!({ "CatalogNamespace": "fn&x=1", "CatalogItemId": "a", "AppName": "b" });
        assert!(super::epic_link(&bad, "launch").is_none());
    }
}
