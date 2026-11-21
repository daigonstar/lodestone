//! MangoHud, the in-game performance overlay (FPS, frame times, CPU/GPU load).
//!
//! Uses MangoHud from the system when it's installed. Otherwise Lodestone sets up the official release
//! in its tools folder (no root needed) and registers its Vulkan layer for your user, in
//! ~/.local/share/vulkan/implicit_layer.d. The layer only switches on for games started with
//! MANGOHUD=1, so nothing changes for other programs.
//!
//! Proton games (DXVK/VKD3D draw with Vulkan) get the layer; native games also get MangoHud's
//! OpenGL hook. Steam starts its own games, so for those set `mangohud %command%` in Steam.

use crate::model::{home, which, LaunchSpec};
use crate::net;
use crate::state;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

fn dir() -> PathBuf {
    state::data_dir().join("tools/mangohud")
}

fn layer_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/share")).join("vulkan/implicit_layer.d")
}

/// Lodestone's own copy: (lib folder holding lib64/ and lib32/, version).
fn managed() -> Option<(PathBuf, String)> {
    let v = fs::read_to_string(dir().join("version.txt")).ok()?.trim().to_string();
    let lib = dir().join("current/usr/lib/mangohud");
    lib.join("lib64/libMangoHud.so").is_file().then_some((lib, v))
}

/// MangoHud installed from the system's packages.
fn system() -> bool {
    which("mangohud").is_some()
}

#[derive(Serialize)]
pub struct Info {
    /// "system", a version Lodestone set up, or None.
    pub installed: Option<String>,
    /// Shown in every game unless a game turns it off.
    pub all_games: bool,
}

pub fn info() -> Info {
    let installed = if system() { Some("system".into()) } else { managed().map(|m| m.1) };
    Info { installed, all_games: crate::settings::load().mangohud }
}

/// Download (or update to) the latest MangoHud release from its official GitHub page.
pub async fn install() -> Result<String, String> {
    let rel = net::get_json("https://api.github.com/repos/flightlessmango/MangoHud/releases/latest", &[("accept", "application/vnd.github+json")]).await?;
    let tag = rel["tag_name"].as_str().ok_or("no release tag")?.to_string();
    if managed().is_some_and(|m| m.1 == tag) {
        register(&managed().unwrap().0)?;
        return Ok(tag);
    }
    // The prebuilt archive, not the source ones.
    let asset = rel["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|a| a["name"].as_str().is_some_and(|n| n.starts_with("MangoHud-") && n.ends_with(".tar.gz") && !n.contains("Source")))
        .ok_or("this MangoHud release has no prebuilt download")?;
    let url = asset["browser_download_url"].as_str().ok_or("no download url")?;
    let bytes = net::client().get(url).send().await.map_err(net::err)?.error_for_status().map_err(net::err)?.bytes().await.map_err(net::err)?;
    let stage = dir().join(format!("stage-{tag}"));
    let _ = fs::remove_dir_all(&stage);
    fs::create_dir_all(&stage).map_err(net::err)?;
    let tgz = stage.join("mangohud.tar.gz");
    fs::write(&tgz, &bytes).map_err(net::err)?;
    let untar = |archive: &Path, to: &Path| std::process::Command::new("tar").arg("-xf").arg(archive).arg("-C").arg(to).status().map(|s| s.success()).unwrap_or(false);
    // MangoHud/MangoHud-package.tar holds the usr/ tree.
    let pkg = stage.join("MangoHud/MangoHud-package.tar");
    let target = dir().join(&tag);
    let _ = fs::remove_dir_all(&target);
    fs::create_dir_all(&target).map_err(net::err)?;
    if !untar(&tgz, &stage) || !pkg.is_file() || !untar(&pkg, &target) {
        let _ = fs::remove_dir_all(&stage);
        return Err("couldn't unpack MangoHud".into());
    }
    let _ = fs::remove_dir_all(&stage);
    let current = dir().join("current");
    let _ = fs::remove_file(&current);
    std::os::unix::fs::symlink(&target, &current).map_err(net::err)?;
    fs::write(dir().join("version.txt"), &tag).map_err(net::err)?;
    register(&current.join("usr/lib/mangohud"))?;
    Ok(tag)
}

/// Point the user's Vulkan layer files at Lodestone's copy (64- and 32-bit games).
fn register(lib: &Path) -> Result<(), String> {
    fs::create_dir_all(layer_dir()).map_err(net::err)?;
    for (arch, sub) in [("x86_64", "lib64"), ("x86", "lib32")] {
        let so = lib.join(sub).join("libMangoHud.so");
        if !so.is_file() {
            continue;
        }
        let json = serde_json::json!({
            "file_format_version": "1.0.0",
            "layer": {
                "name": format!("VK_LAYER_MANGOHUD_overlay_{arch}"),
                "type": "GLOBAL",
                "api_version": "1.3.0",
                "library_path": so.to_string_lossy(),
                "implementation_version": "1",
                "description": "Vulkan Hud Overlay (set up by Lodestone)",
                "functions": { "vkNegotiateLoaderLayerInterfaceVersion": "vkNegotiateLoaderLayerInterfaceVersion" },
                "enable_environment": { "MANGOHUD": "1" },
                "disable_environment": { "DISABLE_MANGOHUD": "1" }
            }
        });
        fs::write(layer_dir().join(format!("MangoHud.lodestone.{arch}.json")), serde_json::to_string_pretty(&json).unwrap()).map_err(net::err)?;
    }
    Ok(())
}

/// MangoHud's own settings file (used by every program MangoHud runs in).
pub fn config_path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config")).join("MangoHud/MangoHud.conf")
}

pub fn config_get() -> (String, String) {
    let p = config_path();
    (p.to_string_lossy().into_owned(), fs::read_to_string(&p).unwrap_or_default())
}

/// Save the settings file (the old one is kept as MangoHud.conf.bak the first time).
pub fn config_save(text: &str) -> Result<(), String> {
    let p = config_path();
    fs::create_dir_all(p.parent().unwrap()).map_err(net::err)?;
    let bak = p.with_extension("conf.bak");
    if p.is_file() && !bak.exists() {
        let _ = fs::copy(&p, &bak);
    }
    let mut t = text.replace("\r\n", "\n");
    if !t.ends_with('\n') {
        t.push('\n');
    }
    fs::write(&p, t).map_err(net::err)
}

/// Show the overlay on a small test window (vkcube), so you can see your settings.
pub fn preview() -> Result<LaunchSpec, String> {
    if info().installed.is_none() {
        return Err("Set up MangoHud first".into());
    }
    let cube = which("vkcube").ok_or("vkcube isn't installed (it comes with the vulkan-tools package)")?;
    let mut s = LaunchSpec::new(cube.to_string_lossy(), &[]);
    s.env.insert("MANGOHUD".into(), "1".into());
    Ok(s)
}

/// Whether this launch runs through Proton/Wine (the Vulkan layer is enough there).
fn is_proton(spec: &LaunchSpec) -> bool {
    spec.env.contains_key("WINEPREFIX") || spec.program.ends_with("umu-run")
}

/// Add MangoHud to a launch. Returns the spec unchanged when MangoHud isn't available.
pub fn apply(mut spec: LaunchSpec) -> LaunchSpec {
    let sys = system();
    let own = if sys { None } else { managed() };
    if !sys && own.is_none() {
        return spec;
    }
    spec.env.insert("MANGOHUD".into(), "1".into());
    if is_proton(&spec) {
        return spec;
    }
    // Native games: MangoHud's own wrapper (system) or its OpenGL hook (Lodestone's copy).
    if sys {
        if let Some(m) = which("mangohud") {
            let prog = std::mem::replace(&mut spec.program, m.to_string_lossy().into_owned());
            spec.args.insert(0, prog);
        }
    } else if let Some((lib, _)) = own {
        // The loader skips the copy that doesn't match the game (32- or 64-bit).
        let shims = [lib.join("lib64/libMangoHud_shim.so"), lib.join("lib32/libMangoHud_shim.so")];
        let mut pre: Vec<String> = shims.iter().filter(|p| p.is_file()).map(|p| p.to_string_lossy().into_owned()).collect();
        if let Some(old) = spec.env.get("LD_PRELOAD").filter(|s| !s.is_empty()) {
            pre.push(old.clone());
        }
        spec.env.insert("LD_PRELOAD".into(), pre.join(":"));
    }
    spec
}

#[cfg(test)]
mod tests {
    use crate::model::LaunchSpec;

    #[test]
    fn proton_launches_only_get_the_layer_switch() {
        let mut s = LaunchSpec::new("/x/umu-run", &["game.exe"]);
        s.env.insert("WINEPREFIX".into(), "/p".into());
        assert!(super::is_proton(&s));
        assert!(!super::is_proton(&LaunchSpec::new("/games/native/start.sh", &[])));
    }
}

#[cfg(test)]
mod live {
    /// `cargo test live_mangohud -- --ignored --nocapture`: sets up MangoHud for real.
    #[test]
    #[ignore]
    fn live_mangohud_setup() {
        let v = tauri::async_runtime::block_on(super::install()).unwrap();
        println!("installed {v}; info {:?}", super::info().installed);
        let s = super::apply(crate::model::LaunchSpec::new("/games/native/run.sh", &[]));
        println!("native env: {:?}", s.env);
    }
}
