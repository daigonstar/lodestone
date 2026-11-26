//! Linux: a screen share's sound, app by app, so Beacon's own sound (your friends' voices and the
//! join/leave chimes) is never part of it.
//!
//! The sound server (PulseAudio/PipeWire) is asked which apps are playing (`pactl`), each wanted one is
//! recorded on its own (`parec --monitor-stream`), and the recordings are mixed here. Nothing on your
//! system is changed: no virtual devices, no rerouting. Nothing is kept either.
//!
//! Left out: Beacon itself, and streams with no app behind them (equalisers and other filters, which
//! replay other apps' sound, Beacon's included).

use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// One 20 ms frame of 48 kHz stereo, in samples.
const FRAME: usize = 960 * 2;
/// An app's recording that falls this far behind is cut back to `KEEP`, so delay can't build up.
const MAX_QUEUED: usize = FRAME * 6;
const KEEP: usize = FRAME * 2;
/// How often to look for apps that started or stopped playing.
const RESCAN: Duration = Duration::from_millis(1000);

/// An app that's playing sound and can be shared on its own.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct PlayingApp {
    /// What the app calls itself; the saved choice.
    pub name: String,
}

struct Stream {
    index: u64,
    app: String,
}

/// The playing streams that belong to apps, Beacon's left out.
fn parse(json: &str) -> Vec<Stream> {
    let Ok(list) = serde_json::from_str::<serde_json::Value>(json) else { return Vec::new() };
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            let p = &s["properties"];
            let name = p["application.name"].as_str().unwrap_or_default();
            let binary = p["application.process.binary"].as_str().unwrap_or_default();
            let ours = |v: &str| v.starts_with("lodestone-beacon");
            let id = p["application.id"].as_str().unwrap_or_default();
            if ours(name) || ours(binary) || id.ends_with(".Lodestone.Beacon") {
                return None;
            }
            let app = if name.is_empty() { binary } else { name };
            (!app.is_empty()).then(|| Stream { index: s["index"].as_u64().unwrap_or_default(), app: app.to_string() })
        })
        .collect()
}

fn streams() -> Result<Vec<Stream>, String> {
    let out = Command::new("pactl")
        .args(["-f", "json", "list", "sink-inputs"])
        // Some locales would write numbers with commas, which isn't JSON.
        .env("LC_NUMERIC", "C")
        .stdin(Stdio::null())
        .output()
        .map_err(|_| "Sharing sound needs pactl and parec (the PulseAudio tools)".to_string())?;
    if !out.status.success() {
        return Err("The sound server didn't answer".into());
    }
    Ok(parse(&String::from_utf8_lossy(&out.stdout)))
}

/// Apps playing sound right now, by name.
pub fn apps() -> Vec<PlayingApp> {
    let mut names: Vec<String> = streams().unwrap_or_default().into_iter().map(|s| s.app).collect();
    names.sort();
    names.dedup();
    names.into_iter().map(|name| PlayingApp { name }).collect()
}

type Queue = Arc<Mutex<VecDeque<i16>>>;

struct Recording {
    child: Child,
    queue: Queue,
}

impl Drop for Recording {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn record(index: u64) -> Option<Recording> {
    let mut child = Command::new("parec")
        .arg(format!("--monitor-stream={index}"))
        .args(["--format=s16le", "--rate=48000", "--channels=2", "--latency-msec=20", "--client-name=lodestone-beacon-share"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut pipe = child.stdout.take()?;
    let queue: Queue = Default::default();
    let q = queue.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 3840];
        let mut half: Vec<u8> = Vec::new();
        // Ends when parec does: its app stopped, or the recording was dropped.
        while let Ok(n @ 1..) = pipe.read(&mut buf) {
            half.extend_from_slice(&buf[..n]);
            // Whole stereo pairs only, so left and right never swap.
            let whole = half.len() / 4 * 4;
            let mut q = q.lock().unwrap();
            q.extend(half[..whole].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])));
            half.drain(..whole);
            if q.len() > MAX_QUEUED {
                let extra = q.len() - KEEP;
                q.drain(..extra);
            }
        }
    });
    Some(Recording { child, queue })
}

/// Add one frame from every queue that has one (a queue still filling is silent for now).
fn mix(queues: &[Queue]) -> Vec<u8> {
    let mut sum = [0i32; FRAME];
    for q in queues {
        let mut q = q.lock().unwrap();
        if q.len() >= FRAME {
            for (s, v) in sum.iter_mut().zip(q.drain(..FRAME)) {
                *s += v as i32;
            }
        }
    }
    sum.iter().flat_map(|&s| (s.clamp(i16::MIN as i32, i16::MAX as i32) as i16).to_le_bytes()).collect()
}

/// The running capture: gives a mixed 20 ms frame (S16LE stereo) 50 times a second until dropped.
pub struct AppCapture {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl AppCapture {
    /// `only`: just that app's sound (picked up whenever it plays); `None`: every app but Beacon.
    pub fn start(only: Option<String>, out: impl Fn(Vec<u8>) + Send + 'static) -> Result<Self, String> {
        // Fail now, with a reason, if the sound server can't be asked.
        streams()?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let thread = std::thread::Builder::new()
            .name("share-sound".into())
            .spawn(move || {
                let mut recordings: HashMap<u64, Recording> = HashMap::new();
                let mut scanned: Option<Instant> = None;
                let mut next = Instant::now();
                while !stopped.load(Ordering::Relaxed) {
                    if scanned.is_none_or(|t| t.elapsed() >= RESCAN) {
                        scanned = Some(Instant::now());
                        if let Ok(now) = streams() {
                            let wanted: Vec<u64> = now.into_iter().filter(|s| only.as_ref().is_none_or(|o| *o == s.app)).map(|s| s.index).collect();
                            recordings.retain(|i, r| wanted.contains(i) && matches!(r.child.try_wait(), Ok(None)));
                            for i in wanted {
                                if !recordings.contains_key(&i) {
                                    if let Some(r) = record(i) {
                                        recordings.insert(i, r);
                                    }
                                }
                            }
                        }
                    }
                    let queues: Vec<Queue> = recordings.values().map(|r| r.queue.clone()).collect();
                    out(mix(&queues));
                    next += Duration::from_millis(20);
                    match next.checked_duration_since(Instant::now()) {
                        Some(wait) => std::thread::sleep(wait),
                        // Fallen behind (a slow rescan, a suspend): carry on from now.
                        None if next.elapsed() > Duration::from_millis(200) => next = Instant::now(),
                        None => {}
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { stop, thread: Some(thread) })
    }
}

impl Drop for AppCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaves_out_beacon_and_filters() {
        let json = r#"[
          {"index":1,"properties":{"application.name":"Firefox","application.process.binary":"firefox"}},
          {"index":2,"properties":{"application.name":"lodestone-beacon","application.process.binary":"lodestone-beacon"}},
          {"index":3,"properties":{"media.name":"Equaliser: Headset"}},
          {"index":4,"properties":{"application.process.binary":"game"}},
          {"index":5,"properties":{"application.name":"Other name","application.id":"io.github.daigonstar.Lodestone.Beacon"}}
        ]"#;
        let apps: Vec<(u64, String)> = parse(json).into_iter().map(|s| (s.index, s.app)).collect();
        assert_eq!(apps, [(1, "Firefox".to_string()), (4, "game".to_string())]);
    }

    #[test]
    fn mixes_without_wrapping_round() {
        let queue = |v: i16, n: usize| -> Queue { Arc::new(Mutex::new(std::iter::repeat_n(v, n).collect())) };
        let (a, b, short) = (queue(30_000, FRAME), queue(10_000, FRAME), queue(5, FRAME - 1));
        let out = mix(&[a, b, short.clone()]);
        assert_eq!(out.len(), FRAME * 2);
        assert_eq!(i16::from_le_bytes([out[0], out[1]]), i16::MAX);
        assert_eq!(short.lock().unwrap().len(), FRAME - 1);
    }
}
