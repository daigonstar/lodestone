//! Battle.net's own local data, read offline from its prefix:
//! - the product catalog it caches as JSON fragments (names, launch codes, artwork, licence rules)
//! - the signed-in account's licences (CachedData.db), which tell us which games you own.

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

/// Games every Battle.net account can install without buying them.
pub const FREE_TO_PLAY: &[&str] = &["Pro", "WTCG", "Hero", "ANBS", "GRY", "S2"];

#[derive(Debug, Clone, Default)]
pub struct Product {
    /// Launch code used by `--exec="launch <code>"` and `battlenet://<code>` (e.g. "Fen", "OSI").
    pub code: String,
    pub name: String,
    pub cover: Option<PathBuf>,
    pub hero: Option<PathBuf>,
    pub licenses: HashSet<u64>,
    /// `.build.info` product ids this game installs as (e.g. "fenris", "pro", "hsb").
    pub tact: HashSet<String>,
    pub genres: Vec<String>,
}

/// "default#GENRE_ACTION_RPG" -> ["Action", "RPG"].
fn genres_from_key(key: &str) -> Vec<String> {
    let k = key.rsplit('#').next().unwrap_or("").trim_start_matches("GENRE_").to_ascii_uppercase();
    let mut v = Vec::new();
    let mut add = |g: &str| {
        if !v.iter().any(|x: &String| x == g) {
            v.push(g.to_string());
        }
    };
    if k.contains("ACTION") { add("Action"); }
    if k.contains("RPG") { add("RPG"); }
    if k.contains("STRATEGY") { add("Strategy"); }
    if k.contains("CARD") { add("Card & Board"); }
    if k.contains("MOBA") { add("MOBA"); }
    if k.contains("SHOOTER") || k.contains("TEAM_BASED") || k.contains("FPS") { add("Shooter"); }
    if k.contains("MMO") { add("MMO"); }
    v
}

fn local_battlenet(drive_c: &Path) -> Option<PathBuf> {
    let users = drive_c.join("users");
    fs::read_dir(users).ok()?.flatten().map(|e| e.path().join("AppData/Local/Battle.net")).find(|p| p.is_dir())
}

fn ints(v: &Value, out: &mut HashSet<u64>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                if k.contains("licen") {
                    match x {
                        Value::Number(n) => out.extend(n.as_u64()),
                        Value::Array(a) => out.extend(a.iter().filter_map(Value::as_u64)),
                        _ => {}
                    }
                }
                ints(x, out);
            }
        }
        Value::Array(a) => a.iter().for_each(|x| ints(x, out)),
        _ => {}
    }
}

fn cached_file(cache: &Path, hash: &str) -> Option<PathBuf> {
    if hash.len() < 4 {
        return None;
    }
    let p = cache.join(&hash[..2]).join(&hash[2..4]).join(hash);
    p.is_file().then_some(p)
}

pub fn catalog(drive_c: &Path) -> HashMap<String, Product> {
    let mut out: HashMap<String, Product> = HashMap::new();
    let Some(cache) = local_battlenet(drive_c).map(|p| p.join("Cache")) else { return out };
    let mut stack = vec![cache.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            // Catalog fragments are JSON; skip images and anything large.
            if e.metadata().map(|m| m.len() > 4_000_000).unwrap_or(true) {
                continue;
            }
            let Ok(bytes) = fs::read(&p) else { continue };
            if bytes.first() != Some(&b'{') {
                continue;
            }
            let Ok(d) = serde_json::from_slice::<Value>(&bytes) else { continue };
            let Some(products) = d["products"].as_array() else { continue };
            let strings = &d["strings"]["default"];
            let files = &d["files"]["default"];
            let art = |key: &Value| key.as_str().and_then(|k| files[k]["hash"].as_str()).and_then(|h| cached_file(&cache, h));
            for prod in products {
                let Some(code) = prod["id"].as_str() else { continue };
                let base = &prod["base"];
                let entry = out.entry(code.to_string()).or_insert_with(|| Product { code: code.to_string(), ..Default::default() });
                if entry.name.is_empty() {
                    if let Some(n) = base["name"].as_str().and_then(|k| strings[k].as_str()) {
                        entry.name = n.to_string();
                    }
                }
                if entry.cover.is_none() {
                    entry.cover = art(&base["key_art"]);
                }
                if entry.hero.is_none() {
                    entry.hero = art(&base["background"]);
                }
                if entry.genres.is_empty() {
                    entry.genres = base["genre"].as_str().map(genres_from_key).unwrap_or_default();
                    // Diablo I/II/III/IV and Immortal.
                    if ["D1", "OSI", "D3", "Fen", "ANBS"].contains(&code) {
                        entry.genres.push(crate::model::ISO_ARPG.into());
                    }
                }
                ints(&d["program_configuration"][code], &mut entry.licenses);
                // Map install ids (`.build.info` Product) to this code. A fragment can hold several products
                // (World of Warcraft: retail / classic / forever); match on default_product_type when it can.
                let ptype = base["default_product_type"].as_str().unwrap_or("");
                if let Some(installs) = d["installs"].as_object() {
                    for (uid, inst) in installs {
                        let mine = products.len() == 1 || uid == ptype || (ptype == "retail" && !products.iter().any(|o| o["base"]["default_product_type"].as_str() == Some(uid.as_str())));
                        if !mine {
                            continue;
                        }
                        if let Some(t) = inst["tact_product"].as_str() {
                            entry.tact.insert(t.to_ascii_lowercase());
                        }
                        ints(inst, &mut entry.licenses);
                    }
                }
            }
        }
    }
    out.retain(|_, p| !p.name.is_empty());
    out
}

/// Licences on the signed-in Battle.net account, or None if nobody has signed in yet.
pub fn account_licenses(drive_c: &Path) -> Option<HashSet<u64>> {
    let db = local_battlenet(drive_c)?.join("CachedData.db");
    let conn = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).ok()?;
    let txt: String = conn
        .query_row("SELECT value FROM key_value_store WHERE key = 'features_cached_data_points'", [], |r| r.get(0))
        .ok()?;
    let v: Value = serde_json::from_str(&txt).ok()?;
    if v["account_id"].as_u64().unwrap_or(0) == 0 {
        return None;
    }
    Some(v["licenses"].as_array()?.iter().filter_map(Value::as_u64).collect())
}

/// Read a Blizzard `.build.info` (pipe-separated table; header names carry `!TYPE:n`) and return its Product.
pub fn build_info_product(path: &Path) -> Option<String> {
    build_info_products(path).into_iter().next()
}

/// Every Product in a `.build.info`: World of Warcraft keeps one row per version in a shared folder.
pub fn build_info_products(path: &Path) -> Vec<String> {
    let Ok(txt) = fs::read_to_string(path) else { return vec![] };
    let mut lines = txt.lines().filter(|l| !l.trim().is_empty());
    let Some(head) = lines.next() else { return vec![] };
    let header: Vec<String> = head.split('|').map(|h| h.split('!').next().unwrap_or("").to_ascii_lowercase()).collect();
    let Some(col) = header.iter().position(|h| h == "product") else { return vec![] };
    let mut out: Vec<String> = Vec::new();
    for row in lines {
        if let Some(p) = row.split('|').nth(col).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
            if !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out
}

// ---------------- World of Warcraft ----------------

/// The playable World of Warcraft versions. Battle.net lists them as one game with a version picker,
/// and owning them comes from your WoW game account rather than a licence.
/// (install uid, product type, `.build.info` product, folder inside "World of Warcraft").
pub const WOW: &[(&str, &str, &str, &str)] = &[
    ("wow", "retail", "wow", "_retail_"),
    ("wowf", "wowf", "wowf", "_wowf_"),
    ("wow_classic", "wow_classic", "wow_classic", "_classic_"),
    ("wow_classic_era", "wow_classic_era", "wow_classic_era", "_classic_era_"),
    ("wow_classic_anniversary", "wow_classic_anniversary", "wow_anniversary", "_anniversary_"),
];

#[derive(Debug, Clone, Default)]
pub struct WowVersion {
    /// Battle.net's install id, also Lodestone's id ("battlenet:<uid>").
    pub uid: String,
    pub tact: String,
    /// Folder of this version inside the shared install (seen in the game's command line).
    pub folder: String,
    pub name: String,
    pub cover: Option<PathBuf>,
    pub hero: Option<PathBuf>,
    /// Retail first, then each Classic version, as Battle.net's version picker has them.
    pub order: usize,
}

/// Names and art from Battle.net's catalog, in Battle.net's order.
pub fn wow_versions(drive_c: &Path) -> Vec<WowVersion> {
    let mut out: Vec<WowVersion> = WOW
        .iter()
        .enumerate()
        .map(|(i, (uid, _, tact, folder))| WowVersion { uid: uid.to_string(), tact: tact.to_string(), folder: folder.to_string(), order: i, ..Default::default() })
        .collect();
    let Some(cache) = local_battlenet(drive_c).map(|p| p.join("Cache")) else { return out };
    let mut stack = vec![cache.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            if e.metadata().map(|m| m.len() > 4_000_000).unwrap_or(true) {
                continue;
            }
            let Ok(bytes) = fs::read(&p) else { continue };
            if bytes.first() != Some(&b'{') {
                continue;
            }
            let Ok(d) = serde_json::from_slice::<Value>(&bytes) else { continue };
            if d["fragment_id"].as_str() != Some("world_of_warcraft") {
                continue;
            }
            let strings = &d["strings"]["default"];
            let files = &d["files"]["default"];
            let text = |key: &Value| key.as_str().and_then(|k| strings[k].as_str()).map(String::from);
            let art = |key: &Value| key.as_str().and_then(|k| files[k]["hash"].as_str()).and_then(|h| cached_file(&cache, h));
            let products = d["products"].as_array().cloned().unwrap_or_default();
            let prod = |id: &str| products.iter().find(|x| x["id"].as_str() == Some(id)).map(|x| x["base"].clone()).unwrap_or(Value::Null);
            let (wow, classic, forever) = (prod("WoW"), prod("WoWC"), prod("WoWF"));
            for v in out.iter_mut() {
                let ptype = WOW[v.order].1;
                let t = &wow["types"][ptype];
                if v.name.is_empty() {
                    v.name = if ptype == "retail" {
                        text(&wow["name"]).unwrap_or_default()
                    } else {
                        // "World of Warcraft Classic" stays as is; "Mists of Pandaria Classic" gets the game's name
                        // in front, and "WoW: Forever" spells it out.
                        text(&t["name"])
                            .map(|n| match n.strip_prefix("WoW") {
                                Some(rest) => format!("World of Warcraft{rest}"),
                                None if n.starts_with("World of Warcraft") => n,
                                None => format!("World of Warcraft: {n}"),
                            })
                            .unwrap_or_default()
                    };
                }
                if v.cover.is_none() {
                    v.cover = match ptype {
                        "retail" => art(&wow["key_art"]),
                        "wowf" => art(&forever["key_art"]).or_else(|| art(&wow["key_art"])),
                        _ => art(&classic["key_art"]).or_else(|| art(&wow["key_art"])),
                    };
                }
                if v.hero.is_none() {
                    v.hero = art(&t["background"]).or_else(|| art(&forever["background"]).filter(|_| ptype == "wowf")).or_else(|| art(&wow["background"]));
                }
            }
        }
    }
    for v in out.iter_mut().filter(|v| v.name.is_empty()) {
        v.name = match v.uid.as_str() {
            "wow" => "World of Warcraft",
            "wowf" => "World of Warcraft: Forever",
            "wow_classic" => "World of Warcraft: Mists of Pandaria Classic",
            "wow_classic_era" => "World of Warcraft Classic",
            _ => "World of Warcraft: Burning Crusade Anniversary",
        }
        .into();
    }
    out
}

/// WoW product types the signed-in account has a game account for, from Battle.net's logs
/// (`GameAccount(WoW:wow_classic region=EU entity=… name=WoW1)`; unowned placeholders have no entity).
pub fn wow_owned(drive_c: &Path) -> HashSet<String> {
    let mut out = HashSet::new();
    let Some(logs) = local_battlenet(drive_c).map(|p| p.join("Logs")) else { return out };
    let mut files: Vec<PathBuf> = fs::read_dir(logs)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("battle.net-")))
        .collect();
    files.sort();
    // The newest few logs: enough to see the account's games without reading weeks of logs.
    for f in files.iter().rev().take(6) {
        let Ok(b) = fs::read(f) else { continue };
        out.extend(wow_owned_in(&String::from_utf8_lossy(&b)));
    }
    out
}

fn wow_owned_in(log: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut rest = log;
    while let Some(i) = rest.find("GameAccount(WoW:") {
        rest = &rest[i + "GameAccount(WoW:".len()..];
        let end = rest.find(')').unwrap_or(rest.len());
        let inner = &rest[..end];
        if inner.contains("entity=") {
            if let Some(t) = inner.split_whitespace().next() {
                out.insert(t.to_string());
            }
        }
    }
    out
}

/// Installed games by folder name (lowercase) -> product, from the Agent's aggregate.json. Installs
/// the Agent does on its own can leave .build.info's Product column empty.
pub fn aggregate_products(drive_c: &Path) -> HashMap<String, String> {
    let Ok(t) = fs::read_to_string(drive_c.join("ProgramData/Battle.net/Agent/aggregate.json")) else { return HashMap::new() };
    let v: Value = serde_json::from_str(&t).unwrap_or_default();
    v["installed"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|g| {
            let product = g["product_id"].as_str()?.to_string();
            let exe = g["icon_path"].as_str()?.replace('\\', "/");
            let folder = exe.rsplit_once('/')?.0.rsplit('/').next()?.to_ascii_lowercase();
            Some((folder, product))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_wow_versions_from_logs_and_build_info() {
        let log = "I x GameAccount added to family: family=WoW account=GameAccount(WoW:wow_classic_era region=EU entity=2:1:2 name=WoW1)\n\
                   I x GameAccount added to family: family=WoW account=GameAccount(WoW:wow_classic_ptr region=XX)\n\
                   D x New selection for product group WoW: account=GameAccount(WoW:retail region=EU entity=2:1:2 name=WoW1)";
        let mut got: Vec<String> = super::wow_owned_in(log).into_iter().collect();
        got.sort();
        assert_eq!(got, ["retail", "wow_classic_era"]);

        let dir = std::env::temp_dir().join(format!("lodestone-wow-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join(".build.info");
        std::fs::write(&f, "Branch!STRING:0|Active!DEC:1|Product!STRING:0\neu|1|wow\neu|1|wow_classic_era\neu|1|wow_anniversary\n").unwrap();
        assert_eq!(super::build_info_products(&f), ["wow", "wow_classic_era", "wow_anniversary"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// `cargo test live_bnet -- --ignored --nocapture`: reads the real Lodestone Battle.net prefix.
    #[test]
    #[ignore]
    fn live_bnet_catalog() {
        let c = crate::proton::drive_c(&crate::proton::prefix("battlenet"));
        let cat = super::catalog(&c);
        let lic = super::account_licenses(&c);
        println!("catalog: {} products; account licences: {:?}", cat.len(), lic);
        for g in crate::stores::winapps::scan() {
            println!("{:<28} {:<22} installed={} cover={} install={:?}", g.id, g.title, g.installed, g.cover.is_some(), g.install.as_ref().map(|s| s.args.clone()));
        }
    }
}
