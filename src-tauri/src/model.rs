use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// How to start a game. Always spawned detached with output sent to a log file.
#[derive(Debug, Clone, Default)]
pub struct LaunchSpec {
    pub program: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub cwd: Option<PathBuf>,
}

impl LaunchSpec {
    pub fn new(program: impl Into<String>, args: &[&str]) -> Self {
        Self {
            program: program.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            env: HashMap::new(),
            cwd: None,
        }
    }
}

/// How to tell that a game is still running by looking at /proc/*/cmdline.
#[derive(Debug, Clone)]
pub enum Detect {
    /// One argument equals this exactly (e.g. Steam's `AppId=1091500`).
    Arg(String),
    /// The joined command line contains this substring (e.g. an install folder name).
    Contains(String),
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Game {
    /// Stable id: `<source>:<native id>`.
    pub id: String,
    /// Store shown in the UI: steam, epic, gog, amazon, battlenet, arenanet, minecraft, lutris, custom...
    pub store: String,
    /// Which tool actually runs it (shown as "via ...").
    pub via: String,
    pub title: String,
    pub installed: bool,
    pub cover: Option<String>,
    pub hero: Option<String>,
    pub size: Option<u64>,
    #[serde(skip)]
    pub launch: Option<LaunchSpec>,
    #[serde(skip)]
    pub detect: Option<Detect>,
    /// Hands installation to the store's own client (e.g. Steam's install dialog).
    #[serde(skip)]
    pub install: Option<LaunchSpec>,
    /// A second way to start it (e.g. through Battle.net instead of directly), and its menu label.
    #[serde(skip)]
    pub alt_launch: Option<LaunchSpec>,
    pub alt_label: Option<String>,
    /// Emulated games: the system ("snes", "psx"…), which is also its library in the sidebar.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    /// The matching RetroAchievements game, for emulated games.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ra_id: Option<u64>,
    /// An emulated game with no emulator installed that can run it yet.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub needs_emulator: bool,
    /// It can start with its server/realm list showing (WoW started directly).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub pick_realm: bool,
    #[serde(skip)]
    pub uninstall: Option<LaunchSpec>,
    pub can_install: bool,
    pub can_uninstall: bool,
    /// Steam app id for Workshop-enabled games.
    pub workshop: Option<u32>,
    /// Wine prefix for games Lodestone runs through Proton itself (enables "Install runtimes").
    pub prefix: Option<String>,
    /// Normalized genres (Action, RPG, Strategy...) for filtering.
    pub genres: Vec<String>,
    /// Franchise/series name when the store knows it (the UI also groups by title).
    pub series: Option<String>,
    /// Kiln game id when Kiln can mod this game.
    pub kiln: Option<String>,
    /// The game has been set up in Kiln (it has a mod list).
    pub kiln_managed: bool,
    /// "available" (a newer build exists) or "updating" (the store is downloading it).
    pub update: Option<String>,
    /// Download size of the pending update, when the store says.
    pub update_size: Option<u64>,
    /// How the update happens: "steam" (Steam client) or "lodestone" (Lodestone downloads it).
    pub update_via: Option<String>,
}

pub fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

pub fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

pub fn first_existing(paths: &[PathBuf]) -> Option<PathBuf> {
    paths.iter().find(|p| p.exists()).cloned()
}

pub fn which(bin: &str) -> Option<PathBuf> {
    crate::host::which(bin)
}

/// Map genre names from different sources onto one shared vocabulary.
pub fn normalize_genre(raw: &str) -> Vec<&'static str> {
    let r = raw.trim().to_ascii_lowercase();
    let out: &[&'static str] = match r.as_str() {
        "action" | "hack and slash/beat 'em up" | "hack and slash" | "beat 'em up" => &["Action"],
        "adventure" | "point-and-click" | "visual novel" => &["Adventure"],
        "role-playing (rpg)" | "rpg" | "role-playing" => &["RPG"],
        "strategy" | "real time strategy (rts)" | "turn-based strategy (tbs)" | "tactical" | "4x" => &["Strategy"],
        "shooter" => &["Shooter"],
        "simulator" | "simulation" => &["Simulation"],
        "sport" | "sports" => &["Sports"],
        "racing" => &["Racing"],
        "platform" | "platformer" => &["Platformer"],
        "puzzle" => &["Puzzle"],
        "fighting" => &["Fighting"],
        "indie" => &["Indie"],
        "casual" | "arcade" | "pinball" | "quiz/trivia" => &["Casual"],
        "massively multiplayer" | "mmo" | "mmorpg" => &["MMO"],
        "moba" => &["MOBA"],
        "card & board game" | "card game" => &["Card & Board"],
        "music" | "rhythm" => &["Music"],
        "sandbox" => &["Sandbox"],
        "horror" | "survival horror" => &["Horror"],
        "free to play" => &["Free to Play"],
        "early access" => &["Early Access"],
        _ => &[],
    };
    out.to_vec()
}

pub fn normalize_genres<'a>(raw: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for r in raw {
        for g in normalize_genre(r) {
            if !v.iter().any(|x| x == g) {
                v.push(g.to_string());
            }
        }
    }
    v
}

/// Steam's numeric genre ids.
pub fn steam_genre(id: u32) -> Option<&'static str> {
    Some(match id {
        1 => "Action",
        2 => "Strategy",
        3 => "RPG",
        4 => "Casual",
        9 => "Racing",
        18 => "Sports",
        23 => "Indie",
        25 => "Adventure",
        28 => "Simulation",
        29 => "MMO",
        37 => "Free to Play",
        70 => "Early Access",
        _ => return None,
    })
}

/// Genre name for Diablo / Path of Exile style games.
pub const ISO_ARPG: &str = "Isometric ARPG";

/// Diablo-likes from Steam user tags: Action RPG + an overhead camera + hack-and-slash/loot,
/// excluding CRPGs and turn-based or party-based RPGs.
pub fn is_isometric_arpg(tags: &std::collections::HashSet<u32>) -> bool {
    const ACTION_RPG: u32 = 4231;
    const CAMERA: [u32; 2] = [5851, 4791]; // Isometric, Top-Down
    const STYLE: [u32; 2] = [1646, 4236]; // Hack and Slash, Loot
    const NOT: [u32; 4] = [4474, 4325, 10695, 14139]; // CRPG, Turn-Based Combat, Party-Based RPG, Turn-Based Tactics
    tags.contains(&ACTION_RPG)
        && CAMERA.iter().any(|t| tags.contains(t))
        && STYLE.iter().any(|t| tags.contains(t))
        && !NOT.iter().any(|t| tags.contains(t))
}
