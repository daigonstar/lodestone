//! Tools Lodestone manages itself in ~/.local/share/lodestone/tools: umu-launcher (runs Proton outside Steam).
//! Plus discovery of Proton builds already on disk.

use crate::model::{home, which};
use crate::net::{self, get_json};
use crate::state;
use serde::Serialize;
use std::fs;
use std::path::PathBuf;

/// PROTONPATH value that makes umu download and use the newest GE-Proton itself.
pub const AUTO_GE: &str = "GE-Proton";

fn tools_dir() -> PathBuf {
    state::data_dir().join("tools")
}

fn umu_dir() -> PathBuf {
    tools_dir().join("umu")
}

/// Managed copy first, then a system install.
pub fn umu_path() -> Option<PathBuf> {
    let managed = umu_dir().join("current/umu-run");
    if managed.is_file() {
        return Some(managed);
    }
    which("umu-run")
}

pub fn umu_version() -> Option<String> {
    fs::read_to_string(umu_dir().join("version.txt")).ok().map(|s| s.trim().to_string()).or_else(|| which("umu-run").map(|_| "system".into()))
}

/// Download (or update to) the latest umu-launcher zipapp from its official GitHub releases.
pub async fn install_umu() -> Result<String, String> {
    let rel = get_json("https://api.github.com/repos/Open-Wine-Components/umu-launcher/releases/latest", &[("accept", "application/vnd.github+json")]).await?;
    let tag = rel["tag_name"].as_str().ok_or("no release tag")?.to_string();
    if fs::read_to_string(umu_dir().join("version.txt")).ok().as_deref().map(str::trim) == Some(tag.as_str()) && umu_dir().join("current/umu-run").is_file() {
        return Ok(tag);
    }
    let asset = rel["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|a| a["name"].as_str().is_some_and(|n| n.ends_with("-zipapp.tar")))
        .ok_or("release has no zipapp")?;
    let url = asset["browser_download_url"].as_str().ok_or("no download url")?;
    let bytes = net::client().get(url).send().await.map_err(net::err)?.error_for_status().map_err(net::err)?.bytes().await.map_err(net::err)?;
    net::verify_release_file(asset, &bytes)?;
    let dir = umu_dir();
    let stage = dir.join(format!("stage-{tag}"));
    let _ = fs::remove_dir_all(&stage);
    fs::create_dir_all(&stage).map_err(net::err)?;
    let tar = stage.join("umu.tar");
    fs::write(&tar, &bytes).map_err(net::err)?;
    let ok = std::process::Command::new("tar").arg("-xf").arg(&tar).arg("-C").arg(&stage).status().map(|s| s.success()).unwrap_or(false);
    if !ok {
        return Err("couldn't unpack umu-launcher".into());
    }
    // The archive contains umu/umu-run.
    let run = walk_find(&stage, "umu-run").ok_or("umu-run missing from archive")?;
    let target = dir.join(&tag);
    let _ = fs::remove_dir_all(&target);
    fs::rename(run.parent().unwrap(), &target).map_err(net::err)?;
    let _ = fs::remove_dir_all(&stage);
    let current = dir.join("current");
    let _ = fs::remove_file(&current);
    link_dir(&target, &current).map_err(net::err)?;
    fs::write(dir.join("version.txt"), &tag).map_err(net::err)?;
    Ok(tag)
}

fn walk_find(dir: &std::path::Path, name: &str) -> Option<PathBuf> {
    for e in fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.is_dir() {
            if let Some(f) = walk_find(&p, name) {
                return Some(f);
            }
        } else if e.file_name() == name {
            return Some(p);
        }
    }
    None
}

#[derive(Serialize, Clone)]
pub struct Proton {
    pub name: String,
    pub path: String,
}

/// Proton builds already on disk (Steam's, ProtonUp-Qt's, Lodestone's, umu's own downloads).
pub fn protons() -> Vec<Proton> {
    let h = home();
    let mut dirs = vec![
        h.join(".local/share/Steam/compatibilitytools.d"),
        h.join(".steam/root/compatibilitytools.d"),
        h.join(".var/app/com.valvesoftware.Steam/.local/share/Steam/compatibilitytools.d"),
        h.join(".local/share/umu/compatibilitytools"),
        state::data_dir().join("proton"),
    ];
    for lib in crate::stores::steam::library_dirs() {
        dirs.push(lib.join("steamapps/common"));
    }
    let mut out: Vec<Proton> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for d in dirs {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if !p.join("proton").is_file() {
                continue;
            }
            let canon = fs::canonicalize(&p).unwrap_or(p.clone());
            if seen.insert(canon) {
                out.push(Proton { name: e.file_name().to_string_lossy().into_owned(), path: p.to_string_lossy().into_owned() });
            }
        }
    }
    // Newest-looking first: GE builds by version number, then the rest.
    out.sort_by(|a, b| natural_key(&b.name).cmp(&natural_key(&a.name)));
    out
}

fn natural_key(s: &str) -> (bool, Vec<u64>) {
    let nums = s.split(|c: char| !c.is_ascii_digit()).filter(|x| !x.is_empty()).filter_map(|x| x.parse().ok()).collect();
    (s.starts_with("GE-Proton"), nums)
}

/// The PROTONPATH to use: the user's choice, else the newest GE-Proton on disk, else umu's auto-download.
pub fn proton_path() -> String {
    let chosen = crate::settings::load().proton;
    if !chosen.is_empty() && (chosen == AUTO_GE || PathBuf::from(&chosen).join("proton").is_file()) {
        return chosen;
    }
    protons().into_iter().find(|p| p.name.starts_with("GE-Proton")).map(|p| p.path).unwrap_or_else(|| AUTO_GE.to_string())
}

// ---------------- single-binary tools: legendary (Epic), gogdl (GOG) ----------------

pub struct BinTool {
    pub name: &'static str,
    pub repo: &'static str,
    /// The release file for Linux.
    pub asset: &'static str,
    /// The release file for Windows: the first of these names the release has.
    pub win_assets: &'static [&'static str],
}

pub const LEGENDARY: BinTool = BinTool { name: "legendary", repo: "derrod/legendary", asset: "legendary_linux_x64", win_assets: &["legendary.exe", "legendary_windows_x86_64.exe"] };
pub const GOGDL: BinTool = BinTool { name: "gogdl", repo: "Heroic-Games-Launcher/heroic-gogdl", asset: "gogdl_linux_x86_64", win_assets: &["gogdl_windows_x86_64.exe", "gogdl.exe"] };

/// What a program file for this system starts with: "MZ" on Windows; on Linux a compiled program
/// (ELF) or a script with a #! line (legendary ships as a self-contained Python zipapp).
fn is_program_for_this_os(bytes: &[u8]) -> bool {
    if cfg!(windows) {
        bytes.starts_with(b"MZ")
    } else {
        bytes.starts_with(b"\x7fELF") || bytes.starts_with(b"#!")
    }
}

const OS_NAME: &str = if cfg!(windows) { "Windows" } else { "Linux" };

impl BinTool {
    fn dir(&self) -> PathBuf {
        tools_dir().join(self.name)
    }

    fn file_name(&self) -> String {
        if cfg!(windows) {
            format!("{}.exe", self.name)
        } else {
            self.name.to_string()
        }
    }

    pub fn path(&self) -> Option<PathBuf> {
        let p = self.dir().join(self.file_name());
        p.is_file().then_some(p)
    }

    pub fn version(&self) -> Option<String> {
        self.path()?;
        fs::read_to_string(self.dir().join("version.txt")).ok().map(|s| s.trim().to_string())
    }

    /// Download (or update to) the latest official release binary.
    pub async fn install(&self) -> Result<String, String> {
        let rel = get_json(&format!("https://api.github.com/repos/{}/releases/latest", self.repo), &[("accept", "application/vnd.github+json")]).await?;
        let tag = rel["tag_name"].as_str().ok_or("no release tag")?.to_string();
        if self.version().as_deref() == Some(tag.as_str()) {
            return Ok(tag);
        }
        let wanted: Vec<&str> = if cfg!(windows) { self.win_assets.to_vec() } else { vec![self.asset] };
        let assets: Vec<&serde_json::Value> = rel["assets"].as_array().into_iter().flatten().collect();
        let asset = wanted
            .iter()
            .find_map(|w| assets.iter().find(|a| a["name"].as_str() == Some(*w)))
            .ok_or_else(|| format!("{} {tag} has no {OS_NAME} build", self.name))?;
        let url = asset["browser_download_url"].as_str().ok_or("no download url")?;
        let bytes = net::client().get(url).send().await.map_err(net::err)?.error_for_status().map_err(net::err)?.bytes().await.map_err(net::err)?;
        net::verify_release_file(asset, &bytes)?;
        if !is_program_for_this_os(&bytes) {
            return Err(format!("{} download isn't a {OS_NAME} program", self.name));
        }
        let dir = self.dir();
        fs::create_dir_all(&dir).map_err(net::err)?;
        let tmp = dir.join(format!("{}.part", self.name));
        fs::write(&tmp, &bytes).map_err(net::err)?;
        crate::os::make_executable(&tmp).map_err(net::err)?;
        let dest = dir.join(self.file_name());
        // Windows won't rename over an existing file.
        if cfg!(windows) && dest.exists() {
            fs::remove_file(&dest).map_err(net::err)?;
        }
        fs::rename(&tmp, dest).map_err(net::err)?;
        fs::write(dir.join("version.txt"), &tag).map_err(net::err)?;
        Ok(tag)
    }

    pub async fn ensure(&self) -> Result<PathBuf, String> {
        if let Some(p) = self.path() {
            return Ok(p);
        }
        self.install().await?;
        self.path().ok_or_else(|| format!("{} didn't install", self.name))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore]
    fn live_install_bin_tools() {
        for t in [super::LEGENDARY, super::GOGDL] {
            let v = tauri::async_runtime::block_on(t.install()).unwrap();
            println!("{} {v} at {:?}", t.name, t.path());
        }
    }
}

/// Point `link` at `target` (the "current" version of a tool). These tools (Wine, Proton and the
/// like) are only used on Linux.
pub fn link_dir(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(not(unix))]
    {
        let _ = (target, link);
        Err(std::io::Error::other("only used on Linux"))
    }
}
