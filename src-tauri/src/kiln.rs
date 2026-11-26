//! Kiln (the mod manager) integration: which library games Kiln can mod or already manages,
//! their mod status, and opening/playing them through Kiln's Guarded Play.
//! Talks to Kiln only through its CLI (`kiln --games/--status/--game/--play`).

use crate::model::{home, which, LaunchSpec};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Deserialize)]
pub struct KilnGame {
    pub id: String,
    pub name: String,
    pub steam_appid: u32,
    #[serde(default)]
    pub alt_appids: Vec<u32>,
    #[serde(default)]
    pub installed: bool,
    #[serde(default)]
    pub managed: bool,
    /// Mods Kiln has downloaded for this game (None from older Kiln versions).
    #[serde(default)]
    pub mods: Option<usize>,
}

/// How to run Kiln: `kiln` on PATH, else the command from its desktop entry
/// (e.g. `env PYTHONPATH=… python3 -m kiln`).
pub fn command() -> Option<Vec<String>> {
    if let Some(p) = which("kiln") {
        return Some(vec![p.to_string_lossy().into_owned()]);
    }
    // Linux: the command from Kiln's desktop entry. Windows has no such entry; Kiln becomes a plug-in.
    #[cfg(windows)]
    return None;
    #[cfg(not(windows))]
    desktop_entry_command()
}

#[cfg(not(windows))]
fn desktop_entry_command() -> Option<Vec<String>> {
    let h = home();
    for f in [h.join(".local/share/applications/kiln.desktop"), "/usr/share/applications/kiln.desktop".into()] {
        let Ok(txt) = std::fs::read_to_string(&f) else { continue };
        let Some(exec) = txt.lines().find_map(|l| l.strip_prefix("Exec=")) else { continue };
        let parts: Vec<String> = exec.split_whitespace().filter(|w| !w.starts_with('%')).map(|w| w.trim_matches('"').to_string()).collect();
        if !parts.is_empty() {
            return Some(parts);
        }
    }
    None
}

fn spec(args: &[&str]) -> Option<LaunchSpec> {
    let mut cmd = command()?;
    let program = cmd.remove(0);
    let mut s = LaunchSpec::new(program, &[]);
    s.args = cmd;
    s.args.extend(args.iter().map(|a| a.to_string()));
    Some(s)
}

fn run_json(args: &[&str]) -> Option<serde_json::Value> {
    let s = spec(args)?;
    let mut c = std::process::Command::new(&s.program);
    c.args(&s.args).current_dir(home());
    let out = crate::host::wrap(c).stdin(std::process::Stdio::null()).output().ok()?;
    if !out.status.success() {
        return None;
    }
    serde_json::from_slice(&out.stdout).ok()
}

static GAMES: Mutex<Option<(Instant, Vec<KilnGame>)>> = Mutex::new(None);

/// Kiln's supported games (cached for a minute; Kiln reads Steam's folders to fill it).
pub fn games() -> Vec<KilnGame> {
    if let Some((t, g)) = GAMES.lock().unwrap().as_ref() {
        if t.elapsed() < Duration::from_secs(60) {
            return g.clone();
        }
    }
    let list: Vec<KilnGame> = run_json(&["--games"]).and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    *GAMES.lock().unwrap() = Some((Instant::now(), list.clone()));
    list
}

/// Steam app id -> Kiln game.
pub fn by_appid() -> HashMap<u32, KilnGame> {
    let mut m = HashMap::new();
    for g in games() {
        for a in std::iter::once(g.steam_appid).chain(g.alt_appids.iter().copied()) {
            m.insert(a, g.clone());
        }
    }
    m
}

pub fn status(id: &str) -> Option<serde_json::Value> {
    run_json(&["--status", id])
}

/// Open Kiln on a game (a running Kiln switches to it).
pub fn open(id: &str) -> Option<LaunchSpec> {
    spec(&["--game", id])
}

/// Kiln's Guarded Play: checks deployment, loaders and launch options, offers fixes, then starts the game.
pub fn play(id: &str) -> Option<LaunchSpec> {
    spec(&["--play", id])
}

/// Play without mods: Kiln takes them out (keeping only the loaders Steam needs) and launches.
/// The next modded Play puts everything back.
pub fn play_vanilla(id: &str) -> Option<LaunchSpec> {
    spec(&["--play-vanilla", id])
}

#[cfg(test)]
mod tests {
    /// `cargo test live_kiln -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_kiln() {
        println!("kiln command: {:?}", super::command());
        let t = std::time::Instant::now();
        let mut games = crate::stores::scan_all();
        crate::apply_kiln(&mut games);
        println!("scan + kiln tagging: {:?}", t.elapsed());
        let mut k: Vec<_> = games.iter().filter(|g| g.kiln.is_some() && (g.installed || g.kiln_managed)).map(|g| (g.kiln_managed, g.title.clone(), g.kiln.clone().unwrap())).collect();
        k.sort();
        k.reverse();
        for (m, t, id) in &k {
            println!("  {} {t} ({id})", if *m { "MODDED " } else { "moddable" });
        }
        let t = std::time::Instant::now();
        let s = super::status("stardewvalley");
        println!("status stardew in {:?}: {}", t.elapsed(), s.map(|v| v.to_string()).unwrap_or_default());
        println!("play spec: {:?}", super::play("skyrimse").map(|s| (s.program, s.args)));
    }
}
