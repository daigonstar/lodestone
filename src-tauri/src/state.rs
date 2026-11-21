//! Per-game user data: playtime, last played, favorites, hidden. ~/.local/share/lodestone/state.json

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Entry {
    #[serde(default)]
    pub playtime: u64,
    #[serde(default)]
    pub last_played: u64,
    #[serde(default)]
    pub favorite: bool,
    #[serde(default)]
    pub hidden: bool,
    /// You set the play time yourself: it's used as is (instead of the store's figure) and
    /// Lodestone keeps adding the time you play from here on.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub manual_playtime: bool,
    /// Lodestone's own count when you last edited it, and the value you set: going back to automatic
    /// restores the count plus anything played since.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playtime_before_edit: Option<(u64, u64)>,
    /// MangoHud for this game: on/off, or None to follow Settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mangohud: Option<bool>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Store {
    #[serde(default)]
    pub games: HashMap<String, Entry>,
}

pub fn data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| crate::model::home().join(".local/share")).join("lodestone")
}

fn file() -> PathBuf {
    data_dir().join("state.json")
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Store {
    pub fn load() -> Self {
        fs::read_to_string(file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        let _ = fs::create_dir_all(data_dir());
        let tmp = file().with_extension("json.tmp");
        if let Ok(txt) = serde_json::to_string_pretty(self) {
            if fs::write(&tmp, txt).is_ok() {
                let _ = fs::rename(tmp, file());
            }
        }
    }

    pub fn entry(&mut self, id: &str) -> &mut Entry {
        self.games.entry(id.to_string()).or_default()
    }
}
