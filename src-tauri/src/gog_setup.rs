//! What GOG Galaxy does after an install and gogdl doesn't: the game's `goggame-<id>.script`
//! (registry keys, save folders, first-run copies, small fix-up programs) and the "support" files gogdl
//! leaves in `gog-support/<id>/app`. Many older games quit at once without these (SimCity 4 needs its
//! `Software\Maxis` keys and Graphics Rules.sgr).
//!
//! It runs once per prefix and game build, just before the game starts: the launch goes through
//! `lodestone --gog-setup <id> <game dir> <umu-run> -- <launch command>`, which sets up, then becomes the game.

use crate::model::LaunchSpec;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const FLAG: &str = "--gog-setup";

fn marker(prefix: &Path, id: &str) -> PathBuf {
    prefix.join(format!(".lodestone-gog-setup-{id}"))
}

/// The game's build, so an update that changes the script runs it again.
fn build(game: &Path, id: &str) -> String {
    let info = fs::read_to_string(game.join(format!("goggame-{id}.info"))).unwrap_or_default();
    let script = fs::read(game.join(format!("goggame-{id}.script"))).unwrap_or_default();
    let v: Value = serde_json::from_str(&info).unwrap_or_default();
    format!("{}:{}", v["buildId"].as_str().unwrap_or(""), script.len())
}

pub fn needed(prefix: &Path, game: &Path, id: &str) -> bool {
    let has_work = game.join(format!("goggame-{id}.script")).is_file() || game.join("gog-support").join(id).join("app").is_dir();
    has_work && fs::read_to_string(marker(prefix, id)).ok().as_deref() != Some(build(game, id).as_str())
}

/// Wrap a launch so setup runs first (only when it's still needed).
pub fn wrap(spec: LaunchSpec, prefix: &Path, game: &Path, id: &str) -> LaunchSpec {
    let Some((program, mut args)) = crate::host::self_command() else { return spec };
    if !needed(prefix, game, id) {
        return spec;
    }
    let Some(umu) = crate::tools::umu_path() else { return spec };
    args.extend([FLAG.to_string(), id.to_string(), game.to_string_lossy().into_owned(), umu.to_string_lossy().into_owned(), "--".into(), spec.program.clone()]);
    args.extend(spec.args.iter().cloned());
    let mut env = spec.env;
    let keys: Vec<String> = env.keys().cloned().collect();
    env.insert(crate::host::FWD_ENV.into(), keys.join(","));
    LaunchSpec { program, args, env, cwd: spec.cwd }
}

/// `lodestone --gog-setup <id> <game dir> <umu-run> -- <program> <args...>`: set up, then exec the program.
/// Runs with the game's environment (WINEPREFIX, PROTONPATH...) already in place.
pub fn main_hook() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) != Some(FLAG) {
        return;
    }
    let (Some(id), Some(game), Some(umu)) = (args.get(2), args.get(3), args.get(4)) else { std::process::exit(2) };
    let rest = args.iter().skip_while(|a| *a != "--").skip(1).cloned().collect::<Vec<_>>();
    let prefix = std::env::var_os("WINEPREFIX").map(PathBuf::from);
    if let Some(pfx) = prefix {
        match run(Path::new(game), id, &pfx, umu) {
            Ok(()) => println!("GOG setup done"),
            Err(e) => eprintln!("GOG setup: {e}"),
        }
    }
    let Some((prog, a)) = rest.split_first() else { std::process::exit(2) };
    use std::os::unix::process::CommandExt;
    let mut c = Command::new(prog);
    c.args(a);
    let e = crate::host::wrap(c).exec();
    eprintln!("couldn't start {prog}: {e}");
    std::process::exit(1);
}

fn win_path(p: &Path) -> String {
    format!("Z:{}", p.to_string_lossy().replace('/', "\\"))
}

/// Fill in Galaxy's placeholders.
fn expand(s: &str, game: &Path, docs: &Path, support: &Path, windows: bool) -> String {
    let p = |x: &Path| if windows { win_path(x) } else { x.to_string_lossy().into_owned() };
    s.replace("{app}", &p(game)).replace("{supportDir}", &p(support)).replace("{userdocs}", &p(docs)).replace("{group}", "GOG.com").replace("{{", "{")
}

fn reg_string(s: &str) -> String {
    // Galaxy scripts double backslashes inconsistently; registry values want single ones.
    let mut v = s.to_string();
    while v.contains("\\\\") {
        v = v.replace("\\\\", "\\");
    }
    format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The script's setRegistry actions as a .reg file.
fn reg_file(actions: &[Value], game: &Path, docs: &Path, support: &Path) -> Option<String> {
    let mut out = String::from("Windows Registry Editor Version 5.00\n");
    let mut any = false;
    for a in actions {
        let i = &a["install"];
        if i["action"].as_str() != Some("setRegistry") {
            continue;
        }
        let r = &i["arguments"];
        let root = match r["root"].as_str().unwrap_or("HKEY_LOCAL_MACHINE") {
            "HKEY_CURRENT_USER" | "HKCU" => "HKEY_CURRENT_USER",
            _ => "HKEY_LOCAL_MACHINE",
        };
        let Some(key) = r["subkey"].as_str() else { continue };
        let data = expand(r["valueData"].as_str().unwrap_or(""), game, docs, support, true);
        let value = match r["valueType"].as_str().unwrap_or("string") {
            "dword" => format!("dword:{:0>8}", data.trim_start_matches('$').trim_start_matches("0x").to_lowercase()),
            _ => reg_string(&data),
        };
        let name = r["valueName"].as_str().filter(|n| !n.is_empty()).map(|n| format!("\"{n}\"")).unwrap_or_else(|| "@".into());
        out.push_str(&format!("\n[{root}\\{key}]\n{name}={value}\n"));
        // 32-bit games read HKLM\Software through WOW6432Node in a 64-bit prefix.
        if root == "HKEY_LOCAL_MACHINE" {
            if let Some(rest) = key.strip_prefix("Software\\").or_else(|| key.strip_prefix("SOFTWARE\\")) {
                out.push_str(&format!("\n[{root}\\Software\\Wow6432Node\\{rest}]\n{name}={value}\n"));
            }
        }
        any = true;
    }
    any.then_some(out)
}

fn copy_tree(from: &Path, to: &Path, overwrite: bool) -> std::io::Result<()> {
    if from.is_dir() {
        fs::create_dir_all(to)?;
        for e in fs::read_dir(from)?.flatten() {
            copy_tree(&e.path(), &to.join(e.file_name()), overwrite)?;
        }
    } else if overwrite || !to.exists() {
        fs::copy(from, to)?;
    }
    Ok(())
}

/// Runs a Windows program in the prefix through umu and waits.
fn umu(umu: &str, args: &[String], cwd: &Path) -> Result<(), String> {
    let mut c = Command::new(umu);
    c.args(args).current_dir(cwd).env("PROTON_VERB", "run");
    let s = crate::host::wrap(c).status().map_err(|e| e.to_string())?;
    s.success().then_some(()).ok_or_else(|| format!("{} exited with {s}", args.first().map(String::as_str).unwrap_or("")))
}

pub fn run(game: &Path, id: &str, prefix: &Path, umu_bin: &str) -> Result<(), String> {
    let support = game.join("gog-support").join(id);
    // Support files for the game folder (configs, .sgr rules); never replace the player's own.
    copy_tree(&support.join("app"), game, false).map_err(|e| format!("support files: {e}"))?;

    let script: Value = fs::read_to_string(game.join(format!("goggame-{id}.script"))).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let actions = script["actions"].as_array().cloned().unwrap_or_default();
    let docs = prefix.join("drive_c/users/steamuser/Documents");

    if let Some(reg) = reg_file(&actions, game, &docs, &support) {
        // Make sure the prefix exists first, then import.
        let f = prefix.join(format!("lodestone-gog-{id}.reg"));
        fs::create_dir_all(prefix).map_err(|e| e.to_string())?;
        fs::write(&f, reg).map_err(|e| e.to_string())?;
        umu(umu_bin, &["regedit".into(), "/S".into(), win_path(&f)], game)?;
        let _ = fs::remove_file(f);
    }

    for a in &actions {
        let i = &a["install"];
        let r = &i["arguments"];
        let path = |k: &str| r[k].as_str().map(|s| PathBuf::from(expand(s, game, &docs, &support, false)));
        match i["action"].as_str().unwrap_or("") {
            "savePath" | "supportData" if r["type"].as_str() == Some("folder") => {
                if let Some(t) = path("target").or_else(|| path("savePath")) {
                    let _ = fs::create_dir_all(&t);
                    if let Some(src) = path("source").filter(|s| s.is_dir()) {
                        let once = r["conditions"].as_array().is_some_and(|c| c.iter().any(|c| c == "onlyOnce"));
                        copy_tree(&src, &t, !once).map_err(|e| format!("copying {}: {e}", src.display()))?;
                    }
                }
            }
            "Execute" => {
                // Small fix-up tools shipped with the game; redistributable installers aren't downloaded.
                let Some(exe) = path("executable").filter(|p| p.is_file()) else { continue };
                let cwd = path("workingDir").filter(|p| p.is_dir()).unwrap_or_else(|| game.to_path_buf());
                let mut args = vec![exe.to_string_lossy().into_owned()];
                args.extend(r["arguments"].as_str().map(|s| expand(s, game, &docs, &support, true)).into_iter().flat_map(|s| s.split_whitespace().map(String::from).collect::<Vec<_>>()));
                if let Err(e) = umu(umu_bin, &args, &cwd) {
                    eprintln!("GOG setup: {e}");
                }
            }
            _ => {}
        }
    }
    fs::write(marker(prefix, id), build(game, id)).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn writes_registry_from_script() {
        let acts = vec![
            json!({"install": {"action": "setRegistry", "arguments": {"root": "HKEY_LOCAL_MACHINE", "subkey": "Software\\Maxis\\SimCity 4", "valueData": "{app}", "valueName": "Install Dir", "valueType": "string"}}}),
            json!({"install": {"action": "setRegistry", "arguments": {"root": "HKEY_LOCAL_MACHINE", "subkey": "Software\\Maxis\\SimCity 4", "valueData": "$0012434f", "valueName": "CacheSize", "valueType": "dword"}}}),
            json!({"install": {"action": "setRegistry", "arguments": {"root": "HKEY_LOCAL_MACHINE", "subkey": "Software\\Maxis\\SimCity 4\\EP1", "valueData": "KEY", "valueType": "string"}}}),
            json!({"install": {"action": "setRegistry", "arguments": {"subkey": "Software\\X", "valueData": ".\\\\", "valueName": "CD Drive"}}}),
        ];
        let r = super::reg_file(&acts, Path::new("/home/a/Games/SC4"), Path::new("/d"), Path::new("/s")).unwrap();
        assert!(r.contains("[HKEY_LOCAL_MACHINE\\Software\\Maxis\\SimCity 4]\n\"Install Dir\"=\"Z:\\\\home\\\\a\\\\Games\\\\SC4\""), "{r}");
        assert!(r.contains("\"CacheSize\"=dword:0012434f"));
        assert!(r.contains("[HKEY_LOCAL_MACHINE\\Software\\Wow6432Node\\Maxis\\SimCity 4\\EP1]\n@=\"KEY\""));
        assert!(r.contains("\"CD Drive\"=\".\\\\\""));
    }

    /// `LODESTONE_GOG_GAME=<dir> LODESTONE_GOG_ID=<id> cargo test live_gog_setup -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_gog_setup() {
        let game = std::env::var("LODESTONE_GOG_GAME").unwrap();
        let id = std::env::var("LODESTONE_GOG_ID").unwrap();
        let pfx = crate::stores::gog::prefix(&id);
        let umu = crate::tools::umu_path().unwrap();
        std::env::set_var("WINEPREFIX", &pfx);
        std::env::set_var("GAMEID", "umu-default");
        std::env::set_var("PROTONPATH", crate::tools::proton_path());
        std::env::set_var("STORE", "gog");
        super::run(Path::new(&game), &id, &pfx, &umu.to_string_lossy()).unwrap();
        assert!(!super::needed(&pfx, Path::new(&game), &id));
    }
}
