//! Blizzard game installs without Battle.net's window, through Battle.net's own download service (the
//! Agent, a local web API that Battle.net drives). Battle.net is started in its tray mode (the way
//! Windows starts it at login), the Agent is asked to install the game, and its progress shows in
//! Lodestone's downloads panel.
//!
//! The Agent answers only to its session token. It writes that token into its own log in the
//! Battle.net prefix each time Battle.net asks for it; Lodestone reads it from there and only ever
//! sends it to the Agent on this PC.
//!
//! The install requests follow https://wowdev.wiki/Agent and the requests Battle.net itself logs.

use super::winapps::{self, WinApp};
use crate::downloads::Status;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const BUSY: &str = "bnet-agent-unavailable";

/// Battle.net's internal ids ("uid") for its games, by launch code. Others use their product code.
const UIDS: &[(&str, &str)] = &[
    ("S1", "s1"),
    ("S2", "s2"),
    ("WOW", "wow"),
    ("WOWF", "wowf"),
    ("WOW_CLASSIC", "wow_classic"),
    ("WOW_CLASSIC_ERA", "wow_classic_era"),
    ("WOW_CLASSIC_ANNIVERSARY", "wow_classic_anniversary"),
    ("PRO", "prometheus"),
    ("W3", "w3"),
    ("WTCG", "hs_beta"),
    ("HERO", "heroes"),
    ("D3", "diablo3"),
    ("FEN", "fenris"),
    ("OSI", "osi"),
    ("RTRO", "rtro"),
    ("VIPR", "viper"),
    ("ODIN", "odin"),
    ("LAZR", "lazarus"),
    ("ZEUS", "zeus"),
    ("FORE", "fore"),
    ("WLBY", "wlby"),
];

pub fn uid_for(code: &str, tact: &str) -> String {
    UIDS.iter().find(|(c, _)| c.eq_ignore_ascii_case(code)).map(|(_, u)| u.to_string()).unwrap_or_else(|| tact.to_ascii_lowercase())
}

struct Job {
    uid: String,
    title: String,
    started: Instant,
    last: Option<Status>,
    done: bool,
}

static JOBS: Mutex<Option<HashMap<String, Job>>> = Mutex::new(None);

fn with_jobs<R>(f: impl FnOnce(&mut HashMap<String, Job>) -> R) -> R {
    f(JOBS.lock().unwrap().get_or_insert_with(HashMap::new))
}

fn app() -> Option<&'static WinApp> {
    winapps::find("battlenet").ok().filter(|a| a.installed())
}

fn agent_dir(prefix: &Path) -> PathBuf {
    prefix.join("drive_c/ProgramData/Battle.net/Agent")
}

/// The newest token the Agent logged.
fn token(prefix: &Path) -> Option<String> {
    let mut logs: Vec<PathBuf> = fs::read_dir(agent_dir(prefix))
        .ok()?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("Agent."))
        .filter_map(|e| fs::read_dir(e.path().join("Logs")).ok())
        .flat_map(|rd| rd.flatten().map(|e| e.path()))
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("Agent-")))
        .collect();
    logs.sort_by_key(|p| fs::metadata(p).and_then(|m| m.modified()).ok());
    let text = String::from_utf8_lossy(&fs::read(logs.last()?).ok()?).into_owned();
    let i = text.rfind("\"authorization\"")?;
    let rest = &text[i + "\"authorization\"".len()..];
    let start = rest.find('"')? + 1;
    let len = rest[start..].find('"')?;
    Some(rest[start..start + len].to_string()).filter(|t| !t.is_empty())
}

/// Local ports the prefix's wineserver listens on (the Agent picks a port at random).
fn wine_ports(prefix: &Path) -> Vec<u16> {
    let want = prefix.to_string_lossy().trim_end_matches('/').to_string();
    let mut inodes = std::collections::HashSet::new();
    for (pfx, socks) in crate::host::wineservers() {
        if pfx.trim_end_matches('/').trim_end_matches("/pfx") == want {
            inodes.extend(socks);
        }
    }
    let tcp = fs::read_to_string("/proc/net/tcp").unwrap_or_default();
    tcp.lines()
        .skip(1)
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            // 0100007F = 127.0.0.1, state 0A = listening.
            let (addr, port) = f.get(1)?.split_once(':')?;
            (addr == "0100007F" && f.get(3) == Some(&"0A") && inodes.contains(*f.get(9)?)).then(|| u16::from_str_radix(port, 16).ok())?
        })
        .collect()
}

/// A tiny HTTP/1.1 client for the Agent on localhost. Returns (status, body).
/// Reads only as much as the reply says it has: Battle.net's own local server keeps connections open.
fn http(port: u16, token: &str, method: &str, path: &str, body: Option<&Value>) -> Result<(u16, Value), String> {
    let mut s = TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_secs(2)).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(4))).ok();
    s.set_write_timeout(Some(Duration::from_secs(2))).ok();
    let payload = body.map(|b| b.to_string()).unwrap_or_default();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUser-Agent: phoenix-agent/1.0\r\nAuthorization: {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    s.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    let mut raw: Vec<u8> = Vec::new();
    let mut buf = [0u8; 16384];
    loop {
        if reply_complete(&raw) {
            break;
        }
        match s.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }
    }
    let text = String::from_utf8_lossy(&raw);
    let (head, rest) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let code: u16 = head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let body = if head.to_ascii_lowercase().contains("transfer-encoding: chunked") { dechunk(rest) } else { rest.to_string() };
    Ok((code, serde_json::from_str(&body).unwrap_or(Value::Null)))
}

/// Whole reply received (headers plus Content-Length bytes, or the last chunk).
fn reply_complete(raw: &[u8]) -> bool {
    let Some(i) = raw.windows(4).position(|w| w == b"\r\n\r\n") else { return false };
    let head = String::from_utf8_lossy(&raw[..i]).to_ascii_lowercase();
    let body = &raw[i + 4..];
    if head.contains("transfer-encoding: chunked") {
        return body.ends_with(b"0\r\n\r\n");
    }
    match head.lines().find_map(|l| l.strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok())) {
        Some(n) => body.len() >= n,
        // No length: only the connection closing ends it.
        None => false,
    }
}

fn dechunk(rest: &str) -> String {
    let mut out = String::new();
    let mut r = rest;
    while let Some((size, tail)) = r.split_once("\r\n") {
        let n = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
        if n == 0 || tail.len() < n {
            break;
        }
        out.push_str(&tail[..n]);
        r = tail[n..].trim_start_matches("\r\n");
    }
    out
}

struct Agent {
    port: u16,
    token: String,
    region: String,
}

static PORT: Mutex<Option<u16>> = Mutex::new(None);

fn connect(prefix: &Path) -> Option<Agent> {
    let token = token(prefix)?;
    let check = |port: u16| match http(port, &token, "GET", "/agent", None) {
        Ok((200, v)) if v["pid"].is_number() => Some(Agent { port, token: token.clone(), region: v["region"].as_str().unwrap_or("us").to_string() }),
        _ => None,
    };
    // The port found last time first; look again only when it stops answering.
    let known = *PORT.lock().unwrap();
    if let Some(a) = known.and_then(check) {
        return Some(a);
    }
    let a = wine_ports(prefix).into_iter().filter(|p| Some(*p) != known).find_map(check);
    *PORT.lock().unwrap() = a.as_ref().map(|a| a.port);
    a
}

/// Start Battle.net in its tray mode if it isn't running, and wait for the Agent.
fn ensure_agent(app: &WinApp) -> Result<Agent, String> {
    let prefix = app.prefix();
    if let Some(a) = connect(&prefix) {
        return Ok(a);
    }
    winapps::bnet_start_minimized(&proton::drive_c(&prefix));
    let spec = app.run(&app.exe_path(), vec!["--autostarted".into()])?;
    crate::launcher::spawn(&spec, crate::launcher::log_path("client-battlenet")).map_err(|e| e.to_string())?;
    let until = Instant::now() + Duration::from_secs(90);
    while Instant::now() < until {
        std::thread::sleep(Duration::from_secs(3));
        if let Some(a) = connect(&prefix) {
            return Ok(a);
        }
    }
    Err(BUSY.into())
}

use crate::proton;

fn win_dir(name: &str) -> String {
    let clean: String = name.chars().filter(|c| !"<>:\"/\\|?*®™".contains(*c)).collect();
    format!("C:\\Program Files (x86)\\{}", clean.trim())
}

/// Start installing a game in the background. `code` is the launch code ("OSI"), `tact` its product.
pub fn install(code: &str, tact: &str, title: &str) -> Result<(), String> {
    let app = app().ok_or("Battle.net isn't set up")?;
    let uid = uid_for(code, tact);
    let key = format!("battlenet:{}", code.to_ascii_lowercase());
    if with_jobs(|j| j.get(&key).is_some_and(|x| !x.done)) {
        return Err(format!("{title} is already installing"));
    }
    let a = ensure_agent(app)?;
    let tact = tact.to_ascii_lowercase();
    let region = a.region.to_ascii_lowercase();
    // Two request shapes: the one current Battle.net sends for itself, and the one wowdev.wiki documents.
    let shapes = [
        (format!("https://{region}.version.battle.net/patch"), tact.clone()),
        (format!("http://{region}.patch.battle.net:1119/{tact}"), "NGDP".to_string()),
    ];
    let mut last = String::new();
    let mut ok = false;
    let mut form = Value::Null;
    for (patch_url, product) in &shapes {
        let _ = http(a.port, &a.token, "POST", "/register", Some(&serde_json::json!({ "instructions_patch_url": patch_url, "instructions_product": product, "uid": uid })));
        let body = serde_json::json!({
            "instructions_dataset": ["torrent", "win", uid, "enUS"],
            "instructions_patch_url": patch_url,
            "instructions_product": product,
            "priority": { "insert_at_head": false, "value": 900 },
            "uid": uid,
        });
        match http(a.port, &a.token, "POST", "/install", Some(&body)) {
            // 201 comes with the install form (default folder, space needed, languages).
            Ok((c, v)) if (200..300).contains(&c) => {
                form = v;
                ok = true;
                break;
            }
            Ok((c, v)) => last = format!("{c} {}", v["error"].as_str().map(String::from).unwrap_or_else(|| v.to_string())),
            Err(e) => last = e,
        }
    }
    if !ok {
        return Err(format!("Battle.net's download service refused the install ({last})"));
    }
    let start = serde_json::json!({
        "finalized": true,
        // Battle.net's own default folder for the game when it offers one.
        "game_dir": form["form"]["game_dir"]["game_dir"].as_str().map(String::from).unwrap_or_else(|| win_dir(title)),
        "language": ["enUS"],
        "selected_asset_locale": "enUS",
        "selected_locale": "enUS",
        "shortcut": "none",
        "tome_torrent": "",
    });
    match http(a.port, &a.token, "POST", &format!("/install/{uid}"), Some(&start)) {
        Ok((c, _)) if (200..300).contains(&c) => {}
        Ok((c, v)) => return Err(format!("Battle.net didn't start the download ({c} {})", v["error"].as_str().unwrap_or(""))),
        Err(e) => return Err(e),
    }
    with_jobs(|j| {
        j.insert(key, Job { uid, title: title.to_string(), started: Instant::now(), last: None, done: false });
    });
    Ok(())
}

fn status_from(key: &str, job: &Job, v: &Value) -> Status {
    let progress = v["progress"].as_f64().unwrap_or(0.0) as f32;
    let installed = v["installed"].as_bool() == Some(true) || v["playable"].as_bool() == Some(true) && progress >= 0.999;
    let rate = v["download_rate"].as_f64().unwrap_or(0.0);
    // Sizes come as [download, ...] lists in current Agents, plain numbers in older ones.
    let num = |x: &Value| x.as_f64().or_else(|| x[0].as_f64()).unwrap_or(0.0);
    let (cur, total) = (num(&v["download_current"]), num(&v["download_total"]));
    let mut detail = String::new();
    if total > 0.0 {
        detail = format!("{:.1} of {:.1} GB", cur / 1e9, total / 1e9);
    }
    if rate > 0.0 {
        detail.push_str(&format!("{}{:.1} MB/s", if detail.is_empty() { "" } else { " · " }, rate / 1e6));
    }
    if v["paused"].as_bool() == Some(true) {
        detail = format!("Paused in Battle.net · {detail}");
    }
    Status {
        id: key.to_string(),
        title: job.title.clone(),
        state: if installed { "done" } else { "downloading" },
        percent: (progress * 100.0).clamp(0.0, 100.0),
        detail: if installed { "Installed".into() } else if detail.is_empty() { "Starting…".into() } else { detail },
        seconds: job.started.elapsed().as_secs(),
        kind: "install",
    }
}

/// Progress of the installs Lodestone started.
pub fn statuses() -> Vec<Status> {
    if with_jobs(|j| j.is_empty()) {
        return vec![];
    }
    let Some(app) = app() else { return vec![] };
    let agent = connect(&app.prefix());
    with_jobs(|jobs| {
        let mut out = Vec::new();
        for (key, job) in jobs.iter_mut() {
            if !job.done {
                if let Some(a) = &agent {
                    if let Ok((200, v)) = http(a.port, &a.token, "GET", &format!("/install/{}", job.uid), None) {
                        let st = status_from(key, job, &v);
                        job.done = st.state == "done";
                        job.last = Some(st);
                    } else if job.last.as_ref().is_some_and(|l| l.percent > 99.0) {
                        // The Agent drops the install record once it's finished.
                        let mut l = job.last.clone().unwrap();
                        l.state = "done";
                        l.detail = "Installed".into();
                        job.done = true;
                        job.last = Some(l);
                    }
                }
            }
            out.push(job.last.clone().unwrap_or(Status {
                id: key.clone(),
                title: job.title.clone(),
                state: "downloading",
                percent: 0.0,
                detail: if agent.is_some() { "Starting…".into() } else { "Waiting for Battle.net…".into() },
                seconds: job.started.elapsed().as_secs(),
                kind: "install",
            }));
        }
        out
    })
}

pub fn is_job(id: &str) -> bool {
    with_jobs(|j| j.contains_key(id))
}

/// Ask the Agent to remove a game (also a half-done install). Returns which request worked.
pub fn uninstall_uid(uid: &str) -> Result<String, String> {
    let a = app().and_then(|app| connect(&app.prefix())).ok_or("Battle.net's download service isn't running")?;
    let body = serde_json::json!({ "uid": uid, "run_compaction": false });
    let tries: [(&str, String, Option<&Value>); 3] = [
        ("POST", "/uninstall".into(), Some(&body)),
        ("DELETE", format!("/install/{uid}"), None),
        ("DELETE", format!("/uninstall/{uid}"), None),
    ];
    let mut seen = Vec::new();
    for (m, path, b) in tries {
        match http(a.port, &a.token, m, &path, b) {
            Ok((c, _)) if (200..300).contains(&c) => return Ok(format!("{m} {path}")),
            Ok((c, _)) => seen.push(format!("{m} {path} {c}")),
            Err(e) => seen.push(e),
        }
    }
    Err(format!("Battle.net didn't remove it ({})", seen.join(", ")))
}

/// Cancel: remove the half-done install.
pub fn cancel(id: &str) -> Result<(), String> {
    let uid = with_jobs(|j| j.get(id).map(|x| x.uid.clone())).ok_or("not a Battle.net install")?;
    let r = uninstall_uid(&uid);
    with_jobs(|j| j.remove(id));
    r.map(|_| ())
}

pub fn dismiss(id: &str) {
    with_jobs(|j| {
        if j.get(id).is_some_and(|x| x.done) {
            j.remove(id);
        }
    });
}

// ---------------- Play without Battle.net's window ----------------

pub const PLAY_FLAG: &str = "--bnet-play";

/// Play goes through `lodestone --bnet-play <umu-run> <Battle.net.exe> <--exec=launch CODE>`
/// (with the prefix's environment), which runs `play_hook` below.
pub fn play_spec(spec: crate::model::LaunchSpec, client: &Path, exec: &str) -> crate::model::LaunchSpec {
    let Some((program, mut args)) = crate::host::self_command() else { return spec };
    args.extend([PLAY_FLAG.into(), spec.program.clone(), client.to_string_lossy().into_owned(), exec.into()]);
    let mut env = spec.env;
    let keys: Vec<String> = env.keys().cloned().collect();
    env.insert(crate::host::FWD_ENV.into(), keys.join(","));
    crate::model::LaunchSpec { program, args, env, cwd: spec.cwd }
}

fn bnet_running() -> bool {
    crate::host::procs().iter().any(|p| {
        let cmd = String::from_utf8_lossy(&p.cmdline);
        cmd.contains("Battle.net\\Battle.net.exe") && !cmd.contains("--type=") && !cmd.contains("umu") && !cmd.contains("--exec")
    })
}

/// Battle.net's UI log has a line once it has started and signed in.
fn bnet_ready_since(prefix: &Path, since: std::time::SystemTime) -> bool {
    let dir = prefix.join("drive_c/users/steamuser/AppData/Local/Battle.net/Logs");
    fs::read_dir(dir).into_iter().flatten().flatten().any(|e| {
        let n = e.file_name().to_string_lossy().into_owned();
        n.starts_with("battle.net-")
            && fs::metadata(e.path()).and_then(|m| m.modified()).is_ok_and(|m| m >= since)
            && fs::read(e.path()).is_ok_and(|b| String::from_utf8_lossy(&b).contains("Attempting to show main window"))
    })
}

pub fn play_hook() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) != Some(PLAY_FLAG) {
        return;
    }
    let (Some(umu), Some(client), Some(exec)) = (args.get(2), args.get(3), args.get(4)) else { std::process::exit(2) };
    let prefix = std::env::var_os("WINEPREFIX").map(PathBuf::from).unwrap_or_default();
    let drive_c = prefix.join("drive_c");
    winapps::bnet_quiet_launches(&drive_c);
    let run = |extra: &[&str]| {
        let mut c = std::process::Command::new(umu);
        c.arg(client).args(extra);
        crate::host::wrap(c).stdin(std::process::Stdio::null()).spawn()
    };
    if !bnet_running() {
        // Battle.net ignores a launch command given with its tray start, so start it first and wait.
        let since = std::time::SystemTime::now();
        if let Err(e) = run(&["--autostarted"]) {
            eprintln!("couldn't start Battle.net: {e}");
            std::process::exit(1);
        }
        let until = Instant::now() + Duration::from_secs(120);
        while Instant::now() < until && !bnet_ready_since(&prefix, since) {
            std::thread::sleep(Duration::from_secs(1));
        }
        std::thread::sleep(Duration::from_secs(2));
        println!("Battle.net is running in the tray");
    }
    // Handed to the running Battle.net, which starts the game and then closes (GameLaunchWindowBehavior).
    match run(&[exec.as_str()]).and_then(|mut c| c.wait()) {
        Ok(_) => println!("asked Battle.net to start the game"),
        Err(e) => eprintln!("couldn't reach Battle.net: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    /// `LODESTONE_BNET_CODE=S1 cargo test live_agent_install -- --ignored --nocapture`: starts an install
    /// through the Agent, watches it for 30 s, then cancels it.
    #[test]
    #[ignore]
    fn live_agent_connect() {
        let app = super::app().unwrap();
        for _ in 0..3 {
            let t = std::time::Instant::now();
            let a = super::connect(&app.prefix());
            println!("found={} in {:?}", a.is_some(), t.elapsed());
        }
    }

    /// `LODESTONE_BNET_UID=s1 cargo test live_agent_uninstall -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_agent_uninstall() {
        let uid = std::env::var("LODESTONE_BNET_UID").unwrap();
        println!("uninstall: {:?}", super::uninstall_uid(&uid));
    }

    #[test]
    #[ignore]
    fn live_agent_install() {
        let code = std::env::var("LODESTONE_BNET_CODE").unwrap_or_else(|_| "S1".into());
        let (code, tact) = crate::stores::winapps::bnet_product(&code).expect("game in Battle.net's catalog");
        println!("code {code} tact {tact} uid {}", super::uid_for(&code, &tact));
        let r = super::install(&code, &tact, "Lodestone test install");
        println!("install: {r:?}");
        if r.is_ok() {
            for _ in 0..6 {
                std::thread::sleep(std::time::Duration::from_secs(5));
                for s in super::statuses() {
                    println!("  {} {:.1}% {}", s.state, s.percent, s.detail);
                }
            }
            println!("cancel: {:?}", super::cancel(&format!("battlenet:{}", code.to_ascii_lowercase())));
        }
    }

    #[test]
    fn knows_when_a_reply_is_complete() {
        assert!(super::reply_complete(b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\n\r\nNot Found"));
        assert!(!super::reply_complete(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nNot"));
        assert!(super::reply_complete(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n\r\n"));
        assert_eq!(super::dechunk("2\r\n{}\r\n0\r\n\r\n"), "{}");
    }

    #[test]
    fn maps_codes_and_reads_progress() {
        assert_eq!(super::uid_for("Pro", "pro"), "prometheus");
        assert_eq!(super::uid_for("WTCG", "hsb"), "hs_beta");
        assert_eq!(super::uid_for("GRY", "gry"), "gry");
        assert_eq!(super::win_dir("Diablo® II: Resurrected"), "C:\\Program Files (x86)\\Diablo II Resurrected");
        let job = super::Job { uid: "osi".into(), title: "D2R".into(), started: std::time::Instant::now(), last: None, done: false };
        let s = super::status_from("battlenet:osi", &job, &json!({"progress": 0.5, "download_current": 5e9, "download_total": 1e10, "download_rate": 2e7}));
        assert_eq!((s.state, s.percent as u32, s.detail.as_str()), ("downloading", 50, "5.0 of 10.0 GB · 20.0 MB/s"));
        let s = super::status_from("battlenet:osi", &job, &json!({"progress": 1.0, "installed": true}));
        assert_eq!(s.state, "done");
    }
}
