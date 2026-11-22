//! Modrinth (no key needed) and CurseForge (personal API key) search, install with dependencies, and updates.

use super::instance::{sha1_file, Instance, Kind, LocalFile};
use crate::net::{self, get_json};
use crate::settings;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::Path;

const MR: &str = "https://api.modrinth.com/v2";
const CF: &str = "https://api.curseforge.com/v1";
const CF_MINECRAFT: u32 = 432;

/// A download's file name exactly as the mod site gives it, but only if it's a plain name: no folders
/// ("/", "\\"), nothing hidden, no "..", no control characters. The name comes from the site's answer,
/// so without this a bad listing could write outside the instance's mods folder.
fn safe_file_name(name: &str) -> Result<&str, String> {
    let ok = !name.is_empty()
        && name.len() <= 255
        && !name.starts_with('.')
        && !name.contains(['/', '\\', '\0'])
        && !name.chars().any(char::is_control);
    if ok {
        Ok(name)
    } else {
        Err(format!("Refused a download with an unsafe file name: {name:?}"))
    }
}

fn enc(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

#[derive(Serialize, Clone, Default)]
pub struct SearchHit {
    pub source: String,
    pub id: String,
    pub slug: String,
    pub title: String,
    pub description: String,
    pub author: String,
    pub icon: Option<String>,
    pub downloads: u64,
    pub url: String,
}

#[derive(Serialize, Clone, Default)]
pub struct InstalledItem {
    #[serde(skip)]
    pub local: Option<LocalFile>,
    pub file: String,
    pub enabled: bool,
    pub size: u64,
    pub title: String,
    pub version: Option<String>,
    pub icon: Option<String>,
    pub project_id: Option<String>,
    pub url: Option<String>,
    /// Newer compatible version on Modrinth, if any.
    pub update: Option<String>,
}

fn cf_key() -> Result<String, String> {
    let k = settings::load().curseforge_api_key;
    if k.trim().is_empty() {
        Err("CurseForge needs an API key. Add one in Settings (free at console.curseforge.com).".into())
    } else {
        Ok(k.trim().to_string())
    }
}

fn cf_class(kind: Kind) -> u32 {
    match kind {
        Kind::Mod => 6,
        Kind::ResourcePack => 12,
        Kind::Shader => 6552,
    }
}

fn cf_loader(loader: &str) -> Option<u32> {
    match loader {
        "forge" => Some(1),
        "fabric" => Some(4),
        "quilt" => Some(5),
        "neoforge" => Some(6),
        _ => None,
    }
}

fn mr_type(kind: Kind) -> &'static str {
    match kind {
        Kind::Mod => "mod",
        Kind::ResourcePack => "resourcepack",
        Kind::Shader => "shader",
    }
}

/// Loaders to filter Modrinth versions by. Quilt can also run Fabric mods.
fn mr_loaders(inst: &Instance, kind: Kind) -> Vec<String> {
    match kind {
        Kind::Mod => match inst.loader.as_str() {
            "quilt" => vec!["quilt".into(), "fabric".into()],
            "vanilla" => vec![],
            l => vec![l.to_string()],
        },
        Kind::ResourcePack => vec!["minecraft".into()],
        Kind::Shader => vec![],
    }
}

// ---------------- search ----------------

pub async fn search(source: &str, inst: &Instance, kind: Kind, query: &str, offset: u32) -> Result<Vec<SearchHit>, String> {
    match source {
        "modrinth" => {
            let mut facets = vec![json!([format!("project_type:{}", mr_type(kind))])];
            if !inst.mc_version.is_empty() {
                facets.push(json!([format!("versions:{}", inst.mc_version)]));
            }
            let loaders = mr_loaders(inst, kind);
            if kind == Kind::Mod && !loaders.is_empty() {
                facets.push(json!(loaders.iter().map(|l| format!("categories:{l}")).collect::<Vec<_>>()));
            }
            let index = if query.trim().is_empty() { "downloads" } else { "relevance" };
            let url = format!(
                "{MR}/search?query={}&facets={}&index={index}&limit=24&offset={offset}",
                enc(query),
                enc(&Value::Array(facets).to_string())
            );
            let v = get_json(&url, &[]).await?;
            Ok(v["hits"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|h| SearchHit {
                    source: "modrinth".into(),
                    id: h["project_id"].as_str().unwrap_or("").into(),
                    slug: h["slug"].as_str().unwrap_or("").into(),
                    title: h["title"].as_str().unwrap_or("").into(),
                    description: h["description"].as_str().unwrap_or("").into(),
                    author: h["author"].as_str().unwrap_or("").into(),
                    icon: h["icon_url"].as_str().filter(|s| !s.is_empty()).map(String::from),
                    downloads: h["downloads"].as_u64().unwrap_or(0),
                    url: format!("https://modrinth.com/{}/{}", h["project_type"].as_str().unwrap_or("mod"), h["slug"].as_str().unwrap_or("")),
                })
                .collect())
        }
        "curseforge" => {
            let key = cf_key()?;
            let mut url = format!(
                "{CF}/mods/search?gameId={CF_MINECRAFT}&classId={}&searchFilter={}&sortField={}&sortOrder=desc&pageSize=24&index={offset}",
                cf_class(kind),
                enc(query),
                if query.trim().is_empty() { 2 } else { 1 }
            );
            if !inst.mc_version.is_empty() {
                url.push_str(&format!("&gameVersion={}", enc(&inst.mc_version)));
            }
            if kind == Kind::Mod {
                if let Some(l) = cf_loader(&inst.loader) {
                    url.push_str(&format!("&modLoaderType={l}"));
                }
            }
            let v = get_json(&url, &[("x-api-key", &key)]).await?;
            Ok(v["data"].as_array().into_iter().flatten().map(cf_hit).collect())
        }
        _ => Err("unknown source".into()),
    }
}

fn cf_hit(m: &Value) -> SearchHit {
    SearchHit {
        source: "curseforge".into(),
        id: m["id"].as_u64().map(|n| n.to_string()).unwrap_or_default(),
        slug: m["slug"].as_str().unwrap_or("").into(),
        title: m["name"].as_str().unwrap_or("").into(),
        description: m["summary"].as_str().unwrap_or("").into(),
        author: m["authors"][0]["name"].as_str().unwrap_or("").into(),
        icon: m["logo"]["thumbnailUrl"].as_str().filter(|s| !s.is_empty()).map(String::from),
        downloads: m["downloadCount"].as_f64().unwrap_or(0.0) as u64,
        url: m["links"]["websiteUrl"].as_str().unwrap_or("").into(),
    }
}

// ---------------- installed files ----------------

/// Local files, identified against Modrinth by hash (titles, icons, project ids) where possible.
pub async fn installed(inst: &Instance, kind: Kind) -> Vec<InstalledItem> {
    let files = {
        let inst = inst.clone();
        tauri::async_runtime::spawn_blocking(move || inst.files(kind)).await.unwrap_or_default()
    };
    let hashes: Vec<String> = files.iter().map(|f| f.sha1.clone()).filter(|h| !h.is_empty()).collect();
    let mut by_hash: HashMap<String, Value> = HashMap::new();
    let mut projects: HashMap<String, Value> = HashMap::new();
    if !hashes.is_empty() {
        if let Ok(r) = net::client().post(format!("{MR}/version_files")).json(&json!({"hashes": hashes, "algorithm": "sha1"})).send().await {
            if let Ok(Value::Object(m)) = r.json::<Value>().await {
                by_hash = m.into_iter().collect();
            }
        }
        let ids: HashSet<String> = by_hash.values().filter_map(|v| v["project_id"].as_str().map(String::from)).collect();
        if !ids.is_empty() {
            let ids: Vec<String> = ids.into_iter().collect();
            let url = format!("{MR}/projects?ids={}", enc(&json!(ids).to_string()));
            if let Ok(v) = get_json(&url, &[]).await {
                for p in v.as_array().into_iter().flatten() {
                    if let Some(id) = p["id"].as_str() {
                        projects.insert(id.to_string(), p.clone());
                    }
                }
            }
        }
    }
    let mut out: Vec<InstalledItem> = files
        .into_iter()
        .map(|f| {
            let ver = by_hash.get(&f.sha1);
            let pid = ver.and_then(|v| v["project_id"].as_str()).map(String::from);
            let proj = pid.as_ref().and_then(|p| projects.get(p));
            let fallback = f.name.clone().unwrap_or_else(|| f.file.trim_end_matches(".disabled").to_string());
            InstalledItem {
                file: f.file.clone(),
                enabled: f.enabled,
                size: f.size,
                title: proj.and_then(|p| p["title"].as_str()).map(String::from).unwrap_or(fallback),
                version: ver.and_then(|v| v["version_number"].as_str()).map(String::from),
                icon: proj.and_then(|p| p["icon_url"].as_str()).filter(|s| !s.is_empty()).map(String::from),
                url: proj.map(|p| format!("https://modrinth.com/{}/{}", p["project_type"].as_str().unwrap_or("mod"), p["slug"].as_str().unwrap_or(""))),
                project_id: pid,
                update: None,
                local: Some(f),
            }
        })
        .collect();
    out.sort_by_key(|i| i.title.to_lowercase());
    out
}

/// Fill `update` for files that Modrinth has a newer compatible version of.
pub async fn check_updates(inst: &Instance, kind: Kind, items: &mut [InstalledItem]) -> Result<usize, String> {
    let hashes: Vec<String> = items.iter().filter(|i| i.project_id.is_some()).filter_map(|i| i.local.as_ref().map(|l| l.sha1.clone())).collect();
    if hashes.is_empty() {
        return Ok(0);
    }
    let mut body = json!({ "hashes": hashes, "algorithm": "sha1" });
    let loaders = mr_loaders(inst, kind);
    if !loaders.is_empty() {
        body["loaders"] = json!(loaders);
    }
    if !inst.mc_version.is_empty() {
        body["game_versions"] = json!([inst.mc_version]);
    }
    let r = net::client().post(format!("{MR}/version_files/update")).json(&body).send().await.map_err(net::err)?;
    let m: Value = r.json().await.map_err(net::err)?;
    let mut n = 0;
    for it in items.iter_mut() {
        let Some(l) = &it.local else { continue };
        let Some(latest) = m.get(&l.sha1) else { continue };
        let same = latest["files"].as_array().into_iter().flatten().any(|f| f["hashes"]["sha1"].as_str() == Some(l.sha1.as_str()));
        if !same {
            it.update = latest["version_number"].as_str().map(String::from);
            n += 1;
        }
    }
    Ok(n)
}

// ---------------- install ----------------

struct Ctx<'a> {
    inst: &'a Instance,
    kind: Kind,
    /// Modrinth project ids already present (skip as dependencies).
    have_mr: HashSet<String>,
    /// Lowercased titles already present, to avoid CurseForge duplicating a Modrinth-installed library.
    have_titles: HashSet<String>,
    installed: Vec<String>,
}

async fn download(url: &str, dest: &Path, sha1: Option<&str>) -> Result<(), String> {
    let r = net::client().get(url).send().await.map_err(net::err)?;
    if !r.status().is_success() {
        return Err(format!("download failed: {}", r.status()));
    }
    let bytes = r.bytes().await.map_err(net::err)?;
    if let Some(p) = dest.parent() {
        std::fs::create_dir_all(p).map_err(net::err)?;
    }
    let tmp = dest.with_extension("lodestone-part");
    std::fs::write(&tmp, &bytes).map_err(net::err)?;
    if let Some(expected) = sha1 {
        let got = sha1_file(&tmp);
        if !got.eq_ignore_ascii_case(expected) {
            let _ = std::fs::remove_file(&tmp);
            return Err("downloaded file failed its checksum".into());
        }
    }
    std::fs::rename(&tmp, dest).map_err(net::err)
}

fn pick_version(versions: &[Value]) -> Option<&Value> {
    versions.iter().find(|v| v["version_type"] == "release").or_else(|| versions.first())
}

fn mr_install<'a>(cx: &'a mut Ctx<'_>, project: String, depth: u8) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
    Box::pin(async move {
        if depth > 6 {
            return Ok(());
        }
        let mut url = format!("{MR}/project/{}/version", enc(&project));
        let mut q = Vec::new();
        let loaders = mr_loaders(cx.inst, cx.kind);
        if !loaders.is_empty() {
            q.push(format!("loaders={}", enc(&json!(loaders).to_string())));
        }
        if !cx.inst.mc_version.is_empty() {
            q.push(format!("game_versions={}", enc(&json!([cx.inst.mc_version]).to_string())));
        }
        if !q.is_empty() {
            url = format!("{url}?{}", q.join("&"));
        }
        let versions = get_json(&url, &[]).await?;
        let versions = versions.as_array().cloned().unwrap_or_default();
        let Some(ver) = pick_version(&versions) else {
            return Err(format!("No version of {project} for Minecraft {} {}", cx.inst.mc_version, cx.inst.loader));
        };
        let files = ver["files"].as_array().cloned().unwrap_or_default();
        let file = files.iter().find(|f| f["primary"] == true).or(files.first()).ok_or("version has no files")?;
        let name = safe_file_name(file["filename"].as_str().ok_or("no filename")?)?.to_string();
        let dest = cx.inst.dir(cx.kind).join(&name);
        if !dest.exists() {
            download(file["url"].as_str().ok_or("no url")?, &dest, file["hashes"]["sha1"].as_str()).await?;
        }
        let proj = get_json(&format!("{MR}/project/{}", enc(&project)), &[]).await.unwrap_or(Value::Null);
        let title = proj["title"].as_str().unwrap_or(&name).to_string();
        cx.inst.register(cx.kind, &name, &title, ver["version_number"].as_str().unwrap_or(""), proj["description"].as_str().unwrap_or(""), None);
        cx.have_mr.insert(ver["project_id"].as_str().unwrap_or(&project).to_string());
        cx.have_titles.insert(title.to_lowercase());
        cx.installed.push(title);

        for d in ver["dependencies"].as_array().cloned().unwrap_or_default() {
            if d["dependency_type"] != "required" {
                continue;
            }
            let pid = match d["project_id"].as_str() {
                Some(p) => p.to_string(),
                None => match d["version_id"].as_str() {
                    Some(vid) => match get_json(&format!("{MR}/version/{vid}"), &[]).await {
                        Ok(v) => v["project_id"].as_str().unwrap_or("").to_string(),
                        Err(_) => continue,
                    },
                    None => continue,
                },
            };
            if pid.is_empty() || cx.have_mr.contains(&pid) {
                continue;
            }
            mr_install(cx, pid, depth + 1).await?;
        }
        Ok(())
    })
}

fn cf_install<'a>(cx: &'a mut Ctx<'_>, key: &'a str, mod_id: u64, depth: u8) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
    Box::pin(async move {
        if depth > 6 {
            return Ok(());
        }
        let m = get_json(&format!("{CF}/mods/{mod_id}"), &[("x-api-key", key)]).await?;
        let m = &m["data"];
        let title = m["name"].as_str().unwrap_or("mod").to_string();
        if depth > 0 && cx.have_titles.contains(&title.to_lowercase()) {
            return Ok(());
        }
        let mut url = format!("{CF}/mods/{mod_id}/files?pageSize=50");
        if !cx.inst.mc_version.is_empty() {
            url.push_str(&format!("&gameVersion={}", enc(&cx.inst.mc_version)));
        }
        if cx.kind == Kind::Mod {
            if let Some(l) = cf_loader(&cx.inst.loader) {
                url.push_str(&format!("&modLoaderType={l}"));
            }
        }
        let files = get_json(&url, &[("x-api-key", key)]).await?;
        let mut files = files["data"].as_array().cloned().unwrap_or_default();
        files.sort_by(|a, b| b["fileDate"].as_str().cmp(&a["fileDate"].as_str()));
        // releaseType 1 = release, 2 = beta, 3 = alpha
        let file = files.iter().find(|f| f["releaseType"] == 1).or(files.first()).cloned();
        let Some(file) = file else {
            return Err(format!("No file of {title} for Minecraft {} {}", cx.inst.mc_version, cx.inst.loader));
        };
        let Some(dl) = file["downloadUrl"].as_str() else {
            return Err(format!("{title}'s author only allows downloads through CurseForge's own app. Get it from its page or Modrinth."));
        };
        let name = safe_file_name(file["fileName"].as_str().ok_or("no file name")?)?.to_string();
        let sha1 = file["hashes"].as_array().into_iter().flatten().find(|h| h["algo"] == 1).and_then(|h| h["value"].as_str()).map(String::from);
        let dest = cx.inst.dir(cx.kind).join(&name);
        if !dest.exists() {
            download(dl, &dest, sha1.as_deref()).await?;
        }
        let id = mod_id.to_string();
        cx.inst.register(cx.kind, &name, &title, file["displayName"].as_str().unwrap_or(""), m["summary"].as_str().unwrap_or(""), Some(("curseforge", &id)));
        cx.have_titles.insert(title.to_lowercase());
        cx.installed.push(title);
        for d in file["dependencies"].as_array().cloned().unwrap_or_default() {
            // relationType 3 = required dependency
            if d["relationType"] == 3 {
                if let Some(dep) = d["modId"].as_u64() {
                    cf_install(cx, key, dep, depth + 1).await?;
                }
            }
        }
        Ok(())
    })
}

/// Install a project (plus required dependencies). Returns the titles that were added.
pub async fn install(source: &str, inst: &Instance, kind: Kind, project: &str) -> Result<Vec<String>, String> {
    let current = installed(inst, kind).await;
    let mut cx = Ctx {
        inst,
        kind,
        have_mr: current.iter().filter_map(|i| i.project_id.clone()).collect(),
        have_titles: current.iter().map(|i| i.title.to_lowercase()).collect(),
        installed: Vec::new(),
    };
    match source {
        "modrinth" => mr_install(&mut cx, project.to_string(), 0).await?,
        "curseforge" => {
            let key = cf_key()?;
            let id: u64 = project.parse().map_err(|_| "bad CurseForge id")?;
            cf_install(&mut cx, &key, id, 0).await?
        }
        _ => return Err("unknown source".into()),
    }
    Ok(cx.installed)
}

/// Replace a file with the newest compatible Modrinth version (old file goes to the trash).
pub async fn update(inst: &Instance, kind: Kind, file: &str) -> Result<String, String> {
    let items = installed(inst, kind).await;
    let it = items.iter().find(|i| i.file == file).ok_or("file not found")?;
    let pid = it.project_id.clone().ok_or("Not a Modrinth file, can't update automatically")?;
    let was_enabled = it.enabled;
    let mut cx = Ctx { inst, kind, have_mr: HashSet::new(), have_titles: HashSet::new(), installed: vec![] };
    // Dependencies already present: only fetch the project itself.
    cx.have_mr = items.iter().filter_map(|i| i.project_id.clone()).filter(|p| p != &pid).collect();
    mr_install(&mut cx, pid, 0).await?;
    let new_files: HashSet<String> = inst.files(kind).into_iter().map(|f| f.file).collect();
    let old_files: HashSet<String> = items.iter().map(|i| i.file.clone()).collect();
    let added: Vec<&String> = new_files.difference(&old_files).collect();
    if !added.is_empty() {
        inst.remove(kind, file)?;
        if !was_enabled {
            for f in added {
                let _ = inst.set_enabled(kind, f, false);
            }
        }
    }
    Ok(it.title.clone())
}

#[cfg(test)]
mod safe_name_tests {
    #[test]
    fn refuses_file_names_that_leave_the_folder() {
        assert_eq!(super::safe_file_name("sodium-fabric-0.6.jar"), Ok("sodium-fabric-0.6.jar"));
        for bad in ["../../.config/autostart/x.desktop", "/etc/passwd", "a/b.jar", "..\\x.jar", ".hidden.jar", "", "a\nb.jar", "a\0b.jar"] {
            assert!(super::safe_file_name(bad).is_err(), "{bad:?} should be refused");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cargo test live_ -- --ignored --nocapture`: hits Modrinth with this machine's instances.
    #[test]
    #[ignore]
    fn live_identify_and_search() {
        tauri::async_runtime::block_on(async {
            for inst in crate::mc::instance::all() {
                println!("{} | {} {} {}", inst.id, inst.mc_version, inst.loader, inst.loader_version);
                let t = std::time::Instant::now();
                let mut items = installed(&inst, Kind::Mod).await;
                let known = items.iter().filter(|i| i.project_id.is_some()).count();
                println!("  mods: {} ({} identified on Modrinth) in {:?}", items.len(), known, t.elapsed());
                let n = check_updates(&inst, Kind::Mod, &mut items).await.unwrap();
                println!("  updates available: {n}: {:?}", items.iter().filter(|i| i.update.is_some()).take(5).map(|i| (&i.title, &i.version, &i.update)).collect::<Vec<_>>());
                println!("  disabled: {:?}", items.iter().filter(|i| !i.enabled).map(|i| &i.title).collect::<Vec<_>>());
                let rp = installed(&inst, Kind::ResourcePack).await;
                println!("  resource packs: {:?}", rp.iter().map(|i| &i.title).collect::<Vec<_>>());
                let hits = search("modrinth", &inst, Kind::Mod, "jei", 0).await.unwrap();
                println!("  search jei: {:?}", hits.iter().take(4).map(|h| &h.title).collect::<Vec<_>>());
                let hits = search("modrinth", &inst, Kind::Shader, "", 0).await.unwrap();
                println!("  top shaders: {:?}", hits.iter().take(4).map(|h| &h.title).collect::<Vec<_>>());
            }
            let ws = crate::workshop::list(246620).await.unwrap();
            println!("workshop 246620: {:?}", ws.iter().take(3).map(|w| (&w.title, w.preview.is_some())).collect::<Vec<_>>());
            let all = crate::stores::steam::scan();
            println!("steam: {} installed, {} owned not installed, {} with workshop", all.iter().filter(|g| g.installed).count(), all.iter().filter(|g| !g.installed).count(), all.iter().filter(|g| g.workshop.is_some()).count());
        });
    }
}
