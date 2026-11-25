//! Game metadata beyond what the stores give us:
//! - your edits (title, cover, banner, genres, series, which Steam game it is), kept in meta.json;
//! - artwork search: Steam's store art (no key needed) and SteamGridDB when you add a free key;
//! - finding games whose cover is missing and filling them in automatically;
//! - details (description, developer, release date, screenshots) and news / patch notes from
//!   Steam's public store APIs. Non-Steam games use the Steam game with the same title, or else
//!   GOG's public catalog, Epic's game metadata, or Wikipedia/Wikidata (Battle.net, EA, Ubisoft...).

use crate::model::Game;
use crate::{net, series, settings, state};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct Override {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hero: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genres: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series: Option<String>,
    /// Use this Steam game's details, news and art.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steam_appid: Option<u32>,
    /// Where the game is installed, when Lodestone's detection missed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_dir: Option<String>,
    /// The program to run for it (optional; .exe files run through Proton).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exe: Option<String>,
    /// Filled in by "Fix missing artwork" rather than by you (your own edits always win).
    #[serde(default)]
    pub auto: bool,
}

/// Run a program you picked for a game: .exe through Proton (in the store's prefix when the game
/// has one), anything else directly, from its own folder.
fn exe_spec(g: &Game, exe: &std::path::Path) -> Option<crate::model::LaunchSpec> {
    let cwd = exe.parent().map(|p| p.to_path_buf());
    let mut s = if exe.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe")) {
        let prefix = g.prefix.as_ref().map(PathBuf::from).unwrap_or_else(|| {
            let safe: String = g.id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' }).collect();
            crate::proton::prefix(&format!("game-{safe}"))
        });
        crate::proton::spec(crate::proton::Run { prefix: &prefix, exe, args: vec![], store: None, game_id: "umu-default" }).ok()?
    } else {
        crate::model::LaunchSpec::new(exe.to_string_lossy(), &[])
    };
    s.cwd = cwd;
    Some(s)
}

/// Your install location: the game counts as installed there, and runs your program if you picked one.
fn apply_location(g: &mut Game, o: &Override) {
    let Some(dir) = o.install_dir.as_ref().map(std::path::Path::new).filter(|d| d.is_dir()) else { return };
    // Epic and GOG games were handed to their store's records when you saved.
    if !g.installed {
        g.installed = true;
        g.can_install = false;
    }
    let exe = o.exe.as_ref().map(std::path::Path::new).filter(|e| e.is_file());
    if let Some(e) = exe {
        if let Some(s) = exe_spec(g, e) {
            g.launch = Some(s);
            if g.prefix.is_none() && e.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe")) {
                let safe: String = g.id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' }).collect();
                g.prefix = Some(crate::proton::prefix(&format!("game-{safe}")).to_string_lossy().into_owned());
            }
        }
    }
    let name = exe.and_then(|e| e.file_name()).or_else(|| dir.file_name()).map(|n| n.to_string_lossy().into_owned());
    if let Some(n) = name {
        g.detect = Some(crate::model::Detect::Contains(n));
    }
}

#[derive(Serialize, Deserialize, Default)]
struct Data {
    #[serde(default)]
    games: HashMap<String, Override>,
    /// Remote cover URL -> whether it exists (checked with HEAD requests).
    #[serde(default)]
    checked: HashMap<String, bool>,
    /// Game id -> Steam app with the same title (None: looked, found nothing).
    #[serde(default)]
    matches: HashMap<String, Option<u32>>,
}

static DATA: Mutex<Option<Data>> = Mutex::new(None);

fn file() -> PathBuf {
    state::data_dir().join("meta.json")
}

fn with<R>(f: impl FnOnce(&mut Data) -> R) -> R {
    let mut g = DATA.lock().unwrap();
    let d = g.get_or_insert_with(|| fs::read_to_string(file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default());
    f(d)
}

fn save() {
    let txt = with(|d| serde_json::to_string(d).unwrap_or_default());
    let _ = fs::create_dir_all(state::data_dir());
    let tmp = file().with_extension("json.tmp");
    if fs::write(&tmp, txt).is_ok() {
        let _ = fs::rename(tmp, file());
    }
}

fn art_dir() -> PathBuf {
    state::data_dir().join("art")
}

// ---------------- edits ----------------

/// Apply your edits to the scanned games.
pub fn apply(games: &mut [Game]) {
    with(|d| {
        for g in games.iter_mut() {
            let Some(o) = d.games.get(&g.id) else { continue };
            if let Some(t) = o.title.as_ref().filter(|t| !t.trim().is_empty()) {
                g.title = t.clone();
            }
            if let Some(c) = &o.cover {
                g.cover = Some(c.clone());
            }
            if let Some(h) = &o.hero {
                g.hero = Some(h.clone());
            }
            if let Some(gs) = &o.genres {
                g.genres = gs.clone();
            }
            if let Some(s) = &o.series {
                g.series = (!s.trim().is_empty()).then(|| s.clone());
            }
            apply_location(g, o);
        }
    })
}

pub fn edited(id: &str) -> bool {
    with(|d| d.games.get(id).is_some_and(|o| !o.auto))
}

/// No cover, a local cover file that's gone, or a remote one that turned out missing.
pub fn art_missing(g: &Game) -> bool {
    // ROMs get their covers from ES-DE's scraper, not from store artwork searches.
    if g.system.is_some() {
        return false;
    }
    match &g.cover {
        None => true,
        Some(c) if c.starts_with("http") => with(|d| d.checked.get(c) == Some(&false)),
        Some(c) => !std::path::Path::new(c).exists(),
    }
}

pub fn get(id: &str) -> Override {
    with(|d| d.games.get(id).cloned().unwrap_or_default())
}

fn ext_of(url: &str, ctype: &str) -> &'static str {
    let u = url.split('?').next().unwrap_or("").to_ascii_lowercase();
    if ctype.contains("png") || u.ends_with(".png") {
        "png"
    } else if ctype.contains("webp") || u.ends_with(".webp") {
        "webp"
    } else {
        "jpg"
    }
}

/// Download artwork so it works offline and doesn't depend on the source staying up.
async fn keep(id: &str, kind: &str, url: &str) -> Result<String, String> {
    if !url.starts_with("http") {
        return if std::path::Path::new(url).is_file() { Ok(url.to_string()) } else { Err(format!("{url} doesn't exist")) };
    }
    let r = net::client().get(url).send().await.map_err(net::err)?;
    if !r.status().is_success() {
        return Err(format!("{} for {url}", r.status()));
    }
    let ctype = r.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    if !ctype.starts_with("image/") {
        return Err("That link isn't an image".into());
    }
    let bytes = r.bytes().await.map_err(net::err)?;
    let safe: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    fs::create_dir_all(art_dir()).map_err(|e| e.to_string())?;
    // A new name each time, so the UI doesn't show a cached old image.
    let path = art_dir().join(format!("{safe}-{kind}-{}.{}", state::now(), ext_of(url, &ctype)));
    fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

fn remove_old_art(old: &Override, new: &Override) {
    for (a, b) in [(&old.cover, &new.cover), (&old.hero, &new.hero)] {
        if let Some(p) = a {
            if Some(p) != b.as_ref() && p.starts_with(&*art_dir().to_string_lossy()) {
                let _ = fs::remove_file(p);
            }
        }
    }
}

pub async fn save_edit(id: &str, mut o: Override) -> Result<(), String> {
    let clean = |s: Option<String>| s.map(|x| x.trim().trim_matches('"').to_string()).filter(|x| !x.is_empty());
    o.install_dir = clean(o.install_dir.take());
    o.exe = clean(o.exe.take());
    if let Some(d) = &o.install_dir {
        if !std::path::Path::new(d).is_dir() {
            return Err(format!("Install folder not found: {d}"));
        }
    }
    if let Some(e) = &o.exe {
        if !std::path::Path::new(e).is_file() {
            return Err(format!("Program not found: {e}"));
        }
        if o.install_dir.is_none() {
            o.install_dir = std::path::Path::new(e).parent().map(|p| p.to_string_lossy().into_owned());
        }
    }
    // Epic and GOG games: tell the store's own records, so Play, updates and uninstall work as usual.
    if let Some(d) = o.install_dir.clone().filter(|d| Some(d) != get(id).install_dir.as_ref()) {
        if let Some(app) = id.strip_prefix("epic:") {
            crate::stores::epic::run(vec!["-y".into(), "import".into(), "--skip-dlcs".into(), app.into(), d.clone()])
                .await
                .map_err(|e| format!("Epic (legendary) couldn't use that folder: {e}"))?;
        } else if let Some(gid) = id.strip_prefix("gog:") {
            crate::stores::gog::set_path(gid, &d)?;
        }
    }
    if let Some(u) = o.cover.clone().filter(|u| !u.is_empty()) {
        o.cover = Some(keep(id, "cover", &u).await?);
    } else {
        o.cover = None;
    }
    if let Some(u) = o.hero.clone().filter(|u| !u.is_empty()) {
        o.hero = Some(keep(id, "hero", &u).await?);
    } else {
        o.hero = None;
    }
    o.auto = false;
    let old = get(id);
    remove_old_art(&old, &o);
    with(|d| {
        if o.steam_appid != old.steam_appid {
            d.matches.remove(id);
        }
        d.games.insert(id.to_string(), o);
    });
    save();
    Ok(())
}

pub fn reset(id: &str) {
    let old = get(id);
    remove_old_art(&old, &Override::default());
    with(|d| d.games.remove(id));
    save();
}

// ---------------- artwork search ----------------

#[derive(Serialize, Clone, Debug)]
pub struct Art {
    /// cover (portrait) or hero (wide banner)
    pub kind: &'static str,
    pub url: String,
    pub thumb: String,
    pub source: &'static str,
    pub name: String,
}

fn steam_art(appid: u64, name: &str) -> Vec<Art> {
    let base = format!("https://shared.steamstatic.com/store_item_assets/steam/apps/{appid}");
    vec![
        Art { kind: "cover", url: format!("{base}/library_600x900_2x.jpg"), thumb: format!("{base}/library_600x900.jpg"), source: "Steam", name: name.into() },
        Art { kind: "hero", url: format!("{base}/library_hero.jpg"), thumb: format!("{base}/library_hero.jpg"), source: "Steam", name: name.into() },
        Art { kind: "hero", url: format!("{base}/header.jpg"), thumb: format!("{base}/header.jpg"), source: "Steam", name: name.into() },
    ]
}

fn sgdb_key() -> Option<String> {
    let k = settings::load().steamgriddb_key.trim().to_string();
    (!k.is_empty()).then_some(k)
}

async fn sgdb(path: &str, key: &str) -> Result<serde_json::Value, String> {
    let auth = format!("Bearer {key}");
    net::get_json(&format!("https://www.steamgriddb.com/api/v2/{path}"), &[("authorization", auth.as_str())]).await
}

/// SteamGridDB images for one of its games (cover grids 600x900, heroes).
async fn sgdb_images(game: u64, name: &str, key: &str, per: usize) -> Vec<Art> {
    let mut out = Vec::new();
    for (kind, path) in [("cover", format!("grids/game/{game}?dimensions=600x900,342x482,660x930")), ("hero", format!("heroes/game/{game}"))] {
        let Ok(v) = sgdb(&path, key).await else { continue };
        for i in v["data"].as_array().into_iter().flatten().take(per) {
            if let (Some(u), Some(t)) = (i["url"].as_str(), i["thumb"].as_str()) {
                out.push(Art { kind, url: u.into(), thumb: t.into(), source: "SteamGridDB", name: name.into() });
            }
        }
    }
    out
}

/// Artwork for a search term from every source available.
pub async fn search(query: &str) -> Result<Vec<Art>, String> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(vec![]);
    }
    let mut out = Vec::new();
    if let Some(key) = sgdb_key() {
        match sgdb(&format!("search/autocomplete/{}", urlencoding::encode(q)), &key).await {
            Ok(v) => {
                for g in v["data"].as_array().into_iter().flatten().take(3) {
                    if let Some(id) = g["id"].as_u64() {
                        out.extend(sgdb_images(id, g["name"].as_str().unwrap_or(""), &key, 8).await);
                    }
                }
            }
            Err(e) if e.contains("401") => return Err("SteamGridDB rejected the API key (Settings → Artwork)".into()),
            Err(_) => {}
        }
    }
    let url = format!("https://store.steampowered.com/api/storesearch/?term={}&cc=us&l=en", urlencoding::encode(q));
    if let Ok(v) = net::get_json(&url, &[]).await {
        for i in v["items"].as_array().into_iter().flatten().take(6) {
            if let Some(id) = i["id"].as_u64() {
                out.extend(steam_art(id, i["name"].as_str().unwrap_or("")));
            }
        }
    }
    Ok(out)
}

// ---------------- missing artwork ----------------

async fn exists(url: &str) -> bool {
    match net::client().head(url).send().await {
        Ok(r) => r.status().is_success(),
        // Network trouble isn't "missing".
        Err(_) => true,
    }
}

async fn parallel<T: Send + 'static, R: Send + 'static, F, Fut>(items: Vec<T>, n: usize, f: F) -> Vec<R>
where
    F: Fn(T) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = R> + Send,
{
    let queue = std::sync::Arc::new(tokio::sync::Mutex::new(items));
    let f = std::sync::Arc::new(f);
    let mut handles = Vec::new();
    for _ in 0..n {
        let (queue, f) = (queue.clone(), f.clone());
        handles.push(tauri::async_runtime::spawn(async move {
            let mut out = Vec::new();
            loop {
                let Some(item) = queue.lock().await.pop() else { break };
                out.push(f(item).await);
            }
            out
        }));
    }
    let mut all = Vec::new();
    for h in handles {
        all.extend(h.await.unwrap_or_default());
    }
    all
}

/// Check every remote cover not checked yet. Returns how many games have no usable cover.
pub async fn check(games: Vec<Game>) -> usize {
    let todo: Vec<String> = {
        let mut v: Vec<String> = games.iter().filter_map(|g| g.cover.clone()).filter(|c| c.starts_with("http")).filter(|c| with(|d| !d.checked.contains_key(c))).collect();
        v.sort();
        v.dedup();
        v
    };
    let results = parallel(todo, 12, |u: String| async move {
        let ok = exists(&u).await;
        (u, ok)
    })
    .await;
    with(|d| d.checked.extend(results));
    save();
    games.iter().filter(|g| art_missing(g)).count()
}

/// The Steam game this one is (its own id, your pick, or a same-title match).
pub async fn steam_appid(id: &str, title: &str) -> Option<u32> {
    if let Some(n) = id.strip_prefix("steam:").and_then(|a| a.parse().ok()) {
        return Some(n);
    }
    if let Some(n) = get(id).steam_appid {
        return Some(n);
    }
    if let Some(a) = series::cached_steam().get(id).and_then(|a| a.parse().ok()) {
        return Some(a);
    }
    if let Some(m) = with(|d| d.matches.get(id).cloned()) {
        return m;
    }
    let found = series::steam_ids_for(title).await.and_then(|v| v.first().and_then(|a| a.parse().ok()));
    with(|d| d.matches.insert(id.to_string(), found));
    save();
    found
}

/// Fill in covers (and banners) for games missing them. Returns how many were fixed.
pub async fn autofix(games: Vec<Game>) -> usize {
    let missing: Vec<Game> = games.into_iter().filter(|g| g.system.is_none() && art_missing(g) && !edited(&g.id)).collect();
    let key = sgdb_key();
    let fixed = parallel(missing, 4, move |g: Game| {
        let key = key.clone();
        async move {
            let mut cands: Vec<String> = Vec::new();
            let appid = steam_appid(&g.id, &g.title).await;
            if let Some(k) = &key {
                let path = match appid {
                    Some(a) => format!("grids/steam/{a}?dimensions=600x900,342x482,660x930"),
                    None => {
                        let found = sgdb(&format!("search/autocomplete/{}", urlencoding::encode(&g.title)), k).await.ok();
                        match found.and_then(|v| v["data"][0]["id"].as_u64()) {
                            Some(id) => format!("grids/game/{id}?dimensions=600x900,342x482,660x930"),
                            None => String::new(),
                        }
                    }
                };
                if !path.is_empty() {
                    if let Ok(v) = sgdb(&path, k).await {
                        cands.extend(v["data"].as_array().into_iter().flatten().filter_map(|i| i["url"].as_str().map(String::from)).take(2));
                    }
                }
            }
            if let Some(a) = appid.filter(|_| !g.id.starts_with("steam:")) {
                cands.push(format!("https://shared.steamstatic.com/store_item_assets/steam/apps/{a}/library_600x900.jpg"));
            }
            for c in cands {
                if exists(&c).await {
                    if let Ok(path) = keep(&g.id, "cover", &c).await {
                        let mut o = get(&g.id);
                        o.cover = Some(path);
                        o.auto = true;
                        with(|d| d.games.insert(g.id.clone(), o));
                        return 1;
                    }
                }
            }
            0
        }
    })
    .await;
    save();
    fixed.into_iter().sum()
}

// ---------------- details and news ----------------

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Details {
    pub steam_appid: Option<u32>,
    pub description: String,
    pub developers: Vec<String>,
    pub publishers: Vec<String>,
    pub release: String,
    pub metacritic: Option<u64>,
    pub website: String,
    pub features: Vec<String>,
    pub screenshots: Vec<(String, String)>,
    pub store_url: String,
    /// Has a Linux version on Steam.
    #[serde(default)]
    pub linux: bool,
    /// Where the details came from: Steam, GOG, Epic or Wikipedia.
    #[serde(default)]
    pub source: String,
}

fn cache_path(kind: &str, appid: u32) -> PathBuf {
    state::data_dir().join("cache").join(kind).join(format!("{appid}.json"))
}

fn read_cache<T: for<'a> Deserialize<'a>>(kind: &str, appid: u32, max_age: u64) -> Option<T> {
    let p = cache_path(kind, appid);
    let age = fs::metadata(&p).ok()?.modified().ok()?.elapsed().ok()?.as_secs();
    if age > max_age {
        return None;
    }
    serde_json::from_str(&fs::read_to_string(p).ok()?).ok()
}

fn write_cache<T: Serialize>(kind: &str, appid: u32, v: &T) {
    let p = cache_path(kind, appid);
    let _ = fs::create_dir_all(p.parent().unwrap());
    let _ = fs::write(p, serde_json::to_string(v).unwrap_or_default());
}

/// Plain text from Steam's HTML / BBCode snippets.
pub fn plain(s: &str) -> String {
    let mut out = String::new();
    let mut tag = false;
    for c in s.chars() {
        match c {
            '<' | '[' => tag = true,
            '>' | ']' if tag => {
                tag = false;
                out.push(' ');
            }
            _ if !tag => out.push(c),
            _ => {}
        }
    }
    let out = out.replace("&quot;", "\"").replace("&amp;", "&").replace("&#39;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&nbsp;", " ");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub async fn details(id: &str, title: &str) -> Result<Details, String> {
    let Some(appid) = steam_appid(id, title).await else { return Ok(other_details(id, title).await) };
    if let Some(d) = read_cache::<Details>("details", appid, 7 * 86400) {
        return Ok(d);
    }
    let v = net::get_json(&format!("https://store.steampowered.com/api/appdetails?appids={appid}&l=english&cc=us"), &[]).await?;
    let d = &v[appid.to_string()]["data"];
    let strs = |k: &str| d[k].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(String::from)).collect::<Vec<_>>();
    let out = Details {
        steam_appid: Some(appid),
        description: plain(d["short_description"].as_str().unwrap_or("")),
        developers: strs("developers"),
        publishers: strs("publishers"),
        release: d["release_date"]["date"].as_str().unwrap_or("").to_string(),
        metacritic: d["metacritic"]["score"].as_u64(),
        website: d["website"].as_str().unwrap_or("").to_string(),
        features: d["categories"].as_array().into_iter().flatten().filter_map(|c| c["description"].as_str().map(String::from)).take(8).collect(),
        screenshots: d["screenshots"].as_array().into_iter().flatten().filter_map(|s| Some((s["path_thumbnail"].as_str()?.to_string(), s["path_full"].as_str()?.to_string()))).take(8).collect(),
        store_url: format!("https://store.steampowered.com/app/{appid}/"),
        linux: d["platforms"]["linux"].as_bool().unwrap_or(false),
        source: "Steam".into(),
    };
    write_cache("details", appid, &out);
    Ok(out)
}

fn other_cache(id: &str) -> PathBuf {
    let safe: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    state::data_dir().join("cache/details-other").join(format!("{safe}.json"))
}

/// Details for games without a Steam match: the store's own catalog first, then Wikipedia.
/// Cached a week (also when nothing was found).
async fn other_details(id: &str, title: &str) -> Details {
    let cache = other_cache(id);
    if let Some(d) = fs::metadata(&cache)
        .ok()
        .and_then(|m| m.modified().ok()?.elapsed().ok())
        .filter(|age| age.as_secs() < 7 * 86400)
        .and_then(|_| serde_json::from_str::<Details>(&fs::read_to_string(&cache).ok()?).ok())
    {
        return d;
    }
    let mut d = match id.split_once(':') {
        Some(("gog", gid)) => gog_details(gid).await.unwrap_or_default(),
        Some(("epic", app)) => epic_details(app).unwrap_or_default(),
        _ => Details::default(),
    };
    // Fill in what the store didn't say (or everything, for stores without a public catalog).
    if d.description.is_empty() || d.developers.is_empty() || d.release.is_empty() {
        if let Some(w) = wikipedia_details(title).await {
            if d.description.is_empty() {
                d.description = w.description;
                d.store_url = w.store_url;
                d.source = w.source;
            }
            if d.developers.is_empty() {
                d.developers = w.developers;
            }
            if d.publishers.is_empty() {
                d.publishers = w.publishers;
            }
            if d.release.is_empty() {
                d.release = w.release;
            }
            if d.website.is_empty() {
                d.website = w.website;
            }
        }
    }
    let _ = fs::create_dir_all(cache.parent().unwrap());
    let _ = fs::write(&cache, serde_json::to_string(&d).unwrap_or_default());
    d
}

/// "2005-11-03T00:00:00+01:00" -> "Nov 3, 2005" (the format Steam uses).
fn pretty_date(iso: &str) -> String {
    let d = iso.trim_start_matches('+');
    let mut it = d.get(..10).unwrap_or("").split('-');
    let (Some(y), Some(m), Some(day)) = (it.next(), it.next().and_then(|m| m.parse::<usize>().ok()), it.next().and_then(|d| d.parse::<u32>().ok())) else { return String::new() };
    const M: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    match (M.get(m.wrapping_sub(1)), day) {
        (Some(mn), 1..=31) => format!("{mn} {day}, {y}"),
        (Some(mn), _) => format!("{mn} {y}"),
        _ => y.to_string(),
    }
}

/// GOG's public catalog: description, developers, publisher, release date, screenshots.
async fn gog_details(gid: &str) -> Option<Details> {
    let v = net::get_json(&format!("https://api.gog.com/v2/games/{gid}?locale=en-US"), &[]).await.ok()?;
    let e = &v["_embedded"];
    let names = |a: &serde_json::Value| a.as_array().into_iter().flatten().filter_map(|x| x["name"].as_str().map(String::from)).collect::<Vec<_>>();
    let mut publishers = names(&e["publishers"]);
    if publishers.is_empty() {
        publishers.extend(e["publisher"]["name"].as_str().map(String::from));
    }
    let shots = e["screenshots"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s["_links"]["self"]["href"].as_str())
        .map(|h| (h.replace("{formatter}", "product_card_v2_thumbnail_271"), h.replace("{formatter}", "ggvgl")))
        .take(8)
        .collect();
    let desc = plain(v["description"].as_str().unwrap_or(""));
    Some(Details {
        description: first_sentences(&desc, 600),
        developers: names(&e["developers"]),
        publishers,
        release: pretty_date(e["product"]["globalReleaseDate"].as_str().unwrap_or("")),
        features: e["features"].as_array().into_iter().flatten().filter_map(|f| f["name"].as_str().map(String::from)).take(8).collect(),
        screenshots: shots,
        store_url: v["_links"]["store"]["href"].as_str().unwrap_or("").to_string(),
        source: "GOG".into(),
        ..Default::default()
    })
}

/// Epic's game metadata, which legendary keeps on disk.
fn epic_details(app: &str) -> Option<Details> {
    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(crate::stores::epic::config_dir().join("metadata").join(format!("{app}.json"))).ok()?).ok()?;
    let m = &v["metadata"];
    let desc = plain(m["description"].as_str().unwrap_or(""));
    Some(Details {
        description: first_sentences(&desc, 600),
        developers: m["developer"].as_str().filter(|s| !s.is_empty()).map(|s| vec![s.to_string()]).unwrap_or_default(),
        release: m["releaseInfo"][0]["dateAdded"].as_str().map(pretty_date).unwrap_or_default(),
        source: "Epic".into(),
        ..Default::default()
    })
}

/// Keep descriptions to a readable length, cut at a sentence end.
fn first_sentences(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    match cut.rfind(". ") {
        Some(i) if i > max / 3 => cut[..=i].to_string(),
        _ => format!("{}…", cut.trim_end()),
    }
}

/// "World of Warcraft: Mists of Pandaria (video game)" -> "world of warcraft mists of pandaria".
fn norm_title(s: &str) -> String {
    let s = s.split(" (").next().unwrap_or(s).to_lowercase();
    s.chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Pick the Wikipedia article for a game: the same title, or else the longest title the game's
/// name starts with ("…: Mists of Pandaria Classic" -> "…: Mists of Pandaria"). None rather than a guess.
fn pick_article<'a>(title: &str, results: &'a [String]) -> Option<&'a String> {
    let want = norm_title(title);
    if let Some(r) = results.iter().find(|r| norm_title(r) == want) {
        return Some(r);
    }
    results
        .iter()
        .filter(|r| {
            let n = norm_title(r);
            n.split(' ').count() >= 2 && (want.starts_with(&format!("{n} ")) || n.starts_with(&format!("{want} ")))
        })
        .max_by_key(|r| norm_title(r).len())
}

/// The game's Wikipedia summary, plus developer, publisher, release date and website from Wikidata.
async fn wikipedia_details(title: &str) -> Option<Details> {
    let q = urlencoding::encode(&format!("{title} video game")).into_owned();
    let s = net::get_json(&format!("https://en.wikipedia.org/w/api.php?action=query&list=search&srsearch={q}&srlimit=6&format=json"), &[]).await.ok()?;
    let results: Vec<String> = s["query"]["search"].as_array().into_iter().flatten().filter_map(|r| r["title"].as_str().map(String::from)).collect();
    let page = pick_article(title, &results)?;
    let sum = net::get_json(&format!("https://en.wikipedia.org/api/rest_v1/page/summary/{}", urlencoding::encode(&page.replace(' ', "_"))), &[]).await.ok()?;
    // Only articles about games or their expansions (not a film or a novel with the same name).
    let what = sum["description"].as_str().unwrap_or("").to_lowercase();
    if !what.contains("game") && !what.contains("expansion") {
        return None;
    }
    let mut d = Details {
        description: first_sentences(sum["extract"].as_str().unwrap_or(""), 600),
        store_url: sum["content_urls"]["desktop"]["page"].as_str().unwrap_or("").to_string(),
        source: "Wikipedia".into(),
        ..Default::default()
    };
    if let Some(q) = sum["wikibase_item"].as_str() {
        if let Ok(w) = net::get_json(&format!("https://www.wikidata.org/wiki/Special:EntityData/{q}.json"), &[]).await {
            let claims = &w["entities"][q]["claims"];
            let ids = |p: &str| claims[p].as_array().into_iter().flatten().filter_map(|c| c["mainsnak"]["datavalue"]["value"]["id"].as_str().map(String::from)).collect::<Vec<_>>();
            let (devs, pubs) = (ids("P178"), ids("P123"));
            let all: Vec<String> = devs.iter().chain(pubs.iter()).cloned().collect();
            let mut labels: HashMap<String, String> = HashMap::new();
            if !all.is_empty() {
                let url = format!("https://www.wikidata.org/w/api.php?action=wbgetentities&ids={}&props=labels&languages=en&format=json", all.join("|"));
                if let Ok(l) = net::get_json(&url, &[]).await {
                    for id in &all {
                        if let Some(n) = l["entities"][id]["labels"]["en"]["value"].as_str() {
                            labels.insert(id.clone(), n.to_string());
                        }
                    }
                }
            }
            let named = |v: &[String]| v.iter().filter_map(|i| labels.get(i).cloned()).collect::<Vec<_>>();
            d.developers = named(&devs);
            d.publishers = named(&pubs);
            // The earliest publication date.
            d.release = claims["P577"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| c["mainsnak"]["datavalue"]["value"]["time"].as_str())
                .min()
                .map(pretty_date)
                .unwrap_or_default();
            d.website = claims["P856"][0]["mainsnak"]["datavalue"]["value"].as_str().unwrap_or("").to_string();
        }
    }
    Some(d)
}

/// ProtonDB's community rating for how a game runs through Proton.
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct ProtonDb {
    pub appid: u32,
    /// platinum, gold, silver, bronze, borked; "pending" when there are too few reports.
    pub tier: String,
    pub trending: String,
    pub confidence: String,
    pub reports: u64,
    pub url: String,
}

fn protondb_from(appid: u32, v: &serde_json::Value) -> ProtonDb {
    let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
    ProtonDb { appid, tier: s("tier"), trending: s("trendingTier"), confidence: s("confidence"), reports: v["total"].as_u64().unwrap_or(0), url: format!("https://www.protondb.com/app/{appid}") }
}

async fn protondb_app(appid: u32) -> Option<ProtonDb> {
    if let Some(p) = read_cache::<ProtonDb>("protondb", appid, 86400) {
        return (!p.tier.is_empty()).then_some(p);
    }
    let out = match net::get_json(&format!("https://www.protondb.com/api/v1/reports/summaries/{appid}.json"), &[]).await {
        Ok(v) => protondb_from(appid, &v),
        // No reports yet: remember that too.
        Err(e) if e.starts_with("404") => ProtonDb { appid, ..Default::default() },
        Err(_) => return None,
    };
    write_cache("protondb", appid, &out);
    (!out.tier.is_empty()).then_some(out)
}

pub async fn protondb(id: &str, title: &str) -> Option<ProtonDb> {
    protondb_app(steam_appid(id, title).await?).await
}

/// The Steam app id when it's already known (no store search).
fn known_appid(id: &str) -> Option<u32> {
    if let Some(n) = id.strip_prefix("steam:").and_then(|a| a.parse().ok()) {
        return Some(n);
    }
    get(id).steam_appid.or_else(|| series::cached_steam().get(id).and_then(|a| a.parse().ok())).or_else(|| with(|d| d.matches.get(id).cloned()).flatten())
}

#[derive(Serialize, Clone)]
pub struct Compat {
    pub id: String,
    /// "native" for Steam games with a Linux version, else the ProtonDB tier.
    pub tier: String,
    pub protondb: Option<ProtonDb>,
}

/// Ratings for many games at once (games without a known Steam match are skipped).
pub async fn compat(ids: Vec<String>) -> Vec<Compat> {
    let native: std::collections::HashSet<u32> = crate::stores::steam::primary_root()
        .map(|r| crate::steam_appinfo::load(&r.0).into_iter().filter(|(_, a)| a.oslist.split(',').any(|o| o.trim() == "linux")).map(|(k, _)| k).collect())
        .unwrap_or_default();
    let items: Vec<(String, u32)> = ids.into_iter().filter_map(|id| known_appid(&id).map(|a| (id, a))).collect();
    parallel(items, 8, move |(id, appid): (String, u32)| {
        let is_native = native.contains(&appid) && id.starts_with("steam:");
        async move {
            let p = protondb_app(appid).await;
            let tier = if is_native { "native".to_string() } else { p.as_ref().map(|p| p.tier.clone()).unwrap_or_default() };
            Compat { id, tier, protondb: p }
        }
    })
    .await
    .into_iter()
    .filter(|c| !c.tier.is_empty())
    .collect()
}

#[derive(Serialize, Deserialize, Clone)]
pub struct News {
    pub game: String,
    pub title: String,
    pub url: String,
    pub date: u64,
    pub summary: String,
    /// Steam's patch-notes tag.
    pub patch: bool,
}

async fn news_for(appid: u32) -> Vec<News> {
    if let Some(n) = read_cache::<Vec<News>>("news", appid, 3 * 3600) {
        return n;
    }
    let url = format!("https://api.steampowered.com/ISteamNews/GetNewsForApp/v2/?appid={appid}&count=4&maxlength=400&feeds=steam_community_announcements");
    let Ok(v) = net::get_json(&url, &[]).await else { return vec![] };
    let out: Vec<News> = v["appnews"]["newsitems"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| {
            let title = n["title"].as_str()?.to_string();
            let tags: Vec<&str> = n["tags"].as_array().into_iter().flatten().filter_map(|t| t.as_str()).collect();
            let lt = title.to_lowercase();
            Some(News {
                game: String::new(),
                patch: tags.contains(&"patchnotes") || lt.contains("patch") || lt.contains("hotfix") || lt.contains("update"),
                url: n["url"].as_str()?.to_string(),
                date: n["date"].as_u64().unwrap_or(0),
                summary: plain(n["contents"].as_str().unwrap_or("")).chars().take(280).collect(),
                title,
            })
        })
        .collect();
    write_cache("news", appid, &out);
    out
}

/// Latest announcements for these games, newest first. `games` is (id, title).
pub async fn news(games: Vec<(String, String)>) -> Vec<News> {
    let lists = parallel(games, 6, |(id, title): (String, String)| async move {
        let Some(appid) = steam_appid(&id, &title).await else { return vec![] };
        news_for(appid).await.into_iter().map(|mut n| {
            n.game = id.clone();
            n
        }).collect::<Vec<_>>()
    })
    .await;
    let mut all: Vec<News> = lists.into_iter().flatten().collect();
    all.sort_by(|a, b| b.date.cmp(&a.date));
    all.dedup_by(|a, b| a.url == b.url);
    all
}

#[cfg(test)]
mod other_tests {
    #[test]
    fn picks_the_right_wikipedia_article() {
        let r = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let wow = r(&["World of Warcraft: Forever", "World of Warcraft", "World of Warcraft: Midnight"]);
        assert_eq!(super::pick_article("World of Warcraft", &wow).unwrap(), "World of Warcraft");
        let mop = r(&["World of Warcraft: Mists of Pandaria", "World of Warcraft", "World of Warcraft: Warlords of Draenor"]);
        assert_eq!(super::pick_article("World of Warcraft: Mists of Pandaria Classic", &mop).unwrap(), "World of Warcraft: Mists of Pandaria");
        let sc = r(&["StarCraft (video game)", "StarCraft II", "StarCraft II: Wings of Liberty"]);
        assert_eq!(super::pick_article("StarCraft II", &sc).unwrap(), "StarCraft II");
        assert!(super::pick_article("Hot Wheels Unleashed", &r(&["Mattel", "Hot Toys"])).is_none());
        assert_eq!(super::pretty_date("+2004-11-23T00:00:00Z"), "Nov 23, 2004");
    }

    #[test]
    fn your_install_folder_marks_the_game_installed() {
        let dir = std::env::temp_dir().join(format!("lodestone-loc-{}", std::process::id()));
        let game_dir = dir.join("My Game");
        std::fs::create_dir_all(&game_dir).unwrap();
        std::fs::write(game_dir.join("run.sh"), "#!/bin/sh\n").unwrap();
        let mut g = crate::model::Game { id: "battlenet:s1".into(), store: "battlenet".into(), title: "StarCraft".into(), can_install: true, ..Default::default() };
        let o = super::Override { install_dir: Some(game_dir.to_string_lossy().into()), exe: Some(game_dir.join("run.sh").to_string_lossy().into()), ..Default::default() };
        super::apply_location(&mut g, &o);
        assert!(g.installed && !g.can_install);
        let l = g.launch.unwrap();
        assert!(l.program.ends_with("run.sh"));
        assert_eq!(l.cwd.as_deref(), Some(game_dir.as_path()));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// `cargo test live_other_details -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_other_details() {
        tauri::async_runtime::block_on(async {
            for t in ["World of Warcraft", "Diablo II: Resurrected", "StarCraft II", "World of Warcraft: Mists of Pandaria Classic"] {
                let d = super::wikipedia_details(t).await.unwrap_or_default();
                println!("{t}: {} | dev {:?} pub {:?} rel {} web {} | {}", d.source, d.developers, d.publishers, d.release, d.website, d.description.chars().take(80).collect::<String>());
            }
            let g = super::gog_details("1207658890").await.unwrap_or_default();
            println!("GOG: dev {:?} pub {:?} rel {} shots {} | {}", g.developers, g.publishers, g.release, g.screenshots.len(), g.description.chars().take(80).collect::<String>());
        });
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_protondb_summary() {
        let v = serde_json::json!({"bestReportedTier": "platinum", "confidence": "strong", "score": 0.71, "tier": "gold", "total": 116, "trendingTier": "gold"});
        let p = super::protondb_from(24780, &v);
        assert_eq!((p.tier.as_str(), p.reports, p.url.as_str()), ("gold", 116, "https://www.protondb.com/app/24780"));
    }

    #[test]
    #[ignore]
    fn live_compat() {
        let ids = ["steam:251570", "steam:24780", "steam:570", "steam:1091500", "custom:nope"].map(String::from).to_vec();
        for c in tauri::async_runtime::block_on(super::compat(ids)) {
            println!("{} {} {:?}", c.id, c.tier, c.protondb.map(|p| p.reports));
        }
    }

    #[test]
    fn strips_markup() {
        assert_eq!(super::plain("<p>Hello &amp; <b>welcome</b></p>[img]x[/img] back"), "Hello & welcome x back");
    }

    /// Lists games missing covers in the real library, then fills them in (like "Find artwork").
    #[test]
    #[ignore]
    fn live_fix_missing_art() {
        tauri::async_runtime::block_on(async {
            let games = crate::stores::scan_all();
            let mut g2 = games.clone();
            super::apply(&mut g2);
            let n = super::check(g2.clone()).await;
            println!("{n} missing:");
            for g in g2.iter().filter(|g| super::art_missing(g)) {
                println!("  {} | {} | {:?}", g.id, g.title, g.cover);
            }
            let fixed = super::autofix(g2).await;
            println!("fixed {fixed}");
        });
    }

    #[test]
    #[ignore]
    fn live_details_and_news() {
        tauri::async_runtime::block_on(async {
            let d = super::details("steam:620", "Portal 2").await.unwrap();
            println!("{} | {:?} | {} | {} shots", d.description, d.developers, d.release, d.screenshots.len());
            assert!(!d.description.is_empty());
            let n = super::news_for(570).await;
            println!("{:?}", n.iter().map(|x| (&x.title, x.patch)).collect::<Vec<_>>());
            let a = super::search("Hollow Knight").await.unwrap();
            println!("{} art hits", a.len());
            assert!(!a.is_empty());
        });
    }
}
