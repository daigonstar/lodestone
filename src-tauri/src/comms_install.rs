//! Installing and updating Lodestone Comms from its official releases (on Lodestone's own GitHub, tagged
//! `comms-v<version>`). Only when you ask: Lodestone looks for a newer version when you open Plug-ins or
//! Comms, and installs or updates only when you press the button.
//!
//! Every download is checked against the SHA-256 checksum GitHub publishes for the file before anything
//! is installed; a release file without one is refused.
//!
//! - Linux: the release's Flatpak bundle, installed for your user with `flatpak install --user` (its
//!   runtime comes from Flathub).
//! - Windows: the release's zip, unpacked into Lodestone's own folder (%LOCALAPPDATA%\Lodestone\Comms),
//!   one folder per version, with `current.txt` naming the one in use.

use crate::plugin_sources::{self, newer, Source};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

fn source() -> Source {
    plugin_sources::find("comms").expect("Comms is an official plug-in")
}

/// Where Lodestone notes which Comms version it installed.
fn marker() -> PathBuf {
    crate::state::data_dir().join("plugins").join("comms-version.txt")
}

/// The Comms version Lodestone installed, if it did.
pub fn installed_version() -> Option<String> {
    fs::read_to_string(marker()).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Is there a Comms to install, or a newer one than yours? (Only asked when you open Plug-ins or Comms.)
pub async fn check(running_version: Option<String>) -> Result<Value, String> {
    let rel = plugin_sources::latest(&source()).await.map_err(|e| if e.contains("no release") { "No Comms release has been published yet".to_string() } else { e })?;
    let have = running_version.or_else(installed_version);
    let update = have.as_deref().is_some_and(|h| newer(&rel.version, h));
    Ok(json!({ "latest": rel.version, "installed": have, "update": update }))
}

/// Download, check and install the newest Comms (or update to it). Returns the version installed.
pub async fn install() -> Result<String, String> {
    let src = source();
    let rel = plugin_sources::latest(&src).await?;
    let bytes = plugin_sources::fetch_verified(&src, &rel).await?;
    let version = rel.version;
    // A running Comms is stopped first (it's replaced underneath it).
    let _ = crate::plugins::stop_plugin(crate::plugins::COMMS);
    install_file(&version, &bytes).await?;
    if let Some(d) = marker().parent() {
        let _ = fs::create_dir_all(d);
    }
    let _ = fs::write(marker(), &version);
    Ok(version)
}

#[cfg(not(windows))]
async fn install_file(version: &str, bytes: &[u8]) -> Result<(), String> {
    plugin_sources::install_flatpak("Comms", version, bytes).await
}

/// Windows: Comms' folders, %LOCALAPPDATA%\Lodestone\Comms.
#[cfg(windows)]
pub fn windows_dir() -> PathBuf {
    dirs::data_local_dir().unwrap_or_default().join("Lodestone").join("Comms")
}

/// Windows: the Comms program in use, if installed.
#[cfg(windows)]
pub fn windows_exe() -> Option<PathBuf> {
    let current = fs::read_to_string(windows_dir().join("current.txt")).ok()?;
    let exe = windows_dir().join(current.trim()).join("Lodestone Comms").join("bin").join("lodestone-comms.exe");
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
    if !target.join("Lodestone Comms").join("bin").join("lodestone-comms.exe").is_file() {
        return Err("That zip doesn't contain Comms".into());
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
    /// Windows: install a Comms zip from disk the way a download is installed (touches the real
    /// %LOCALAPPDATA%, so only on purpose): LODESTONE_TEST_COMMS_ZIP=<zip> cargo test -- --ignored
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn installs_a_windows_zip() {
        let zip = std::env::var("LODESTONE_TEST_COMMS_ZIP").expect("set LODESTONE_TEST_COMMS_ZIP");
        let bytes = std::fs::read(zip).unwrap();
        tauri::async_runtime::block_on(super::install_file("0.0.0-test", &bytes)).unwrap();
        let exe = super::windows_exe().expect("Comms found after installing");
        assert!(exe.to_string_lossy().contains("0.0.0-test"));
    }
}
