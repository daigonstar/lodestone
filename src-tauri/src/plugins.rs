//! Plug-ins: separate programs that show their interface inside Lodestone (see beacon/PLUGIN.md).
//! Lodestone Beacon is the only one so far.
//!
//! A plug-in runs as its own program (normally its own Flatpak, with its own sandbox and data). Lodestone
//! talks to it only over its standard input and output, and answers only the requests you approved.
//! It never hands a plug-in sign-in data, keys, cookies, file paths, or the ability to run programs.

use crate::{host, settings};
use serde::Serialize;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

pub const BEACON: &str = "beacon";
const BEACON_FLATPAK: &str = "io.github.daigonstar.Lodestone.Beacon";

/// What each permission means, in the words Lodestone shows when asking.
pub fn describe(p: &str) -> Option<&'static str> {
    match p {
        "friends:read" => Some("See your Steam and Epic friends list: names, avatars and whether they're online"),
        "chat:open" => Some("Open a friend's Steam chat with an invite ready to paste (it never sends anything itself)"),
        "game:read" => Some("See the name of the game you're playing from Lodestone, to show your friends in Beacon (end-to-end encrypted)"),
        _ => None,
    }
}

/// The plug-in's input pipe; taking it out (and dropping it) closes the pipe, which tells the plug-in to quit.
type Input = Arc<Mutex<Option<ChildStdin>>>;

struct Running {
    child: Child,
    stdin: Input,
    hello: Option<Value>,
}

static BEACON_PROC: Mutex<Option<Running>> = Mutex::new(None);

#[derive(Serialize)]
pub struct Status {
    pub installed: bool,
    pub running: bool,
    pub ui: Option<String>,
    pub version: Option<String>,
    /// Permissions the plug-in asks for, each with its plain-words meaning.
    pub requested: Vec<(String, String)>,
    pub needs_approval: bool,
}

/// How to start Beacon: its Flatpak, or a `lodestone-beacon` program on the PATH. On Windows, the Beacon
/// Lodestone installed (beacon_install).
fn beacon_command() -> Option<Command> {
    #[cfg(windows)]
    {
        let exe = crate::beacon_install::windows_exe()?;
        let mut c = Command::new(exe);
        c.arg("--lodestone-bridge");
        return Some(c);
    }
    #[allow(unreachable_code)]
    let mut info = Command::new("flatpak");
    info.args(["info", BEACON_FLATPAK]);
    let has_flatpak = host::wrap(info).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
    if has_flatpak {
        let mut c = Command::new("flatpak");
        c.args(["run", BEACON_FLATPAK, "--lodestone-bridge"]);
        return Some(host::wrap(c));
    }
    let bin = host::which("lodestone-beacon")?;
    let mut c = Command::new(bin);
    c.arg("--lodestone-bridge");
    Some(host::wrap(c))
}

fn approved() -> Vec<String> {
    settings::load().plugin_permissions.get(BEACON).cloned().unwrap_or_default()
}

pub fn status() -> Status {
    let mut guard = BEACON_PROC.lock().unwrap();
    // Forget a plug-in that has exited.
    if guard.as_mut().is_some_and(|r| r.child.try_wait().ok().flatten().is_some()) {
        *guard = None;
    }
    let hello = guard.as_ref().and_then(|r| r.hello.clone());
    let running = guard.is_some();
    drop(guard);
    let requested: Vec<String> = hello.as_ref().and_then(|h| h["permissions"].as_array().cloned()).unwrap_or_default().into_iter().filter_map(|p| p.as_str().map(String::from)).collect();
    let ok = approved();
    Status {
        installed: running || beacon_command().is_some(),
        running,
        ui: hello.as_ref().and_then(|h| h["ui"].as_str()).filter(|u| u.starts_with("http://127.0.0.1:")).map(String::from),
        version: hello.as_ref().and_then(|h| h["version"].as_str()).map(String::from),
        needs_approval: requested.iter().any(|p| !ok.contains(p)),
        requested: requested.iter().map(|p| (p.clone(), describe(p).unwrap_or("Unknown permission (Lodestone won't grant it)").to_string())).collect(),
    }
}

/// Start Beacon if it isn't running and wait (up to 15 s) for its hello.
pub fn start(app: AppHandle) -> Result<Status, String> {
    {
        let guard = BEACON_PROC.lock().unwrap();
        if guard.is_some() {
            drop(guard);
            return Ok(status());
        }
    }
    let mut cmd = beacon_command().ok_or("Lodestone Beacon isn't installed")?;
    let log = crate::launcher::log_path("lodestone-beacon");
    let err = std::fs::File::create(&log).map(Stdio::from).unwrap_or(Stdio::null());
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(err).spawn().map_err(|e| format!("Couldn't start Lodestone Beacon: {e}"))?;
    let stdin: Input = Arc::new(Mutex::new(Some(child.stdin.take().ok_or("no stdin")?)));
    let stdout = child.stdout.take().ok_or("no stdout")?;
    *BEACON_PROC.lock().unwrap() = Some(Running { child, stdin: stdin.clone(), hello: None });
    std::thread::spawn(move || read_loop(app, stdout, stdin));
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(15) {
        let s = status();
        if s.ui.is_some() || !s.running {
            return Ok(s);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err("Lodestone Beacon didn't start in time".into())
}

pub fn approve(perms: Vec<String>) -> Result<Status, String> {
    let mut s = settings::load();
    let list: Vec<String> = perms.into_iter().filter(|p| describe(p).is_some()).collect();
    s.plugin_permissions.insert(BEACON.into(), list);
    settings::save(&s)?;
    Ok(status())
}

/// Send Lodestone's colours to the plug-in so it looks native.
pub fn send_theme(vars: Value) {
    let stdin = BEACON_PROC.lock().unwrap().as_ref().map(|r| r.stdin.clone());
    if let Some(s) = stdin {
        write_line(&s, &json!({ "type": "event", "name": "theme", "data": vars }));
    }
}

fn write_line(stdin: &Mutex<Option<ChildStdin>>, v: &Value) {
    if let Some(s) = stdin.lock().unwrap().as_mut() {
        let _ = writeln!(s, "{v}");
        let _ = s.flush();
    }
}

fn read_loop(app: AppHandle, stdout: std::process::ChildStdout, stdin: Input) {
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        match msg["type"].as_str() {
            Some("hello") => {
                if let Some(r) = BEACON_PROC.lock().unwrap().as_mut() {
                    r.hello = Some(msg);
                }
                let _ = app.emit("beacon-ready", ());
            }
            Some("request") => {
                let id = msg["id"].clone();
                let reply = match handle(&app, msg["method"].as_str().unwrap_or_default(), &msg["params"]) {
                    Ok(v) => json!({ "type": "response", "id": id, "ok": v }),
                    Err(e) => json!({ "type": "response", "id": id, "error": e }),
                };
                write_line(&stdin, &reply);
            }
            _ => {}
        }
    }
    *BEACON_PROC.lock().unwrap() = None;
    let _ = app.emit("beacon-stopped", ());
}

/// Answer one bridge request, only if you approved its permission.
fn handle(app: &AppHandle, method: &str, params: &Value) -> Result<Value, String> {
    let needed = match method {
        "friends" => "friends:read",
        "open_chat" => "chat:open",
        "playing" => "game:read",
        _ => return Err("unknown method".into()),
    };
    if !approved().iter().any(|p| p == needed) {
        return Err("not allowed (approve it in Lodestone)".into());
    }
    match method {
        "friends" => {
            let (list, _) = tauri::async_runtime::block_on(crate::friends::list(false));
            Ok(list.iter().map(|f| json!({ "store": f.store, "id": f.id, "name": f.name, "avatar": f.avatar, "status": f.status })).collect())
        }
        // The game you're playing from Lodestone (its title only), or null.
        "playing" => {
            use tauri::Manager;
            let inner = app.state::<crate::Shared>();
            let now = crate::running_now(&inner);
            Ok(now.iter().find(|r| r.state == "running").map(|r| json!({ "title": r.title })).unwrap_or(Value::Null))
        }
        "open_chat" => {
            let store = params["store"].as_str().unwrap_or_default();
            let id = params["id"].as_str().unwrap_or_default();
            let text = params["text"].as_str().unwrap_or_default();
            if text.is_empty() || text.len() > 500 || !matches!(store, "steam" | "epic") {
                return Err("bad request".into());
            }
            // Steam IDs are digits; anything else would let a plug-in steer the steam:// link.
            if store == "steam" && !(id.len() <= 20 && !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())) {
                return Err("bad friend ID".into());
            }
            // Lodestone's window copies the text and says so; for Steam, Lodestone opens the chat itself
            // (a fixed steam:// link with the checked ID).
            let _ = app.emit("beacon-open-chat", json!({ "store": store, "id": id, "text": text }));
            if store == "steam" {
                let url = format!("steam://friends/message/{id}");
                let _ = crate::launcher::spawn(&crate::os::open_spec(&url), crate::launcher::log_path("xdg-open"));
            }
            Ok(json!(true))
        }
        _ => unreachable!(),
    }
}

/// A plug-in Lodestone knows about, for the Plug-ins page.
#[derive(Serialize)]
pub struct PluginInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub installed: bool,
    pub running: bool,
    pub version: Option<String>,
    /// Every permission this plug-in can ask for: (permission, plain words, granted).
    pub permissions: Vec<(String, String, bool)>,
    pub install_hint: &'static str,
}

/// The plug-ins Lodestone knows (only Beacon so far).
pub fn list() -> Vec<PluginInfo> {
    let s = status();
    let granted = approved();
    let mut perms: Vec<String> = s.requested.iter().map(|(p, _)| p.clone()).collect();
    for p in ["friends:read", "chat:open", "game:read"] {
        if !perms.iter().any(|x| x == p) {
            perms.push(p.to_string());
        }
    }
    vec![PluginInfo {
        id: BEACON,
        name: "Lodestone Beacon",
        description: "End-to-end encrypted chat with your friends, on servers you or your friends run yourselves.",
        installed: s.installed,
        running: s.running,
        version: s.version,
        permissions: perms.iter().map(|p| (p.clone(), describe(p).unwrap_or("Unknown permission").to_string(), granted.contains(p))).collect(),
        install_hint: "Beacon isn't installed. Lodestone can install it from its official releases on GitHub (it's in early development).",
    }]
}

/// Stop a running plug-in (it starts again when you open it).
pub fn stop_plugin(id: &str) -> Result<(), String> {
    if id != BEACON {
        return Err("Unknown plug-in".into());
    }
    stop();
    Ok(())
}

/// Stop Beacon (when you ask, or when Lodestone quits): closing its input makes it exit by itself; it's
/// killed only if it hasn't after two seconds.
pub fn stop() {
    let running = BEACON_PROC.lock().unwrap().take();
    if let Some(mut r) = running {
        drop(r.stdin.lock().unwrap().take());
        let _ = r.child.wait_timeout_or_kill();
    }
}

trait WaitOrKill {
    fn wait_timeout_or_kill(&mut self) -> std::io::Result<()>;
}

impl WaitOrKill for Child {
    fn wait_timeout_or_kill(&mut self) -> std::io::Result<()> {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(2) {
            if self.try_wait()?.is_some() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        self.kill()
    }
}
