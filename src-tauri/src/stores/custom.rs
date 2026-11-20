//! Games you add yourself: any command line (native binary, `umu-run game.exe`, a script...).
//! Stored in ~/.config/lodestone/games.json.

use crate::model::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomGame {
    pub id: String,
    pub title: String,
    /// Run through `sh -c`, so quoting and env assignments work.
    pub command: String,
    #[serde(default = "default_store")]
    pub store: String,
    #[serde(default)]
    pub cover: Option<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Optional /proc command-line substring used to tell when the game is running.
    #[serde(default)]
    pub detect: Option<String>,
    /// "proton" runs `command` (a .exe path) through Proton in its own prefix.
    #[serde(default)]
    pub runner: Option<String>,
}

fn default_store() -> String {
    "custom".into()
}

pub fn file() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| home().join(".config")).join("lodestone/games.json")
}

pub fn load() -> Vec<CustomGame> {
    fs::read_to_string(file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn save(list: &[CustomGame]) -> std::io::Result<()> {
    let f = file();
    if let Some(d) = f.parent() {
        fs::create_dir_all(d)?;
    }
    fs::write(f, serde_json::to_string_pretty(list)?)
}

pub fn scan() -> Vec<Game> {
    load()
        .into_iter()
        .map(|c| {
            if c.runner.as_deref() == Some("proton") {
                let exe = PathBuf::from(c.command.trim().trim_matches('"'));
                let launch = crate::proton::spec(crate::proton::Run {
                    prefix: &crate::proton::prefix(&format!("custom-{}", c.id)),
                    exe: &exe,
                    args: vec![],
                    store: None,
                    game_id: "umu-default",
                })
                .ok();
                return Game {
                    id: format!("custom:{}", c.id),
                    store: c.store,
                    via: "Proton".into(),
                    title: c.title,
                    installed: exe.is_file(),
                    cover: c.cover,
                    launch,
                    detect: exe.file_name().map(|n| Detect::Contains(n.to_string_lossy().into_owned())),
                    prefix: Some(path_str(&crate::proton::prefix(&format!("custom-{}", c.id)))),
                    ..Default::default()
                };
            }
            let mut spec = LaunchSpec::new("sh", &["-c", &c.command]);
            spec.env = c.env.clone();
            let detect = c.detect.clone().filter(|d| !d.is_empty()).map(Detect::Contains).or_else(|| {
                // Default: the name of the first .exe or executable path in the command.
                c.command
                    .split_whitespace()
                    .map(|w| w.trim_matches(|ch| ch == '"' || ch == '\''))
                    .find(|w| w.contains('/'))
                    .and_then(|w| Path::new(w).file_name())
                    .map(|n| Detect::Contains(n.to_string_lossy().into_owned()))
            });
            Game {
                id: format!("custom:{}", c.id),
                store: c.store,
                via: "Custom".into(),
                title: c.title,
                installed: true,
                cover: c.cover,
                hero: None,
                size: None,
                launch: Some(spec),
                detect,
                ..Default::default()
            }
        })
        .collect()
}
