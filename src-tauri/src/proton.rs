//! Run Windows programs through Proton via umu-launcher, no Steam needed. One Wine prefix per app/game.

use crate::model::LaunchSpec;
use crate::state;
use crate::tools;
use std::path::{Path, PathBuf};

pub fn prefixes_dir() -> PathBuf {
    state::data_dir().join("prefixes")
}

pub fn prefix(name: &str) -> PathBuf {
    let safe: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    prefixes_dir().join(safe)
}

/// A Windows path inside a prefix, e.g. `C:\Program Files (x86)\Battle.net\Battle.net.exe`.
/// Steam's Proton prefixes (compatdata/<id>) keep the Wine prefix one level down, in pfx/.
pub fn drive_c(prefix: &Path) -> PathBuf {
    // On Windows the "prefix" is the real system drive (C:\\), so paths inside it are real paths.
    if cfg!(windows) {
        return prefix.to_path_buf();
    }
    let d = prefix.join("drive_c");
    if !d.is_dir() && prefix.join("pfx/drive_c").is_dir() {
        return prefix.join("pfx/drive_c");
    }
    d
}

pub struct Run<'a> {
    pub prefix: &'a Path,
    pub exe: &'a Path,
    pub args: Vec<String>,
    /// protonfixes store name (battlenet, egs, gog, ea, ubisoft...) so per-store fixes apply.
    pub store: Option<&'a str>,
    /// umu-database id; "umu-default" when the game has none.
    pub game_id: &'a str,
}

/// Common runtimes many Windows games expect but Wine only partly reimplements:
/// DirectX helper libraries (d3dx9, shader compilers) and the Visual C++ 2015-2022 runtime.
pub const RUNTIMES: &[&str] = &["d3dx9", "d3dx10_43", "d3dx11_43", "d3dcompiler_43", "d3dcompiler_47", "vcrun2022"];

/// `umu-run winetricks -q <verbs>` in a prefix (umu runs winetricks inside Proton's runtime).
pub fn winetricks(prefix: &Path, verbs: &[&str]) -> Result<LaunchSpec, String> {
    let umu = tools::umu_path().ok_or("Proton isn't set up yet. Open Settings → Compatibility → Set up Proton.")?;
    let mut args = vec!["winetricks".to_string(), "-q".to_string()];
    args.extend(verbs.iter().map(|v| v.to_string()));
    let mut s = LaunchSpec { program: umu.to_string_lossy().into_owned(), args, ..Default::default() };
    s.env.insert("WINEPREFIX".into(), prefix.to_string_lossy().into_owned());
    s.env.insert("GAMEID".into(), "umu-default".into());
    s.env.insert("PROTONPATH".into(), tools::proton_path());
    Ok(s)
}

pub fn spec(r: Run) -> Result<LaunchSpec, String> {
    let umu = tools::umu_path().ok_or("Proton isn't set up yet. Open Settings → Compatibility → Set up Proton.")?;
    let _ = std::fs::create_dir_all(r.prefix);
    let mut args = vec![r.exe.to_string_lossy().into_owned()];
    args.extend(r.args);
    let mut s = LaunchSpec {
        program: umu.to_string_lossy().into_owned(),
        args,
        cwd: r.exe.parent().map(Path::to_path_buf),
        ..Default::default()
    };
    s.env.insert("WINEPREFIX".into(), r.prefix.to_string_lossy().into_owned());
    s.env.insert("GAMEID".into(), r.game_id.into());
    s.env.insert("PROTONPATH".into(), tools::proton_path());
    if let Some(st) = r.store {
        s.env.insert("STORE".into(), st.into());
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    /// `cargo test live_proton -- --ignored --nocapture`: installs umu and runs `cmd /c ver` through Proton.
    #[test]
    #[ignore]
    fn live_proton_runs_cmd() {
        let v = tauri::async_runtime::block_on(crate::tools::install_umu()).expect("umu install");
        println!("umu {v} at {:?}; proton {}", crate::tools::umu_path(), crate::tools::proton_path());
        let pfx = std::env::temp_dir().join(format!("lodestone-pfx-{}", std::process::id()));
        // Inside the prefix, cmd.exe lives in system32; Proton resolves bare "cmd" too.
        let spec = super::spec(super::Run { prefix: &pfx, exe: std::path::Path::new("cmd"), args: vec!["/c".into(), "ver".into()], store: None, game_id: "umu-default" }).unwrap();
        let out = std::process::Command::new(&spec.program).args(&spec.args).envs(&spec.env).env("PROTON_VERB", "run").output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        println!("exit {:?}\nstdout: {}\nstderr tail: {}", out.status.code(), text.trim(), String::from_utf8_lossy(&out.stderr).lines().rev().take(6).collect::<Vec<_>>().join(" | "));
        assert!(text.contains("Windows") || text.contains("Version"), "cmd /c ver didn't print a version");
        assert!(pfx.join("drive_c").is_dir());
        let _ = std::fs::remove_dir_all(pfx);
    }
}
