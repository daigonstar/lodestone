//! RetroAchievements (retroachievements.org): names, box art and details for your ROMs, and the
//! achievements for each game with your progress. Needs your RA username and Web API key (Settings).
//!
//! How ROMs are matched, following RA's docs (docs.retroachievements.org, "Game identification"):
//! - RA's game list for each system, with every ROM fingerprint (MD5) it knows, is downloaded once and
//!   kept for a week.
//! - Each ROM is fingerprinted the way RA does it for its system (whole-file MD5, with a few header and
//!   byte-order rules; arcade games by file name) and looked up in that list. Fingerprints are cached by
//!   path, size and date, so a file is only read once. ROMs or systems Lodestone can't fingerprint yet
//!   (disc systems, .7z) are matched by name.
//! - Everything matches on your PC: no ROM or fingerprint is sent to RA. RA only gets your username and
//!   key, with requests for its game lists, a game's details and your progress.
//!
//! RA is preferred for ROMs: a matched game takes RA's title and box art; ES-DE's are the fallback.

use crate::{net, settings, state};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const API: &str = "https://retroachievements.org/API";
pub const MEDIA: &str = "https://media.retroachievements.org";

/// ES-DE system folder -> RetroAchievements console ID.
const CONSOLES: &[(&str, u32)] = &[
    ("megadrive", 1), ("genesis", 1), ("megadrivejp", 1), ("n64", 2), ("snes", 3), ("sfc", 3), ("snesna", 3), ("gb", 4),
    ("gba", 5), ("gbc", 6), ("nes", 7), ("famicom", 7), ("fds", 7), ("pcengine", 8), ("tg16", 8), ("segacd", 9), ("megacd", 9),
    ("megacdjp", 9), ("sega32x", 10), ("sega32xjp", 10), ("sega32xna", 10), ("mastersystem", 11), ("mark3", 11), ("psx", 12),
    ("atarilynx", 13), ("ngp", 14), ("ngpc", 14), ("gamegear", 15), ("gc", 16), ("atarijaguar", 17), ("nds", 18), ("ps2", 21),
    ("odyssey2", 23), ("pokemini", 24), ("atari2600", 25), ("arcade", 27), ("mame", 27), ("fbneo", 27), ("cps1", 27),
    ("cps2", 27), ("cps3", 27), ("neogeo", 27), ("virtualboy", 28), ("msx", 29), ("msx1", 29), ("msx2", 29), ("sg-1000", 33),
    ("amstradcpc", 37), ("apple2", 38), ("saturn", 39), ("saturnjp", 39), ("dreamcast", 40), ("psp", 41), ("3do", 43),
    ("colecovision", 44), ("intellivision", 45), ("vectrex", 46), ("pcfx", 49), ("atari7800", 51), ("wonderswan", 53),
    ("wonderswancolor", 53), ("neogeocd", 56), ("channelf", 57), ("supervision", 63), ("megaduck", 69), ("arduboy", 71),
    ("wasm4", 72), ("pcenginecd", 76), ("tg-cd", 76),
];

pub fn console_for(system: &str) -> Option<u32> {
    CONSOLES.iter().find(|c| c.0 == system).map(|c| c.1)
}

// ---------------- account ----------------

fn creds() -> Option<(String, String)> {
    let s = settings::load();
    let (u, k) = (s.ra_user.trim().to_string(), s.ra_key.trim().to_string());
    (!u.is_empty() && !k.is_empty()).then_some((u, k))
}

pub fn signed_in() -> bool {
    creds().is_some()
}

async fn get(endpoint: &str, params: &[(&str, String)]) -> Result<Value, String> {
    let (_, key) = creds().ok_or("Add your RetroAchievements username and Web API key in Settings")?;
    let mut url = format!("{API}/{endpoint}.php?y={}", urlencoding::encode(&key));
    for (k, v) in params {
        url.push_str(&format!("&{k}={}", urlencoding::encode(v)));
    }
    let r = net::client().get(&url).send().await.map_err(net::err)?;
    let status = r.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err("RetroAchievements didn't accept the username or Web API key".into());
    }
    if status.as_u16() == 429 {
        return Err("RetroAchievements is limiting requests; try again in a minute".into());
    }
    if !status.is_success() {
        return Err(format!("RetroAchievements answered {status}"));
    }
    r.json().await.map_err(net::err)
}

/// A field in either of the API's spellings: "ImageBoxArt" or "imageBoxArt", "ID" or "id".
fn field<'a>(v: &'a Value, name: &str) -> &'a Value {
    if !v[name].is_null() {
        return &v[name];
    }
    let mut c = name.chars();
    let camel: String = c.next().map(|f| f.to_lowercase().chain(c).collect()).unwrap_or_default();
    if !v[camel.as_str()].is_null() {
        return &v[camel.as_str()];
    }
    &v[name.to_ascii_lowercase().as_str()]
}

fn s(v: &Value, name: &str) -> String {
    match field(v, name) {
        Value::String(x) => x.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

fn n(v: &Value, name: &str) -> u64 {
    let f = field(v, name);
    f.as_u64().or_else(|| f.as_str().and_then(|x| x.parse().ok())).unwrap_or(0)
}

// ---------------- game lists ----------------

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct RaGame {
    pub id: u64,
    pub title: String,
    pub icon: String,
    pub achievements: u64,
    pub points: u64,
    pub hashes: Vec<String>,
}

fn dir() -> PathBuf {
    state::data_dir().join("retroachievements")
}

/// Create the cache folder readable only by you: it holds your achievement progress.
fn ensure_dir(d: &Path) -> std::io::Result<()> {
    fs::create_dir_all(d)?;
    crate::os::set_private(&dir(), true);
    Ok(())
}

fn list_file(console: u32) -> PathBuf {
    dir().join(format!("console-{console}.json"))
}

fn parse_list(v: &Value) -> Vec<RaGame> {
    v.as_array()
        .into_iter()
        .flatten()
        .map(|g| RaGame {
            id: n(g, "ID"),
            title: s(g, "Title"),
            icon: s(g, "ImageIcon"),
            achievements: n(g, "NumAchievements"),
            points: n(g, "Points"),
            hashes: field(g, "Hashes").as_array().into_iter().flatten().filter_map(|h| h.as_str().map(|x| x.to_ascii_lowercase())).collect(),
        })
        .filter(|g| g.id > 0)
        .collect()
}

/// The cached game list for a console (no network).
pub fn cached_list(console: u32) -> Vec<RaGame> {
    static C: Mutex<Option<HashMap<u32, (std::time::SystemTime, Vec<RaGame>)>>> = Mutex::new(None);
    let f = list_file(console);
    let modified = fs::metadata(&f).and_then(|m| m.modified()).ok();
    let mut g = C.lock().unwrap();
    let cache = g.get_or_insert_with(HashMap::new);
    if let (Some(m), Some((at, list))) = (modified, cache.get(&console)) {
        if *at == m {
            return list.clone();
        }
    }
    let list: Vec<RaGame> = fs::read_to_string(&f).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    if let Some(m) = modified {
        cache.insert(console, (m, list.clone()));
    }
    list
}

/// Download the game lists (with fingerprints) for these consoles, if older than a week.
pub async fn refresh_lists(consoles: &[u32]) -> Result<usize, String> {
    ensure_dir(&dir()).map_err(|e| e.to_string())?;
    let mut fetched = 0;
    for c in consoles {
        let f = list_file(*c);
        let fresh = fs::metadata(&f).and_then(|m| m.modified()).ok().and_then(|m| m.elapsed().ok()).is_some_and(|age| age.as_secs() < 7 * 86400);
        if fresh {
            continue;
        }
        let v = get("API_GetGameList", &[("i", c.to_string()), ("h", "1".into()), ("f", "1".into())]).await?;
        let list = parse_list(&v);
        fs::write(&f, serde_json::to_string(&list).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        fetched += 1;
        // Be gentle with RA (their docs ask clients not to hammer the API).
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    Ok(fetched)
}

// ---------------- fingerprints (RA's hashing rules) ----------------

/// How RA fingerprints a system's ROMs.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Method {
    WholeFile,
    /// Skip a header of this many bytes if the file starts with the magic bytes.
    Header(&'static [u8], usize),
    /// SNES / PC Engine: skip 512 bytes if the size is 512 more than a multiple of this.
    Copier(u64),
    N64,
    /// Arcade: the file name without its extension.
    FileName,
}

fn method(system: &str) -> Option<Method> {
    Some(match system {
        "nes" | "famicom" => Method::Header(b"NES\x1a", 16),
        "fds" => Method::Header(b"FDS\x1a", 16),
        "snes" | "sfc" | "snesna" => Method::Copier(8192),
        "pcengine" | "tg16" => Method::Copier(131072),
        "atari7800" => Method::Header(b"\x01ATARI7800", 128),
        "atarilynx" => Method::Header(b"LYNX\0", 64),
        "n64" => Method::N64,
        "arcade" | "mame" | "fbneo" | "cps1" | "cps2" | "cps3" | "neogeo" => Method::FileName,
        "gb" | "gbc" | "gba" | "megadrive" | "genesis" | "megadrivejp" | "mastersystem" | "mark3" | "gamegear" | "sega32x" | "sega32xjp" | "sega32xna" | "atari2600"
        | "virtualboy" | "ngp" | "ngpc" | "wonderswan" | "wonderswancolor" | "colecovision" | "intellivision" | "vectrex" | "sg-1000" | "pokemini" | "supervision"
        | "megaduck" | "arduboy" | "wasm4" | "atarijaguar" | "channelf" | "odyssey2" => Method::WholeFile,
        _ => return None,
    })
}

fn md5_hex(bytes: &[u8]) -> String {
    use md5::{Digest, Md5};
    Md5::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// RA's fingerprint of a ROM's contents for a system (after unzipping, if it was zipped).
fn fingerprint_bytes(m: Method, data: &[u8]) -> String {
    match m {
        Method::WholeFile | Method::FileName => md5_hex(data),
        Method::Header(magic, skip) => md5_hex(if data.starts_with(magic) && data.len() > skip { &data[skip..] } else { data }),
        Method::Copier(unit) => md5_hex(if data.len() as u64 % unit == 512 { &data[512..] } else { data }),
        Method::N64 => {
            // .v64 (byte-swapped) and .n64 (little-endian) become .z64 (big-endian) first.
            let mut d = data.to_vec();
            match d.get(..4) {
                Some([0x37, 0x80, 0x40, 0x12]) => d.chunks_exact_mut(2).for_each(|c| c.swap(0, 1)),
                Some([0x40, 0x12, 0x37, 0x80]) => d.chunks_exact_mut(4).for_each(|c| c.reverse()),
                _ => {}
            }
            md5_hex(&d)
        }
    }
}

/// A ROM's RA fingerprint, or None when Lodestone can't compute it (unknown system, .7z, too big).
pub fn fingerprint(system: &str, path: &Path) -> Option<String> {
    let m = method(system)?;
    if m == Method::FileName {
        let stem = path.file_stem()?.to_string_lossy().into_owned();
        return Some(md5_hex(stem.as_bytes()));
    }
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > 256 << 20 {
        return None;
    }
    let lower = path.to_string_lossy().to_ascii_lowercase();
    let data = if lower.ends_with(".zip") {
        // Zipped cartridge ROMs: the single game file inside.
        let f = fs::File::open(path).ok()?;
        let mut z = zip::ZipArchive::new(f).ok()?;
        let i = (0..z.len()).find(|i| z.by_index(*i).is_ok_and(|e| e.is_file()))?;
        let mut e = z.by_index(i).ok()?;
        let mut out = Vec::new();
        e.by_ref().take(256 << 20).read_to_end(&mut out).ok()?;
        out
    } else if lower.ends_with(".7z") {
        return None;
    } else {
        fs::read(path).ok()?
    };
    Some(fingerprint_bytes(m, &data))
}

#[derive(Serialize, Deserialize, Default)]
struct HashCache {
    /// path -> (size, modified seconds, fingerprint)
    files: HashMap<String, (u64, u64, String)>,
}

static HASHES: Mutex<Option<HashCache>> = Mutex::new(None);

fn hash_file() -> PathBuf {
    dir().join("fingerprints.json")
}

fn file_key(path: &Path) -> Option<(String, u64, u64)> {
    let m = fs::metadata(path).ok()?;
    let t = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    Some((path.to_string_lossy().into_owned(), m.len(), t))
}

/// A cached fingerprint (no file reading).
pub fn cached_fingerprint(path: &Path) -> Option<String> {
    let (k, size, t) = file_key(path)?;
    let mut g = HASHES.lock().unwrap();
    let c = g.get_or_insert_with(|| fs::read_to_string(hash_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default());
    c.files.get(&k).filter(|e| e.0 == size && e.1 == t).map(|e| e.2.clone())
}

/// Fingerprint the given ROMs that aren't cached yet (reads them; run in the background).
pub fn fingerprint_all(roms: &[(String, PathBuf)]) -> usize {
    let mut done = 0;
    for (system, path) in roms {
        if cached_fingerprint(path).is_some() {
            continue;
        }
        let (Some(fp), Some((k, size, t))) = (fingerprint(system, path), file_key(path)) else { continue };
        let mut g = HASHES.lock().unwrap();
        g.get_or_insert_with(HashCache::default).files.insert(k, (size, t, fp));
        done += 1;
        if done % 200 == 0 {
            save_hashes(g.as_ref().unwrap());
        }
    }
    if let Some(c) = HASHES.lock().unwrap().as_ref() {
        save_hashes(c);
    }
    done
}

fn save_hashes(c: &HashCache) {
    let _ = ensure_dir(&dir());
    let _ = fs::write(hash_file(), serde_json::to_string(c).unwrap_or_default());
}

// ---------------- matching ----------------

/// "Legend of Zelda, The: A Link to the Past" and "The Legend of Zelda - A Link to the Past (USA)"
/// both become "legend of zelda a link to the past".
pub fn norm(title: &str) -> String {
    let mut out = String::new();
    let mut depth = 0i32;
    for c in title.to_lowercase().chars() {
        match c {
            '(' | '[' | '~' if depth == 0 => depth += 1,
            '(' | '[' => depth += 1,
            ')' | ']' => depth = (depth - 1).max(0),
            '~' => depth = (depth - 1).max(0),
            _ if depth == 0 => out.push(if c.is_alphanumeric() { c } else { ' ' }),
            _ => {}
        }
    }
    let words: Vec<&str> = out.split_whitespace().filter(|w| *w != "the").collect();
    words.join(" ")
}

/// "Legend of Zelda, The: A Link to the Past" -> "The Legend of Zelda: A Link to the Past"
pub fn display_title(ra: &str) -> String {
    if let Some(i) = ra.find(", The") {
        let rest = &ra[i + 5..];
        if rest.is_empty() || rest.starts_with(':') || rest.starts_with(" -") {
            return format!("The {}{}", &ra[..i], rest);
        }
    }
    ra.to_string()
}

/// Match a ROM to an RA game: by its cached fingerprint first, then by name (skipping RA's hacks,
/// homebrew and subsets, which a plain name shouldn't match).
pub fn match_rom(system: &str, path: &Path, title: &str) -> Option<RaGame> {
    let console = console_for(system)?;
    let list = cached_list(console);
    if list.is_empty() {
        return None;
    }
    if let Some(fp) = cached_fingerprint(path) {
        if let Some(g) = list.iter().find(|g| g.hashes.contains(&fp)) {
            return Some(g.clone());
        }
    }
    let want = norm(title);
    if want.is_empty() {
        return None;
    }
    list.iter().filter(|g| !g.title.starts_with('~') && !g.title.contains("[Subset")).find(|g| norm(&g.title) == want).cloned()
}

// ---------------- a game's details, achievements and your progress ----------------

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Achievement {
    pub id: u64,
    pub title: String,
    pub description: String,
    pub points: u64,
    pub badge: String,
    /// When you earned it (softcore / hardcore), "" if not.
    pub earned: String,
    pub earned_hardcore: String,
    pub display_order: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct GameInfo {
    pub id: u64,
    pub title: String,
    pub console: String,
    pub developer: String,
    pub publisher: String,
    pub genre: String,
    pub released: String,
    pub box_art: String,
    pub title_image: String,
    pub ingame_image: String,
    pub achievements: Vec<Achievement>,
    pub earned: usize,
    pub earned_hardcore: usize,
    pub points_total: u64,
    pub points_earned: u64,
    pub url: String,
}

fn media(path: &str) -> String {
    if path.is_empty() || path.starts_with("http") { path.to_string() } else { format!("{MEDIA}{path}") }
}

fn parse_game(v: &Value) -> GameInfo {
    let mut achievements: Vec<Achievement> = match field(v, "Achievements") {
        Value::Object(m) => m.values().cloned().collect(),
        Value::Array(a) => a.clone(),
        _ => vec![],
    }
    .iter()
    .map(|a| Achievement {
        id: n(a, "ID"),
        title: s(a, "Title"),
        description: s(a, "Description"),
        points: n(a, "Points"),
        badge: s(a, "BadgeName"),
        earned: s(a, "DateEarned"),
        earned_hardcore: s(a, "DateEarnedHardcore"),
        display_order: n(a, "DisplayOrder") as i64,
    })
    .collect();
    achievements.sort_by_key(|a| (a.display_order, a.id));
    let earned = achievements.iter().filter(|a| !a.earned.is_empty() || !a.earned_hardcore.is_empty()).count();
    let earned_hardcore = achievements.iter().filter(|a| !a.earned_hardcore.is_empty()).count();
    let id = n(v, "ID");
    GameInfo {
        id,
        title: display_title(&s(v, "Title")),
        console: s(v, "ConsoleName"),
        developer: s(v, "Developer"),
        publisher: s(v, "Publisher"),
        genre: s(v, "Genre"),
        released: s(v, "Released"),
        box_art: media(&s(v, "ImageBoxArt")),
        title_image: media(&s(v, "ImageTitle")),
        ingame_image: media(&s(v, "ImageIngame")),
        points_total: achievements.iter().map(|a| a.points).sum(),
        points_earned: achievements.iter().filter(|a| !a.earned.is_empty() || !a.earned_hardcore.is_empty()).map(|a| a.points).sum(),
        earned,
        earned_hardcore,
        achievements,
        url: format!("https://retroachievements.org/game/{id}"),
    }
}

fn info_file(id: u64) -> PathBuf {
    dir().join("games").join(format!("{id}.json"))
}

/// A game's details with your progress: cached for 5 minutes (progress changes as you play).
pub async fn game_info(id: u64) -> Result<GameInfo, String> {
    let f = info_file(id);
    if let Some(i) = fs::metadata(&f).and_then(|m| m.modified()).ok().and_then(|m| m.elapsed().ok()).filter(|a| a.as_secs() < 300).and_then(|_| fs::read_to_string(&f).ok()).and_then(|t| serde_json::from_str::<GameInfo>(&t).ok()) {
        return Ok(i);
    }
    let (user, _) = creds().ok_or("Add your RetroAchievements username and Web API key in Settings")?;
    let v = get("API_GetGameInfoAndUserProgress", &[("g", id.to_string()), ("u", user)]).await?;
    let info = parse_game(&v);
    let _ = ensure_dir(f.parent().unwrap());
    let _ = fs::write(&f, serde_json::to_string(&info).unwrap_or_default());
    Ok(info)
}

/// Box art already fetched for a game (no network), for the library.
pub fn cached_box_art(id: u64) -> Option<String> {
    fs::read_to_string(info_file(id)).ok().and_then(|t| serde_json::from_str::<GameInfo>(&t).ok()).map(|i| i.box_art).filter(|b| !b.is_empty())
}

/// Fetch details (box art) for matched games that don't have them yet, slowly. Returns how many.
/// Games whose details failed are skipped until Lodestone restarts, so they aren't retried in a loop.
pub async fn fetch_missing_art(ids: Vec<u64>, max: usize) -> usize {
    static FAILED: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());
    let mut n = 0;
    let todo: Vec<u64> = {
        let failed = FAILED.lock().unwrap();
        ids.into_iter().filter(|id| !failed.contains(id) && !info_file(*id).is_file()).take(max).collect()
    };
    for id in todo {
        if game_info(id).await.is_ok() {
            n += 1;
        } else {
            FAILED.lock().unwrap().push(id);
        }
        tokio::time::sleep(std::time::Duration::from_millis(700)).await;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fingerprints_like_retroachievements() {
        // NES: a 16-byte iNES header is skipped.
        let mut nes = b"NES\x1a".to_vec();
        nes.extend([0u8; 12]);
        nes.extend(b"PRG");
        assert_eq!(fingerprint_bytes(Method::Header(b"NES\x1a", 16), &nes), md5_hex(b"PRG"));
        // SNES: a 512-byte copier header is skipped only when the size says there is one.
        let mut snes = vec![0xAAu8; 512];
        snes.extend(vec![1u8; 8192]);
        assert_eq!(fingerprint_bytes(Method::Copier(8192), &snes), md5_hex(&vec![1u8; 8192]));
        assert_eq!(fingerprint_bytes(Method::Copier(8192), &vec![1u8; 8192]), md5_hex(&vec![1u8; 8192]));
        // N64: byte-swapped (.v64) and little-endian (.n64) hash the same as big-endian (.z64).
        let z64 = [0x80, 0x37, 0x12, 0x40, 1, 2, 3, 4];
        let v64 = [0x37, 0x80, 0x40, 0x12, 2, 1, 4, 3];
        let n64 = [0x40, 0x12, 0x37, 0x80, 4, 3, 2, 1];
        let want = md5_hex(&z64);
        assert_eq!(fingerprint_bytes(Method::N64, &v64), want);
        assert_eq!(fingerprint_bytes(Method::N64, &n64), want);
        assert_eq!(fingerprint_bytes(Method::N64, &z64), want);
        // Arcade: the file name without its extension, not the contents.
        assert_eq!(fingerprint("mame", Path::new("/roms/mame/galaga.zip")), Some(md5_hex(b"galaga")));
    }

    #[test]
    fn matches_names_and_reads_both_spellings() {
        assert_eq!(norm("Legend of Zelda, The: A Link to the Past"), norm("The Legend of Zelda - A Link to the Past (USA)"));
        assert_eq!(norm("Pokemon - Emerald Version (USA, Europe)"), "pokemon emerald version");
        assert_eq!(norm("~Hack~ Super Mario World"), "super mario world");
        assert_eq!(display_title("Legend of Zelda, The: A Link to the Past"), "The Legend of Zelda: A Link to the Past");
        let pascal = json!([{"ID": 3684, "Title": "Gley Lancer", "ImageIcon": "/Images/020895.png", "NumAchievements": 44, "Points": 595, "Hashes": ["8BD4A97783CDA077C342173DF0A9B51E"]}]);
        let camel = json!([{"id": 3684, "title": "Gley Lancer", "imageIcon": "/Images/020895.png", "numAchievements": 44, "points": 595, "hashes": ["8bd4a97783cda077c342173df0a9b51e"]}]);
        assert_eq!(parse_list(&pascal), parse_list(&camel));
        assert_eq!(parse_list(&pascal)[0].hashes, ["8bd4a97783cda077c342173df0a9b51e"]);
    }

    #[test]
    fn reads_game_progress() {
        let v = json!({"ID": 1, "Title": "Sonic the Hedgehog", "ConsoleName": "Mega Drive", "Developer": "Sonic Team", "ImageBoxArt": "/Images/051872.png",
            "Achievements": {
                "9": {"ID": 9, "Title": "Speed", "Description": "Go fast", "Points": 10, "BadgeName": "250336", "DisplayOrder": 2, "DateEarned": "2024-01-01 10:00:00", "DateEarnedHardcore": "2024-01-01 10:00:00"},
                "5": {"ID": 5, "Title": "Rings", "Description": "Collect", "Points": 5, "BadgeName": "250335", "DisplayOrder": 1}
            }});
        let g = parse_game(&v);
        assert_eq!(g.achievements.iter().map(|a| a.id).collect::<Vec<_>>(), [5, 9]);
        assert_eq!((g.earned, g.earned_hardcore, g.points_earned, g.points_total), (1, 1, 10, 15));
        assert_eq!(g.box_art, "https://media.retroachievements.org/Images/051872.png");
        assert_eq!(g.url, "https://retroachievements.org/game/1");
    }
}

#[cfg(test)]
mod live {
    /// `cargo test live_fingerprints -- --ignored --nocapture`: fingerprint every ROM (cached afterwards).
    #[test]
    #[ignore]
    fn live_fingerprints() {
        let _ = crate::stores::emulation::scan();
        let roms = crate::stores::emulation::roms();
        let t = std::time::Instant::now();
        let n = super::fingerprint_all(&roms);
        let with: usize = roms.iter().filter(|(_, p)| super::cached_fingerprint(p).is_some()).count();
        println!("{} ROMs, {n} fingerprinted now, {with} have a fingerprint, in {:?}", roms.len(), t.elapsed());
        for (s, p) in roms.iter().filter(|(_, p)| super::cached_fingerprint(p).is_some()).take(3) {
            println!("{s}: {} -> {}", p.file_name().unwrap().to_string_lossy(), super::cached_fingerprint(p).unwrap());
        }
        let t = std::time::Instant::now();
        let n2 = super::fingerprint_all(&roms);
        println!("second run: {n2} new in {:?}", t.elapsed());
    }
}
