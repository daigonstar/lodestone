//! Emulation: your ROMs, one library per system, in the folder layout EmuDeck (and RetroDECK, ES-DE)
//! use: `<Emulation>/roms/<system>/`.
//!
//! - Where: `~/Emulation`, `~/retrodeck`, and `Emulation`/`retrodeck` folders on other drives
//!   (`/run/media/...`, `/mnt/...`), or a folder you pick in Settings.
//! - Which files are games: each system folder's `systeminfo.txt` (written by EmuDeck/ES-DE) lists its
//!   file types and launch commands; folders without one use Lodestone's own table below.
//! - Names and covers: ES-DE's `gamelist.xml` and `downloaded_media/<system>/covers`, when present.
//! - Playing: the first launch command whose emulator is installed (Flatpak, a program on the PATH, or
//!   an AppImage in ~/Applications). Lodestone only runs ROMs you already have; it never downloads games.

use crate::model::{home, path_str, Detect, Game, LaunchSpec};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const STORE: &str = "emulation";

/// Systems for ROM folders without a systeminfo.txt: (id, full name, file types, launch command in
/// ES-DE's format). Reviewed against EmuDeck's defaults.
const SYSTEMS: &[(&str, &str, &str, &str)] = &[
    ("nes", "Nintendo Entertainment System", ".nes .fds .unf .unif .zip .7z", "%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/mesen_libretro.so %ROM%"),
    ("snes", "Super Nintendo", ".sfc .smc .swc .fig .bs .st .zip .7z", "%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/snes9x_libretro.so %ROM%"),
    ("n64", "Nintendo 64", ".n64 .v64 .z64 .zip .7z", "%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/mupen64plus_next_libretro.so %ROM%"),
    ("gb", "Game Boy", ".gb .zip .7z", "%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/gambatte_libretro.so %ROM%"),
    ("gbc", "Game Boy Color", ".gbc .zip .7z", "%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/gambatte_libretro.so %ROM%"),
    ("gba", "Game Boy Advance", ".gba .zip .7z", "%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/mgba_libretro.so %ROM%"),
    ("nds", "Nintendo DS", ".nds .zip .7z", "%EMULATOR_MELONDS% %ROM%"),
    ("n3ds", "Nintendo 3DS", ".3ds .3dsx .cci .cxi .app .cia", "%EMULATOR_AZAHAR% %ROM%"),
    ("gc", "Nintendo GameCube", ".iso .gcm .gcz .ciso .rvz .wia .wbfs .dol .elf", "%EMULATOR_DOLPHIN% -b -e %ROM%"),
    ("wii", "Nintendo Wii", ".iso .wbfs .rvz .wia .gcz .ciso .wad .dol .elf", "%EMULATOR_DOLPHIN% -b -e %ROM%"),
    ("wiiu", "Nintendo Wii U", ".wud .wux .wua .rpx .elf", "%EMULATOR_CEMU% -g %ROM%"),
    ("switch", "Nintendo Switch", ".nsp .xci .nca .nro .nso", "%EMULATOR_RYUBING% %ROM%"),
    ("psx", "Sony PlayStation", ".cue .chd .m3u .pbp .iso .img .ccd .toc .ecm", "%EMULATOR_DUCKSTATION% -batch %ROM%"),
    ("ps2", "Sony PlayStation 2", ".iso .chd .cso .gz .bin .m3u", "%EMULATOR_PCSX2% -batch %ROM%"),
    ("ps3", "Sony PlayStation 3", ".ps3 .ps3dir", "%EMULATOR_RPCS3% --no-gui %ROM%"),
    ("psp", "Sony PlayStation Portable", ".iso .cso .chd .pbp .elf", "%EMULATOR_PPSSPP% %ROM%"),
    ("genesis", "Sega Genesis", ".md .gen .smd .bin .zip .7z", "%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/genesis_plus_gx_libretro.so %ROM%"),
    ("megadrive", "Sega Mega Drive", ".md .gen .smd .bin .zip .7z", "%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/genesis_plus_gx_libretro.so %ROM%"),
    ("mastersystem", "Sega Master System", ".sms .bin .zip .7z", "%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/genesis_plus_gx_libretro.so %ROM%"),
    ("gamegear", "Sega Game Gear", ".gg .zip .7z", "%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/genesis_plus_gx_libretro.so %ROM%"),
    ("segacd", "Sega CD", ".cue .chd .m3u .iso", "%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/genesis_plus_gx_libretro.so %ROM%"),
    ("sega32x", "Sega 32X", ".32x .bin .zip .7z", "%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/picodrive_libretro.so %ROM%"),
    ("saturn", "Sega Saturn", ".cue .chd .m3u .ccd .iso", "%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/mednafen_saturn_libretro.so %ROM%"),
    ("dreamcast", "Sega Dreamcast", ".cdi .gdi .chd .cue .m3u", "%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/flycast_libretro.so %ROM%"),
    ("pcengine", "PC Engine / TurboGrafx-16", ".pce .zip .7z", "%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/mednafen_pce_libretro.so %ROM%"),
    ("atari2600", "Atari 2600", ".a26 .bin .zip .7z", "%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/stella_libretro.so %ROM%"),
    ("atarilynx", "Atari Lynx", ".lnx .zip .7z", "%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/handy_libretro.so %ROM%"),
    ("ngp", "Neo Geo Pocket", ".ngp .zip .7z", "%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/mednafen_ngp_libretro.so %ROM%"),
    ("ngpc", "Neo Geo Pocket Color", ".ngc .ngp .zip .7z", "%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/mednafen_ngp_libretro.so %ROM%"),
    ("wonderswan", "WonderSwan", ".ws .zip .7z", "%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/mednafen_wswan_libretro.so %ROM%"),
    ("wonderswancolor", "WonderSwan Color", ".wsc .ws .zip .7z", "%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/mednafen_wswan_libretro.so %ROM%"),
    ("arcade", "Arcade", ".zip .7z", "%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/fbneo_libretro.so %ROM%"),
    ("mame", "MAME", ".zip .7z", "%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/mame_libretro.so %ROM%"),
    ("xbox", "Microsoft Xbox", ".iso", "%EMULATOR_XEMU% -dvd_path %ROM%"),
    ("virtualboy", "Nintendo Virtual Boy", ".vb .zip .7z", "%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/mednafen_vb_libretro.so %ROM%"),
    ("3do", "3DO", ".iso .chd .cue", "%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/opera_libretro.so %ROM%"),
];

/// Emulators the launch commands refer to (%EMULATOR_<NAME>%): (name, display name, Flatpak app ids,
/// program names on the PATH, AppImage name prefixes in ~/Applications).
const EMULATORS: &[(&str, &str, &[&str], &[&str], &[&str])] = &[
    ("RETROARCH", "RetroArch", &["org.libretro.RetroArch"], &["retroarch"], &["RetroArch"]),
    ("DUCKSTATION", "DuckStation", &["org.duckstation.DuckStation"], &["duckstation-qt"], &["DuckStation"]),
    ("PCSX2", "PCSX2", &["net.pcsx2.PCSX2"], &["pcsx2-qt", "pcsx2"], &["pcsx2"]),
    ("DOLPHIN", "Dolphin", &["org.DolphinEmu.dolphin-emu"], &["dolphin-emu"], &["Dolphin"]),
    ("PRIMEHACK", "PrimeHack", &["io.github.shiiion.primehack"], &["primehack"], &["PrimeHack"]),
    ("PPSSPP", "PPSSPP", &["org.ppsspp.PPSSPP"], &["PPSSPPSDL", "ppsspp"], &["PPSSPP"]),
    ("RPCS3", "RPCS3", &["net.rpcs3.RPCS3"], &["rpcs3"], &["rpcs3"]),
    ("AZAHAR", "Azahar", &["org.azahar_emu.Azahar"], &["azahar"], &["azahar"]),
    ("RYUBING", "Ryubing", &["io.github.ryubing.Ryujinx"], &["ryujinx", "Ryujinx"], &["ryujinx", "ryubing"]),
    ("XEMU", "xemu", &["app.xemu.xemu"], &["xemu"], &["xemu"]),
    ("MELONDS", "melonDS", &["net.kuribo64.melonDS"], &["melonDS"], &["melonDS"]),
    ("CEMU", "Cemu", &["info.cemu.Cemu"], &["cemu", "Cemu"], &["Cemu"]),
    ("VITA3K", "Vita3K", &[], &["Vita3K"], &["Vita3K"]),
    ("MAME", "MAME", &["org.mamedev.MAME"], &["mame"], &[]),
    ("FLYCAST", "Flycast", &["org.flycast.Flycast"], &["flycast"], &["flycast"]),
    ("SCUMMVM", "ScummVM", &["org.scummvm.ScummVM"], &["scummvm"], &[]),
];

/// EmuDeck/ES-DE folders in roms/ that aren't game systems (shortcuts to emulators, PC launchers, ports
/// run through other tools).
const NOT_SYSTEMS: &[&str] = &["emulators", "portmaster", "ports", "desktop", "lutris", "steam", "epic", "flatpak", "android", "kodi", "windows", "windows3x", "windows9x", "emulation"];

/// Folders inside a system folder that hold media, not games.
const SKIP_DIRS: &[&str] = &["images", "media", "videos", "manuals", "screenshots", "covers", "marquees", "boxart", "snap", "titles", "wheel"];

#[derive(Debug, Clone, PartialEq)]
struct SystemInfo {
    id: String,
    name: String,
    exts: Vec<String>,
    /// Launch commands in ES-DE's format, preferred first.
    commands: Vec<String>,
}

/// Read EmuDeck/ES-DE's systeminfo.txt ("Heading:" lines followed by their value lines).
fn parse_systeminfo(text: &str) -> Option<SystemInfo> {
    let mut sections: HashMap<String, Vec<String>> = HashMap::new();
    let mut cur = String::new();
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() {
            continue;
        }
        if l.ends_with(':') && !l.contains('%') {
            cur = l.trim_end_matches(':').to_ascii_lowercase();
            continue;
        }
        sections.entry(cur.clone()).or_default().push(l.to_string());
    }
    let first = |k: &str| sections.get(k).and_then(|v| v.first().cloned());
    let id = first("system name")?;
    let exts = first("supported file extensions")
        .unwrap_or_default()
        .split_whitespace()
        .map(|e| e.to_ascii_lowercase())
        .filter(|e| e.starts_with('.'))
        .collect::<Vec<_>>();
    let mut commands = sections.get("launch command").cloned().unwrap_or_default();
    commands.extend(sections.get("alternative launch commands").cloned().unwrap_or_default());
    let mut seen = HashSet::new();
    let exts = exts.into_iter().filter(|e| seen.insert(e.clone())).collect();
    Some(SystemInfo { name: first("full system name").unwrap_or_else(|| id.clone()), id, exts, commands })
}

fn builtin(id: &str) -> Option<SystemInfo> {
    SYSTEMS.iter().find(|s| s.0 == id).map(|(id, name, exts, cmd)| SystemInfo {
        id: id.to_string(),
        name: name.to_string(),
        exts: exts.split_whitespace().map(String::from).collect(),
        commands: vec![cmd.to_string()],
    })
}

// ---------------- where the ROMs are ----------------

/// Emulation folders that have a `roms` folder, without duplicates.
pub fn roots() -> Vec<PathBuf> {
    let h = home();
    let mut cands: Vec<PathBuf> = Vec::new();
    let custom = crate::settings::load().emulation_dir;
    let custom = custom.trim();
    if !custom.is_empty() {
        cands.push(match custom.strip_prefix("~/") {
            Some(r) => h.join(r),
            None => PathBuf::from(custom),
        });
    }
    cands.extend([h.join("Emulation"), h.join("retrodeck")]);
    // Other drives: /run/media/<user>/<drive>/..., /media/<user>/<drive>/..., /mnt/<drive>/...
    let mut drives: Vec<PathBuf> = Vec::new();
    for base in ["/run/media", "/media"] {
        for u in fs::read_dir(base).into_iter().flatten().flatten() {
            drives.extend(fs::read_dir(u.path()).into_iter().flatten().flatten().map(|d| d.path()));
        }
    }
    drives.extend(fs::read_dir("/mnt").into_iter().flatten().flatten().map(|d| d.path()));
    for d in drives {
        for name in ["Emulation", "retrodeck"] {
            cands.push(d.join(name));
            // A copied Emulation folder one level down (e.g. a backup drive's Emulation/Emulation).
            cands.push(d.join(name).join(name));
            cands.push(d.join("Emulation").join(name));
        }
    }
    let mut seen = HashSet::new();
    cands
        .into_iter()
        .filter(|c| c.join("roms").is_dir())
        .filter(|c| seen.insert(fs::canonicalize(c.join("roms")).unwrap_or(c.join("roms"))))
        .collect()
}

/// Where ES-DE keeps downloaded artwork for these roots.
fn media_roots(root: &Path) -> Vec<PathBuf> {
    let h = home();
    let mut v = vec![root.join("tools/downloaded_media"), root.join("ES-DE/downloaded_media"), h.join("ES-DE/downloaded_media"), h.join(".emulationstation/downloaded_media")];
    if let Some(p) = root.parent() {
        v.push(p.join("retrodeck/ES-DE/downloaded_media"));
        v.push(p.join("Emulation/tools/downloaded_media"));
    }
    v.into_iter().filter(|p| p.is_dir()).collect()
}

// ---------------- reading a system folder ----------------

#[derive(Debug, Clone, PartialEq)]
pub struct Rom {
    /// Path inside the system folder ("Disc Games/Game (USA).chd").
    pub rel: String,
    pub path: PathBuf,
    pub size: u64,
}

/// The games in a system folder: files (or, for PS3-style systems, folders) with one of the system's
/// file types, up to two folders deep. Files a .cue/.m3u/.gdi lists, and a .bin next to a .cue of the
/// same name, are part of that game, not games of their own.
fn scan_system(dir: &Path, exts: &[String]) -> Vec<Rom> {
    let ext_of = |name: &str| -> Option<String> {
        let lower = name.to_ascii_lowercase();
        exts.iter().filter(|e| lower.ends_with(e.as_str())).max_by_key(|e| e.len()).cloned()
    };
    let mut found: Vec<Rom> = Vec::new();
    let mut part_of: HashSet<PathBuf> = HashSet::new();
    fn walk(base: &Path, dir: &Path, depth: u8, ext_of: &dyn Fn(&str) -> Option<String>, found: &mut Vec<Rom>, part_of: &mut HashSet<PathBuf>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let p = e.path();
            let Ok(meta) = fs::metadata(&p) else { continue };
            let ext = ext_of(&name);
            if meta.is_dir() {
                if ext.is_some() {
                    // A game that's a folder (PS3 "Game.ps3", ".ps3dir"…).
                    found.push(Rom { rel: rel_of(base, &p), path: p, size: 0 });
                } else if depth < 2 && !SKIP_DIRS.contains(&name.to_ascii_lowercase().as_str()) {
                    walk(base, &p, depth + 1, ext_of, found, part_of);
                }
                continue;
            }
            let Some(ext) = ext else { continue };
            if matches!(ext.as_str(), ".cue" | ".m3u" | ".gdi") {
                for f in listed_files(&p) {
                    part_of.insert(f);
                }
            }
            if ext == ".cue" {
                part_of.insert(p.with_extension("bin"));
            }
            found.push(Rom { rel: rel_of(base, &p), size: meta.len(), path: p });
        }
    }
    walk(dir, dir, 0, &ext_of, &mut found, &mut part_of);
    found.retain(|r| !part_of.contains(&r.path));
    found
}

fn rel_of(base: &Path, p: &Path) -> String {
    p.strip_prefix(base).map(|r| r.to_string_lossy().into_owned()).unwrap_or_else(|_| path_str(p))
}

/// Files a .cue, .m3u or .gdi points to (in the same folder or below it).
fn listed_files(sheet: &Path) -> Vec<PathBuf> {
    let Ok(text) = fs::read_to_string(sheet) else { return vec![] };
    let dir = sheet.parent().unwrap_or(Path::new("."));
    let lower = sheet.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    text.lines()
        .filter_map(|l| {
            let l = l.trim();
            let name = match lower.as_str() {
                "cue" => l.strip_prefix("FILE ").and_then(|r| r.split('"').nth(1)).map(String::from),
                "m3u" => (!l.is_empty() && !l.starts_with('#')).then(|| l.to_string()),
                // gdi: "<track> <lba> <type> <size> <file> <offset>", the file possibly quoted
                _ => l.split('"').nth(1).map(String::from).or_else(|| l.split_whitespace().nth(4).map(String::from)),
            }?;
            // Only plain names or paths below the sheet's folder count.
            let p = Path::new(&name);
            (!p.is_absolute() && !name.contains("..")).then(|| dir.join(p))
        })
        .collect()
}

/// "Pokemon X (Europe) (En,Ja,Fr) [!].3ds" -> "Pokemon X"; tags in brackets go, the name stays.
pub fn clean_title(file_name: &str, ext: Option<&str>) -> String {
    let stem = match ext {
        Some(e) if file_name.to_ascii_lowercase().ends_with(e) => &file_name[..file_name.len() - e.len()],
        _ => Path::new(file_name).file_stem().map(|s| s.to_str().unwrap_or(file_name)).unwrap_or(file_name),
    };
    let mut out = String::new();
    let mut depth = 0i32;
    for c in stem.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = (depth - 1).max(0),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    let t = out.replace('_', " ");
    // Leftovers of double extensions ("Game (USA).nkit.iso") and doubled spaces.
    let t = t.trim().trim_end_matches(".nkit").trim_end_matches(['-', ',']).trim();
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    // "Adventures of Batman & Robin, The" -> "The Adventures of Batman & Robin"
    match t.rsplit_once(", The") {
        Some((head, "")) => format!("The {head}"),
        _ if t.is_empty() => stem.to_string(),
        _ => t,
    }
}

/// ES-DE's gamelist.xml: file path (as written, "./x.nsp") -> (name, image path).
fn gamelist(dir: &Path) -> HashMap<String, (String, Option<PathBuf>)> {
    let Ok(xml) = fs::read_to_string(dir.join("gamelist.xml")) else { return HashMap::new() };
    let tag = |block: &str, t: &str| -> Option<String> {
        let open = format!("<{t}>");
        let start = block.find(&open)? + open.len();
        let end = block[start..].find(&format!("</{t}>"))? + start;
        Some(unescape_xml(block[start..end].trim()))
    };
    let mut out = HashMap::new();
    for block in xml.split("<game").skip(1) {
        let Some(path) = tag(block, "path") else { continue };
        let name = tag(block, "name").unwrap_or_default();
        let image = tag(block, "image").and_then(|i| inside(dir, &i));
        out.insert(path.trim_start_matches("./").to_string(), (name, image));
    }
    out
}

fn unescape_xml(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// A relative path from a gamelist, only if it stays inside the system folder.
fn inside(dir: &Path, rel: &str) -> Option<PathBuf> {
    let rel = rel.trim_start_matches("./");
    let p = Path::new(rel);
    (!p.is_absolute() && !rel.contains("..")).then(|| dir.join(p)).filter(|p| p.is_file())
}

fn cover_for(media: &[PathBuf], system: &str, rom: &Rom) -> Option<PathBuf> {
    let stem = Path::new(&rom.rel).file_stem()?.to_string_lossy().into_owned();
    for m in media {
        for kind in ["covers", "miximages", "screenshots"] {
            for ext in ["png", "jpg", "jpeg", "webp"] {
                let p = m.join(system).join(kind).join(format!("{stem}.{ext}"));
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    None
}

// ---------------- emulators ----------------

/// How to start an installed emulator: program and the arguments before the emulator's own.
#[derive(Debug, Clone, PartialEq)]
pub struct Emulator {
    pub key: &'static str,
    pub name: &'static str,
    pub program: String,
    pub pre_args: Vec<String>,
    /// For RetroArch: the folder its cores are in.
    pub cores: Option<PathBuf>,
}

/// Installed Flatpak apps (checked on the host when Lodestone is a Flatpak), cached for a minute.
fn flatpak_apps() -> HashSet<String> {
    static C: Mutex<Option<(Instant, HashSet<String>)>> = Mutex::new(None);
    if let Some((at, v)) = C.lock().unwrap().as_ref() {
        if at.elapsed() < Duration::from_secs(60) {
            return v.clone();
        }
    }
    let mut cmd = std::process::Command::new("flatpak");
    cmd.args(["list", "--app", "--columns=application"]);
    let apps: HashSet<String> = crate::host::wrap(cmd)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
        .unwrap_or_default();
    *C.lock().unwrap() = Some((Instant::now(), apps.clone()));
    apps
}

/// Where the Flatpak RetroArch keeps its cores (and where Lodestone puts the ones it downloads).
pub fn flatpak_cores_dir() -> PathBuf {
    home().join(".var/app/org.libretro.RetroArch/config/retroarch/cores")
}

fn retroarch_cores(flatpak: bool) -> Option<PathBuf> {
    let h = home();
    if flatpak {
        return Some(flatpak_cores_dir());
    }
    let mut c = Vec::new();
    c.extend([h.join(".config/retroarch/cores"), PathBuf::from("/usr/lib/libretro"), PathBuf::from("/usr/lib64/libretro"), PathBuf::from("/usr/lib/x86_64-linux-gnu/libretro")]);
    c.into_iter().find(|p| p.is_dir())
}

/// Every emulator Lodestone can find, by key ("RETROARCH", "DOLPHIN"…). Flatpak first, then a program
/// on the PATH, then an AppImage in ~/Applications.
pub fn installed_emulators() -> HashMap<&'static str, Emulator> {
    let apps = flatpak_apps();
    let appimages: Vec<PathBuf> = fs::read_dir(home().join("Applications")).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("appimage"))).collect();
    let mut out = HashMap::new();
    for (key, name, flatpaks, bins, images) in EMULATORS {
        let found = if let Some(id) = flatpaks.iter().find(|id| apps.contains(**id)) {
            Some(Emulator { key, name, program: "flatpak".into(), pre_args: vec!["run".into(), id.to_string()], cores: (*key == "RETROARCH").then(|| retroarch_cores(true)).flatten() })
        } else if let Some(p) = bins.iter().find_map(|b| crate::host::which(b)) {
            Some(Emulator { key, name, program: path_str(&p), pre_args: vec![], cores: (*key == "RETROARCH").then(|| retroarch_cores(false)).flatten() })
        } else {
            images
                .iter()
                .find_map(|prefix| appimages.iter().find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().to_ascii_lowercase().starts_with(&prefix.to_ascii_lowercase()))))
                .map(|p| Emulator { key, name, program: path_str(p), pre_args: vec![], cores: None })
        };
        if let Some(e) = found {
            out.insert(*key, e);
        }
    }
    out
}

/// Turn an ES-DE launch command into a program and arguments for this ROM, if its emulator (and, for
/// RetroArch, its core) is installed. Commands that need ES-DE-only features are skipped.
fn build_launch(command: &str, rom: &Path, sys_dir: &Path, emus: &HashMap<&'static str, Emulator>) -> Option<(LaunchSpec, &'static str)> {
    let mut words = command.split_whitespace().peekable();
    let mut cwd: Option<PathBuf> = None;
    let mut emu: Option<&Emulator> = None;
    let mut args: Vec<String> = Vec::new();
    let game_dir = rom.parent().unwrap_or(sys_dir);
    while let Some(w) = words.next() {
        match w {
            "%ENABLESHORTCUTS%" | "%RUNINBACKGROUND%" | "%HIDEWINDOW%" | "%ESCAPESPECIALS%" => {}
            _ if w.starts_with("%STARTDIR%=") => {
                cwd = Some(match &w["%STARTDIR%=".len()..] {
                    "%GAMEDIR%" | "%GAMEDIRRAW%" => game_dir.to_path_buf(),
                    _ => sys_dir.to_path_buf(),
                });
            }
            _ if w.starts_with("%INJECT%") || w.starts_with("%EMULATOR_OS-SHELL%") || w.starts_with("%PRECOMMAND") => return None,
            _ if w.starts_with("%EMULATOR_") => {
                let key = w.trim_start_matches("%EMULATOR_").trim_end_matches('%');
                let e = emus.get(key)?;
                emu = Some(e);
                args.extend(e.pre_args.iter().cloned());
            }
            _ => {
                let mut a = w.to_string();
                if a.contains("%CORE_RETROARCH%") {
                    let cores = emu.and_then(|e| e.cores.as_ref())?;
                    a = a.replace("%CORE_RETROARCH%", &path_str(cores));
                    if !Path::new(&a).is_file() {
                        return None; // the core isn't installed in RetroArch
                    }
                }
                let stem = rom.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                a = a
                    .replace("%ROMRAW%", &path_str(rom))
                    .replace("%ROM%", &path_str(rom))
                    .replace("%ROMPATH%", &path_str(sys_dir))
                    .replace("%GAMEDIRRAW%", &path_str(game_dir))
                    .replace("%GAMEDIR%", &path_str(game_dir))
                    .replace("%BASENAME%", &stem)
                    .replace("%FILENAME%", &rom.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
                if a.contains('%') && a.matches('%').count() >= 2 {
                    return None; // a placeholder Lodestone doesn't know
                }
                args.push(a);
            }
        }
    }
    let e = emu?;
    let mut spec = LaunchSpec::new(e.program.clone(), &[]);
    spec.args = args;
    spec.cwd = cwd.or_else(|| Some(game_dir.to_path_buf()));
    Some((spec, e.name))
}

// ---------------- installing emulators and cores ----------------

const CORES_URL: &str = "https://buildbot.libretro.com/nightly/linux/x86_64/latest";
const FLATHUB_REPO: &str = "https://dl.flathub.org/repo/flathub.flatpakrepo";

/// Install what a system's chosen way of playing needs: the emulator from Flathub (a background job
/// in Downloads, then its core) or just the RetroArch core. Returns "installing" (watch Downloads),
/// "done", or "already".
/// The Flatpak app ID an installed emulator runs from, if it's a Flatpak.
fn flatpak_id(e: &Emulator) -> Option<&str> {
    (e.program == "flatpak").then(|| e.pre_args.get(1).map(String::as_str)).flatten()
}

const MENU_MARK: &str = "# Hidden from the app menu by Lodestone. Delete this file to show it again.";

/// The menu entries Flatpak exported for an app (user and system installs).
fn exported_entries(app_id: &str) -> Vec<PathBuf> {
    let dirs = [home().join(".local/share/flatpak/exports/share/applications"), PathBuf::from("/var/lib/flatpak/exports/share/applications")];
    let mut out = Vec::new();
    for d in dirs {
        for e in fs::read_dir(&d).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            // Flatpak names exported entries "<app id>.desktop" or "<app id>.<more>.desktop"; a bare
            // prefix match would also catch another app whose ID starts the same way.
            if (name == format!("{app_id}.desktop") || name.starts_with(&format!("{app_id}."))) && name.ends_with(".desktop") && !out.iter().any(|p: &PathBuf| p.file_name() == Some(e.file_name().as_os_str())) {
                out.push(e.path());
            }
        }
    }
    out
}

fn menu_override(entry: &Path) -> PathBuf {
    home().join(".local/share/applications").join(entry.file_name().unwrap_or_default())
}

/// An exported menu entry with NoDisplay=true added to its [Desktop Entry] section.
fn hidden_entry(original: &str) -> String {
    let mut out = format!("{MENU_MARK}\n");
    let mut in_main = false;
    for line in original.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_main = t == "[Desktop Entry]";
            out.push_str(line);
            out.push('\n');
            if in_main {
                out.push_str("NoDisplay=true\n");
            }
            continue;
        }
        if in_main && t.starts_with("NoDisplay") {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn menu_hidden(app_id: &str) -> bool {
    exported_entries(app_id).iter().any(|e| fs::read_to_string(menu_override(e)).is_ok_and(|t| t.starts_with(MENU_MARK)))
}

/// Show or hide an emulator in the desktop's app menu. Hiding puts a copy of Flatpak's menu entry with
/// NoDisplay=true in ~/.local/share/applications (which wins over Flatpak's); showing removes that copy.
/// Files Lodestone didn't write are never touched.
pub fn set_menu_entry(app_id: &str, show: bool) -> Result<(), String> {
    let entries = exported_entries(app_id);
    if entries.is_empty() && !show {
        return Err("This emulator has no menu entry to hide".into());
    }
    for e in entries {
        let target = menu_override(&e);
        let ours = fs::read_to_string(&target).ok().map(|t| t.starts_with(MENU_MARK));
        if show {
            if ours == Some(true) {
                fs::remove_file(&target).map_err(|err| err.to_string())?;
            }
        } else {
            if ours == Some(false) {
                return Err(format!("{} already has its own menu entry in ~/.local/share/applications; Lodestone left it alone", app_id));
            }
            let text = fs::read_to_string(&e).map_err(|err| err.to_string())?;
            let _ = fs::create_dir_all(target.parent().unwrap());
            fs::write(&target, hidden_entry(&text)).map_err(|err| err.to_string())?;
        }
    }
    // Ask the desktop to notice (not needed everywhere; harmless when missing).
    let mut cmd = std::process::Command::new("update-desktop-database");
    cmd.arg(home().join(".local/share/applications"));
    let _ = crate::host::wrap(cmd).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
    Ok(())
}

/// Show or hide the emulator a system plays with in the app menu.
pub fn set_system_menu_entry(system: &str, index: usize, show: bool) -> Result<(), String> {
    let sys = systems().into_iter().find(|s| s.id == system).ok_or("Unknown system")?;
    let opt = sys.options.into_iter().find(|o| o.index == index).ok_or("Unknown emulator choice")?;
    let def = EMULATORS.iter().find(|e| e.1 == opt.emulator).ok_or("Unknown emulator")?;
    let emus = installed_emulators();
    let id = emus.get(def.0).and_then(flatpak_id).ok_or("Only Flatpak emulators can be hidden from here")?;
    set_menu_entry(id, show)
}

pub async fn install(system: &str, index: usize, menu: bool) -> Result<&'static str, String> {
    let sys = systems().into_iter().find(|s| s.id == system).ok_or("Unknown system")?;
    let opt = sys.options.into_iter().find(|o| o.index == index).ok_or("Unknown emulator choice")?;
    let def = EMULATORS.iter().find(|e| e.1 == opt.emulator).ok_or("Lodestone can't install this emulator")?;
    let core = opt.core.clone();
    if let Some(c) = &core {
        if !valid_core(c) {
            return Err(format!("Not a RetroArch core name: {c}"));
        }
    }
    let emus = tauri::async_runtime::spawn_blocking(installed_emulators).await.map_err(|e| e.to_string())?;
    if !emus.contains_key(def.0) {
        let app_id = *def.2.first().ok_or_else(|| format!("{} isn't on Flathub; install it yourself", def.1))?;
        // Flathub for your user (already there on most systems), then the emulator, in the background.
        let mut add = std::process::Command::new("flatpak");
        add.args(["remote-add", "--user", "--if-not-exists", "flathub", FLATHUB_REPO]);
        let _ = crate::host::wrap(add).stdin(std::process::Stdio::null()).output();
        let spec = LaunchSpec::new("flatpak", &["install", "--user", "-y", "--noninteractive", "flathub", app_id]);
        // After Flatpak finishes: hide its menu entry if you asked to, then RetroArch's core.
        let core = core.filter(|_| def.0 == "RETROARCH");
        let finish: Option<Box<dyn FnOnce() -> Result<(), String> + Send>> = Some(Box::new(move || {
            if !menu {
                if let Err(e) = set_menu_entry(app_id, false) {
                    eprintln!("menu entry for {app_id}: {e}");
                }
            }
            match core {
                Some(c) => tauri::async_runtime::block_on(download_core(&c)),
                None => Ok(()),
            }
        }));
        crate::downloads::start(&format!("emulation-install:{}", def.0.to_ascii_lowercase()), &format!("Installing {}", def.1), &spec, finish)?;
        return Ok("installing");
    }
    match core {
        Some(c) if !emus[def.0].cores.as_ref().is_some_and(|d| d.join(&c).is_file()) => {
            if emus[def.0].program != "flatpak" {
                return Err(format!("Install the {} core from RetroArch's own menu (Online Updater → Core Downloader).", core_label(&c)));
            }
            download_core(&c).await?;
            Ok("done")
        }
        _ => Ok("already"),
    }
}

/// Download a RetroArch core from libretro's official build server into the Flatpak RetroArch's
/// cores folder, checked against the CRC32 libretro publishes for the core.
pub async fn download_core(core: &str) -> Result<(), String> {
    download_core_to(core, &flatpak_cores_dir()).await
}

async fn download_core_to(core: &str, dir: &Path) -> Result<(), String> {
    if !valid_core(core) {
        return Err(format!("Not a RetroArch core name: {core}"));
    }
    let zip_name = format!("{core}.zip");
    let index = crate::net::client().get(format!("{CORES_URL}/.index-extended")).send().await.map_err(crate::net::err)?.error_for_status().map_err(crate::net::err)?.text().await.map_err(crate::net::err)?;
    let want = core_crc(&index, &zip_name).ok_or_else(|| format!("libretro has no {} core for Linux", core_label(core)))?;
    let bytes = crate::net::client().get(format!("{CORES_URL}/{zip_name}")).send().await.map_err(crate::net::err)?.error_for_status().map_err(crate::net::err)?.bytes().await.map_err(crate::net::err)?;
    // libretro's CRC32 is for the core itself (the file inside the zip), which is what gets run.
    let so = unzip_one(&bytes, core)?;
    if crc32fast::hash(&so) != want {
        return Err(format!("The {} core download didn't match libretro's checksum, so it wasn't installed", core_label(core)));
    }
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let tmp = dir.join(format!("{core}.part"));
    fs::write(&tmp, so).map_err(|e| e.to_string())?;
    fs::rename(&tmp, dir.join(core)).map_err(|e| e.to_string())
}

/// The CRC32 libretro lists for a core zip in .index-extended ("<date> <crc32 hex> <file>" per line). It's
/// the CRC32 of the core inside the zip.
fn core_crc(index: &str, file: &str) -> Option<u32> {
    index.lines().find_map(|l| {
        let mut w = l.split_whitespace();
        let (_, crc, name) = (w.next()?, w.next()?, w.next()?);
        (name == file).then(|| u32::from_str_radix(crc, 16).ok())?
    })
}

/// The one file named `name` from a zip (at most 512 MB).
fn unzip_one(bytes: &[u8], name: &str) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let mut f = z.by_name(name).map_err(|_| format!("{name} isn't in the download"))?;
    let mut out = Vec::new();
    f.by_ref().take(512 << 20).read_to_end(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

// ---------------- the library ----------------

/// One way to play a system: a launch command from its systeminfo (or Lodestone's table).
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct EmuOption {
    /// Position in the system's launch commands (what the choice is saved as).
    pub index: usize,
    /// "RetroArch · Snes9x", "DuckStation"
    pub label: String,
    pub emulator: String,
    /// RetroArch core file ("snes9x_libretro.so"), when it's a RetroArch core.
    pub core: Option<String>,
    pub installed: bool,
    /// Lodestone can install what's missing (a Flathub emulator, a core from libretro).
    pub can_install: bool,
    /// For an installed Flatpak emulator: whether it shows in your desktop's app menu.
    pub in_menu: Option<bool>,
}

/// One system with games, for the sidebar and its library.
#[derive(Debug, Clone, serde::Serialize)]
pub struct System {
    pub id: String,
    pub name: String,
    pub games: usize,
    /// Emulators installed for it (display names), and whether any can play it.
    pub emulators: Vec<String>,
    /// Every way to play it, and the one used (your choice, or else the first installed).
    pub options: Vec<EmuOption>,
    pub chosen: Option<usize>,
}

/// A RetroArch core name Lodestone will download and pass on: "snes9x_libretro.so" style only.
pub fn valid_core(core: &str) -> bool {
    core.strip_suffix("_libretro.so").is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'))
}

/// "snes9x_libretro.so" -> "Snes9x", "mupen64plus_next_libretro.so" -> "Mupen64Plus Next"
fn core_label(core: &str) -> String {
    let n = core.trim_end_matches("_libretro.so");
    let known = [("snes9x", "Snes9x"), ("mgba", "mGBA"), ("gambatte", "Gambatte"), ("genesis_plus_gx", "Genesis Plus GX"), ("picodrive", "PicoDrive"), ("mupen64plus_next", "Mupen64Plus-Next"), ("parallel_n64", "ParaLLEl N64"), ("swanstation", "SwanStation"), ("pcsx_rearmed", "PCSX ReARMed"), ("mednafen_psx_hw", "Beetle PSX HW"), ("mednafen_psx", "Beetle PSX"), ("bsnes", "bsnes"), ("mesen", "Mesen"), ("nestopia", "Nestopia"), ("fceumm", "FCEUmm"), ("flycast", "Flycast"), ("dolphin", "Dolphin"), ("melondsds", "melonDS DS"), ("melonds", "melonDS"), ("desmume", "DeSmuME"), ("ppsspp", "PPSSPP"), ("mame", "MAME"), ("fbneo", "FinalBurn Neo"), ("vba_next", "VBA Next"), ("vbam", "VBA-M")];
    known.iter().find(|k| k.0 == n).map(|k| k.1.to_string()).unwrap_or_else(|| n.split('_').map(|w| { let mut c = w.chars(); c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default() }).collect::<Vec<_>>().join(" "))
}

/// What a launch command needs: (emulator key, RetroArch core).
fn command_needs(command: &str) -> Option<(String, Option<String>)> {
    let key = command.split_whitespace().find_map(|w| w.strip_prefix("%EMULATOR_")?.strip_suffix('%').map(String::from))?;
    let core = command.split_whitespace().find_map(|w| w.strip_prefix("%CORE_RETROARCH%/").map(String::from));
    Some((key, core))
}

fn options_for(info: &SystemInfo, emus: &HashMap<&'static str, Emulator>) -> Vec<EmuOption> {
    info.commands
        .iter()
        .enumerate()
        .filter_map(|(index, c)| {
            if c.contains("%INJECT%") || c.contains("%EMULATOR_OS-SHELL%") {
                return None;
            }
            let (key, core) = command_needs(c)?;
            let def = EMULATORS.iter().find(|e| e.0 == key)?;
            let emu = emus.get(def.0);
            let core_ok = match (&core, emu.and_then(|e| e.cores.as_ref())) {
                (None, _) => true,
                (Some(c), Some(dir)) => dir.join(c).is_file(),
                (Some(_), None) => false,
            };
            let label = match &core {
                Some(c) => format!("{} · {}", def.1, core_label(c)),
                None => def.1.to_string(),
            };
            let flatpak = !def.2.is_empty();
            Some(EmuOption {
                index,
                label,
                emulator: def.1.to_string(),
                installed: emu.is_some() && core_ok,
                // RetroArch cores come from libretro's server, so only for the Flatpak or a missing RetroArch.
                can_install: (emu.is_none() && flatpak) || (emu.is_some() && !core_ok && core.as_deref().is_some_and(valid_core) && emu.is_some_and(|e| e.program == "flatpak")),
                in_menu: emu.and_then(flatpak_id).map(|id| !menu_hidden(id)),
                core,
            })
        })
        .collect()
}

/// Your saved choice for a system, if it's still one of its options.
fn chosen(system: &str, options: &[EmuOption]) -> Option<usize> {
    let saved = crate::settings::load().emu_choice.get(system).copied();
    saved.filter(|i| options.iter().any(|o| o.index == *i)).or_else(|| options.iter().find(|o| o.installed).map(|o| o.index))
}

static SYSTEMS_SEEN: Mutex<Vec<System>> = Mutex::new(Vec::new());
static ROMS_SEEN: Mutex<Vec<(String, PathBuf)>> = Mutex::new(Vec::new());

/// (system, ROM path) of every game in the last scan.
pub fn roms() -> Vec<(String, PathBuf)> {
    ROMS_SEEN.lock().unwrap().clone()
}

pub fn systems() -> Vec<System> {
    SYSTEMS_SEEN.lock().unwrap().clone()
}

pub fn scan() -> Vec<Game> {
    let emus = installed_emulators();
    let mut out = Vec::new();
    let mut seen: HashSet<(String, String, u64)> = HashSet::new();
    let mut by_system: HashMap<String, System> = HashMap::new();
    let mut rom_paths: Vec<(String, PathBuf)> = Vec::new();
    for root in roots() {
        let media = media_roots(&root);
        let Ok(rd) = fs::read_dir(root.join("roms")) else { continue };
        let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        dirs.sort();
        for dir in dirs {
            let folder = dir.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            if NOT_SYSTEMS.contains(&folder.as_str()) {
                continue;
            }
            let info = fs::read_to_string(dir.join("systeminfo.txt")).ok().and_then(|t| parse_systeminfo(&t)).or_else(|| builtin(&folder));
            let Some(info) = info.filter(|i| !i.exts.is_empty()) else { continue };
            let names = gamelist(&dir);
            let options = options_for(&info, &emus);
            let pick = chosen(&info.id, &options);
            // The chosen way first, then the others as fallbacks.
            let ordered: Vec<&String> = pick.and_then(|i| info.commands.get(i)).into_iter().chain(info.commands.iter().enumerate().filter(|(i, _)| Some(*i) != pick).map(|(_, c)| c)).collect();
            for rom in scan_system(&dir, &info.exts) {
                // The same ROM in two copies of an Emulation folder (a backup, RetroDECK and EmuDeck).
                if !seen.insert((info.id.clone(), rom.rel.clone(), rom.size)) {
                    continue;
                }
                let file = Path::new(&rom.rel).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let ext = info.exts.iter().filter(|e| file.to_ascii_lowercase().ends_with(e.as_str())).max_by_key(|e| e.len()).map(String::as_str);
                let listed = names.get(&rom.rel);
                let mut title = listed.map(|l| l.0.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| clean_title(&file, ext));
                let es_cover = listed.and_then(|l| l.1.clone()).or_else(|| cover_for(&media, &info.id, &rom)).map(|p| path_str(&p));
                // RetroAchievements first (your choice): its title and box art, then ES-DE's, then
                // libretro's thumbnails, then RA's icon.
                let ra = super::retroachievements::match_rom(&info.id, &rom.path, &title);
                let stem = Path::new(&file).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let cover = match &ra {
                    Some(g) => {
                        title = super::retroachievements::display_title(&g.title);
                        super::retroachievements::cached_box_art(g.id)
                            .or(es_cover)
                            .or_else(|| super::libretro_thumbs::cover(&info.id, &stem, &[&g.title, &title]))
                            .or_else(|| (!g.icon.is_empty()).then(|| format!("{}{}", super::retroachievements::MEDIA, g.icon)))
                    }
                    None => es_cover.or_else(|| super::libretro_thumbs::cover(&info.id, &stem, &[&title])),
                };
                rom_paths.push((info.id.clone(), rom.path.clone()));
                let launch = ordered.iter().find_map(|c| build_launch(c, &rom.path, &dir, &emus));
                let no_emulator = launch.is_none();
                let via = match &launch {
                    Some((_, emu)) => format!("{} · {emu}", info.name),
                    None => format!("{} · no emulator yet", info.name),
                };
                out.push(Game {
                    id: format!("emu:{}:{}", info.id, rom.rel),
                    store: STORE.into(),
                    via,
                    title,
                    installed: true,
                    cover,
                    ra_id: ra.as_ref().map(|g| g.id),
                    size: (rom.size > 0).then_some(rom.size),
                    launch: launch.map(|l| l.0),
                    detect: Some(Detect::Contains(file)),
                    system: Some(info.id.clone()),
                    needs_emulator: no_emulator,
                    ..Default::default()
                });
                let s = by_system.entry(info.id.clone()).or_insert_with(|| System { id: info.id.clone(), name: info.name.clone(), games: 0, emulators: vec![], options: options.clone(), chosen: pick });
                s.games += 1;
            }
            if let Some(s) = by_system.get_mut(&info.id) {
                for c in &info.commands {
                    for w in c.split_whitespace().filter(|w| w.starts_with("%EMULATOR_")) {
                        if let Some(e) = emus.get(w.trim_start_matches("%EMULATOR_").trim_end_matches('%')) {
                            if !s.emulators.contains(&e.name.to_string()) {
                                s.emulators.push(e.name.to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    let mut list: Vec<System> = by_system.into_values().collect();
    list.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    *SYSTEMS_SEEN.lock().unwrap() = list;
    *ROMS_SEEN.lock().unwrap() = rom_paths;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_core_names_and_libretro_checksums() {
        assert!(valid_core("snes9x_libretro.so") && valid_core("mupen64plus_next_libretro.so"));
        for bad in ["../x_libretro.so", "snes9x.so", "_libretro.so", "Snes9x_libretro.so", "a/b_libretro.so"] {
            assert!(!valid_core(bad), "{bad}");
        }
        let index = "2026-11-24 5474b5a7 mgba_libretro.so.zip\n2026-11-24 c2b3ee5d snes9x_libretro.so.zip\n";
        assert_eq!(core_crc(index, "snes9x_libretro.so.zip"), Some(0xc2b3ee5d));
        assert_eq!(core_crc(index, "bsnes_libretro.so.zip"), None);
        assert_eq!(core_label("mupen64plus_next_libretro.so"), "Mupen64Plus-Next");
        assert_eq!(core_label("beetle_new_thing_libretro.so"), "Beetle New Thing");
    }

    #[test]
    fn hides_menu_entries_with_nodisplay() {
        let src = "[Desktop Entry]\nName=RetroArch\nNoDisplay=false\nExec=flatpak run org.libretro.RetroArch\n\n[Desktop Action x]\nNoDisplay=false\n";
        let out = hidden_entry(src);
        assert!(out.starts_with(MENU_MARK));
        assert!(out.contains("[Desktop Entry]\nNoDisplay=true\nName=RetroArch\nExec="));
        assert_eq!(out.matches("NoDisplay=false").count(), 1, "other sections stay as they were");
    }

    #[test]
    fn unzips_one_file() {
        use std::io::Write;
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            w.start_file("snes9x_libretro.so", zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(b"ELF...").unwrap();
            w.finish().unwrap();
        }
        assert_eq!(unzip_one(buf.get_ref(), "snes9x_libretro.so").unwrap(), b"ELF...");
        assert!(unzip_one(buf.get_ref(), "other_libretro.so").is_err());
    }

    #[test]
    fn reads_emudecks_systeminfo() {
        let t = "System name:\npsx\n\nFull system name:\nSony PlayStation\n\nSupported file extensions:\n.bin .BIN .chd .CHD .cue .CUE .m3u .M3U\n\nLaunch command:\n%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/swanstation_libretro.so %ROM%\n\nAlternative launch commands:\n%EMULATOR_DUCKSTATION% -batch %ROM%\n\nPlatform (for scraping):\npsx\n";
        let s = parse_systeminfo(t).unwrap();
        assert_eq!((s.id.as_str(), s.name.as_str()), ("psx", "Sony PlayStation"));
        assert_eq!(s.exts, [".bin", ".chd", ".cue", ".m3u"]);
        assert_eq!(s.commands.len(), 2);
    }

    #[test]
    fn cleans_rom_titles() {
        assert_eq!(clean_title("Pokemon X (Europe) (En,Ja,Fr,De,Es,It,Ko).3ds", Some(".3ds")), "Pokemon X");
        assert_eq!(clean_title("Aladdin (U) [!].smc", Some(".smc")), "Aladdin");
        assert_eq!(clean_title("Adventures of Batman & Robin, The (U).smc", Some(".smc")), "The Adventures of Batman & Robin");
        assert_eq!(clean_title("Metal Gear Solid - The Twin Snakes (USA) (Disc 1).nkit.iso", Some(".iso")), "Metal Gear Solid - The Twin Snakes");
    }

    #[test]
    fn scans_a_system_folder_like_es_de() {
        let d = std::env::temp_dir().join(format!("lodestone-roms-{}", std::process::id()));
        let w = |p: &str, body: &str| {
            let f = d.join(p);
            fs::create_dir_all(f.parent().unwrap()).unwrap();
            fs::write(f, body).unwrap();
        };
        w("Game A (USA).cue", "FILE \"Game A (USA).bin\" BINARY\n");
        w("Game A (USA).bin", "x");
        w("Multi/Game B.m3u", "Game B (Disc 1).chd\nGame B (Disc 2).chd\n");
        w("Multi/Game B (Disc 1).chd", "x");
        w("Multi/Game B (Disc 2).chd", "x");
        w("images/Game A-image.png", "x");
        w("notes.txt", "x");
        fs::create_dir_all(d.join("Some Game.ps3dir")).unwrap();
        let exts: Vec<String> = [".cue", ".bin", ".chd", ".m3u", ".ps3dir"].iter().map(|s| s.to_string()).collect();
        let mut rels: Vec<String> = scan_system(&d, &exts).into_iter().map(|r| r.rel).collect();
        rels.sort();
        assert_eq!(rels, ["Game A (USA).cue", "Multi/Game B.m3u", "Some Game.ps3dir"]);
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn builds_launch_commands_for_installed_emulators_only() {
        let d = std::env::temp_dir().join(format!("lodestone-cores-{}", std::process::id()));
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("snes9x_libretro.so"), "x").unwrap();
        let mut emus = HashMap::new();
        emus.insert("RETROARCH", Emulator { key: "RETROARCH", name: "RetroArch", program: "flatpak".into(), pre_args: vec!["run".into(), "org.libretro.RetroArch".into()], cores: Some(d.clone()) });
        let rom = Path::new("/roms/snes/Game (USA).sfc");
        let (spec, name) = build_launch("%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/snes9x_libretro.so %ROM%", rom, Path::new("/roms/snes"), &emus).unwrap();
        assert_eq!(name, "RetroArch");
        assert_eq!(spec.program, "flatpak");
        assert_eq!(spec.args, ["run", "org.libretro.RetroArch", "-L", &format!("{}/snes9x_libretro.so", d.display()), "/roms/snes/Game (USA).sfc"]);
        // Core not installed, emulator not installed, or an ES-DE-only feature: not usable.
        assert!(build_launch("%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/bsnes_libretro.so %ROM%", rom, Path::new("/roms/snes"), &emus).is_none());
        assert!(build_launch("%EMULATOR_DOLPHIN% -b -e %ROM%", rom, Path::new("/roms/snes"), &emus).is_none());
        assert!(build_launch("%ENABLESHORTCUTS% %EMULATOR_OS-SHELL% %ROM%", rom, Path::new("/roms/snes"), &emus).is_none());
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn gamelist_paths_stay_in_the_folder() {
        let d = std::env::temp_dir().join(format!("lodestone-gl-{}", std::process::id()));
        fs::create_dir_all(d.join("images")).unwrap();
        fs::write(d.join("images/a.jpg"), "x").unwrap();
        fs::write(d.join("gamelist.xml"), "<gameList><game><path>./A.nsp</path><name>Alpha &amp; Omega</name><image>./images/a.jpg</image></game><game><path>./B.nsp</path><name>B</name><image>../../etc/passwd</image></game></gameList>").unwrap();
        let g = gamelist(&d);
        assert_eq!(g["A.nsp"].0, "Alpha & Omega");
        assert!(g["A.nsp"].1.is_some());
        assert!(g["B.nsp"].1.is_none());
        let _ = fs::remove_dir_all(d);
    }

    /// `cargo test live_core_download -- --ignored --nocapture`: a real core into a temporary folder.
    #[test]
    #[ignore]
    fn live_core_download() {
        let d = std::env::temp_dir().join(format!("lodestone-core-{}", std::process::id()));
        tauri::async_runtime::block_on(download_core_to("snes9x_libretro.so", &d)).unwrap();
        let f = d.join("snes9x_libretro.so");
        let head = fs::read(&f).unwrap();
        println!("{} bytes, ELF: {}", head.len(), head.starts_with(b"\x7fELF"));
        assert!(head.starts_with(b"\x7fELF"));
        let _ = fs::remove_dir_all(d);
    }

    /// `cargo test live_emulation -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_emulation() {
        println!("roots: {:?}", roots());
        println!("emulators: {:?}", installed_emulators().keys().collect::<Vec<_>>());
        let t = Instant::now();
        let games = scan();
        println!("{} games in {:?}", games.len(), t.elapsed());
        for s in systems() {
            println!("{:12} {:40} {:6} {:?}", s.id, s.name, s.games, s.emulators);
        }
        for g in games.iter().filter(|g| g.cover.is_some()).take(3).chain(games.iter().filter(|g| g.launch.is_some()).take(3)) {
            println!("{} | {} | {} | cover {} | launch {:?}", g.id, g.title, g.via, g.cover.is_some(), g.launch.as_ref().map(|l| (&l.program, &l.args)));
        }
    }
}
