//! Game downloads run by Lodestone itself (Epic via legendary, GOG via gogdl), with progress from their logs.

use crate::launcher;
use crate::model::LaunchSpec;
use serde::Serialize;
use std::collections::HashMap;
use std::process::Child;
use std::sync::Mutex;
use std::time::Instant;

struct Job {
    title: String,
    child: Child,
    log: std::path::PathBuf,
    started: Instant,
    /// Called once when the download finishes successfully (e.g. record the GOG install folder).
    finish: Option<Box<dyn FnOnce() -> Result<(), String> + Send>>,
    result: Option<Result<(), String>>,
}

static JOBS: Mutex<Option<HashMap<String, Job>>> = Mutex::new(None);

#[derive(Serialize, Clone, Default)]
pub struct Status {
    pub id: String,
    pub title: String,
    /// downloading, done, failed
    pub state: &'static str,
    pub percent: f32,
    pub detail: String,
    pub seconds: u64,
    /// install, update, uninstall, runtimes
    pub kind: &'static str,
    /// The library game this is for ("epic:Quail"), and its store.
    pub game_id: String,
    pub store: String,
    /// Bytes downloaded so far and in total (when the store says).
    pub downloaded: Option<u64>,
    pub total: Option<u64>,
    /// Download and disk-write speed, bytes per second.
    pub speed: Option<f64>,
    pub disk: Option<f64>,
    /// Seconds left, as the downloader estimates.
    pub eta: Option<u64>,
    /// Size on disk once installed, and where it goes.
    pub install_size: Option<u64>,
    pub path: Option<String>,
}

pub fn start(id: &str, title: &str, spec: &LaunchSpec, finish: Option<Box<dyn FnOnce() -> Result<(), String> + Send>>) -> Result<(), String> {
    let mut jobs = JOBS.lock().unwrap();
    let jobs = jobs.get_or_insert_with(HashMap::new);
    if jobs.get(id).is_some_and(|j| j.result.is_none()) {
        return Err(format!("{title} is already downloading"));
    }
    let log = launcher::log_path(&format!("install-{id}"));
    let child = launcher::spawn(spec, log.clone()).map_err(|e| e.to_string())?;
    jobs.insert(id.to_string(), Job { title: title.into(), child, log, started: Instant::now(), finish, result: None });
    Ok(())
}

pub fn cancel(id: &str) {
    if let Some(j) = JOBS.lock().unwrap().get_or_insert_with(HashMap::new).remove(id) {
        let mut c = j.child;
        // The downloader runs in its own process group (see launcher::spawn): stop it and its workers.
        let _ = std::process::Command::new("kill").args(["-TERM", "--", &format!("-{}", c.id())]).status();
        let _ = c.kill();
        let _ = c.wait();
    }
}

/// What a downloader's log says: legendary (Epic) and gogdl (GOG) print the same progress block.
#[derive(Default, Debug, PartialEq)]
struct Stats {
    pct: f32,
    last: String,
    downloaded: Option<u64>,
    total: Option<u64>,
    speed: Option<f64>,
    disk: Option<f64>,
    eta: Option<u64>,
    install_size: Option<u64>,
    path: Option<String>,
}

/// "674.89 MiB" -> bytes.
fn bytes_of(s: &str) -> Option<u64> {
    let mut it = s.split_whitespace();
    let n: f64 = it.next()?.trim_end_matches(',').parse().ok()?;
    let mult = match it.next().unwrap_or("").trim_end_matches(|c: char| !c.is_ascii_alphabetic()) {
        "KiB" | "KB" => 1024.0,
        "MiB" | "MB" => 1048576.0,
        "GiB" | "GB" => 1073741824.0,
        "TiB" => 1099511627776.0,
        "B" | "bytes" => 1.0,
        _ => return None,
    };
    Some((n * mult) as u64)
}

/// "00:01:10" -> seconds.
fn hms(s: &str) -> Option<u64> {
    let p: Vec<u64> = s.trim().split(':').map(|x| x.trim().parse().ok()).collect::<Option<_>>()?;
    (p.len() == 3).then(|| p[0] * 3600 + p[1] * 60 + p[2])
}

fn stats_of(tail: &str) -> Stats {
    let mut s = Stats::default();
    let mut download_size = None;
    for line in tail.lines() {
        let after = |k: &str| line.split_once(k).map(|x| x.1.trim().to_string());
        if let Some(rest) = after("Progress:") {
            let num: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
            if let Ok(p) = num.parse::<f32>() {
                s.pct = p;
            }
            // gogdl: "47.25 15000/32000," (bytes written of the install).
            if let Some((a, b)) = rest.split_whitespace().nth(1).and_then(|x| x.trim_end_matches(',').split_once('/')) {
                if let (Ok(a), Ok(b)) = (a.parse::<u64>(), b.parse::<u64>()) {
                    if b > 100_000 {
                        s.total = Some(b);
                        s.downloaded = Some(a);
                    }
                }
            }
            s.eta = rest.split("ETA:").nth(1).and_then(hms);
        } else if let Some(rest) = after("Downloaded:") {
            if download_size.is_some() {
                s.downloaded = rest.split(',').next().and_then(bytes_of);
            }
        } else if let Some(rest) = after("+ Download") {
            s.speed = rest.trim_start_matches(['-', '\t', ' ']).split(" (").next().and_then(|x| bytes_of(&x.replace("/s", "")).map(|b| b as f64));
        } else if let Some(rest) = after("+ Disk") {
            s.disk = rest.trim_start_matches(['-', '\t', ' ']).split(" (").next().and_then(|x| bytes_of(&x.replace("/s", "")).map(|b| b as f64));
        } else if let Some(rest) = after("Download size:") {
            download_size = rest.split(" (").next().and_then(bytes_of);
            s.total = download_size;
        } else if let Some(rest) = after("Install size:") {
            s.install_size = bytes_of(&rest);
        } else if let Some(rest) = after("Install path:") {
            s.path = Some(rest);
        }
    }
    s.last = tail.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").to_string();
    s
}

/// The whole log's header (sizes, path) plus the latest progress block.
fn stats(log: &std::path::Path) -> Stats {
    let Ok(bytes) = std::fs::read(log) else { return Stats::default() };
    let head_end = bytes.len().min(16_000);
    let tail_start = bytes.len().saturating_sub(16_000);
    let text = if tail_start <= head_end {
        String::from_utf8_lossy(&bytes).into_owned()
    } else {
        format!("{}\n{}", String::from_utf8_lossy(&bytes[..head_end]), String::from_utf8_lossy(&bytes[tail_start..]))
    };
    stats_of(&text)
}

/// Last "Progress: NN.NN" in the log and its last line.
fn progress(log: &std::path::Path) -> (f32, String) {
    let s = stats(log);
    (s.pct, s.last)
}

/// Poll all jobs; finished ones run their finish step once and report done/failed until dismissed.
pub fn statuses() -> Vec<Status> {
    let mut guard = JOBS.lock().unwrap();
    let jobs = guard.get_or_insert_with(HashMap::new);
    let mut out = Vec::new();
    for (id, j) in jobs.iter_mut() {
        if j.result.is_none() {
            if let Ok(Some(code)) = j.child.try_wait() {
                j.result = Some(if code.success() {
                    j.finish.take().map(|f| f()).unwrap_or(Ok(()))
                } else {
                    Err(progress(&j.log).1)
                });
            }
        }
        let st = stats(&j.log);
        let running = j.result.is_none();
        let (state, percent, detail) = match &j.result {
            None => ("downloading", st.pct.min(100.0), st.last.clone()),
            Some(Ok(())) => ("done", 100.0, String::new()),
            Some(Err(e)) => ("failed", st.pct.min(100.0), e.clone()),
        };
        let kind = if id.starts_with("update-") { "update" } else if id.starts_with("runtimes-") { "runtimes" } else { "install" };
        out.push(Status {
            id: id.clone(),
            title: j.title.clone(),
            state,
            percent,
            detail,
            seconds: j.started.elapsed().as_secs(),
            kind,
            downloaded: st.downloaded,
            total: st.total,
            speed: st.speed.filter(|_| running),
            disk: st.disk.filter(|_| running),
            eta: st.eta.filter(|_| running),
            install_size: st.install_size,
            path: st.path,
            ..Default::default()
        });
    }
    out.sort_by(|a, b| a.title.cmp(&b.title));
    out
}

pub fn dismiss(id: &str) {
    let mut g = JOBS.lock().unwrap();
    let jobs = g.get_or_insert_with(HashMap::new);
    if jobs.get(id).is_some_and(|j| j.result.is_some()) {
        jobs.remove(id);
    }
}

// ---------------- game, store, speed, history ----------------

/// "update-epic:Quail" -> "epic:Quail".
pub fn game_of(id: &str) -> String {
    let mut s = id;
    for p in ["update-", "uninstall-", "runtimes-"] {
        s = s.strip_prefix(p).unwrap_or(s);
    }
    s.to_string()
}

static SPEED: Mutex<Option<HashMap<String, (Instant, u64, f64)>>> = Mutex::new(None);

/// Fill in game, store and (when the source has none) a speed worked out from bytes over time.
pub fn finish(list: &mut [Status]) {
    let mut g = SPEED.lock().unwrap();
    let m = g.get_or_insert_with(HashMap::new);
    for s in list.iter_mut() {
        if s.game_id.is_empty() {
            s.game_id = game_of(&s.id);
        }
        if s.store.is_empty() {
            s.store = s.game_id.split(':').next().unwrap_or("").to_string();
        }
        if s.state != "downloading" {
            m.remove(&s.id);
            continue;
        }
        let Some(now_b) = s.downloaded else { continue };
        let now = Instant::now();
        let e = m.entry(s.id.clone()).or_insert((now, now_b, 0.0));
        let dt = now.duration_since(e.0).as_secs_f64();
        if dt >= 1.0 {
            let rate = now_b.saturating_sub(e.1) as f64 / dt;
            // Smooth it so it doesn't jump around between polls.
            e.2 = if e.2 == 0.0 { rate } else { e.2 * 0.6 + rate * 0.4 };
            *e = (now, now_b, e.2);
        }
        if s.speed.is_none() && e.2 > 0.0 {
            s.speed = Some(e.2);
        }
        if s.eta.is_none() {
            if let (Some(t), Some(v)) = (s.total, s.speed.filter(|v| *v > 1.0)) {
                s.eta = Some((t.saturating_sub(now_b) as f64 / v) as u64);
            }
        }
    }
}

#[derive(Serialize, serde::Deserialize, Clone, Debug)]
pub struct Past {
    pub id: String,
    pub game_id: String,
    pub store: String,
    pub title: String,
    pub kind: String,
    /// done or failed
    pub state: String,
    pub detail: String,
    /// Unix time it finished.
    pub finished: u64,
    /// How long it took.
    pub seconds: u64,
    pub total: Option<u64>,
}

fn history_file() -> std::path::PathBuf {
    crate::state::data_dir().join("downloads_history.json")
}

pub fn history() -> Vec<Past> {
    std::fs::read_to_string(history_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn clear_history() {
    let _ = std::fs::remove_file(history_file());
}

/// Save finished jobs to the history once each (newest first, last 200).
pub fn record(list: &[Status]) {
    static DONE: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);
    let now = crate::state::now();
    let mut g = DONE.lock().unwrap();
    let seen = g.get_or_insert_with(HashMap::new);
    let mut new = Vec::new();
    for s in list.iter().filter(|s| s.state == "done" || s.state == "failed") {
        let started = now.saturating_sub(s.seconds);
        // The same job is reported on every poll until it's dismissed.
        if seen.get(&s.id).is_some_and(|t| t.abs_diff(started) < 120) {
            continue;
        }
        seen.insert(s.id.clone(), started);
        new.push(Past {
            id: s.id.clone(),
            game_id: s.game_id.clone(),
            store: s.store.clone(),
            title: s.title.clone(),
            kind: s.kind.to_string(),
            state: s.state.to_string(),
            detail: s.detail.clone(),
            finished: now,
            seconds: s.seconds,
            total: s.total.or(s.downloaded),
        });
    }
    if new.is_empty() {
        return;
    }
    let mut h = history();
    // A job seen again after Lodestone restarted (Steam's own updates) is already in the file.
    new.retain(|n| !h.iter().any(|p| p.id == n.id && p.finished.abs_diff(n.finished) < n.seconds + 120));
    new.extend(h.drain(..));
    new.truncate(200);
    let _ = std::fs::create_dir_all(crate::state::data_dir());
    let _ = std::fs::write(history_file(), serde_json::to_string(&new).unwrap_or_default());
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_progress_from_both_engines() {
        let dir = std::env::temp_dir().join(format!("lodestone-dl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("log");
        // legendary
        std::fs::write(&f, "[DLManager] INFO: = Progress: 12.50% (100/800), Running for 00:00:10, ETA: 00:01:10\n[DLManager] INFO:  - Downloaded: 1.00 GiB\n").unwrap();
        assert_eq!(super::progress(&f).0, 12.5);
        // gogdl
        std::fs::write(&f, "[PROGRESS] INFO: = Progress: 3.10 1000/32000, Running for: 00:00:02, ETA: 00:01:00\n[PROGRESS] INFO: = Progress: 47.25 15000/32000, Running for: 00:00:20, ETA: 00:00:22\n").unwrap();
        assert_eq!(super::progress(&f).0, 47.25);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn reads_sizes_speed_and_eta() {
        let legendary = "[Core] INFO: Install path: /home/me/Games/Epic/TwentyXX\n[cli] INFO: Install size: 1586.50 MiB\n[cli] INFO: Download size: 674.89 MiB (Compression savings: 57.5%)\n\
[DLManager] INFO: = Progress: 0.07% (4/5342), Running for 00:00:01, ETA: 00:22:17\n[DLManager] INFO:  - Downloaded: 17.98 MiB, Written: 4.00 MiB\n\
[DLManager] INFO:  + Download\t- 17.97 MiB/s (raw) / 46.98 MiB/s (decompressed)\n[DLManager] INFO:  + Disk\t- 2.00 MiB/s (write) / 0.00 MiB/s (read)\n";
        let s = super::stats_of(legendary);
        assert_eq!(s.path.as_deref(), Some("/home/me/Games/Epic/TwentyXX"));
        assert_eq!(s.total, Some((674.89 * 1048576.0) as u64));
        assert_eq!(s.downloaded, Some((17.98 * 1048576.0) as u64));
        assert_eq!(s.eta, Some(22 * 60 + 17));
        assert_eq!(s.speed, Some((17.97 * 1048576.0) as u64 as f64));
        assert_eq!(s.disk, Some(2.0 * 1048576.0));
        assert_eq!(s.install_size, Some((1586.5 * 1048576.0) as u64));

        let gogdl = "[PROGRESS] INFO: = Progress: 0.40 90111001/22258050474, Running for: 00:00:01, ETA: 00:04:21\n[PROGRESS] INFO: = Downloaded: 43.33 MiB, Written: 85.94 MiB\n[PROGRESS] INFO:  + Download\t- 42.96 MiB/s (raw) / 167.42 MiB/s (decompressed)\n";
        let g = super::stats_of(gogdl);
        assert_eq!((g.downloaded, g.total, g.eta), (Some(90111001), Some(22258050474), Some(261)));
        assert!(g.speed.unwrap() > 40.0 * 1048576.0);
        assert_eq!(super::game_of("update-epic:Quail"), "epic:Quail");
    }
}
