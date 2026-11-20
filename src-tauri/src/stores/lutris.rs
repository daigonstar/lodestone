//! Lutris games. This is also where Battle.net, Guild Wars 2 (ArenaNet), EA app, Ubisoft Connect etc.
//! live on Linux, so we relabel them by store.

use crate::model::*;
use rusqlite::{Connection, OpenFlags};
use std::path::PathBuf;

const FLATPAK_ID: &str = "net.lutris.Lutris";

fn classify(slug: &str, name: &str) -> &'static str {
    let s = format!("{slug} {name}").to_ascii_lowercase();
    let has = |k: &[&str]| k.iter().any(|w| s.contains(w));
    if has(&["battle.net", "battlenet", "overwatch", "diablo", "warcraft", "hearthstone", "starcraft", "heroes-of-the-storm", "call-of-duty"]) && !s.contains("world at war") {
        "battlenet"
    } else if has(&["guild wars", "guild-wars", "arenanet"]) {
        "arenanet"
    } else if has(&["ea app", "ea-app", "origin"]) {
        "ea"
    } else if has(&["ubisoft", "uplay"]) {
        "ubisoft"
    } else if has(&["rockstar"]) {
        "rockstar"
    } else {
        "lutris"
    }
}

pub fn scan() -> Vec<Game> {
    let h = home();
    let mut out = Vec::new();
    for (data, flatpak) in [
        (h.join(".var/app").join(FLATPAK_ID).join("data/lutris"), true),
        (h.join(".local/share/lutris"), false),
    ] {
        let db = data.join("pga.db");
        if !db.is_file() || (!flatpak && which("lutris").is_none()) {
            continue;
        }
        let Ok(conn) = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX) else {
            continue;
        };
        let Ok(mut stmt) = conn.prepare(
            "SELECT id, slug, name, COALESCE(runner,''), COALESCE(directory,'') FROM games WHERE installed = 1",
        ) else {
            continue;
        };
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?))
        });
        let Ok(rows) = rows else { continue };
        let cache = if flatpak { h.join(".var/app").join(FLATPAK_ID).join("cache/lutris") } else { h.join(".cache/lutris") };
        for (id, slug, name, runner, dir) in rows.flatten() {
            let art = |sub: &str, ext: &str| -> Option<String> {
                [data.join(sub), cache.join(sub)]
                    .into_iter()
                    .map(|d: PathBuf| d.join(format!("{slug}.{ext}")))
                    .find(|p| p.is_file())
                    .map(|p| path_str(&p))
            };
            let url = format!("lutris:rungameid/{id}");
            let launch = if flatpak {
                LaunchSpec::new("flatpak", &["run", FLATPAK_ID, &url])
            } else {
                LaunchSpec::new("lutris", &[&url])
            };
            let detect = std::path::Path::new(&dir)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .filter(|n| n.len() >= 4)
                .map(Detect::Contains);
            out.push(Game {
                id: format!("lutris:{id}"),
                store: classify(&slug, &name).into(),
                via: format!("Lutris ({runner})"),
                title: name,
                installed: true,
                cover: art("coverart", "jpg"),
                hero: art("banners", "jpg"),
                size: None,
                launch: Some(launch),
                detect,
                ..Default::default()
            });
        }
    }
    out
}
