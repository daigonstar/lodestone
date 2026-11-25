//! Box art from libretro's thumbnail collection, the one RetroArch uses: a fallback cover for ROMs that
//! RetroAchievements and ES-DE have no art for. No key and no account.
//!
//! The images live in libretro's GitHub repositories (github.com/libretro-thumbnails), one per system,
//! named after the No-Intro / Redump name of the game. Lodestone lists a system's `Named_Boxarts` folder
//! through GitHub's API (two requests per system, refreshed weekly and cached in
//! `<data>/libretro-thumbnails/`) and matches ROMs to it on your PC. Only the system's name is sent; your
//! ROMs and their names aren't. The images are then loaded from raw.githubusercontent.com.

use crate::{net, state};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const API: &str = "https://api.github.com/repos/libretro-thumbnails";
const RAW: &str = "https://raw.githubusercontent.com/libretro-thumbnails";
const WEEK: u64 = 7 * 24 * 3600;

/// ES-DE system folder -> libretro-thumbnails repository.
const REPOS: &[(&str, &str)] = &[
    ("nes", "Nintendo_-_Nintendo_Entertainment_System"),
    ("famicom", "Nintendo_-_Nintendo_Entertainment_System"),
    ("fds", "Nintendo_-_Family_Computer_Disk_System"),
    ("snes", "Nintendo_-_Super_Nintendo_Entertainment_System"),
    ("sfc", "Nintendo_-_Super_Nintendo_Entertainment_System"),
    ("n64", "Nintendo_-_Nintendo_64"),
    ("gb", "Nintendo_-_Game_Boy"),
    ("gbc", "Nintendo_-_Game_Boy_Color"),
    ("gba", "Nintendo_-_Game_Boy_Advance"),
    ("nds", "Nintendo_-_Nintendo_DS"),
    ("n3ds", "Nintendo_-_Nintendo_3DS"),
    ("gc", "Nintendo_-_GameCube"),
    ("wii", "Nintendo_-_Wii"),
    ("wiiu", "Nintendo_-_Wii_U"),
    ("virtualboy", "Nintendo_-_Virtual_Boy"),
    ("psx", "Sony_-_PlayStation"),
    ("ps2", "Sony_-_PlayStation_2"),
    ("ps3", "Sony_-_PlayStation_3"),
    ("psp", "Sony_-_PlayStation_Portable"),
    ("psvita", "Sony_-_PlayStation_Vita"),
    ("genesis", "Sega_-_Mega_Drive_-_Genesis"),
    ("megadrive", "Sega_-_Mega_Drive_-_Genesis"),
    ("mastersystem", "Sega_-_Master_System_-_Mark_III"),
    ("gamegear", "Sega_-_Game_Gear"),
    ("segacd", "Sega_-_Mega-CD_-_Sega_CD"),
    ("megacd", "Sega_-_Mega-CD_-_Sega_CD"),
    ("sega32x", "Sega_-_32X"),
    ("saturn", "Sega_-_Saturn"),
    ("dreamcast", "Sega_-_Dreamcast"),
    ("sg-1000", "Sega_-_SG-1000"),
    ("pcengine", "NEC_-_PC_Engine_-_TurboGrafx_16"),
    ("tg16", "NEC_-_PC_Engine_-_TurboGrafx_16"),
    ("pcenginecd", "NEC_-_PC_Engine_CD_-_TurboGrafx-CD"),
    ("atari2600", "Atari_-_2600"),
    ("atari5200", "Atari_-_5200"),
    ("atari7800", "Atari_-_7800"),
    ("atarijaguar", "Atari_-_Jaguar"),
    ("atarilynx", "Atari_-_Lynx"),
    ("ngp", "SNK_-_Neo_Geo_Pocket"),
    ("ngpc", "SNK_-_Neo_Geo_Pocket_Color"),
    ("neogeocd", "SNK_-_Neo_Geo_CD"),
    ("wonderswan", "Bandai_-_WonderSwan"),
    ("wonderswancolor", "Bandai_-_WonderSwan_Color"),
    ("xbox", "Microsoft_-_Xbox"),
    ("xbox360", "Microsoft_-_Xbox_360"),
    ("3do", "The_3DO_Company_-_3DO"),
    ("msx", "Microsoft_-_MSX"),
    ("arcade", "MAME"),
    ("mame", "MAME"),
];

pub fn repo_for(system: &str) -> Option<&'static str> {
    REPOS.iter().find(|(s, _)| *s == system).map(|(_, r)| *r)
}

#[derive(Serialize, Deserialize, Default)]
struct List {
    fetched: u64,
    names: Vec<String>,
}

fn dir() -> PathBuf {
    state::data_dir().join("libretro-thumbnails")
}

fn list_file(repo: &str) -> PathBuf {
    dir().join(format!("{repo}.json"))
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// libretro's file-name rule: these characters become `_` in thumbnail names.
pub fn thumb_name(name: &str) -> String {
    name.chars().map(|c| if "&*/:`<>?\\|\"".contains(c) { '_' } else { c }).collect()
}

/// Lower-case words without the bracketed tags, "the" or punctuation: "Legend of Zelda, The (USA)" and
/// "The Legend of Zelda" both become "legend of zelda".
pub fn norm(title: &str) -> String {
    super::retroachievements::norm(&title.replace('_', " "))
}

/// Prefer English releases, then the name with the fewest tags.
fn rank(name: &str) -> (u8, usize) {
    let region = if name.contains("(USA") || name.contains("USA)") || name.contains(", USA") {
        0
    } else if name.contains("(World") {
        1
    } else if name.contains("Europe") {
        2
    } else {
        3
    };
    (region, name.len())
}

/// A system's thumbnail names, indexed for matching. Built from the cache on first use.
struct Index {
    exact: std::collections::HashSet<String>,
    by_norm: HashMap<String, String>,
}

fn index_of(names: &[String]) -> Index {
    let mut by_norm: HashMap<String, String> = HashMap::new();
    for n in names {
        let key = norm(n);
        if key.is_empty() {
            continue;
        }
        match by_norm.get(&key) {
            Some(cur) if rank(cur) <= rank(n) => {}
            _ => {
                by_norm.insert(key, n.clone());
            }
        }
    }
    Index { exact: names.iter().cloned().collect(), by_norm }
}

static INDEXES: Mutex<Option<HashMap<String, Index>>> = Mutex::new(None);

fn cached_names(repo: &str) -> Vec<String> {
    fs::read_to_string(list_file(repo)).ok().and_then(|t| serde_json::from_str::<List>(&t).ok()).map(|l| l.names).unwrap_or_default()
}

/// Forget the in-memory indexes (after the lists were refreshed).
fn reset() {
    *INDEXES.lock().unwrap() = None;
}

fn url(repo: &str, name: &str) -> String {
    format!("{RAW}/{repo}/HEAD/Named_Boxarts/{}.png", urlencoding::encode(name))
}

/// Box art for a ROM, from the cached list only (no network): the ROM's own file name first (No-Intro
/// sets match exactly), then each title given, by name.
pub fn cover(system: &str, file_stem: &str, titles: &[&str]) -> Option<String> {
    let repo = repo_for(system)?;
    let mut guard = INDEXES.lock().unwrap();
    let all = guard.get_or_insert_with(HashMap::new);
    let idx = all.entry(repo.to_string()).or_insert_with(|| index_of(&cached_names(repo)));
    if idx.exact.is_empty() {
        return None;
    }
    let stem = thumb_name(file_stem);
    if idx.exact.contains(&stem) {
        return Some(url(repo, &stem));
    }
    for t in std::iter::once(file_stem).chain(titles.iter().copied()) {
        // Older sets say "Disney's Finding Nemo" where No-Intro says "Finding Nemo".
        for t in [Some(t), t.strip_prefix("Disney's ")].into_iter().flatten() {
            if let Some(n) = idx.by_norm.get(&norm(t)) {
                return Some(url(repo, n));
            }
        }
    }
    None
}

async fn github(url: &str) -> Result<Value, String> {
    let r = net::client().get(url).header("Accept", "application/vnd.github+json").send().await.map_err(net::err)?;
    let status = r.status();
    if status.as_u16() == 403 || status.as_u16() == 429 {
        return Err("GitHub is limiting requests; libretro box art will be tried again later".into());
    }
    if !status.is_success() {
        return Err(format!("GitHub answered {status}"));
    }
    r.json().await.map_err(net::err)
}

/// File names in a tree listing: `.png` files only, without symlinks (raw GitHub would send the link's
/// text instead of an image).
fn png_names(tree: &Value) -> Vec<String> {
    tree["tree"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter(|i| i["type"] == "blob" && i["mode"] != "120000")
                .filter_map(|i| i["path"].as_str()?.strip_suffix(".png").map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

async fn fetch_names(repo: &str) -> Result<Vec<String>, String> {
    // The whole tree is too big for GitHub's API on large systems, so: the top folder, then only the
    // Named_Boxarts folder in it.
    let top = github(&format!("{API}/{repo}/git/trees/HEAD")).await?;
    let sha = top["tree"]
        .as_array()
        .and_then(|t| t.iter().find(|i| i["path"] == "Named_Boxarts"))
        .and_then(|i| i["sha"].as_str())
        .ok_or("no Named_Boxarts folder")?
        .to_string();
    let boxes = github(&format!("{API}/{repo}/git/trees/{sha}")).await?;
    Ok(png_names(&boxes))
}

/// Refresh the lists for these systems if they are over a week old. Returns how many were fetched.
pub async fn refresh(systems: &[String]) -> usize {
    let mut repos: Vec<&str> = systems.iter().filter_map(|s| repo_for(s)).collect();
    repos.sort_unstable();
    repos.dedup();
    let _ = fs::create_dir_all(dir());
    let mut fetched = 0;
    for repo in repos {
        let fresh = fs::read_to_string(list_file(repo))
            .ok()
            .and_then(|t| serde_json::from_str::<List>(&t).ok())
            .is_some_and(|l| now().saturating_sub(l.fetched) < WEEK);
        if fresh {
            continue;
        }
        match fetch_names(repo).await {
            Ok(names) if !names.is_empty() => {
                let list = List { fetched: now(), names };
                if let Ok(t) = serde_json::to_string(&list) {
                    let _ = fs::write(list_file(repo), t);
                    fetched += 1;
                }
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("libretro thumbnails ({repo}): {e}");
                if e.contains("limiting") {
                    break;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    if fetched > 0 {
        reset();
    }
    fetched
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn names_and_ranks_like_libretro() {
        assert_eq!(thumb_name("Sonic & Knuckles (World)"), "Sonic _ Knuckles (World)");
        assert_eq!(thumb_name("Mega Man X: Command Mission"), "Mega Man X_ Command Mission");
        assert_eq!(norm("Legend of Zelda, The - A Link to the Past (USA)"), norm("The Legend of Zelda - A Link to the Past"));
        let idx = index_of(&["Aladdin (Japan)".into(), "Aladdin (USA)".into(), "Aladdin (Europe)".into()]);
        assert_eq!(idx.by_norm.get("aladdin").unwrap(), "Aladdin (USA)");
    }

    #[test]
    fn lists_pngs_without_symlinks() {
        let tree = json!({"tree": [
            {"path": "Aladdin (USA).png", "type": "blob", "mode": "100644"},
            {"path": "Aladdin (USA, Europe).png", "type": "blob", "mode": "120000"},
            {"path": "notes.txt", "type": "blob", "mode": "100644"},
            {"path": "sub", "type": "tree", "mode": "040000"}
        ]});
        assert_eq!(png_names(&tree), vec!["Aladdin (USA)".to_string()]);
        assert_eq!(
            url("Sega_-_Mega_Drive_-_Genesis", "Aladdin (USA)"),
            "https://raw.githubusercontent.com/libretro-thumbnails/Sega_-_Mega_Drive_-_Genesis/HEAD/Named_Boxarts/Aladdin%20%28USA%29.png"
        );
    }

    /// Real lists from GitHub for your systems, and how many ROMs gain a cover:
    /// `cargo test --release live_thumbs -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_thumbs() {
        let count = |g: &[crate::model::Game]| g.iter().filter(|g| g.cover.is_some()).count();
        let before = crate::stores::emulation::scan();
        let systems: Vec<String> = crate::stores::emulation::systems().iter().map(|s| s.id.clone()).collect();
        let fetched = tauri::async_runtime::block_on(refresh(&systems));
        let after = crate::stores::emulation::scan();
        let from_libretro = after.iter().filter(|g| g.cover.as_deref().is_some_and(|c| c.starts_with(RAW))).count();
        println!("{} ROMs; lists fetched {fetched}; covers {} -> {} ({from_libretro} from libretro)", after.len(), count(&before), count(&after));
        for g in after.iter().filter(|g| g.cover.is_none()).take(15) {
            println!("  no cover: [{}] {}", g.system.as_deref().unwrap_or(""), g.title);
        }
    }
}
