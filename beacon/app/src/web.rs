//! The interface page and its local API, on 127.0.0.1 only. Every API call needs the per-launch secret
//! (sent by the page as `x-beacon-token`) and the exact Host header, so other programs and web pages can't
//! use it.

use crate::Shared;
use axum::body::Body;
use axum::extract::{Query, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::StreamExt;
use lodestone_beacon_client::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

const INDEX: &str = include_str!("../ui/index.html");
const SCRIPT: &str = include_str!("../ui/app.js");
const STYLE: &str = include_str!("../ui/style.css");
const EMOJI: &str = include_str!("../ui/emoji.js");

/// Only this page's own code runs, it only talks to its own plug-in, and only Lodestone may frame it.
const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' https: data: blob:; base-uri 'none'; form-action 'none'; frame-ancestors tauri://localhost http://tauri.localhost";

pub fn router(app: Shared) -> Router {
    let api = Router::new()
        .route("/api/state", get(state))
        .route("/api/join", post(join))
        .route("/api/setup", post(setup))
        .route("/api/communities", post(create_community))
        .route("/api/channels", post(create_channel))
        .route("/api/invites", post(create_invite))
        .route("/api/messages", get(messages))
        .route("/api/send", post(send))
        .route("/api/react", post(react))
        .route("/api/status", post(set_status))
        .route("/api/devices", get(devices))
        .route("/api/devices/remove", post(remove_device))
        .route("/api/pair/start", post(pair_start))
        .route("/api/pair/decide", post(pair_decide))
        .route("/api/pair/claim", post(pair_claim))
        .route("/api/pair/check", post(pair_check))
        .route("/api/profiles", get(profiles))
        .route("/api/profile", post(set_profile).layer(axum::extract::DefaultBodyLimit::max(1024 * 1024)))
        .route("/api/avatar", get(avatar))
        .route("/api/prefs", get(get_prefs).post(set_prefs))
        .route("/api/upload", post(upload).layer(axum::extract::DefaultBodyLimit::max(16 * 1024 * 1024)))
        .route("/api/file", get(open_file))
        .route("/api/friends", get(friends))
        .route("/api/invite-friend", post(invite_friend))
        .route("/api/open-chat", post(open_chat))
        .route("/api/mentions", get(get_mentions))
        .route("/api/mentions/clear", post(clear_mentions))
        .route("/api/events", get(events))
        .route("/api/manage", post(manage))
        .route("/api/icon", get(icon).post(set_icon))
        .route("/api/banner", get(banner).post(set_banner).layer(axum::extract::DefaultBodyLimit::max(1024 * 1024)))
        .route("/api/probe", post(probe))
        .route("/api/voice", get(voice_status))
        .route("/api/voice/join", post(voice_join))
        .route("/api/voice/leave", post(voice_leave))
        .route("/api/voice/state", post(voice_state))
        .route("/api/voice/devices", get(voice_devices))
        .route("/api/voice/settings", post(voice_settings))
        .route("/api/voice/test", post(voice_test))
        .route("/api/voice/ptt", post(voice_ptt))
        .route("/api/voice/share", post(voice_share))
        .route("/api/voice/screens", get(voice_screens))
        .route("/api/voice/user-volume", post(voice_user_volume))
        .route("/api/voice/watch", post(voice_watch))
        .route("/api/voice/picture", get(voice_picture))
        .route("/api/voice/camera", post(voice_camera))
        .route("/api/voice/see-cameras", post(voice_see_cameras))
        .route("/api/voice/cameras", get(voice_cameras))
        .route_layer(middleware::from_fn_with_state(app.clone(), guard));
    Router::new()
        .route("/", get(|| async { page(INDEX, "text/html; charset=utf-8") }))
        .route("/app.js", get(|| async { page(SCRIPT, "text/javascript; charset=utf-8") }))
        .route("/style.css", get(|| async { page(STYLE, "text/css; charset=utf-8") }))
        .route("/emoji.js", get(|| async { page(EMOJI, "text/javascript; charset=utf-8") }))
        .merge(api)
        .layer(middleware::from_fn_with_state(app.clone(), host_check))
        .with_state(app)
}

fn page(body: &'static str, kind: &'static str) -> Response {
    let mut r = body.into_response();
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(kind));
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP));
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

/// Refuse requests addressed to any other server name (a web page using DNS rebinding to reach us).
async fn host_check(State(app): State<Shared>, req: Request, next: Next) -> Response {
    let ok = req.headers().get(header::HOST).and_then(|h| h.to_str().ok()) == Some(app.authority.as_str());
    if !ok {
        return (StatusCode::FORBIDDEN, "wrong server").into_response();
    }
    next.run(req).await
}

/// API calls need the per-launch secret.
async fn guard(State(app): State<Shared>, req: Request, next: Next) -> Response {
    let ok = req.headers().get("x-beacon-token").and_then(|h| h.to_str().ok()) == Some(app.token.as_str());
    if !ok {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "not allowed" }))).into_response();
    }
    next.run(req).await
}

struct Fail(String);

impl IntoResponse for Fail {
    fn into_response(self) -> Response {
        (StatusCode::BAD_REQUEST, Json(json!({ "error": self.0 }))).into_response()
    }
}

impl<E: std::fmt::Display> From<E> for Fail {
    fn from(e: E) -> Self {
        Fail(e.to_string())
    }
}

type R = Result<Json<Value>, Fail>;

fn client(app: &Shared, server: &str) -> Result<Arc<Client>, Fail> {
    app.servers.get(server).ok_or_else(|| Fail("You haven't joined that server".into()))
}

async fn ensure_signed_in(c: &Client) -> Result<(), Fail> {
    if !c.server.signed_in() {
        c.sign_in().await?;
    }
    Ok(())
}

/// Everything the interface shows: servers, your communities on each, their channels and members.
async fn state(State(app): State<Shared>) -> R {
    let mut servers = Vec::new();
    let mut pairing = Vec::new();
    for (url, c) in app.servers.all() {
        if let (None, Some(user)) = (c.username(), c.pairing()) {
            pairing.push(json!({ "url": url, "username": user, "fingerprint": c.fingerprint() }));
        }
        let Some(username) = c.username() else { continue };
        let joined: Vec<String> = c.channels().map(|v| v.into_iter().map(|x| x.0).collect()).unwrap_or_default();
        let mut entry = json!({ "url": url, "username": username, "communities": [], "status": "online", "status_text": "" });
        let communities = async {
            ensure_signed_in(&c).await?;
            let me = c.server.get("/v1/me").await?;
            let status = (me["status"].clone(), me["status_text"].clone());
            let mut out = Vec::new();
            for s in me["communities"].as_array().into_iter().flatten() {
                let id = s["id"].as_str().unwrap_or_default();
                let mut sp = c.server.get(&format!("/v1/communities/{id}")).await?;
                for ch in sp["channels"].as_array_mut().into_iter().flatten() {
                    let ready = ch["id"].as_str().is_some_and(|i| joined.iter().any(|j| j == i));
                    ch["ready"] = json!(ready);
                }
                out.push(sp);
            }
            Ok::<_, Fail>((out, status))
        }
        .await;
        match communities {
            Ok((s, (status, text))) => {
                entry["communities"] = json!(s);
                entry["status"] = status;
                entry["status_text"] = text;
            }
            Err(e) => entry["error"] = json!(e.0),
        }
        servers.push(entry);
    }
    Ok(Json(json!({ "servers": servers, "pairing": pairing, "bridge": app.bridge.connected(), "theme": app.bridge.theme.lock().unwrap().clone() })))
}

/// "https://server/join/CODE" -> (server URL, code).
fn parse_link(link: &str) -> Option<(String, String)> {
    let link = link.trim();
    let (base, code) = link.rsplit_once("/join/")?;
    let code = code.trim_matches('/');
    (!base.is_empty() && !code.is_empty() && !code.contains('/')).then(|| (base.to_string(), code.to_string()))
}

#[derive(Deserialize)]
struct JoinReq {
    link: String,
    username: Option<String>,
}

/// Join a community from an invite link: registers on that server first if you're new there.
async fn join(State(app): State<Shared>, Json(r): Json<JoinReq>) -> R {
    let (server, code) = parse_link(&r.link).ok_or_else(|| Fail("That doesn't look like an invite link (…/join/…)".into()))?;
    let server = lodestone_beacon_client::discover::discover(&server).await.map_err(Fail)?;
    let c = app.servers.open(&server).map_err(Fail)?;
    let community = if c.username().is_some() {
        ensure_signed_in(&c).await?;
        c.accept_invite(&code).await?
    } else {
        let user = r.username.filter(|u| !u.trim().is_empty()).ok_or_else(|| Fail("Choose a username for this server".into()))?;
        let reg = c.register(&user, &code).await?;
        reg["joined_community"].as_str().unwrap_or_default().to_string()
    };
    let _ = c.sync().await;
    let _ = app.events.send(json!({ "type": "state" }).to_string());
    Ok(Json(json!({ "server": server, "community": community })))
}

#[derive(Deserialize)]
struct SetupReq {
    server: String,
    code: String,
    username: String,
}

/// Become the first (admin) account on a new server, with its setup code.
async fn setup(State(app): State<Shared>, Json(r): Json<SetupReq>) -> R {
    let server = lodestone_beacon_client::discover::discover(&r.server).await.map_err(Fail)?;
    let c = app.servers.open(&server).map_err(Fail)?;
    if c.username().is_some() {
        return Err(Fail("You already have an account on that server".into()));
    }
    c.register(&r.username, &r.code).await?;
    Ok(Json(json!({ "server": c.server.url })))
}

#[derive(Deserialize)]
struct CommunityReq {
    server: String,
    name: String,
}

async fn create_community(State(app): State<Shared>, Json(r): Json<CommunityReq>) -> R {
    let c = client(&app, &r.server)?;
    ensure_signed_in(&c).await?;
    let id = c.create_community(&r.name).await?;
    let _ = app.events.send(json!({ "type": "state" }).to_string());
    Ok(Json(json!({ "community": id })))
}

#[derive(Deserialize)]
struct ChannelReq {
    server: String,
    community: String,
    name: String,
    #[serde(default)]
    kind: String,
}

async fn create_channel(State(app): State<Shared>, Json(r): Json<ChannelReq>) -> R {
    let c = client(&app, &r.server)?;
    ensure_signed_in(&c).await?;
    let id = if r.kind == "voice" { c.create_voice_channel(&r.community, &r.name).await? } else { c.create_channel(&r.community, &r.name).await? };
    let _ = app.events.send(json!({ "type": "state" }).to_string());
    Ok(Json(json!({ "channel": id })))
}

#[derive(Deserialize)]
struct InviteReq {
    server: String,
    community: String,
    #[serde(default = "five")]
    max_uses: i64,
}

fn five() -> i64 {
    5
}

async fn create_invite(State(app): State<Shared>, Json(r): Json<InviteReq>) -> R {
    let c = client(&app, &r.server)?;
    ensure_signed_in(&c).await?;
    Ok(Json(c.create_invite(&r.community, r.max_uses.clamp(1, 100), 7 * 24 * 3600).await?))
}

#[derive(Deserialize)]
struct MessagesQuery {
    server: String,
    channel: String,
}

async fn messages(State(app): State<Shared>, Query(q): Query<MessagesQuery>) -> R {
    let c = client(&app, &q.server)?;
    let reactions: Vec<Value> = c.reactions(&q.channel)?.into_iter().map(|(seq, emoji, users)| json!({ "seq": seq, "emoji": emoji, "users": users })).collect();
    Ok(Json(json!({ "messages": c.messages(&q.channel, 300)?, "reactions": reactions, "me": c.username() })))
}

#[derive(Deserialize)]
struct SendReq {
    server: String,
    channel: String,
    text: String,
}

async fn send(State(app): State<Shared>, Json(r): Json<SendReq>) -> R {
    let text = r.text.trim();
    if text.is_empty() || text.len() > 8000 {
        return Err(Fail("Messages need 1 to 8000 characters".into()));
    }
    let c = client(&app, &r.server)?;
    ensure_signed_in(&c).await?;
    let m = c.send_text(&r.channel, text).await?;
    let _ = app.events.send(json!({ "type": "messages", "server": r.server, "channels": [r.channel] }).to_string());
    Ok(Json(json!(m)))
}

#[derive(Deserialize)]
struct ReactReq {
    server: String,
    channel: String,
    seq: i64,
    emoji: String,
    on: bool,
}

async fn react(State(app): State<Shared>, Json(r): Json<ReactReq>) -> R {
    let c = client(&app, &r.server)?;
    ensure_signed_in(&c).await?;
    c.react(&r.channel, r.seq, &r.emoji, r.on).await?;
    let _ = app.events.send(json!({ "type": "messages", "server": r.server, "channels": [r.channel] }).to_string());
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct UploadReq {
    server: String,
    channel: String,
    name: String,
    /// The file, base64.
    data: String,
    #[serde(default)]
    caption: String,
}

async fn upload(State(app): State<Shared>, Json(r): Json<UploadReq>) -> R {
    let bytes = lodestone_beacon_client::server::unb64(&r.data).ok_or_else(|| Fail("Bad file".into()))?;
    let c = client(&app, &r.server)?;
    ensure_signed_in(&c).await?;
    let m = c.send_file(&r.channel, &r.name, &bytes, r.caption.trim()).await?;
    let _ = app.events.send(json!({ "type": "messages", "server": r.server, "channels": [r.channel] }).to_string());
    Ok(Json(json!(m)))
}

#[derive(Deserialize)]
struct FileQuery {
    server: String,
    channel: String,
    seq: i64,
}

/// A decrypted attachment for the page. Only real images (checked by content) are served as images;
/// anything else is an inert download, so a file can never run as a page in this origin.
async fn open_file(State(app): State<Shared>, Query(q): Query<FileQuery>) -> Response {
    let Ok(c) = client(&app, &q.server) else { return StatusCode::NOT_FOUND.into_response() };
    if ensure_signed_in(&c).await.is_err() {
        return StatusCode::NOT_FOUND.into_response();
    }
    match c.open_file(&q.channel, q.seq).await {
        Ok((bytes, mime, name)) => {
            let mut r = Response::new(Body::from(bytes));
            let h = r.headers_mut();
            let image = mime.starts_with("image/");
            h.insert(header::CONTENT_TYPE, HeaderValue::from_str(if image { &mime } else { "application/octet-stream" }).unwrap_or(HeaderValue::from_static("application/octet-stream")));
            if !image {
                let safe: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || ".-_ ".contains(c) { c } else { '_' }).collect();
                if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{safe}\"")) {
                    h.insert(header::CONTENT_DISPOSITION, v);
                }
            }
            h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
            h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'none'; sandbox"));
            h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            r
        }
        Err(e) => (StatusCode::NOT_FOUND, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

#[derive(Deserialize)]
struct StatusReq {
    status: String,
    #[serde(default)]
    text: String,
}

/// Set your status on every server you've joined (it's one "you", wherever your friends are).
async fn set_status(State(app): State<Shared>, Json(r): Json<StatusReq>) -> R {
    let mut errors = Vec::new();
    for (url, c) in app.servers.all() {
        if c.username().is_none() {
            continue;
        }
        let res = async {
            ensure_signed_in(&c).await?;
            c.server.call("PUT", "/v1/me/status", Some(json!({ "status": r.status, "text": r.text }))).await?;
            Ok::<_, Fail>(())
        }
        .await;
        if let Err(e) = res {
            errors.push(format!("{url}: {}", e.0));
        }
    }
    let _ = app.events.send(json!({ "type": "state" }).to_string());
    if errors.is_empty() {
        Ok(Json(json!({ "ok": true })))
    } else {
        Err(Fail(errors.join("; ")))
    }
}

#[derive(Deserialize)]
struct ServerQuery {
    server: String,
}

/// Profiles this device knows on a server, and your own.
async fn profiles(State(app): State<Shared>, Query(q): Query<ServerQuery>) -> R {
    let c = client(&app, &q.server)?;
    let (name, color, has_avatar) = c.my_profile();
    let playing = c.activities();
    let mut list: Vec<Value> = c.profiles()?.into_iter().map(|(user, name, color, has)| json!({ "user": user, "name": name, "color": color, "has_avatar": has, "playing": playing.get(&user) })).collect();
    // People playing something who haven't set a profile.
    for (user, game) in &playing {
        if !list.iter().any(|p| p["user"] == json!(user)) {
            list.push(json!({ "user": user, "name": "", "color": "", "has_avatar": false, "playing": game }));
        }
    }
    let me = c.username().unwrap_or_default();
    Ok(Json(json!({ "me": { "user": c.username(), "name": name, "color": color, "has_avatar": has_avatar, "playing": playing.get(&me) }, "profiles": list })))
}

#[derive(Deserialize)]
struct ProfileReq {
    name: String,
    #[serde(default)]
    color: String,
    /// base64 picture; "" removes it; missing keeps it.
    avatar: Option<String>,
}

/// Set your profile on every server you've joined (sent end-to-end encrypted to your communities).
async fn set_profile(State(app): State<Shared>, Json(r): Json<ProfileReq>) -> R {
    let avatar = match &r.avatar {
        None => None,
        Some(a) if a.is_empty() => Some(Vec::new()),
        Some(a) => Some(lodestone_beacon_client::server::unb64(a).ok_or_else(|| Fail("Bad picture".into()))?),
    };
    let mut errors = Vec::new();
    for (url, c) in app.servers.all() {
        if c.username().is_none() {
            continue;
        }
        let res = async {
            ensure_signed_in(&c).await?;
            c.set_profile(&r.name, r.color.trim(), avatar.as_deref()).await?;
            Ok::<_, Fail>(())
        }
        .await;
        if let Err(e) = res {
            errors.push(format!("{url}: {}", e.0));
        }
        let _ = app.events.send(json!({ "type": "profiles", "server": url }).to_string());
    }
    if errors.is_empty() { Ok(Json(json!({ "ok": true }))) } else { Err(Fail(errors.join("; "))) }
}

#[derive(Deserialize)]
struct AvatarQuery {
    server: String,
    user: String,
}

/// A profile picture, decrypted, served only as an image.
async fn avatar(State(app): State<Shared>, Query(q): Query<AvatarQuery>) -> Response {
    let Ok(c) = client(&app, &q.server) else { return StatusCode::NOT_FOUND.into_response() };
    let _ = ensure_signed_in(&c).await;
    match c.avatar(&q.user).await {
        Ok((bytes, mime)) if mime.starts_with("image/") => {
            let mut r = Response::new(Body::from(bytes));
            let h = r.headers_mut();
            h.insert(header::CONTENT_TYPE, HeaderValue::from_str(&mime).unwrap_or(HeaderValue::from_static("image/png")));
            h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
            h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'none'; sandbox"));
            h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            r
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

const THEMES: &[&str] = &["lodestone", "midnight", "light", "contrast"];

/// Theme and accent, saved in Beacon's own folder (the page's address changes every launch, so the
/// browser's own storage wouldn't keep them).
async fn get_prefs(State(app): State<Shared>) -> R {
    let p: Value = std::fs::read_to_string(app.servers.prefs_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(json!({}));
    Ok(Json(json!({ "theme": p["theme"].as_str().filter(|t| THEMES.contains(t)).unwrap_or("lodestone"), "accent": p["accent"].as_str().unwrap_or("") })))
}

#[derive(Deserialize)]
struct Prefs {
    theme: String,
    #[serde(default)]
    accent: String,
}

async fn set_prefs(State(app): State<Shared>, Json(r): Json<Prefs>) -> R {
    if !THEMES.contains(&r.theme.as_str()) {
        return Err(Fail("Unknown theme".into()));
    }
    let accent = r.accent.trim();
    if !accent.is_empty() && !(accent.len() == 7 && accent.starts_with('#') && accent[1..].bytes().all(|b| b.is_ascii_hexdigit())) {
        return Err(Fail("Accent colours look like #7c5cff".into()));
    }
    std::fs::write(app.servers.prefs_file(), json!({ "theme": r.theme, "accent": accent }).to_string()).map_err(|e| Fail(e.to_string()))?;
    Ok(Json(json!({ "ok": true })))
}

/// Your devices on a server, this device's fingerprint, and devices waiting for your approval.
async fn devices(State(app): State<Shared>, Query(q): Query<ServerQuery>) -> R {
    let c = client(&app, &q.server)?;
    ensure_signed_in(&c).await?;
    let list: Vec<Value> = c.devices().await?.into_iter().map(|(k, f, this)| json!({ "key": k, "fingerprint": f, "this": this })).collect();
    let waiting: Vec<Value> = c.pair_waiting().await?.into_iter().map(|(k, f)| json!({ "key": k, "fingerprint": f })).collect();
    Ok(Json(json!({ "fingerprint": c.fingerprint(), "devices": list, "waiting": waiting })))
}

#[derive(Deserialize)]
struct DeviceReq {
    server: String,
    device_key: String,
    #[serde(default)]
    approve: bool,
}

async fn remove_device(State(app): State<Shared>, Json(r): Json<DeviceReq>) -> R {
    let c = client(&app, &r.server)?;
    ensure_signed_in(&c).await?;
    c.remove_device(&r.device_key).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn pair_start(State(app): State<Shared>, Json(r): Json<ServerQuery>) -> R {
    let c = client(&app, &r.server)?;
    ensure_signed_in(&c).await?;
    let (code, expires) = c.pair_start().await?;
    Ok(Json(json!({ "code": code, "expires": expires, "server": c.server.url.trim_start_matches("https://") })))
}

async fn pair_decide(State(app): State<Shared>, Json(r): Json<DeviceReq>) -> R {
    let c = client(&app, &r.server)?;
    ensure_signed_in(&c).await?;
    c.pair_decide(&r.device_key, r.approve).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct PairClaim {
    server: String,
    code: String,
}

/// On a new device: offer this device to an account, using a code from one of its devices.
async fn pair_claim(State(app): State<Shared>, Json(r): Json<PairClaim>) -> R {
    let server = lodestone_beacon_client::discover::discover(&r.server).await.map_err(Fail)?;
    let c = app.servers.open(&server).map_err(Fail)?;
    if c.username().is_some() {
        return Err(Fail("This device already has an account on that server".into()));
    }
    let (username, fingerprint) = c.pair_claim(&r.code).await?;
    let _ = app.events.send(json!({ "type": "state" }).to_string());
    Ok(Json(json!({ "server": server, "username": username, "fingerprint": fingerprint })))
}

async fn pair_check(State(app): State<Shared>, Json(r): Json<ServerQuery>) -> R {
    let c = app.servers.get(&r.server).ok_or_else(|| Fail("Unknown server".into()))?;
    let status = c.pair_check().await?;
    if status != "waiting" {
        let _ = app.events.send(json!({ "type": "state" }).to_string());
    }
    Ok(Json(json!({ "status": status })))
}

/// Your Steam and Epic friends, through the bridge (needs the friends:read permission in Lodestone).
async fn friends(State(app): State<Shared>) -> R {
    Ok(Json(json!({ "friends": app.bridge.request("friends", json!({})).await.map_err(Fail)? })))
}

#[derive(Deserialize)]
struct InviteFriendReq {
    server: String,
    community: String,
}

/// Make a single-use invite for a friend: the message to send them. The page shows it, copies it when
/// you click (the reliable way to set the clipboard), then asks Lodestone to open their chat.
async fn invite_friend(State(app): State<Shared>, Json(r): Json<InviteFriendReq>) -> R {
    let c = client(&app, &r.server)?;
    ensure_signed_in(&c).await?;
    let inv = c.create_invite(&r.community, 1, 7 * 24 * 3600).await?;
    let link = inv["link"].as_str().unwrap_or_default();
    Ok(Json(json!({ "link": link, "text": format!("Join me on Lodestone Beacon: {link}") })))
}

#[derive(Deserialize)]
struct OpenChatReq {
    store: String,
    id: String,
    text: String,
}

/// Ask Lodestone to open a friend's chat (it also copies the text, and checks everything it's given).
async fn open_chat(State(app): State<Shared>, Json(r): Json<OpenChatReq>) -> R {
    app.bridge.request("open_chat", json!({ "store": r.store, "id": r.id, "text": r.text })).await.map_err(Fail)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct ManageReq {
    server: String,
    method: String,
    path: String,
    #[serde(default)]
    body: Option<Value>,
}

/// Community settings calls (profile, channels, roles, bans, invites), passed to the server with this
/// device's session. Only these server paths are allowed, and the server checks each permission itself.
async fn manage(State(app): State<Shared>, Json(r): Json<ManageReq>) -> R {
    let ok_path = (r.path.starts_with("/v1/communities/") || r.path.starts_with("/v1/channels/")) && !r.path.contains("..") && !r.path.contains("/messages");
    if !ok_path || !["GET", "PATCH", "PUT", "POST", "DELETE"].contains(&r.method.as_str()) {
        return Err(Fail("Not a settings request".into()));
    }
    let c = client(&app, &r.server)?;
    ensure_signed_in(&c).await?;
    let v = c.server.call(&r.method, &r.path, r.body).await?;
    let _ = app.events.send(json!({ "type": "state" }).to_string());
    Ok(Json(v))
}

#[derive(Deserialize)]
struct IconQuery {
    server: String,
    community: String,
}

/// A community's icon, fetched from its server for the page.
async fn icon(State(app): State<Shared>, Query(q): Query<IconQuery>) -> Response {
    community_image(&app, q, "icon").await
}

/// A community's banner, fetched the same way.
async fn banner(State(app): State<Shared>, Query(q): Query<IconQuery>) -> Response {
    community_image(&app, q, "banner").await
}

async fn community_image(app: &Shared, q: IconQuery, what: &str) -> Response {
    let Ok(c) = client(app, &q.server) else { return StatusCode::NOT_FOUND.into_response() };
    if ensure_signed_in(&c).await.is_err() || !q.community.bytes().all(|b| b.is_ascii_hexdigit()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    match c.server.get_raw(&format!("/v1/communities/{}/{what}", q.community)).await {
        Ok((bytes, kind)) if kind.starts_with("image/") && !kind.contains("svg") => {
            let mut r = Response::new(Body::from(bytes));
            r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_str(&kind).unwrap_or(HeaderValue::from_static("image/png")));
            r.headers_mut().insert("x-content-type-options", HeaderValue::from_static("nosniff"));
            r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            r
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(Deserialize)]
struct SetIcon {
    server: String,
    community: String,
    /// The image, base64.
    data: String,
}

async fn set_icon(State(app): State<Shared>, Json(r): Json<SetIcon>) -> R {
    put_community_image(&app, r, "icon").await
}

async fn set_banner(State(app): State<Shared>, Json(r): Json<SetIcon>) -> R {
    put_community_image(&app, r, "banner").await
}

#[derive(Deserialize)]
struct ProbeReq {
    server: String,
}

/// The setup guide's "is it reachable?" check: asks what was typed for its Beacon details (nothing is sent but the request).
async fn probe(Json(r): Json<ProbeReq>) -> R {
    let url = lodestone_beacon_client::discover::discover(&r.server).await.map_err(Fail)?;
    Ok(Json(json!({ "server": url })))
}

async fn put_community_image(app: &Shared, r: SetIcon, what: &str) -> R {
    let c = client(app, &r.server)?;
    ensure_signed_in(&c).await?;
    if !r.community.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Fail("Bad community".into()));
    }
    let bytes = lodestone_beacon_client::server::unb64(&r.data).ok_or_else(|| Fail("Bad image".into()))?;
    c.server.put_raw(&format!("/v1/communities/{}/{what}", r.community), bytes).await?;
    let _ = app.events.send(json!({ "type": "state" }).to_string());
    Ok(Json(json!({ "ok": true })))
}

/// Live updates for the page, as newline-separated JSON (read with fetch, so the secret stays in a header).
async fn events(State(app): State<Shared>) -> Response {
    let rx = app.events.subscribe();
    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(e) => return Some((Ok::<_, std::io::Error>(format!("{e}\n")), rx)),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => return None,
            }
        }
    });
    let hello = futures_util::stream::once(async { Ok::<_, std::io::Error>("{\"type\":\"hello\"}\n".to_string()) });
    let mut r = Response::new(Body::from_stream(hello.chain(stream)));
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/x-ndjson"));
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_invite_links() {
        assert_eq!(super::parse_link("https://beacon.invalid/join/abc123"), Some(("https://beacon.invalid".into(), "abc123".into())));
        assert_eq!(super::parse_link(" http://127.0.0.1:8080/join/xyz/ "), Some(("http://127.0.0.1:8080".into(), "xyz".into())));
        assert_eq!(super::parse_link("https://beacon.invalid/"), None);
        assert_eq!(super::parse_link("https://x/join/a/b"), None);
    }
}

// Calls (see voice.rs).

async fn voice_status(State(app): State<Shared>) -> R {
    Ok(Json(crate::voice::status(&app).await))
}

#[derive(Deserialize)]
struct VoiceJoin {
    server: String,
    channel: String,
    #[serde(default)]
    muted: bool,
    #[serde(default)]
    deafened: bool,
}

async fn voice_join(State(app): State<Shared>, Json(r): Json<VoiceJoin>) -> R {
    let c = client(&app, &r.server)?;
    ensure_signed_in(&c).await?;
    let v = crate::voice::join(&app, c, &r.server, &r.channel, r.muted, r.deafened).await.map_err(Fail)?;
    let _ = app.events.send(json!({ "type": "state" }).to_string());
    Ok(Json(v))
}

async fn voice_leave(State(app): State<Shared>) -> R {
    crate::voice::leave(&app).await;
    let _ = app.events.send(json!({ "type": "state" }).to_string());
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct VoiceState {
    muted: bool,
    deafened: bool,
}

async fn voice_state(State(app): State<Shared>, Json(r): Json<VoiceState>) -> R {
    crate::voice::set_state(&app, r.muted, r.deafened).await.map_err(Fail)?;
    Ok(Json(json!({ "ok": true })))
}

async fn voice_devices(State(app): State<Shared>) -> R {
    let list = crate::voice::devices(&app).await.map_err(Fail)?;
    Ok(Json(json!({ "devices": list })))
}

async fn voice_settings(State(app): State<Shared>, Json(r): Json<crate::voice::Settings>) -> R {
    app.voice.set_settings(r).map_err(Fail)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct OnOff {
    on: bool,
}

async fn voice_test(State(app): State<Shared>, Json(r): Json<OnOff>) -> R {
    let a = app.clone();
    tokio::task::spawn_blocking(move || crate::voice::mic_test(&a, r.on)).await?.map_err(Fail)?;
    Ok(Json(json!({ "ok": true })))
}

async fn voice_ptt(State(app): State<Shared>, Json(r): Json<OnOff>) -> R {
    crate::voice::ptt(&app, r.on);
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct ShareReq {
    on: bool,
    #[serde(default)]
    source: Option<String>,
}

async fn voice_share(State(app): State<Shared>, Json(r): Json<ShareReq>) -> R {
    if r.on { crate::voice::start_share(&app, r.source).await } else { crate::voice::stop_share(&app).await }.map_err(Fail)?;
    Ok(Json(json!({ "ok": true })))
}

async fn voice_screens(State(_app): State<Shared>) -> R {
    let list = tokio::task::spawn_blocking(crate::voice::screen_sources).await?;
    Ok(Json(json!({ "sources": list })))
}

#[derive(Deserialize)]
struct WatchReq {
    slot: usize,
    on: bool,
}

async fn voice_watch(State(app): State<Shared>, Json(r): Json<WatchReq>) -> R {
    crate::voice::watch(&app, r.slot, r.on).await.map_err(Fail)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct SlotQuery {
    slot: usize,
    #[serde(default)]
    cam: bool,
}

async fn voice_camera(State(app): State<Shared>, Json(r): Json<OnOff>) -> R {
    crate::voice::set_camera(&app, r.on).await.map_err(Fail)?;
    Ok(Json(json!({ "ok": true })))
}

async fn voice_see_cameras(State(app): State<Shared>, Json(r): Json<OnOff>) -> R {
    crate::voice::see_cameras(&app, r.on).await.map_err(Fail)?;
    Ok(Json(json!({ "ok": true })))
}

async fn voice_cameras(State(_app): State<Shared>) -> R {
    Ok(Json(json!({ "cameras": crate::voice::cameras().await.map_err(Fail)? })))
}

/// A share you're watching, as a stream of JPEG pictures, each preceded by its length (4 bytes, big
/// endian). The page reads it with fetch (so the per-launch secret stays in a header) and shows each
/// picture as it arrives. Ends when you stop watching.
async fn voice_picture(State(app): State<Shared>, Query(q): Query<SlotQuery>) -> Response {
    let Some(pic) = crate::voice::picture(&app, q.slot, q.cam).await else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "You're not watching that share" }))).into_response();
    };
    let stream = futures_util::stream::unfold((pic, 0u64), |(pic, last)| async move {
        loop {
            let found = {
                let notified = pic.changed.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                let newest = pic.jpeg.lock().unwrap().clone();
                match newest.filter(|(n, _)| *n != last) {
                    Some(x) => Some(x),
                    // Nobody else holds the picture any more: you stopped watching.
                    None if std::sync::Arc::strong_count(&pic) <= 1 => return None,
                    None => {
                        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), notified).await;
                        None
                    }
                }
            };
            if let Some((n, jpeg)) = found {
                let mut out = (jpeg.len() as u32).to_be_bytes().to_vec();
                out.extend_from_slice(&jpeg);
                return Some((Ok::<_, std::io::Error>(out), (pic, n)));
            }
        }
    });
    let mut r = Response::new(Body::from_stream(stream));
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

#[derive(Deserialize)]
struct UserVolume {
    server: String,
    username: String,
    volume: f64,
}

async fn voice_user_volume(State(app): State<Shared>, Json(r): Json<UserVolume>) -> R {
    crate::voice::set_user_volume(&app, &r.server, &r.username, r.volume).await.map_err(Fail)?;
    Ok(Json(json!({ "ok": true })))
}

/// Unread mentions of you, by "server|channel".
async fn get_mentions(State(app): State<Shared>) -> R {
    Ok(Json(json!(app.mentions.lock().unwrap().clone())))
}

#[derive(Deserialize)]
struct ChannelRef {
    server: String,
    channel: String,
}

async fn clear_mentions(State(app): State<Shared>, Json(r): Json<ChannelRef>) -> R {
    app.mentions.lock().unwrap().remove(&format!("{}|{}", r.server.trim_end_matches('/'), r.channel));
    Ok(Json(json!({ "ok": true })))
}
