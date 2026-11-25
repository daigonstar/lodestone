//! Updates for the programs Lodestone installs: emulators, and on Windows the stores' launchers. Only
//! when you ask (Settings → Programs → Check for updates), never in the background, and only for the
//! packages Lodestone itself would install (anything else is refused).
//!
//! - Windows: winget's list of available upgrades (`winget upgrade`), updated with `winget upgrade`.
//! - Linux: Flathub's (`flatpak remote-ls --updates`), updated with `flatpak update`.
//!
//! Checking asks winget's or Flathub's package list; updating downloads from each program's publisher,
//! the same way installing does.

use crate::model::LaunchSpec;
use serde::Serialize;
use std::process::{Command, Stdio};

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Update {
    pub id: String,
    pub name: String,
    /// The version you have, when known.
    pub have: Option<String>,
    pub latest: Option<String>,
}

/// Every package Lodestone manages: (package id, name).
pub fn managed() -> Vec<(&'static str, &'static str)> {
    let mut v = crate::stores::emulation::managed_packages();
    if cfg!(windows) {
        for a in crate::stores::winapps::apps() {
            if let Some(id) = crate::stores::winapps::winget_id(a.id) {
                v.push((id, a.name));
            }
        }
    }
    v
}

/// Read winget's upgrade table. Matched by package ID rather than column headings, so it works whatever
/// language Windows uses; winget may shorten a long ID with "…", which still matches.
fn parse_winget(out: &str, managed: &[(&'static str, &'static str)]) -> Vec<Update> {
    let mut found = Vec::new();
    for line in out.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        for (i, w) in words.iter().enumerate() {
            let hit = managed.iter().find(|(id, _)| {
                w.eq_ignore_ascii_case(id) || w.strip_suffix('…').is_some_and(|p| p.len() >= 6 && id.to_ascii_lowercase().starts_with(&p.to_ascii_lowercase()))
            });
            if let Some((id, name)) = hit {
                if !found.iter().any(|u: &Update| u.id == *id) {
                    let ver = |j: usize| words.get(j).map(|v| v.to_string()).filter(|v| !v.eq_ignore_ascii_case("winget"));
                    found.push(Update { id: id.to_string(), name: name.to_string(), have: ver(i + 1), latest: ver(i + 2) });
                }
                break;
            }
        }
    }
    found
}

/// Flatpak's list of available updates ("application<TAB>version" per line).
fn parse_flatpak(out: &str, managed: &[(&'static str, &'static str)]) -> Vec<Update> {
    out.lines()
        .filter_map(|l| {
            let mut parts = l.split('\t');
            let app = parts.next()?.trim();
            let (id, name) = managed.iter().find(|(id, _)| *id == app)?;
            Some(Update { id: id.to_string(), name: name.to_string(), have: None, latest: parts.next().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) })
        })
        .collect()
}

/// What can be updated now.
pub fn check() -> Result<Vec<Update>, String> {
    let managed = managed();
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("winget");
        c.args(["upgrade", "--source", "winget", "--accept-source-agreements", "--disable-interactivity"]);
        c
    } else {
        let mut c = Command::new("flatpak");
        c.args(["remote-ls", "--updates", "--app", "--columns=application,version"]);
        crate::host::wrap(c)
    };
    let out = cmd.stdin(Stdio::null()).stderr(Stdio::null()).output().map_err(|e| format!("Couldn't check for updates: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(if cfg!(windows) { parse_winget(&text, &managed) } else { parse_flatpak(&text, &managed) })
}

/// The command that updates one package Lodestone manages.
pub fn update_spec(id: &str) -> Result<(LaunchSpec, &'static str), String> {
    let (id, name) = managed().into_iter().find(|(m, _)| *m == id).ok_or("Lodestone doesn't manage that program")?;
    let spec = if cfg!(windows) { crate::winget::upgrade_spec(id) } else { LaunchSpec::new("flatpak", &["update", "-y", "--noninteractive", id]) };
    Ok((spec, name))
}

#[cfg(test)]
mod tests {
    const M: &[(&str, &str)] = &[("Libretro.RetroArch", "RetroArch"), ("EpicGames.EpicGamesLauncher", "Epic Games Launcher"), ("org.ppsspp.PPSSPP", "PPSSPP")];

    #[test]
    fn reads_wingets_upgrade_table_in_any_language() {
        let out = "Nom              ID                       Version  Disponible Source\n----\nRetroArch        Libretro.RetroArch       1.22.1   1.22.2     winget\nEpic Games Lau…  EpicGames.EpicGamesLaun… 1.3.200  1.3.210.0  winget\nSomething        Other.Thing              1.0      2.0        winget\n3 mises à niveau disponibles.";
        let u = super::parse_winget(out, M);
        assert_eq!(u.len(), 2);
        assert_eq!((u[0].id.as_str(), u[0].have.as_deref(), u[0].latest.as_deref()), ("Libretro.RetroArch", Some("1.22.1"), Some("1.22.2")));
        assert_eq!((u[1].id.as_str(), u[1].latest.as_deref()), ("EpicGames.EpicGamesLauncher", Some("1.3.210.0")));
    }

    #[test]
    fn reads_flatpaks_update_list() {
        let u = super::parse_flatpak("org.ppsspp.PPSSPP\t1.20.4\norg.other.App\t2.0\n", M);
        assert_eq!(u, vec![super::Update { id: "org.ppsspp.PPSSPP".into(), name: "PPSSPP".into(), have: None, latest: Some("1.20.4".into()) }]);
    }
}
