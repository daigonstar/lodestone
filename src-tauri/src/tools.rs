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
    std::os::unix::fs::symlink(&target, &current).map_err(net::err)?;
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
    pub asset: &'static str,
}

pub const LEGENDARY: BinTool = BinTool { name: "legendary", repo: "derrod/legendary", asset: "legendary_linux_x64" };
pub const GOGDL: BinTool = BinTool { name: "gogdl", repo: "Heroic-Games-Launcher/heroic-gogdl", asset: "gogdl_linux_x86_64" };

impl BinTool {
    fn dir(&self) -> PathBuf {
        tools_dir().join(self.name)
    }

    pub fn path(&self) -> Option<PathBuf> {
        let p = self.dir().join(self.name);
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
        let asset = rel["assets"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|a| a["name"].as_str() == Some(self.asset))
            .ok_or_else(|| format!("{} {tag} has no Linux build", self.name))?;
        let url = asset["browser_download_url"].as_str().ok_or("no download url")?;
        let bytes = net::client().get(url).send().await.map_err(net::err)?.error_for_status().map_err(net::err)?.bytes().await.map_err(net::err)?;
        // A compiled program, or a self-contained Python zipapp (legendary ships as one).
        if !(bytes.starts_with(b"\x7fELF") || bytes.starts_with(b"#!")) {
            return Err(format!("{} download isn't a Linux program", self.name));
        }
        let dir = self.dir();
        fs::create_dir_all(&dir).map_err(net::err)?;
        let tmp = dir.join(format!("{}.part", self.name));
        fs::write(&tmp, &bytes).map_err(net::err)?;
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755)).map_err(net::err)?;
        fs::rename(&tmp, dir.join(self.name)).map_err(net::err)?;
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
