//! Minecraft instances from ATLauncher and Prism Launcher, launched straight into the game.

use crate::model::*;
use std::fs;
use std::path::Path;

const ATL_FLATPAK: &str = "com.atlauncher.ATLauncher";
const PRISM_FLATPAK: &str = "org.prismlauncher.PrismLauncher";

fn icon_in(dir: &Path) -> Option<String> {
    ["instance.png", "icon.png"].iter().map(|n| dir.join(n)).find(|p| p.is_file()).map(|p| path_str(&p))
}

pub fn scan() -> Vec<Game> {
    let h = home();
    let mut out = Vec::new();

    // ATLauncher
    for (dir, flatpak) in [
        (h.join(".var/app").join(ATL_FLATPAK).join("data/instances"), true),
        (h.join(".local/share/atlauncher/instances"), false),
    ] {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let path = e.path();
            let Ok(txt) = fs::read_to_string(path.join("instance.json")) else { continue };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) else { continue };
            let folder = e.file_name().to_string_lossy().into_owned();
            let l = v.get("launcher");
            let name = l.and_then(|l| l.get("name")).and_then(|x| x.as_str()).unwrap_or(&folder).to_string();
            let ver = l.and_then(|l| l.get("version")).and_then(|x| x.as_str()).unwrap_or("");
            let args = ["--launch", folder.as_str(), "--close-launcher", "--no-console", "--no-launcher-update"];
            let launch = if flatpak {
                let mut a = vec!["run", ATL_FLATPAK];
                a.extend(args);
                LaunchSpec::new("flatpak", &a)
            } else {
                LaunchSpec::new("atlauncher", &args)
            };
            out.push(Game {
                id: format!("atlauncher:{folder}"),
                store: "minecraft".into(),
                via: format!("ATLauncher · {ver}").trim_end_matches(" · ").to_string(),
                title: format!("Minecraft: {name}"),
                installed: true,
                cover: icon_in(&path),
                hero: None,
                size: None,
                launch: Some(launch),
                detect: Some(Detect::Contains(format!("instances/{folder}"))),
                genres: vec!["Sandbox".into()],
                series: Some("Minecraft".into()),
                ..Default::default()
            });
        }
    }

    // Prism Launcher
    for (dir, flatpak) in [
        (h.join(".var/app").join(PRISM_FLATPAK).join("data/PrismLauncher/instances"), true),
        (h.join(".local/share/PrismLauncher/instances"), false),
    ] {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let path = e.path();
            let Ok(cfg) = fs::read_to_string(path.join("instance.cfg")) else { continue };
            let folder = e.file_name().to_string_lossy().into_owned();
            let name = cfg.lines().find_map(|l| l.strip_prefix("name=")).unwrap_or(&folder).to_string();
            let launch = if flatpak {
                LaunchSpec::new("flatpak", &["run", PRISM_FLATPAK, "-l", &folder])
            } else {
                LaunchSpec::new("prismlauncher", &["-l", &folder])
            };
            out.push(Game {
                id: format!("prism:{folder}"),
                store: "minecraft".into(),
                via: "Prism Launcher".into(),
                title: format!("Minecraft: {name}"),
                installed: true,
                cover: None,
                hero: None,
                size: None,
                launch: Some(launch),
                detect: Some(Detect::Contains(format!("instances/{folder}"))),
                genres: vec!["Sandbox".into()],
                series: Some("Minecraft".into()),
                ..Default::default()
            });
        }
    }
    out
}
