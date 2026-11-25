//! Spawns games fully detached (no terminal, output to a log file) and tracks them via /proc
//! (on the host when Lodestone is a Flatpak, see host.rs).

use crate::model::{Detect, Game, LaunchSpec};
use crate::state;
use serde::Serialize;
use std::collections::HashMap;
use std::fs::{self, File};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// How long we wait for a game's process to show up before giving up on a launch.
const LAUNCH_GRACE: Duration = Duration::from_secs(150);

pub fn log_path(id: &str) -> PathBuf {
    let safe: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    state::data_dir().join("logs").join(format!("{safe}.log"))
}

/// The environment Lodestone was started with, before any library changed it (see main()).
pub static ORIGINAL_ENV: std::sync::OnceLock<HashMap<String, String>> = std::sync::OnceLock::new();

/// Variables Lodestone or its libraries set for themselves that must not reach games:
/// SSL_CERT_* point at host paths that don't exist inside Proton's runtime container (breaking
/// winetricks downloads), cargo's LD_LIBRARY_PATH, and our own WebKit workaround.
fn scrub(cmd: &mut Command) {
    let orig = ORIGINAL_ENV.get();
    let had = |k: &str| orig.is_some_and(|o| o.contains_key(k));
    // Always: these point at host paths that don't exist inside Proton's runtime container, and tools like
    // cargo-tauri set them for everything they start. legendary/gogdl bring their own CA bundles.
    cmd.env_remove("SSL_CERT_FILE").env_remove("SSL_CERT_DIR");
    if !had("WEBKIT_DISABLE_DMABUF_RENDERER") {
        cmd.env_remove("WEBKIT_DISABLE_DMABUF_RENDERER");
    }
    if let Ok(v) = std::env::var("LD_LIBRARY_PATH") {
        if !had("LD_LIBRARY_PATH") || v.contains("/target/debug/") || v.contains("/target/release/") {
            cmd.env_remove("LD_LIBRARY_PATH");
        }
    }
}

pub fn spawn(spec: &LaunchSpec, log: PathBuf) -> std::io::Result<Child> {
    if let Some(d) = log.parent() {
        fs::create_dir_all(d)?;
    }
    let out = File::create(&log)?;
    let mut cmd = Command::new(&spec.program);
    scrub(&mut cmd);
    cmd.args(&spec.args).envs(&spec.env);
    if let Some(d) = spec.cwd.as_ref().filter(|d| d.is_dir()) {
        cmd.current_dir(d);
    }
    let mut cmd = crate::host::wrap(cmd);
    cmd.stdin(Stdio::null()).stdout(out.try_clone()?).stderr(out);
    // Detached: keeps running if Lodestone closes, and no terminal signals reach it.
    crate::os::detach(&mut cmd);
    cmd.spawn()
}

pub struct Session {
    pub title: String,
    started: Instant,
    first_seen: Option<Instant>,
    child: Option<Child>,
    child_done: bool,
    detect: Option<Detect>,
}

#[derive(Serialize, Clone)]
pub struct Running {
    pub id: String,
    pub title: String,
    /// "launching" until the game's process is seen, then "running".
    pub state: &'static str,
    pub seconds: u64,
}

pub enum Tick {
    Alive(Running),
    Stopped { played: u64 },
}

impl Session {
    pub fn new(game: &Game, child: Option<Child>) -> Self {
        Self {
            title: game.title.clone(),
            started: Instant::now(),
            first_seen: None,
            child,
            child_done: false,
            detect: game.detect.clone(),
        }
    }

    pub fn tick(&mut self, id: &str, procs: &[String]) -> Tick {
        let child_alive = match &mut self.child {
            Some(c) if !self.child_done => match c.try_wait() {
                Ok(None) => true,
                _ => {
                    self.child_done = true;
                    false
                }
            },
            _ => false,
        };
        let present = self.detect.as_ref().is_some_and(|d| matches(d, procs));
        if present && self.first_seen.is_none() {
            self.first_seen = Some(Instant::now());
        }
        let waiting = self.detect.is_some() && self.first_seen.is_none() && self.started.elapsed() < LAUNCH_GRACE;
        if present || child_alive || waiting {
            let since = self.first_seen.unwrap_or(self.started);
            Tick::Alive(Running {
                id: id.to_string(),
                title: self.title.clone(),
                state: if waiting { "launching" } else { "running" },
                seconds: since.elapsed().as_secs(),
            })
        } else {
            let played = match self.first_seen {
                Some(t) => t.elapsed().as_secs(),
                // Never matched by /proc, but the launcher process lived on: count that.
                None if self.detect.is_none() || self.started.elapsed() > LAUNCH_GRACE => self.started.elapsed().as_secs(),
                None => 0,
            };
            Tick::Stopped { played }
        }
    }
}

fn matches(d: &Detect, procs: &[String]) -> bool {
    match d {
        Detect::Arg(a) => {
            let needle = format!("\0{a}\0");
            procs.iter().any(|p| p.contains(&needle))
        }
        Detect::Contains(s) => procs.iter().any(|p| p.contains(s.as_str())),
    }
}

/// Every process's command line, args separated (and wrapped) by NUL so exact-arg matching works.
pub fn process_table() -> Vec<String> {
    crate::host::procs()
        .into_iter()
        .map(|p| p.cmdline)
        .filter(|b| !b.is_empty())
        .map(|b| {
            let mut s = String::from("\0");
            s.push_str(&String::from_utf8_lossy(&b));
            if !s.ends_with('\0') {
                s.push('\0');
            }
            s
        })
        .collect()
}

pub type Sessions = HashMap<String, Session>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Detect, Game, LaunchSpec};

    #[test]
    fn games_dont_inherit_lodestones_cert_override() {
        std::env::set_var("SSL_CERT_FILE", "/etc/ssl/cert.pem");
        let log = std::env::temp_dir().join(format!("lodestone-env-{}.log", std::process::id()));
        let mut c = spawn(&LaunchSpec::new("sh", &["-c", "echo \"[$SSL_CERT_FILE]\""]), log.clone()).unwrap();
        c.wait().unwrap();
        assert_eq!(std::fs::read_to_string(&log).unwrap().trim(), "[]");
        let _ = std::fs::remove_file(log);
    }

    #[test]
    fn spawns_detached_and_tracks_until_exit() {
        let marker = format!("lodestone-test-{}", std::process::id());
        // The shell stays running for the whole sleep (the `true` after it), and its command line holds
        // the marker. (No `exec -a`: dash, Ubuntu's sh, doesn't have it.)
        let spec = LaunchSpec::new("sh", &["-c", &format!("sleep 3; true {marker}")]);
        let game = Game {
            id: "custom:test".into(), store: "custom".into(), via: "Custom".into(), title: "Test".into(),
            installed: true, cover: None, hero: None, size: None,
            launch: Some(spec.clone()), detect: Some(Detect::Contains(marker.clone())),
            ..Default::default()
        };
        let log = std::env::temp_dir().join(format!("{marker}.log"));
        let child = spawn(&spec, log).unwrap();
        let mut s = Session::new(&game, Some(child));
        std::thread::sleep(Duration::from_millis(400));
        match s.tick("custom:test", &process_table()) {
            Tick::Alive(r) => assert_eq!(r.state, "running"),
            Tick::Stopped { .. } => panic!("should be running"),
        }
        std::thread::sleep(Duration::from_secs(3));
        match s.tick("custom:test", &process_table()) {
            Tick::Stopped { played } => assert!((2..=4).contains(&played), "played {played}"),
            Tick::Alive(_) => panic!("should have stopped"),
        }
    }
}
