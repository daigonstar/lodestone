//! Steam in the background. Lodestone keeps the Steam client running silently (tray only) and
//! installs, updates and uninstalls Steam games without Steam's windows:
//!
//! - Steam reads app manifests only when it starts. So Lodestone stops Steam for a few seconds,
//!   writes the change (a new manifest marked "update required" to install, a cleared update
//!   schedule to update now, removed files to uninstall), and restarts it with `-silent`.
//!   Several requests made close together share one restart.
//! - Progress comes from Steam's own content log and the manifests, shown in Lodestone's
//!   downloads panel. Updates Steam starts by itself show up there too.
//!
//! Nothing is restarted while a Steam game is running: callers fall back to Steam's dialogs.

use crate::downloads::Status;
use crate::launcher;
use crate::stores::steam;
use crate::vdf;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Returned when a Steam game is running, so Steam can't be restarted right now.
pub const BUSY: &str = "steam-busy";

pub fn steam_running() -> bool {
    crate::host::procs().iter().any(|p| p.comm == "steam")
}

/// Steam starts every game through its "reaper" with `SteamLaunch AppId=N`.
fn game_running() -> bool {
    launcher::process_table().iter().any(|p| p.contains("\0SteamLaunch\0"))
}

pub fn enabled() -> bool {
    crate::settings::load().steam_background && steam::primary_root().is_some()
}

/// Start Steam minimized to the tray if it isn't running.
pub fn start_silent() {
    if steam_running() {
        return;
    }
    if let Some(spec) = steam::client_cmd(&["-silent"]) {
        let _ = launcher::spawn(&spec, launcher::log_path("steam-background"));
    }
}

fn stop_steam() -> Result<(), String> {
    if !steam_running() {
        return Ok(());
    }
    let spec = steam::client_cmd(&["-shutdown"]).ok_or("Steam isn't installed")?;
    let _ = launcher::spawn(&spec, launcher::log_path("steam-background"));
    for _ in 0..60 {
        std::thread::sleep(Duration::from_millis(500));
        if !steam_running() {
            // Let it finish flushing its files.
            std::thread::sleep(Duration::from_millis(800));
            return Ok(());
        }
    }
    Err("Steam didn't close in time".into())
}

// ---------------- requests ----------------

enum Edit {
    Write(PathBuf, String),
    Update(PathBuf),
    Remove(Vec<PathBuf>),
}

struct Job {
    key: String,
    edit: Edit,
}

#[derive(Default)]
struct Queue {
    jobs: Vec<Job>,
    worker: bool,
}

static QUEUE: Mutex<Queue> = Mutex::new(Queue { jobs: Vec::new(), worker: false });

#[derive(Clone)]
struct Track {
    appid: u32,
    title: String,
    /// install, update, uninstall
    kind: &'static str,
    /// Byte offset in Steam's content log when this started (only later lines count).
    offset: u64,
    started: Instant,
    /// When Steam came back up with the change applied.
    applied: Option<Instant>,
    /// Steam started this itself (an automatic update); dropped soon after it ends.
    auto: bool,
    result: Option<(Instant, Result<(), String>)>,
}

static TRACKS: Mutex<Option<HashMap<String, Track>>> = Mutex::new(None);
/// Where in the content log Lodestone started watching for Steam's own updates.
static WATCH_FROM: Mutex<Option<u64>> = Mutex::new(None);

fn with_tracks<R>(f: impl FnOnce(&mut HashMap<String, Track>) -> R) -> R {
    f(TRACKS.lock().unwrap().get_or_insert_with(HashMap::new))
}

fn content_log() -> Option<PathBuf> {
    steam::primary_root().map(|(r, _)| r.join("logs/content_log.txt"))
}

fn log_len() -> u64 {
    content_log().and_then(|p| fs::metadata(p).ok()).map(|m| m.len()).unwrap_or(0)
}

fn enqueue(key: String, edit: Edit) {
    let mut q = QUEUE.lock().unwrap();
    q.jobs.push(Job { key, edit });
    if q.worker {
        return;
    }
    q.worker = true;
    std::thread::spawn(|| loop {
        // Gather requests made close together into one restart.
        std::thread::sleep(Duration::from_secs(2));
        let jobs = {
            let mut q = QUEUE.lock().unwrap();
            if q.jobs.is_empty() {
                q.worker = false;
                return;
            }
            std::mem::take(&mut q.jobs)
        };
        let fail = |jobs: &[Job], e: &str| {
            with_tracks(|t| {
                for j in jobs {
                    if let Some(tr) = t.get_mut(&j.key) {
                        tr.result = Some((Instant::now(), Err(e.to_string())));
                    }
                }
            })
        };
        if game_running() {
            fail(&jobs, "A Steam game started; try again after it closes");
            continue;
        }
        if let Err(e) = stop_steam() {
            fail(&jobs, &e);
            continue;
        }
        for j in &jobs {
            let res = apply(&j.edit);
            with_tracks(|t| {
                if let Some(tr) = t.get_mut(&j.key) {
                    match (&res, tr.kind) {
                        (Err(e), _) => tr.result = Some((Instant::now(), Err(e.clone()))),
                        (Ok(()), "uninstall") => tr.result = Some((Instant::now(), Ok(()))),
                        _ => {}
                    }
                }
            });
        }
        start_silent();
        with_tracks(|t| {
            for j in &jobs {
                if let Some(tr) = t.get_mut(&j.key) {
                    tr.applied = Some(Instant::now());
                }
            }
        });
    });
}

/// Set a top-level key in an app manifest (keeps everything else as Steam wrote it).
fn set_key(txt: &str, key: &str, val: &str) -> String {
    let want = format!("\"{key}\"");
    let mut depth = 0;
    let mut done = false;
    let mut out: Vec<String> = Vec::new();
    for line in txt.lines() {
        let t = line.trim();
        if t == "{" {
            depth += 1;
        } else if t == "}" {
            if depth == 1 && !done {
                out.push(format!("\t{want}\t\t\"{val}\""));
                done = true;
            }
            depth -= 1;
        } else if depth == 1 && !done && t.starts_with(&want) {
            out.push(format!("\t{want}\t\t\"{val}\""));
            done = true;
            continue;
        }
        out.push(line.to_string());
    }
    out.join("\n") + "\n"
}

fn apply(edit: &Edit) -> Result<(), String> {
    match edit {
        Edit::Write(path, body) => fs::write(path, body).map_err(|e| format!("Couldn't write {}: {e}", path.display())),
        Edit::Update(path) => {
            let txt = fs::read_to_string(path).map_err(|e| e.to_string())?;
            let v = vdf::parse(&txt);
            let flags: u32 = v.get("AppState").and_then(|a| a.str("StateFlags")).and_then(|s| s.parse().ok()).unwrap_or(4);
            // "Update required" + no delay: Steam updates it as soon as it starts.
            let txt = set_key(&txt, "StateFlags", &(flags | 2).to_string());
            let txt = set_key(&txt, "ScheduledAutoUpdate", "0");
            fs::write(path, txt).map_err(|e| e.to_string())
        }
        Edit::Remove(paths) => {
            for p in paths {
                let r = if p.is_dir() { fs::remove_dir_all(p) } else if p.exists() { fs::remove_file(p) } else { Ok(()) };
                r.map_err(|e| format!("Couldn't delete {}: {e}", p.display()))?;
            }
            Ok(())
        }
    }
}

fn free_space(p: &Path) -> u64 {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(p.as_os_str().as_bytes()) else { return 0 };
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return 0;
    }
    s.f_bavail as u64 * s.f_frsize as u64
}

/// A folder name that is safe to join under steamapps/common.
fn safe_dir(name: &str) -> Option<&str> {
    let n = name.trim();
    (!n.is_empty() && n != "." && n != ".." && !n.contains('/') && !n.contains('\0')).then_some(n)
}

fn check_ready(appid: u32) -> Result<(), String> {
    if game_running() {
        return Err(BUSY.into());
    }
    let busy = with_tracks(|t| t.values().any(|tr| tr.appid == appid && tr.result.is_none() && !tr.auto));
    if busy {
        return Err("Lodestone is already working on this game".into());
    }
    Ok(())
}

fn track(key: &str, appid: u32, title: &str, kind: &'static str) {
    with_tracks(|t| {
        t.insert(key.into(), Track { appid, title: title.into(), kind, offset: log_len(), started: Instant::now(), applied: None, auto: false, result: None });
    });
}

/// Install a game: Steam downloads it after a silent restart. Its library is the one with the most free space.
pub fn install(appid: u32, title: &str) -> Result<(), String> {
    check_ready(appid)?;
    if steam::manifest_path(appid).is_some() {
        return Err(format!("{title} is already installed or installing in Steam"));
    }
    let (name, dir) = steam::app_meta(appid).unwrap_or_default();
    let name = if name.is_empty() { title.to_string() } else { name };
    let fallback: String = name.chars().filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')).collect();
    let dir = safe_dir(&dir).or(safe_dir(&fallback)).ok_or("Steam doesn't know this game's folder name")?.to_string();
    let lib = steam::library_dirs()
        .into_iter()
        .filter(|l| l.join("steamapps").is_dir())
        .max_by_key(|l| free_space(l))
        .ok_or("No Steam library found")?;
    let q = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let body = format!(
        "\"AppState\"\n{{\n\t\"appid\"\t\t\"{appid}\"\n\t\"Universe\"\t\t\"1\"\n\t\"name\"\t\t\"{}\"\n\t\"StateFlags\"\t\t\"1026\"\n\t\"installdir\"\t\t\"{}\"\n\t\"LastUpdated\"\t\t\"0\"\n\t\"SizeOnDisk\"\t\t\"0\"\n\t\"buildid\"\t\t\"0\"\n\t\"BytesToDownload\"\t\t\"0\"\n\t\"BytesDownloaded\"\t\t\"0\"\n\t\"AutoUpdateBehavior\"\t\t\"0\"\n\t\"AllowOtherDownloadsWhileRunning\"\t\t\"0\"\n\t\"ScheduledAutoUpdate\"\t\t\"0\"\n}}\n",
        q(&name),
        q(&dir)
    );
    let key = format!("steam:{appid}");
    track(&key, appid, title, "install");
    enqueue(key, Edit::Write(lib.join(format!("steamapps/appmanifest_{appid}.acf")), body));
    Ok(())
}

/// Update now instead of when Steam's scheduler gets to it.
pub fn update(appid: u32, title: &str) -> Result<(), String> {
    check_ready(appid)?;
    let m = steam::manifest_path(appid).ok_or(format!("{title} isn't installed in Steam"))?;
    let key = format!("update-steam:{appid}");
    track(&key, appid, &format!("Updating {title}"), "update");
    enqueue(key, Edit::Update(m));
    Ok(())
}

/// Files Steam keeps for an installed app in its library. The Proton prefix (compatdata, with
/// saves) is kept, like Steam does when it uninstalls.
fn app_files(appid: u32, manifest: &Path, with_game: bool) -> Result<Vec<PathBuf>, String> {
    let apps = manifest.parent().ok_or("bad manifest path")?.to_path_buf();
    let mut paths = vec![manifest.to_path_buf()];
    if with_game {
        let txt = fs::read_to_string(manifest).map_err(|e| e.to_string())?;
        let v = vdf::parse(&txt);
        let dir = v.get("AppState").and_then(|a| a.str("installdir")).unwrap_or("").to_string();
        let dir = safe_dir(&dir).ok_or("This game's install folder is missing from Steam's manifest")?;
        paths.push(apps.join("common").join(dir));
    }
    for sub in ["downloading", "temp", "shadercache", "workshop/content", "workshop/downloads", "workshop/temp"] {
        paths.push(apps.join(sub).join(appid.to_string()));
    }
    paths.push(apps.join(format!("workshop/appworkshop_{appid}.acf")));
    Ok(paths)
}

pub fn uninstall(appid: u32, title: &str) -> Result<(), String> {
    check_ready(appid)?;
    let m = steam::manifest_path(appid).ok_or(format!("{title} isn't installed in Steam"))?;
    let paths = app_files(appid, &m, true)?;
    let key = format!("uninstall-steam:{appid}");
    track(&key, appid, &format!("Uninstalling {title}"), "uninstall");
    enqueue(key, Edit::Remove(paths));
    Ok(())
}

/// Cancel a Lodestone-started install: drop the half-downloaded game.
pub fn cancel(key: &str) -> Result<(), String> {
    let Some(tr) = with_tracks(|t| t.get(key).cloned()) else { return Ok(()) };
    if tr.kind != "install" || tr.auto {
        return Err("Steam finishes updates by itself; this one can't be cancelled from Lodestone".into());
    }
    if game_running() {
        return Err("Close the running Steam game first".into());
    }
    // Still waiting for the restart: just drop the request.
    {
        let mut q = QUEUE.lock().unwrap();
        let before = q.jobs.len();
        q.jobs.retain(|j| j.key != key);
        if q.jobs.len() != before {
            with_tracks(|t| t.remove(key));
            return Ok(());
        }
    }
    with_tracks(|t| t.remove(key));
    if let Some(m) = steam::manifest_path(tr.appid) {
        let flags: u32 = fs::read_to_string(&m).ok().and_then(|t| vdf::parse(&t).get("AppState")?.str("StateFlags")?.parse().ok()).unwrap_or(0);
        let paths = app_files(tr.appid, &m, flags & 4 == 0)?;
        enqueue(format!("cancel-{key}"), Edit::Remove(paths));
    }
    Ok(())
}

pub fn dismiss(key: &str) {
    with_tracks(|t| {
        if t.get(key).is_some_and(|tr| tr.result.is_some()) {
            t.remove(key);
        }
    });
}

// ---------------- progress ----------------

#[derive(Default, Debug, PartialEq)]
struct Seen {
    any: bool,
    running: bool,
    delayed: bool,
    /// (download bytes, stage bytes) from "update started"
    totals: Option<(u64, u64)>,
    finished: Option<Result<(), String>>,
    /// Steam wrote "finished update" (the new files are in place).
    updated: bool,
    /// Steam took it off its schedule unfinished ("Suspended"): paused, not failed.
    paused: bool,
}

fn after_colon(line: &str) -> &str {
    line.split_once(" : ").map(|x| x.1).unwrap_or("")
}

fn total_of(s: &str, what: &str) -> Option<u64> {
    s.split(',').find_map(|p| p.trim().strip_prefix(what)?.trim().split('/').nth(1)?.trim().parse().ok())
}

/// What Steam's content log says about one app, from the given lines (oldest first).
fn read_app(lines: &[&str], appid: u32) -> Seen {
    let tag = format!("AppID {appid} ");
    let mut s = Seen::default();
    for l in lines {
        let Some(i) = l.find(&tag) else { continue };
        let rest = &l[i + tag.len()..];
        s.any = true;
        if rest.starts_with("state changed") {
            let st = after_colon(rest);
            let run = st.contains("Update Running");
            if run && !s.running {
                s.finished = None;
                s.updated = false;
                s.paused = false;
            }
            s.running = run;
            s.delayed = st.contains("Update delayed");
        } else if rest.starts_with("update started") {
            let st = after_colon(rest);
            s.totals = Some((total_of(st, "download").unwrap_or(0), total_of(st, "stage").unwrap_or(0)));
        } else if rest.starts_with("finished update") {
            s.updated = true;
        } else if rest.starts_with("scheduler finished") {
            let res = rest.split("(result ").nth(1).and_then(|r| r.split(',').next()).unwrap_or("No Error");
            s.running = false;
            // "Suspended" is Steam handing over to another download: done if the update finished
            // first, otherwise paused until Steam picks it up again. Neither is a failure.
            s.finished = match res {
                "No Error" => Some(Ok(())),
                "Suspended" if s.updated => Some(Ok(())),
                "Suspended" => {
                    s.paused = true;
                    None
                }
                other => Some(Err(format!("Steam: {other}"))),
            };
        }
    }
    s
}

fn read_log_from(offset: u64) -> String {
    let Some(p) = content_log() else { return String::new() };
    let Ok(mut f) = fs::File::open(p) else { return String::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    // The log was rotated: start over. Never read more than the last 4 MB.
    let start = if offset > len { 0 } else { offset }.max(len.saturating_sub(4 << 20));
    let _ = f.seek(SeekFrom::Start(start));
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

fn manifest_info(appid: u32) -> Option<(String, u32, u64, u64)> {
    let m = steam::manifest_path(appid)?;
    let v = vdf::parse(&fs::read_to_string(m).ok()?);
    let a = v.get("AppState")?;
    let n = |k: &str| a.str(k).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    Some((a.str("name").unwrap_or("").to_string(), n("StateFlags") as u32, n("BytesDownloaded"), n("BytesToDownload")))
}

/// The byte counts in a game's appmanifest, and the game's folder name.
#[derive(Default, Debug, PartialEq)]
struct Sizes {
    downloaded: u64,
    to_download: u64,
    staged: u64,
    to_stage: u64,
    installdir: String,
}

fn manifest_sizes(text: &str) -> Option<Sizes> {
    let v = vdf::parse(text);
    let a = v.get("AppState")?;
    let n = |k: &str| a.str(k).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    Some(Sizes {
        downloaded: n("BytesDownloaded"),
        to_download: n("BytesToDownload"),
        staged: n("BytesStaged"),
        to_stage: n("BytesToStage"),
        installdir: a.str("installdir").unwrap_or("").to_string(),
    })
}

/// How far a Steam download is: (percent, bytes done, bytes in total).
///
/// Steam only rewrites the manifest's byte counts now and then, so they can sit at 0 for most of a
/// download. What grows steadily is the game's folder under steamapps/downloading, where Steam puts the
/// new files before swapping them in; it's measured against the bytes to stage. Whichever source is
/// further along wins. (This doesn't depend on having seen the update start, so updates Steam began
/// before Lodestone opened are measured too.)
fn progress(appid: u32, seen: &Seen, kind: &str) -> (f32, Option<u64>, Option<u64>) {
    let Some(m) = steam::manifest_path(appid) else { return (0.0, None, None) };
    let s = fs::read_to_string(&m).ok().and_then(|t| manifest_sizes(&t)).unwrap_or_default();
    let mut in_folder = disk_used(&m.with_file_name("downloading").join(appid.to_string()));
    // A new install usually goes straight into the game's own folder instead.
    if kind == "install" && !s.installdir.is_empty() {
        in_folder = in_folder.max(disk_used(&m.with_file_name("common").join(&s.installdir)));
    }
    progress_from(&s, in_folder, seen.totals.map(|t| t.1))
}

/// Disk space a folder really uses (blocks written), not its files' apparent sizes: Steam reserves
/// full-size files up front, which would otherwise look finished straight away.
fn disk_used(p: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    let Ok(rd) = fs::read_dir(p) else { return 0 };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => disk_used(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.blocks() * 512).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

fn progress_from(s: &Sizes, in_folder: u64, logged_stage: Option<u64>) -> (f32, Option<u64>, Option<u64>) {
    let to_stage = if s.to_stage > 0 { s.to_stage } else { logged_stage.unwrap_or(0) };
    let staged = s.staged.max(in_folder).min(to_stage);
    let pct = |d: u64, t: u64| if t > 0 { d as f32 / t as f32 * 100.0 } else { 0.0 };
    let by_stage = pct(staged, to_stage);
    let by_download = pct(s.downloaded, s.to_download);
    let percent = by_stage.max(by_download).clamp(0.0, 99.0);
    if to_stage > 0 && by_stage >= by_download {
        (percent, Some(staged), Some(to_stage))
    } else if s.to_download > 0 {
        (percent, Some(s.downloaded), Some(s.to_download))
    } else {
        (percent, None, None)
    }
}

/// Steam work for the downloads panel: Lodestone's requests and updates Steam runs by itself.
pub fn statuses() -> Vec<Status> {
    if steam::primary_root().is_none() {
        return vec![];
    }
    let watch = *WATCH_FROM.lock().unwrap().get_or_insert_with(|| log_len().saturating_sub(64 << 10));
    let from = with_tracks(|t| t.values().map(|tr| tr.offset).min()).map_or(watch, |o| o.min(watch));
    let text = read_log_from(from);
    let lines: Vec<&str> = text.lines().collect();
    let up = steam_running();

    // Updates Steam started itself.
    if up {
        let mut ids: Vec<u32> = lines
            .iter()
            .filter(|l| l.contains("state changed") && l.contains("Update Running"))
            .filter_map(|l| l.split("AppID ").nth(1)?.split(' ').next()?.parse().ok())
            .collect();
        ids.sort_unstable();
        ids.dedup();
        for id in ids {
            if with_tracks(|t| t.values().any(|tr| tr.appid == id)) || !read_app(&lines, id).running {
                continue;
            }
            let Some((name, flags, _, _)) = manifest_info(id) else { continue };
            let installed = flags & 4 != 0;
            let (key, title, kind) = if installed {
                (format!("update-steam:{id}"), format!("Updating {name}"), "update")
            } else {
                (format!("steam:{id}"), name, "install")
            };
            with_tracks(|t| {
                t.insert(key, Track { appid: id, title, kind, offset: watch, started: Instant::now(), applied: Some(Instant::now()), auto: true, result: None })
            });
        }
    }

    let mut out = Vec::new();
    with_tracks(|t| {
        for (key, tr) in t.iter_mut() {
            if tr.result.is_none() && tr.kind != "uninstall" {
                // Only lines written after this request count.
                let own: Vec<&str> = if tr.offset > from {
                    let mut cut = ((tr.offset - from) as usize).min(text.len());
                    while !text.is_char_boundary(cut) {
                        cut += 1;
                    }
                    text[cut..].lines().collect()
                } else {
                    lines.clone()
                };
                let seen = read_app(&own, tr.appid);
                if let Some(r) = seen.finished.clone().filter(|_| tr.applied.is_some() && !seen.running) {
                    tr.result = Some((Instant::now(), r));
                } else if seen.delayed && !seen.running {
                    tr.result = Some((Instant::now(), Err("Steam put this off until later. Steam → Downloads can start it now.".into())));
                } else if tr.applied.is_some_and(|a| a.elapsed() > Duration::from_secs(150)) && !seen.any {
                    tr.result = Some((Instant::now(), Err("Steam didn't start the download".into())));
                } else if tr.auto && !up {
                    tr.result = Some((Instant::now(), Err("Steam closed".into())));
                }
                let (state, pct, detail) = match &tr.result {
                    Some(_) => ("", 0.0, String::new()),
                    None if tr.applied.is_none() => ("downloading", 0.0, "Handing it to Steam…".to_string()),
                    None if seen.paused && !seen.running => ("downloading", progress(tr.appid, &seen, tr.kind).0, "Paused by Steam (another download went first); it carries on by itself".to_string()),
                    None if !seen.running => ("downloading", 0.0, "Waiting for Steam…".to_string()),
                    None => ("downloading", progress(tr.appid, &seen, tr.kind).0, "Steam is downloading".to_string()),
                };
                if !state.is_empty() {
                    // Bytes from the manifest and Steam's download folder; Lodestone works out the speed and
                    // time left from how fast they grow.
                    let (downloaded, total) = if seen.running { let (_, d, t) = progress(tr.appid, &seen, tr.kind); (d, t) } else { (None, None) };
                    // The game's own folder, from the manifest's installdir.
                    let path = steam::manifest_path(tr.appid).and_then(|m| {
                        let dir = fs::read_to_string(&m).ok().and_then(|t| manifest_sizes(&t)).map(|s| s.installdir).filter(|d| !d.is_empty())?;
                        Some(m.parent()?.join("common").join(dir).to_string_lossy().into_owned())
                    });
                    out.push(Status { id: key.clone(), title: tr.title.clone(), state, percent: pct, detail, seconds: tr.started.elapsed().as_secs(), kind: tr.kind, downloaded, total, path, ..Default::default() });
                    continue;
                }
            }
            match &tr.result {
                Some((_, Ok(()))) => out.push(Status { id: key.clone(), title: tr.title.clone(), state: "done", percent: 100.0, detail: String::new(), seconds: tr.started.elapsed().as_secs(), kind: tr.kind, ..Default::default() }),
                Some((_, Err(e))) => out.push(Status { id: key.clone(), title: tr.title.clone(), state: "failed", percent: 0.0, detail: e.clone(), seconds: tr.started.elapsed().as_secs(), kind: tr.kind, ..Default::default() }),
                None => out.push(Status { id: key.clone(), title: tr.title.clone(), state: "downloading", percent: 0.0, detail: "Uninstalling…".into(), seconds: tr.started.elapsed().as_secs(), kind: tr.kind, ..Default::default() }),
            }
        }
        // Steam's own updates leave the panel shortly after they finish.
        t.retain(|_, tr| !(tr.auto && tr.result.as_ref().is_some_and(|(at, _)| at.elapsed() > Duration::from_secs(8))));
    });
    out
}

#[cfg(test)]
mod suspended_tests {
    /// Steam's real log for a redistributables update that finished, then was "Suspended" off the
    /// schedule as Grim Dawn's install took over. It was reported as failed.
    #[test]
    fn suspended_after_finishing_is_success() {
        let log = "AppID 228980 state changed : Update Required,Fully Installed,Update Queued,Update Running,
AppID 228980 update started : download 0/42914928, store 0/0, reuse 0/0, delta 0/0, stage 0/43001447 
AppID 228980 state changed : Update Required,Fully Installed,Update Queued,Update Running,Update Started,
AppID 228980 finished update, 8 mounted depots (BuildID 24098430) : 228983 (1)
AppID 228980 state changed : Fully Installed,Update Queued,
AppID 228980 state changed : Fully Installed,
AppID 228980 scheduler finished : removed from schedule (result Suspended, state 0xc) ";
        let lines: Vec<&str> = log.lines().collect();
        let s = super::read_app(&lines, 228980);
        assert_eq!(s.finished, Some(Ok(())));
        // Suspended before finishing: paused, not failed.
        let s = super::read_app(&lines[..3].iter().copied().chain(["AppID 228980 scheduler finished : removed from schedule (result Suspended, state 0x6) "]).collect::<Vec<_>>(), 228980);
        assert_eq!((s.finished, s.paused), (None, true));
        // A real error is still an error.
        let s = super::read_app(&lines[..3].iter().copied().chain(["AppID 228980 scheduler finished : removed from schedule (result No connection, state 0x6) "]).collect::<Vec<_>>(), 228980);
        assert_eq!(s.finished, Some(Err("Steam: No connection".into())));
    }
}

#[cfg(test)]
mod progress_tests {
    use super::*;

    #[test]
    fn measures_updates_steam_started_on_its_own() {
        // The tester's case: Steam left the manifest at 0 while the download folder filled up.
        let s = Sizes { downloaded: 0, to_download: 4_600_000_000, staged: 0, to_stage: 5_100_000_000, installdir: "rocketleague".into() };
        let (pct, done, total) = progress_from(&s, 3_900_000_000, None);
        assert!((76.0..77.0).contains(&pct), "{pct}");
        assert_eq!((done, total), (Some(3_900_000_000), Some(5_100_000_000)));
        // Manifest ahead of the folder (Steam just saved it): the manifest wins.
        let s = Sizes { downloaded: 4_000_000_000, to_download: 4_600_000_000, staged: 0, to_stage: 5_100_000_000, installdir: String::new() };
        assert_eq!(progress_from(&s, 0, None).1, Some(4_000_000_000));
        // Nothing known yet.
        assert_eq!(progress_from(&Sizes::default(), 0, None), (0.0, None, None));
        let m = manifest_sizes("\"AppState\" { \"installdir\" \"rocketleague\" \"BytesToStage\" \"5\" \"BytesStaged\" \"2\" }").unwrap();
        assert_eq!((m.installdir.as_str(), m.staged, m.to_stage), ("rocketleague", 2, 5));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "[2026-11-21 07:09:59] AppID 3483 scheduler update : Priority Auto Update, not played for 99160700 seconds, update disabled for 0 seconds
[2026-11-21 07:09:59] AppID 3483 state changed : Update Required,Update Queued,Update Running,Update Started,
[2026-11-21 07:10:01] AppID 3483 update started : download 0/14690192, store 0/0, reuse 0/0, delta 0/0, stage 0/23356380
[2026-11-21 07:10:01] AppID 3483 App update changed : Running Update,Downloading,Staging,
[2026-11-21 07:10:02] AppID 3483 state changed : Fully Installed,Update Queued,Update Running,
[2026-11-21 07:10:02] AppID 3483 finished update, 1 mounted depots (BuildID 250754) : 3484 (4053752754002852222),
[2026-11-21 07:10:02] AppID 3483 state changed : Fully Installed,
[2026-11-21 07:10:02] AppID 3483 scheduler finished : removed from schedule (result No Error, state 0xc)
[2026-11-20 20:24:42] AppID 2016590 state changed : Update Required,Fully Installed,Update Queued, (Update delayed for 1234005 secs)
";

    #[test]
    fn reads_steam_content_log() {
        let lines: Vec<&str> = LOG.lines().collect();
        let s = read_app(&lines[..4], 3483);
        assert!(s.running && s.finished.is_none());
        assert_eq!(s.totals, Some((14690192, 23356380)));
        let s = read_app(&lines, 3483);
        assert!(!s.running);
        assert_eq!(s.finished, Some(Ok(())));
        let d = read_app(&lines, 2016590);
        assert!(d.delayed && !d.running);
        assert!(!read_app(&lines, 1).any);
    }

    #[test]
    fn sets_manifest_keys_at_top_level_only() {
        let m = "\"AppState\"\n{\n\t\"appid\"\t\t\"1\"\n\t\"StateFlags\"\t\t\"4\"\n\t\"InstalledDepots\"\n\t{\n\t\t\"2\"\n\t\t{\n\t\t\t\"size\"\t\t\"5\"\n\t\t}\n\t}\n}\n";
        let out = set_key(&set_key(m, "StateFlags", "6"), "ScheduledAutoUpdate", "0");
        let v = vdf::parse(&out);
        let a = v.get("AppState").unwrap();
        assert_eq!(a.str("StateFlags"), Some("6"));
        assert_eq!(a.str("ScheduledAutoUpdate"), Some("0"));
        assert!(a.get("InstalledDepots").is_some());
    }

    #[test]
    fn refuses_unsafe_folder_names() {
        assert_eq!(safe_dir("Peggle Extreme"), Some("Peggle Extreme"));
        assert_eq!(safe_dir(".."), None);
        assert_eq!(safe_dir("a/b"), None);
        assert_eq!(safe_dir("  "), None);
    }
}

#[cfg(test)]
mod live {
    use super::*;

    fn wait(key: &str, secs: u64) -> Status {
        let t = Instant::now();
        loop {
            std::thread::sleep(Duration::from_millis(1500));
            let st = statuses();
            if let Some(s) = st.iter().find(|s| s.id == key) {
                println!("{:>3}s {} {} {:.1}% {}", t.elapsed().as_secs(), s.title, s.state, s.percent, s.detail);
                if s.state != "downloading" {
                    return s.clone();
                }
            }
            assert!(t.elapsed().as_secs() < secs, "timed out");
        }
    }

    /// Needs Steam and owning Peggle Extreme (3483, ~15 MB). Run: LODESTONE_LIVE_APP=3483 cargo test live_cycle -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_cycle() {
        let id: u32 = std::env::var("LODESTONE_LIVE_APP").expect("set LODESTONE_LIVE_APP").parse().unwrap();
        if steam::manifest_path(id).is_some() {
            uninstall(id, "test game").unwrap();
            assert_eq!(wait(&format!("uninstall-steam:{id}"), 90).state, "done");
            assert!(steam::manifest_path(id).is_none());
        }
        install(id, "test game").unwrap();
        assert_eq!(wait(&format!("steam:{id}"), 300).state, "done");
        let (_, flags, _, _) = manifest_info(id).unwrap();
        assert_eq!(flags, 4, "installed");
        update(id, "test game").unwrap();
        assert_eq!(wait(&format!("update-steam:{id}"), 120).state, "done");
        assert!(steam_running());
    }
}

#[cfg(test)]
mod live_status {
    /// `cargo test live_steam_downloads -- --ignored --nocapture`: what the Downloads tab would show now.
    #[test]
    #[ignore]
    fn live_steam_downloads() {
        for _ in 0..2 {
            for s in super::statuses() {
                println!("{} | {} | {} | {:.1}% | {:?}/{:?} | {} | {:?}", s.id, s.title, s.state, s.percent, s.downloaded, s.total, s.detail, s.path);
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
        println!("steam running: {}", super::steam_running());
    }
}
