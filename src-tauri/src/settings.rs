//! App settings in ~/.config/lodestone/settings.json.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    /// CurseForge's API needs a personal key (free at console.curseforge.com). Modrinth needs none.
    #[serde(default)]
    pub curseforge_api_key: String,
    /// Path to a Proton build, "GE-Proton" (umu auto-downloads the latest), or empty for automatic.
    #[serde(default)]
    pub proton: String,
    /// Where Lodestone installs Epic/GOG games (subfolders Epic/ and GOG/). Empty = ~/Games.
    #[serde(default)]
    pub games_dir: String,
    /// Optional Steam Web API key: exact owned-games list (refunds drop off). steamcommunity.com/dev/apikey
    #[serde(default)]
    pub steam_api_key: String,
    /// Optional SteamGridDB API key (free at steamgriddb.com/profile/preferences/api) for artwork search.
    #[serde(default)]
    pub steamgriddb_key: String,
    /// The first-run setup guide has been completed or skipped.
    #[serde(default)]
    pub onboarded: bool,
    /// Play Kiln-managed games through Kiln's Guarded Play (checks mods before launching).
    #[serde(default = "yes")]
    pub kiln_play: bool,
    /// Keep Steam running silently and install/update/uninstall Steam games without its windows.
    #[serde(default = "yes")]
    pub steam_background: bool,
    /// Show MangoHud (performance overlay) in every game Lodestone starts, unless a game turns it off.
    #[serde(default)]
    pub mangohud: bool,
    /// An Emulation folder (with roms/<system> inside) to use besides the ones found automatically.
    #[serde(default)]
    pub emulation_dir: String,
    /// RetroAchievements username and Web API key (retroachievements.org → Settings → Keys).
    #[serde(default)]
    pub ra_user: String,
    #[serde(default)]
    pub ra_key: String,
    /// The emulator chosen per system: system id -> index of its launch command.
    #[serde(default)]
    pub emu_choice: std::collections::HashMap<String, usize>,
    /// Emulators Lodestone installs get an app menu entry (off: hidden, since you launch them here).
    #[serde(default)]
    pub emu_menu_entry: bool,
}

fn yes() -> bool {
    true
}

pub fn games_dir() -> PathBuf {
    let s = load().games_dir;
    let s = s.trim();
    if s.is_empty() {
        crate::model::home().join("Games")
    } else if let Some(rest) = s.strip_prefix("~/") {
        crate::model::home().join(rest)
    } else {
        PathBuf::from(s)
    }
}

fn file() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| crate::model::home().join(".config")).join("lodestone/settings.json")
}

pub fn load() -> Settings {
    fs::read_to_string(file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| Settings { kiln_play: true, steam_background: true, ..Default::default() })
}

pub fn save(s: &Settings) -> Result<(), String> {
    let f = file();
    if let Some(d) = f.parent() {
        fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    // It holds API keys: keep it private to the user, from the moment it's created (fs::write would
    // create it readable by everyone for an instant before the permissions were tightened).
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let text = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    let mut file = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&f).map_err(|e| e.to_string())?;
    let _ = fs::set_permissions(&f, fs::Permissions::from_mode(0o600)); // files made before this change
    file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    Ok(())
}
