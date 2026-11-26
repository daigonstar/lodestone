//! Where plug-ins come from: GitHub releases, checked only when you open the Plug-ins page and installed
//! only when you press the button.
//!
//! - **Official** plug-ins are a fixed list inside Lodestone (`official()`), so a clean install knows
//!   nothing else.
//! - **Third-party** plug-ins come from repositories you add yourself (stored only in your own settings,
//!   like a Beacon server address). They are always labelled third-party.
//!
//! A release is used only if its file for this system is named `…-linux-x86_64.flatpak` (Linux) or
//! `…-windows-x86_64.zip` (Windows), is downloaded from that same repository's releases, and carries the
//! SHA-256 checksum GitHub publishes. An optional `lodestone-plugin.json` in the release says what the
//! plug-in is and which permissions it will ask for, so you can read it before installing.

use crate::net;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    /// Official: the plug-in id ("beacon", "kiln"). Third-party: "owner/repo".
    pub id: String,
    pub repo: String,
    /// Official plug-ins that share a repository tell their releases apart by tag prefix ("beacon-v").
    pub tag_prefix: &'static str,
    pub official: bool,
    /// The ending of this system's file in a release. Normally a Flatpak bundle on Linux and a zip on
    /// Windows; Kiln (Python, which needs the host's Steam and Proton) ships a zip of its source on Linux.
    pub suffix: &'static str,
}

/// The plug-ins Lodestone ships knowing about.
pub fn official() -> Vec<Source> {
    vec![
        Source { id: "beacon".into(), repo: "daigonstar/lodestone".into(), tag_prefix: "beacon-v", official: true, suffix: DEFAULT_SUFFIX },
        Source { id: "kiln".into(), repo: "daigonstar/lodestone".into(), tag_prefix: "kiln-v", official: true, suffix: KILN_SUFFIX },
    ]
}

/// "owner/repo" as GitHub allows it; nothing else is accepted (it ends up in a web address).
pub fn parse_repo(s: &str) -> Option<String> {
    let s = s.trim().trim_start_matches("https://github.com/").trim_end_matches('/').trim_end_matches(".git");
    let (owner, name) = s.split_once('/')?;
    let ok = |p: &str| !p.is_empty() && p.len() <= 100 && p != "." && p != ".." && p.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
    (ok(owner) && ok(name)).then(|| format!("{owner}/{name}"))
}

/// Repositories you added (third-party).
pub fn user() -> Vec<Source> {
    crate::settings::load().plugin_repos.iter().filter_map(|r| parse_repo(r)).map(|r| Source { id: r.clone(), repo: r, tag_prefix: "", official: false, suffix: DEFAULT_SUFFIX }).collect()
}

pub fn add_repo(input: &str) -> Result<String, String> {
    let repo = parse_repo(input).ok_or("That isn't a GitHub repository. Use owner/repo, like someone/their-plugin")?;
    if official().iter().any(|s| s.repo.eq_ignore_ascii_case(&repo)) {
        return Err("That repository is already one of Lodestone's own plug-ins".into());
    }
    let mut s = crate::settings::load();
    if !s.plugin_repos.iter().any(|r| r.eq_ignore_ascii_case(&repo)) {
        s.plugin_repos.push(repo.clone());
        crate::settings::save(&s)?;
    }
    Ok(repo)
}

pub fn remove_repo(repo: &str) -> Result<(), String> {
    let mut s = crate::settings::load();
    s.plugin_repos.retain(|r| !r.eq_ignore_ascii_case(repo));
    crate::settings::save(&s)
}

/// A plug-in id Lodestone will install: an official one, or a repository you added.
pub fn find(id: &str) -> Option<Source> {
    official().into_iter().chain(user()).find(|s| s.id.eq_ignore_ascii_case(id))
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// "embedded" or "companion" (see docs/plugin-api-notes.md).
    #[serde(default)]
    pub kind: String,
    /// Bridge permissions it will ask for (shown in plain words when known).
    #[serde(default)]
    pub permissions: Vec<String>,
}

pub struct Release {
    pub version: String,
    pub asset: Value,
    pub manifest: Option<Manifest>,
}

#[cfg(not(windows))]
const DEFAULT_SUFFIX: &str = "-linux-x86_64.flatpak";
#[cfg(windows)]
const DEFAULT_SUFFIX: &str = "-windows-x86_64.zip";
/// Kiln: its Python source, run with the system's Python and PySide6 (no Windows build yet).
#[cfg(not(windows))]
const KILN_SUFFIX: &str = "-linux-any.zip";
#[cfg(windows)]
const KILN_SUFFIX: &str = "-windows-x86_64.zip";

fn release_url(repo: &str) -> String {
    format!("https://api.github.com/repos/{repo}/releases?per_page=30")
}

fn download_prefix(repo: &str) -> String {
    format!("https://github.com/{repo}/releases/download/")
}

/// A version Lodestone will put in a file name: letters, digits, dots and dashes.
fn clean_version(v: &str) -> Option<String> {
    (!v.is_empty() && v.len() <= 40 && v.chars().all(|c| c.is_ascii_alphanumeric() || ".-".contains(c))).then(|| v.to_string())
}

/// Pick the newest usable release from GitHub's list (newest first): not a draft, and not a pre-release unless
/// `test_releases` (an opt-in on the Plug-ins page), with the tag prefix and a file for this system.
pub fn pick(list: &Value, tag_prefix: &str, suffix: &str, test_releases: bool) -> Option<(String, Value, Value)> {
    for rel in list.as_array()?.iter().filter(|r| r["draft"] != json!(true) && (test_releases || r["prerelease"] != json!(true))) {
        let tag = rel["tag_name"].as_str().unwrap_or_default();
        let Some(rest) = tag.strip_prefix(tag_prefix) else { continue };
        let rest = if tag_prefix.is_empty() { rest.strip_prefix('v').unwrap_or(rest) } else { rest };
        let Some(version) = clean_version(rest) else { continue };
        let assets = rel["assets"].as_array().cloned().unwrap_or_default();
        if let Some(a) = assets.iter().find(|a| a["name"].as_str().is_some_and(|n| n.ends_with(suffix))) {
            return Some((version, a.clone(), json!(assets)));
        }
    }
    None
}

pub async fn latest(src: &Source) -> Result<Release, String> {
    let list = net::get_json(&release_url(&src.repo), &[("accept", "application/vnd.github+json")]).await?;
    let (version, asset, assets) = pick(&list, src.tag_prefix, src.suffix, crate::settings::load().plugin_test_releases).ok_or_else(|| format!("{} has no release for this system yet", src.repo))?;
    // The optional description of the plug-in (small, and only read as text).
    let mut manifest = None;
    if let Some(m) = assets.as_array().into_iter().flatten().find(|a| a["name"].as_str() == Some("lodestone-plugin.json")) {
        let url = m["browser_download_url"].as_str().unwrap_or_default();
        if url.starts_with(&download_prefix(&src.repo)) && m["size"].as_u64().unwrap_or(u64::MAX) <= 16 * 1024 {
            if let Ok(txt) = net::client().get(url).send().await.map_err(net::err)?.error_for_status().map_err(net::err)?.text().await {
                manifest = serde_json::from_str(&txt).ok();
            }
        }
    }
    Ok(Release { version, asset, manifest })
}

/// Download this release's file and check it against GitHub's published checksum. A file without a
/// checksum, or from anywhere but the repository's own releases, is refused.
pub async fn fetch_verified(src: &Source, rel: &Release) -> Result<Vec<u8>, String> {
    if !rel.asset["digest"].as_str().is_some_and(|d| d.starts_with("sha256:")) {
        return Err(format!("That {} release has no published checksum, so Lodestone won't install it", src.repo));
    }
    let url = rel.asset["browser_download_url"].as_str().ok_or("no download address")?;
    if !url.starts_with(&download_prefix(&src.repo)) {
        return Err(format!("That download isn't from {}'s own releases", src.repo));
    }
    let bytes = net::client().get(url).send().await.map_err(net::err)?.error_for_status().map_err(net::err)?.bytes().await.map_err(net::err)?;
    net::verify_release_file(&rel.asset, &bytes)?;
    Ok(bytes.to_vec())
}

/// "0.2.10" > "0.2.9": compare the numbers, not the text.
pub fn newer(a: &str, b: &str) -> bool {
    let parse = |v: &str| v.split(['.', '-']).map(|p| p.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    parse(a) > parse(b)
}

fn safe_name(id: &str) -> String {
    id.replace('/', "__")
}

fn marker(id: &str) -> PathBuf {
    crate::state::data_dir().join("plugins").join(format!("{}-version.txt", safe_name(id)))
}

pub fn installed_version(id: &str) -> Option<String> {
    fs::read_to_string(marker(id)).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Is there something to install, or a newer version than yours? Third-party repos get the same answer
/// plus what the plug-in says about itself.
pub async fn check(src: &Source) -> Result<Value, String> {
    let rel = latest(src).await?;
    let have = installed_version(&src.id);
    let update = have.as_deref().is_some_and(|h| newer(&rel.version, h));
    Ok(json!({ "id": src.id, "repo": src.repo, "official": src.official, "latest": rel.version, "installed": have, "update": update, "manifest": rel.manifest }))
}

#[cfg(test)]
pub async fn install_for_test(src: &Source, version: &str, bytes: &[u8]) -> Result<(), String> {
    install_file(src, version, bytes).await
}

/// Install (or update) a plug-in other than Beacon, which has its own folder layout (beacon_install).
pub async fn install(src: &Source) -> Result<String, String> {
    let rel = latest(src).await?;
    let bytes = fetch_verified(src, &rel).await?;
    install_file(src, &rel.version, &bytes).await?;
    let m = marker(&src.id);
    if let Some(d) = m.parent() {
        let _ = fs::create_dir_all(d);
    }
    fs::write(m, &rel.version).map_err(|e| e.to_string())?;
    Ok(rel.version)
}

#[cfg(not(windows))]
async fn install_file(src: &Source, version: &str, bytes: &[u8]) -> Result<(), String> {
    if src.suffix.ends_with(".zip") {
        return unzip_version(&src.id, version, bytes);
    }
    install_flatpak(&src.id, version, bytes).await
}

/// Install a Flatpak bundle for your user (its runtime comes from Flathub); inside Lodestone's own
/// Flatpak this runs on the host, like Lodestone's other Flatpak commands.
#[cfg(not(windows))]
pub async fn install_flatpak(label: &str, version: &str, bytes: &[u8]) -> Result<(), String> {
    use std::process::{Command, Stdio};
    if !bytes.starts_with(b"flatpak") {
        return Err("That download isn't a Flatpak bundle".into());
    }
    let dir = crate::state::data_dir().join("downloads");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = dir.join(format!("plugin-{}-{version}.flatpak", safe_name(label)));
    fs::write(&file, bytes).map_err(|e| e.to_string())?;
    let mut c = Command::new("flatpak");
    c.args(["install", "--user", "-y", "--noninteractive"]).arg(&file);
    let out = tauri::async_runtime::spawn_blocking(move || crate::host::wrap(c).stdin(Stdio::null()).output())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("Couldn't run flatpak: {e}"))?;
    let _ = fs::remove_file(&file);
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr);
        return Err(format!("Flatpak couldn't install {label}: {}", why.lines().last().unwrap_or("unknown error")));
    }
    Ok(())
}

#[cfg(windows)]
async fn install_file(src: &Source, version: &str, bytes: &[u8]) -> Result<(), String> {
    unzip_version(&src.id, version, bytes)
}

/// Where Lodestone puts a plug-in it unpacks (a zip): Windows %LOCALAPPDATA%\Lodestone\Plugins\<plug-in>,
/// Linux ~/.local/share/lodestone/plugins/<plug-in> (the id is "owner__repo" for third-party ones). One
/// folder per version inside, and `current.txt` names the one in use.
pub fn unpacked_dir(id: &str) -> PathBuf {
    #[cfg(windows)]
    let base = dirs::data_local_dir().unwrap_or_default().join("Lodestone").join("Plugins");
    #[cfg(not(windows))]
    let base = crate::state::data_dir().join("plugins");
    base.join(safe_name(id))
}

/// The folder of the version in use, if one is installed.
pub fn current_unpacked(id: &str) -> Option<PathBuf> {
    let base = unpacked_dir(id);
    let v = fs::read_to_string(base.join("current.txt")).ok()?;
    let dir = base.join(v.trim());
    (!v.trim().is_empty() && !v.contains(['/', '\\']) && dir.is_dir()).then_some(dir)
}

fn unzip_version(id: &str, version: &str, bytes: &[u8]) -> Result<(), String> {
    let base = unpacked_dir(id);
    let target = base.join(version);
    let _ = fs::remove_dir_all(&target);
    unzip_into(bytes, &target)?;
    fs::write(base.join("current.txt"), version).map_err(|e| e.to_string())?;
    // Older versions are removed once the new one is in place.
    for e in fs::read_dir(&base).map_err(|e| e.to_string())?.flatten() {
        if e.path().is_dir() && e.file_name() != version {
            let _ = fs::remove_dir_all(e.path());
        }
    }
    Ok(())
}

/// Unpack a zip into `target`, refusing any name that would land outside it.
fn unzip_into(bytes: &[u8], target: &std::path::Path) -> Result<(), String> {
    fs::create_dir_all(target).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|_| "That download isn't a zip".to_string())?;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = f.enclosed_name() else {
            let _ = fs::remove_dir_all(target);
            return Err("The zip has a file outside its folder".into());
        };
        let out = target.join(rel);
        if f.is_dir() {
            fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(p) = out.parent() {
            fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        let mut w = fs::File::create(&out).map_err(|e| e.to_string())?;
        std::io::copy(&mut f, &mut w).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_names_are_checked() {
        assert_eq!(parse_repo("daigonstar/kiln").as_deref(), Some("daigonstar/kiln"));
        assert_eq!(parse_repo(" https://github.com/a-b/c_d.git/ ").as_deref(), Some("a-b/c_d"));
        for bad in ["", "kiln", "a/b/c", "../x", "a/..", "a b/c", "a/b?x=1", "a/b#c", "http://evil.invalid/a/b"] {
            assert_eq!(parse_repo(bad), None, "{bad}");
        }
    }

    #[test]
    fn compares_versions_as_numbers() {
        assert!(newer("0.2.10", "0.2.9"));
        assert!(newer("0.3.0", "0.2.99"));
        assert!(!newer("0.2.0", "0.2.0"));
        assert!(!newer("0.1.9", "0.2.0"));
    }

    #[test]
    fn picks_the_newest_usable_release() {
        let asset = |n: &str| json!({ "name": n });
        const SUFFIX: &str = DEFAULT_SUFFIX;
        let list = json!([
            { "tag_name": "beacon-v0.4.0", "draft": true, "assets": [asset(&format!("x-0.4.0{SUFFIX}"))] },
            { "tag_name": "v9.9.9", "assets": [asset(&format!("lodestone{SUFFIX}"))] },
            { "tag_name": "beacon-v0.3.0", "prerelease": true, "assets": [asset(&format!("x-0.3.0{SUFFIX}"))] },
            { "tag_name": "beacon-v0.2.0", "assets": [asset("notes.txt")] },
            { "tag_name": "beacon-v0.1.0", "assets": [asset(&format!("x-0.1.0{SUFFIX}"))] },
            { "tag_name": "beacon-v../../x", "assets": [asset(&format!("y{SUFFIX}"))] },
        ]);
        let (v, a, _) = pick(&list, "beacon-v", SUFFIX, false).unwrap();
        assert_eq!((v.as_str(), a["name"].as_str().unwrap()), ("0.1.0", format!("x-0.1.0{SUFFIX}").as_str()));
        // No prefix (a third-party repo): the leading "v" is dropped.
        assert_eq!(pick(&list, "", SUFFIX, false).unwrap().0, "9.9.9");
        assert!(pick(&list, "kiln-v", SUFFIX, false).is_none());
        // With test releases switched on, the pre-release is offered; drafts never are.
        let (v, _, _) = pick(&list, "beacon-v", SUFFIX, true).unwrap();
        assert_eq!(v, "0.3.0");
    }

    fn make_zip(names: &[&str]) -> Vec<u8> {
        use std::io::Write;
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for n in names {
            w.start_file(*n, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(b"x").unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn unzips_inside_its_folder_only() {
        let dir = std::env::temp_dir().join(format!("lodestone-unzip-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        unzip_into(&make_zip(&["kiln/__main__.py", "lodestone-plugin.json"]), &dir.join("ok")).unwrap();
        assert!(dir.join("ok/kiln/__main__.py").is_file());
        assert!(unzip_into(&make_zip(&["../escape.txt"]), &dir.join("bad")).is_err());
        assert!(!dir.join("escape.txt").exists());
        assert!(unzip_into(b"not a zip", &dir.join("bad2")).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn kiln_ships_its_source_as_a_zip_on_linux() {
        let k = find("kiln").unwrap();
        assert_eq!(k.suffix.ends_with(".zip"), true);
        #[cfg(not(windows))]
        assert!(find("beacon").unwrap().suffix.ends_with(".flatpak"));
    }

    #[test]
    fn only_listed_plugins_can_be_installed() {
        assert!(find("beacon").is_some());
        assert!(find("somebody/else").is_none());
    }
}
