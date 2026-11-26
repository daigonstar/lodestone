//! Lodestone Beacon on Android (first version): your identity on your phone, and approving new PCs by scanning the
//! QR code they show. The screens are plain HTML in `../ui`; this file is everything they can ask the phone to do.
//!
//! Nothing here talks to anyone except the Beacon servers you add (the same calls the desktop app makes), and the
//! identity never leaves the phone except sealed inside an approval you confirmed (see `approve`).

use lodestone_beacon_client::identity::{fingerprint_of, Identity};
use lodestone_beacon_client::{discover, server, Client};
use serde_json::{json, Value};
use std::path::PathBuf;
use tauri::Manager;

type R = Result<Value, String>;

struct Data(PathBuf);

impl Data {
    fn identity_file(&self) -> PathBuf {
        self.0.join("identity.json")
    }
    fn servers_file(&self) -> PathBuf {
        self.0.join("servers.json")
    }
    /// One data folder per server, named after its address.
    fn server_dir(&self, url: &str) -> PathBuf {
        let name: String = url.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
        self.0.join("servers").join(name)
    }
    fn identity(&self) -> Option<(Identity, String)> {
        Identity::load(&self.identity_file())
    }
    fn servers(&self) -> Vec<String> {
        std::fs::read_to_string(self.servers_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }
    fn remember_server(&self, url: &str) -> Result<(), String> {
        let mut list = self.servers();
        if !list.iter().any(|u| u == url) {
            list.push(url.to_string());
        }
        std::fs::write(self.servers_file(), json!(list).to_string()).map_err(|e| format!("Can't save: {e}"))
    }
    fn client(&self, url: &str) -> Result<Client, String> {
        let dir = self.server_dir(url);
        std::fs::create_dir_all(&dir).map_err(|e| format!("Can't make a folder for this server: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
        Client::open(&dir, url).map_err(|e| e.to_string())
    }
}

fn need_identity(d: &Data) -> Result<(Identity, String), String> {
    d.identity().ok_or_else(|| "Set up your identity first".to_string())
}

fn valid_username(name: &str) -> Result<String, String> {
    let name = name.trim().to_ascii_lowercase();
    let ok = (2..=32).contains(&name.len()) && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_.-".contains(c));
    if ok { Ok(name) } else { Err("Usernames are 2 to 32 characters: a-z 0-9 _ . -".into()) }
}

/// What the home screen shows.
#[tauri::command]
fn status(d: tauri::State<Data>) -> R {
    let servers: Vec<Value> = d.servers().iter().map(|u| json!({ "url": u, "username": d.client(u).ok().and_then(|c| c.username()) })).collect();
    Ok(match d.identity() {
        Some((id, name)) => json!({ "identity": { "fingerprint": id.fingerprint(), "username": name }, "servers": servers }),
        None => json!({ "identity": null, "servers": servers }),
    })
}

/// Make a new identity. The 24 recovery words come back once for the screen to show.
#[tauri::command]
fn identity_create(d: tauri::State<Data>, username: String) -> R {
    if d.identity().is_some() {
        return Err("This phone already has an identity".into());
    }
    let name = valid_username(&username)?;
    let id = Identity::generate();
    id.save(&d.identity_file(), &name).map_err(|e| format!("Can't save your identity: {e}"))?;
    Ok(json!({ "phrase": id.phrase(), "fingerprint": id.fingerprint() }))
}

/// Bring an identity back from its 24 words.
#[tauri::command]
fn identity_restore(d: tauri::State<Data>, phrase: String, username: String) -> R {
    if d.identity().is_some() {
        return Err("This phone already has an identity".into());
    }
    let name = valid_username(&username)?;
    let id = Identity::from_phrase(&phrase)?;
    id.save(&d.identity_file(), &name).map_err(|e| format!("Can't save your identity: {e}"))?;
    Ok(json!({ "fingerprint": id.fingerprint() }))
}

/// Show the recovery words again (the screen asks for a tap first).
#[tauri::command]
fn identity_phrase(d: tauri::State<Data>) -> R {
    let (id, _) = need_identity(&d)?;
    Ok(json!({ "phrase": id.phrase() }))
}

/// Put this phone on a server: `mode` is "register" (a new account, with an invite or the server's setup code) or
/// "recover" (you already have an account there; your identity proves it, no approval needed).
#[tauri::command]
async fn server_add(d: tauri::State<'_, Data>, server: String, username: String, code: String, mode: String) -> R {
    let (id, default_name) = need_identity(&d)?;
    let url = discover::discover(&server).await?;
    let c = d.client(&url)?;
    if c.username().is_some() {
        return Err("This phone already has an account on that server".into());
    }
    let name = if mode == "recover" {
        c.recover(&id).await.map_err(|e| e.to_string())?
    } else {
        let name = valid_username(if username.trim().is_empty() { &default_name } else { &username })?;
        if code.trim().is_empty() {
            return Err("Enter an invite code or the server's setup code".into());
        }
        c.register(&name, &code, Some(&id)).await.map_err(|e| e.to_string())?;
        name
    };
    d.remember_server(&url)?;
    Ok(json!({ "server": url, "username": name }))
}

/// A parsed `beacon-pair:2|server|device key|time|signature|key` QR code from a PC.
struct Scan {
    server: String,
    device_key: String,
    ts: i64,
    signature: String,
    key: [u8; 32],
}

fn parse_scan(text: &str) -> Result<Scan, String> {
    let bad = || "That isn't a Beacon QR code from a PC. On the PC choose “Sign in with my phone”.".to_string();
    let parts: Vec<&str> = text.trim().split('|').collect();
    let ["beacon-pair:2", server, device_key, ts, signature, key] = parts.as_slice() else { return Err(bad()) };
    let key: [u8; 32] = server::unb64(key).and_then(|k| k.try_into().ok()).ok_or_else(bad)?;
    Ok(Scan { server: server.to_string(), device_key: device_key.to_string(), ts: ts.parse().map_err(|_| bad())?, signature: signature.to_string(), key })
}

/// Check a scanned code and say what it asks, so the screen can show the fingerprint to compare. Changes nothing.
#[tauri::command]
fn scan_inspect(d: tauri::State<Data>, text: String) -> R {
    let s = parse_scan(&text)?;
    let ours = d.servers().iter().any(|u| *u == s.server);
    if !ours {
        return Err(format!("This phone isn't on {}. Add that server here first.", s.server.trim_start_matches("https://")));
    }
    let age = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|t| t.as_secs() as i64).unwrap_or(0) - s.ts;
    if age.abs() > 600 {
        return Err("That QR code has expired. Make a new one on the PC.".into());
    }
    Ok(json!({ "server": s.server, "fingerprint": fingerprint_of(&s.device_key) }))
}

/// Approve the PC: add it to your account and hand it your identity, sealed with the key from its QR code (so the
/// server can't read it). Only called after the person has compared the fingerprints and tapped Approve.
#[tauri::command]
async fn approve(d: tauri::State<'_, Data>, text: String) -> R {
    let s = parse_scan(&text)?;
    let (id, _) = need_identity(&d)?;
    if !d.servers().iter().any(|u| *u == s.server) {
        return Err("This phone isn't on that server".into());
    }
    let c = d.client(&s.server)?;
    c.sign_in().await.map_err(|e| e.to_string())?;
    let fingerprint = c.adopt(&s.device_key, s.ts, &s.signature, Some(id.seal(&s.key))).await.map_err(|e| e.to_string())?;
    Ok(json!({ "fingerprint": fingerprint }))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
            std::fs::create_dir_all(&dir)?;
            app.manage(Data(dir));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![status, identity_create, identity_restore, identity_phrase, server_add, scan_inspect, approve])
        .run(tauri::generate_context!())
        .expect("error while running Lodestone Beacon");
}
