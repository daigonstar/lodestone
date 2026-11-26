//! Installing and updating Lodestone Beacon from its official releases (on Lodestone's own GitHub, tagged
//! `beacon-v<version>`). Only when you ask: Lodestone looks for a newer version when you open Plug-ins or
//! Beacon, and installs or updates only when you press the button.
//!
//! Every download is checked against the SHA-256 checksum GitHub publishes for the file before anything
//! is installed; a release file without one is refused.
//!
//! - Linux: the release's Flatpak bundle, installed for your user with `flatpak install --user` (its
//!   runtime comes from Flathub).
//! - Windows: the release's zip, unpacked into Lodestone's own folder (%LOCALAPPDATA%\Lodestone\Beacon),
//!   one folder per version, with `current.txt` naming the one in use.

use crate::net;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

const RELEASES: &str = "https://api.github.com/repos/daigonstar/lodestone/releases?per_page=30";
const TAG: &str = "beacon-v";

#[cfg(not(windows))]
fn asset_name(version: &str) -> String {
    format!("lodestone-beacon-{version}-linux-x86_64.flatpak")
}
#[cfg(windows)]
fn asset_name(version: &str) -> String {
    format!("lodestone-beacon-{version}-windows-x86_64.zip")
}

/// The newest Beacon release: its version and this platform's file.
async fn latest() -> Result<(String, Value), String> {
    let list = net::get_json(RELEASES, &[("accept", "application/vnd.github+json")]).await?;
    let rel = list
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["draft"] != json!(true))
        .find(|r| r["tag_name"].as_str().is_some_and(|t| t.starts_with(TAG)))
        .ok_or("No Beacon release has been published yet")?;
    let version = rel["tag_name"].as_str().unwrap_or_default()[TAG.len()..].to_string();
    if !version.chars().all(|c| c.is_ascii_alphanumeric() || ".-".contains(c)) || version.is_empty() {
        return Err("That release's version isn't one Lodestone understands".into());
    }
    let want = asset_name(&version);
    let asset = rel["assets"].as_array().into_iter().flatten().find(|a| a["name"].as_str() == Some(want.as_str())).cloned().ok_or_else(|| format!("Beacon {version} has no build for this system yet"))?;
    Ok((version, asset))
}

/// Where Lodestone notes which Beacon version it installed.
fn marker() -> PathBuf {
    crate::state::data_dir().join("plugins").join("beacon-version.txt")
}

/// The Beacon version Lodestone installed, if it did.
pub fn installed_version() -> Option<String> {
    fs::read_to_string(marker()).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// "0.2.10" > "0.2.9": compare the numbers, not the text.
fn newer(a: &str, b: &str) -> bool {
    let parse = |v: &str| v.split(['.', '-']).map(|p| p.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    parse(a) > parse(b)
}

/// Is there a Beacon to install, or a newer one than yours? (Only asked when you open Plug-ins or Beacon.)
pub async fn check(running_version: Option<String>) -> Result<Value, String> {
    let (version, _) = latest().await?;
    let have = running_version.or_else(installed_version);
    let update = have.as_deref().is_some_and(|h| newer(&version, h));
    Ok(json!({ "latest": version, "installed": have, "update": update }))
}

/// Download, check and install the newest Beacon (or update to it). Returns the version installed.
pub async fn install() -> Result<String, String> {
    let (version, asset) = latest().await?;
    // A Beacon file must carry GitHub's checksum (net::verify_release_file accepts old files without one).
    if !asset["digest"].as_str().is_some_and(|d| d.starts_with("sha256:")) {
        return Err("That Beacon release has no published checksum, so Lodestone won't install it".into());
    }
    let url = asset["browser_download_url"].as_str().ok_or("no download address")?;
    if !url.starts_with("https://github.com/daigonstar/lodestone/releases/download/") {
        return Err("That download isn't from Lodestone's own releases".into());
    }
    let bytes = net::client().get(url).send().await.map_err(net::err)?.error_for_status().map_err(net::err)?.bytes().await.map_err(net::err)?;
    net::verify_release_file(&asset, &bytes)?;
    // A running Beacon is stopped first (it's replaced underneath it).
    let _ = crate::plugins::stop_plugin(crate::plugins::BEACON);
    install_file(&version, &bytes).await?;
    if let Some(d) = marker().parent() {
        let _ = fs::create_dir_all(d);
    }
    let _ = fs::write(marker(), &version);
    Ok(version)
}

#[cfg(not(windows))]
async fn install_file(version: &str, bytes: &[u8]) -> Result<(), String> {
    use std::process::{Command, Stdio};
    if !bytes.starts_with(b"flatpak") {
        return Err("That download isn't a Flatpak bundle".into());
    }
    let dir = crate::state::data_dir().join("downloads");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = dir.join(format!("lodestone-beacon-{version}.flatpak"));
    fs::write(&file, bytes).map_err(|e| e.to_string())?;
    // Installed for you (not system-wide), with its runtime from Flathub; from inside Lodestone's own
    // Flatpak this runs on the host, like Lodestone's other Flatpak commands.
    let mut c = Command::new("flatpak");
    c.args(["install", "--user", "-y", "--noninteractive"]).arg(&file);
    let out = tauri::async_runtime::spawn_blocking(move || crate::host::wrap(c).stdin(Stdio::null()).output())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("Couldn't run flatpak: {e}"))?;
    let _ = fs::remove_file(&file);
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr);
        return Err(format!("Flatpak couldn't install Beacon: {}", why.lines().last().unwrap_or("unknown error")));
    }
    Ok(())
}

/// Windows: Beacon' folders, %LOCALAPPDATA%\Lodestone\Beacon.
#[cfg(windows)]
pub fn windows_dir() -> PathBuf {
    dirs::data_local_dir().unwrap_or_default().join("Lodestone").join("Beacon")
}

/// Windows: the Beacon program in use, if installed.
#[cfg(windows)]
pub fn windows_exe() -> Option<PathBuf> {
    let current = fs::read_to_string(windows_dir().join("current.txt")).ok()?;
    let exe = windows_dir().join(current.trim()).join("Lodestone Beacon").join("bin").join("lodestone-beacon.exe");
    exe.is_file().then_some(exe)
}

#[cfg(windows)]
async fn install_file(version: &str, bytes: &[u8]) -> Result<(), String> {
    let base = windows_dir();
    let target = base.join(version);
    let _ = fs::remove_dir_all(&target);
    fs::create_dir_all(&target).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|_| "That download isn't a zip".to_string())?;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| e.to_string())?;
        // Only names inside the target folder (no "..", no absolute paths).
        let Some(rel) = f.enclosed_name() else { return Err("The zip has a file outside its folder".into()) };
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
    if !target.join("Lodestone Beacon").join("bin").join("lodestone-beacon.exe").is_file() {
        return Err("That zip doesn't contain Beacon".into());
    }
    fs::write(base.join("current.txt"), version).map_err(|e| e.to_string())?;
    // Older versions are removed once the new one is in place.
    for e in fs::read_dir(&base).map_err(|e| e.to_string())?.flatten() {
        if e.path().is_dir() && e.file_name() != version {
            let _ = fs::remove_dir_all(e.path());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    /// Windows: install a Beacon zip from disk the way a download is installed (touches the real
    /// %LOCALAPPDATA%, so only on purpose): LODESTONE_TEST_BEACON_ZIP=<zip> cargo test -- --ignored
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn installs_a_windows_zip() {
        let zip = std::env::var("LODESTONE_TEST_BEACON_ZIP").expect("set LODESTONE_TEST_BEACON_ZIP");
        let bytes = std::fs::read(zip).unwrap();
        tauri::async_runtime::block_on(super::install_file("0.0.0-test", &bytes)).unwrap();
        let exe = super::windows_exe().expect("Beacon found after installing");
        assert!(exe.to_string_lossy().contains("0.0.0-test"));
    }

    #[test]
    fn compares_versions_as_numbers() {
        assert!(super::newer("0.2.10", "0.2.9"));
        assert!(super::newer("0.3.0", "0.2.99"));
        assert!(!super::newer("0.2.0", "0.2.0"));
        assert!(!super::newer("0.1.9", "0.2.0"));
    }
}
