//! Reader for Steam's binary app cache (appcache/appinfo.vdf, format v28/v29).
//! Gives us names, types and Workshop support for every app the client knows about, offline.

use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct AppInfo {
    pub name: String,
    pub kind: String,
    pub workshop: bool,
    pub genres: Vec<u32>,
    pub franchise: Option<String>,
    pub tags: std::collections::HashSet<u32>,
    /// Folder name under steamapps/common.
    pub installdir: String,
    /// Platforms Steam lists ("windows,macos,linux").
    pub oslist: String,
}

struct Reader<'a> {
    b: &'a [u8],
    strings: Option<Vec<String>>,
}

impl<'a> Reader<'a> {
    fn u32(&self, p: usize) -> Option<u32> {
        Some(u32::from_le_bytes(self.b.get(p..p + 4)?.try_into().ok()?))
    }

    fn cstr(&self, p: usize) -> Option<(String, usize)> {
        let end = p + self.b.get(p..)?.iter().position(|&c| c == 0)?;
        Some((String::from_utf8_lossy(&self.b[p..end]).into_owned(), end + 1))
    }

    fn key(&self, p: usize) -> Option<(String, usize)> {
        match &self.strings {
            Some(s) => Some((s.get(self.u32(p)? as usize)?.clone(), p + 4)),
            None => self.cstr(p),
        }
    }

    /// Walks one binary KeyValues object, calling `f(path, value)` for string/int leaves.
    fn walk(&self, mut p: usize, path: &mut Vec<String>, f: &mut dyn FnMut(&[String], String)) -> Option<usize> {
        loop {
            let t = *self.b.get(p)?;
            p += 1;
            if t == 8 || t == 11 {
                return Some(p);
            }
            let (k, np) = self.key(p)?;
            p = np;
            match t {
                0 => {
                    path.push(k);
                    p = self.walk(p, path, f)?;
                    path.pop();
                }
                1 => {
                    let (v, np) = self.cstr(p)?;
                    p = np;
                    path.push(k);
                    f(path, v);
                    path.pop();
                }
                2 | 3 | 4 | 6 => {
                    let v = self.u32(p)?;
                    p += 4;
                    path.push(k);
                    f(path, (v as i32).to_string());
                    path.pop();
                }
                7 | 10 => p += 8,
                _ => return None,
            }
        }
    }
}

pub fn load(steam_root: &Path) -> HashMap<u32, AppInfo> {
    let mut out = HashMap::new();
    let Ok(b) = std::fs::read(steam_root.join("appcache/appinfo.vdf")) else { return out };
    let mut r = Reader { b: &b, strings: None };
    let Some(magic) = r.u32(0) else { return out };
    let mut p = match magic {
        0x07564429 => {
            // v29: keys are indexes into a string table stored at the end of the file.
            let off = u64::from_le_bytes(match b.get(8..16) {
                Some(s) => s.try_into().unwrap(),
                None => return out,
            }) as usize;
            let Some(n) = r.u32(off) else { return out };
            let table: Vec<String> = b[off + 4..]
                .split(|&c| c == 0)
                .take(n as usize)
                .map(|s| String::from_utf8_lossy(s).into_owned())
                .collect();
            r.strings = Some(table);
            16
        }
        0x07564428 => 8,
        _ => return out,
    };
    while let (Some(appid), Some(size)) = (r.u32(p), r.u32(p + 4)) {
        if appid == 0 {
            break;
        }
        let body = p + 8;
        let mut info = AppInfo::default();
        let mut path = Vec::new();
        // associations: {"0": {"type": "franchise", "name": "..."}, ...} arrive as separate leaves.
        let mut assoc: Vec<(String, String, String)> = Vec::new();
        r.walk(body + 60, &mut path, &mut |path, v| {
            if path.len() == 3 && path[1] == "config" && path[2] == "installdir" {
                info.installdir = v;
                return;
            }
            if path.len() < 3 || path[1] != "common" {
                return;
            }
            match (path.len(), path[2].to_ascii_lowercase().as_str()) {
                (3, "name") => info.name = v,
                (3, "type") => info.kind = v.to_ascii_lowercase(),
                (3, "oslist") => info.oslist = v.to_ascii_lowercase(),
                (3, "workshop_visible") => info.workshop = v == "1",
                (4, "genres") => info.genres.extend(v.parse::<u32>().ok()),
                (4, "store_tags") => info.tags.extend(v.parse::<u32>().ok()),
                (5, "associations") => assoc.push((path[3].clone(), path[4].clone(), v)),
                _ => {}
            }
        });
        let mut idx: Vec<&String> = assoc.iter().map(|a| &a.0).collect();
        idx.dedup();
        info.franchise = idx.iter().find_map(|i| {
            let is_franchise = assoc.iter().any(|a| &a.0 == *i && a.1 == "type" && a.2 == "franchise");
            is_franchise.then(|| assoc.iter().find(|a| &a.0 == *i && a.1 == "name").map(|a| a.2.clone())).flatten()
        });
        if !info.name.is_empty() {
            out.insert(appid, info);
        }
        p = body + size as usize;
    }
    out
}

/// App ids in the user's library. Two sources, because neither is complete on its own:
/// - localconfig.vdf: apps this PC has local state for (played, installed, configured)
/// - appcache/librarycache/<appid>: Steam caches library art for every app in your library,
///   including ones never opened on this PC (e.g. a newly bought game).
/// Callers filter to type "game" using appinfo.vdf.
pub fn library_ids(steam_root: &Path) -> Vec<u32> {
    let mut ids: Vec<u32> = std::fs::read_dir(steam_root.join("appcache/librarycache"))
        .map(|rd| rd.flatten().filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok()).collect())
        .unwrap_or_default();
    let Ok(rd) = std::fs::read_dir(steam_root.join("userdata")) else { return ids };
    for user in rd.flatten() {
        let Ok(txt) = std::fs::read_to_string(user.path().join("config/localconfig.vdf")) else { continue };
        let v = crate::vdf::parse(&txt);
        let apps = v
            .get("UserLocalConfigStore")
            .and_then(|x| x.get("Software"))
            .and_then(|x| x.get("Valve"))
            .and_then(|x| x.get("Steam"))
            .and_then(|x| x.get("apps"));
        if let Some(apps) = apps {
            ids.extend(apps.entries().iter().filter_map(|(k, _)| k.parse::<u32>().ok()));
        }
    }
    ids.sort_unstable();
    ids.dedup();
    ids
}
