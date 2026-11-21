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
    let txt = fs::read_to_string(path).ok()?;
    let mut lines = txt.lines().filter(|l| !l.trim().is_empty());
    let header: Vec<String> = lines.next()?.split('|').map(|h| h.split('!').next().unwrap_or("").to_ascii_lowercase()).collect();
    let col = header.iter().position(|h| h == "product")?;
    let row = lines.next()?;
    row.split('|').nth(col).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
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
