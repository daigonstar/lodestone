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

#[derive(Serialize, Clone)]
pub struct Status {
    pub id: String,
    pub title: String,
    /// downloading, done, failed
    pub state: &'static str,
    pub percent: f32,
    pub detail: String,
    pub seconds: u64,
    /// install, update, uninstall
    pub kind: &'static str,
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

/// Last "Progress: NN.NN" in the log (both legendary and gogdl print that).
fn progress(log: &std::path::Path) -> (f32, String) {
    let Ok(bytes) = std::fs::read(log) else { return (0.0, String::new()) };
    let tail = String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(16_000)..]).into_owned();
    let mut pct = 0.0;
    for line in tail.lines() {
        if let Some(rest) = line.split("Progress:").nth(1) {
            let num: String = rest.trim().chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
            if let Ok(p) = num.parse::<f32>() {
                pct = p;
            }
        }
    }
    let last = tail.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").to_string();
    (pct, last)
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
        let (pct, last) = progress(&j.log);
        let (state, percent, detail) = match &j.result {
            None => ("downloading", pct, last),
            Some(Ok(())) => ("done", 100.0, String::new()),
            Some(Err(e)) => ("failed", pct, e.clone()),
        };
        let kind = if id.starts_with("update-") { "update" } else { "install" };
        out.push(Status { id: id.clone(), title: j.title.clone(), state, percent, detail, seconds: j.started.elapsed().as_secs(), kind });
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
}
