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

/// How to run Kiln, or None when it isn't installed or you've turned the Kiln plug-in off
/// (Plug-ins page). Everything in Lodestone that talks to Kiln goes through here, so switching the
/// plug-in off hides all of it.
pub fn command() -> Option<Vec<String>> {
    if !crate::plugins::enabled(crate::plugins::KILN) {
        return None;
    }
    locate()
}

/// Is Kiln installed here, whether or not the plug-in is switched on?
pub fn installed() -> bool {
    locate().is_some()
}

/// Find Kiln. Linux: `kiln` on the PATH, the release Lodestone installed, else the command in its desktop entry
/// (e.g. `env PYTHONPATH=… python3 -m kiln`). Windows: `kiln.exe` on the PATH, or in
/// %LOCALAPPDATA%\Lodestone\Kiln or %LOCALAPPDATA%\Programs\Kiln. (Kiln has no Windows build yet, so
/// on Windows this finds nothing until it does.)
fn locate() -> Option<Vec<String>> {
    #[cfg(windows)]
    {
        if let Some(p) = which("kiln.exe") {
            return Some(vec![p.to_string_lossy().into_owned()]);
        }
        let local = dirs::data_local_dir()?;
        // The release Lodestone installed (plugin_sources): kiln.exe in the folder of the version in use.
        if let Some(dir) = crate::plugin_sources::current_unpacked("kiln") {
            let exe = dir.join("kiln.exe");
            if exe.is_file() {
                return Some(vec![exe.to_string_lossy().into_owned()]);
            }
        }
        for dir in [local.join("Lodestone").join("Kiln"), local.join("Programs").join("Kiln")] {
            let exe = dir.join("kiln.exe");
            if exe.is_file() {
                return Some(vec![exe.to_string_lossy().into_owned()]);
            }
        }
        None
    }
    #[cfg(not(windows))]
    {
        if let Some(p) = which("kiln") {
            return Some(vec![p.to_string_lossy().into_owned()]);
        }
        // The release Lodestone installed (plugin_sources): Kiln's source in a folder of its own, run with
        // the system's Python (and the PySide6 it needs).
        if let Some(dir) = crate::plugin_sources::current_unpacked("kiln") {
            return Some(vec!["env".into(), format!("PYTHONPATH={}", dir.to_string_lossy()), "python3".into(), "-m".into(), "kiln".into()]);
        }
        let h = home();
        for f in [h.join(".local/share/applications/kiln.desktop"), "/usr/share/applications/kiln.desktop".into()] {
            let Ok(txt) = std::fs::read_to_string(&f) else { continue };
            if let Some(cmd) = desktop_exec(&txt) {
                return Some(cmd);
            }
        }
        None
    }
}

static DEPS: Mutex<Option<(Instant, Option<String>)>> = Mutex::new(None);

/// What Kiln needs that this PC lacks (it runs on the system's Python), in plain words; None when it's
/// all there or Kiln isn't installed. Cached for a minute.
pub fn missing_deps() -> Option<String> {
    if cfg!(windows) || !installed() {
        return None;
    }
    if let Some((t, v)) = DEPS.lock().unwrap().as_ref() {
        if t.elapsed() < Duration::from_secs(60) {
            return v.clone();
        }
    }
    let mut c = std::process::Command::new("python3");
    c.args(["-c", "import PySide6.QtWidgets, PySide6.QtWebEngineWidgets, requests"]);
    let ok = crate::host::wrap(c).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
    let v = (!ok).then(|| "Kiln needs Python with PySide6 (including QtWebEngine) and requests. On Arch: sudo pacman -S pyside6 python-requests qt6-webengine libarchive".to_string());
    *DEPS.lock().unwrap() = Some((Instant::now(), v.clone()));
    v
}

/// The command from a desktop entry's `Exec=` line, without the `%f`-style placeholders.
#[cfg_attr(windows, allow(dead_code))]
fn desktop_exec(entry: &str) -> Option<Vec<String>> {
    let exec = entry.lines().find_map(|l| l.strip_prefix("Exec="))?;
    let parts: Vec<String> = exec.split_whitespace().filter(|w| !w.starts_with('%')).map(|w| w.trim_matches('"').to_string()).collect();
    (!parts.is_empty()).then_some(parts)
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

/// Forget the cached game list (after the Kiln plug-in is switched on or off).
pub fn forget() {
    *GAMES.lock().unwrap() = None;
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
    #[test]
    fn desktop_exec_drops_placeholders() {
        let e = "[Desktop Entry]\nName=Kiln\nExec=env PYTHONPATH=/opt/kiln python3 -m kiln %U\n";
        assert_eq!(super::desktop_exec(e), Some(vec!["env".into(), "PYTHONPATH=/opt/kiln".into(), "python3".into(), "-m".into(), "kiln".into()]));
        assert_eq!(super::desktop_exec("[Desktop Entry]\nName=Kiln\n"), None);
        assert_eq!(super::desktop_exec("Exec=%U\n"), None);
    }

    /// Linux: install a Kiln release zip from disk the way a download is installed, into a throwaway data
    /// folder, then ask the installed Kiln for its games:
    /// LODESTONE_TEST_KILN_ZIP=<zip> cargo test installs_a_kiln_zip -- --ignored --nocapture
    #[cfg(not(windows))]
    #[test]
    #[ignore]
    fn installs_a_kiln_zip() {
        let zip = std::env::var("LODESTONE_TEST_KILN_ZIP").expect("set LODESTONE_TEST_KILN_ZIP");
        let tmp = std::env::temp_dir().join(format!("lodestone-kiln-test-{}", std::process::id()));
        std::env::set_var("XDG_DATA_HOME", &tmp);
        std::env::set_var("PATH", "/usr/local/bin:/usr/bin:/bin");
        let bytes = std::fs::read(zip).unwrap();
        let src = crate::plugin_sources::find("kiln").unwrap();
        tauri::async_runtime::block_on(crate::plugin_sources::install_for_test(&src, "0.1.0", &bytes)).unwrap();
        println!("command: {:?}", super::locate());
        let games = super::run_json(&["--games"]).expect("installed Kiln answers --games");
        println!("{} games", games.as_array().map_or(0, |a| a.len()));
        assert!(games.as_array().is_some_and(|a| a.len() > 30));
        let _ = std::fs::remove_dir_all(&tmp);
    }

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
