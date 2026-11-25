//! The interface page and its local API, on 127.0.0.1 only. Every API call needs the per-launch secret
//! (sent by the page as `x-comms-token`) and the exact Host header, so other programs and web pages can't
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
use lodestone_comms_client::Client;
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
        .route("/api/spaces", post(create_space))
        .route("/api/channels", post(create_channel))
        .route("/api/invites", post(create_invite))
        .route("/api/messages", get(messages))
        .route("/api/send", post(send))
        .route("/api/react", post(react))
        .route("/api/friends", get(friends))
        .route("/api/invite-friend", post(invite_friend))
        .route("/api/events", get(events))
        .route("/api/manage", post(manage))
        .route("/api/icon", get(icon).post(set_icon))
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

/// Refuse requests addressed to any other host name (a web page using DNS rebinding to reach us).
async fn host_check(State(app): State<Shared>, req: Request, next: Next) -> Response {
    let ok = req.headers().get(header::HOST).and_then(|h| h.to_str().ok()) == Some(app.authority.as_str());
    if !ok {
        return (StatusCode::FORBIDDEN, "wrong host").into_response();
    }
    next.run(req).await
}

/// API calls need the per-launch secret.
async fn guard(State(app): State<Shared>, req: Request, next: Next) -> Response {
    let ok = req.headers().get("x-comms-token").and_then(|h| h.to_str().ok()) == Some(app.token.as_str());
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

fn client(app: &Shared, host: &str) -> Result<Arc<Client>, Fail> {
    app.hosts.get(host).ok_or_else(|| Fail("You haven't joined that host".into()))
}

async fn ensure_signed_in(c: &Client) -> Result<(), Fail> {
    if !c.host.signed_in() {
        c.sign_in().await?;
    }
    Ok(())
}

/// Everything the interface shows: hosts, your servers on each, their channels and members.
async fn state(State(app): State<Shared>) -> R {
    let mut hosts = Vec::new();
    for (url, c) in app.hosts.all() {
        let Some(username) = c.username() else { continue };
        let joined: Vec<String> = c.channels().map(|v| v.into_iter().map(|x| x.0).collect()).unwrap_or_default();
        let mut entry = json!({ "url": url, "username": username, "spaces": [] });
        let spaces = async {
            ensure_signed_in(&c).await?;
            let me = c.host.get("/v1/me").await?;
            let mut out = Vec::new();
            for s in me["spaces"].as_array().into_iter().flatten() {
                let id = s["id"].as_str().unwrap_or_default();
                let mut sp = c.host.get(&format!("/v1/spaces/{id}")).await?;
                for ch in sp["channels"].as_array_mut().into_iter().flatten() {
                    let ready = ch["id"].as_str().is_some_and(|i| joined.iter().any(|j| j == i));
                    ch["ready"] = json!(ready);
                }
                out.push(sp);
            }
            Ok::<_, Fail>(out)
        }
        .await;
        match spaces {
            Ok(s) => entry["spaces"] = json!(s),
            Err(e) => entry["error"] = json!(e.0),
        }
        hosts.push(entry);
    }
    Ok(Json(json!({ "hosts": hosts, "bridge": app.bridge.connected(), "theme": app.bridge.theme.lock().unwrap().clone() })))
}

/// "https://host/join/CODE" -> (host URL, code).
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

/// Join a server from an invite link: registers on that host first if you're new there.
async fn join(State(app): State<Shared>, Json(r): Json<JoinReq>) -> R {
    let (host, code) = parse_link(&r.link).ok_or_else(|| Fail("That doesn't look like an invite link (…/join/…)".into()))?;
    let host = lodestone_comms_client::discover::discover(&host).await.map_err(Fail)?;
    let c = app.hosts.open(&host).map_err(Fail)?;
    let space = if c.username().is_some() {
        ensure_signed_in(&c).await?;
        c.accept_invite(&code).await?
    } else {
        let user = r.username.filter(|u| !u.trim().is_empty()).ok_or_else(|| Fail("Choose a username for this host".into()))?;
        let reg = c.register(&user, &code).await?;
        reg["joined_space"].as_str().unwrap_or_default().to_string()
    };
    let _ = c.sync().await;
    let _ = app.events.send(json!({ "type": "state" }).to_string());
    Ok(Json(json!({ "host": host, "space": space })))
}

#[derive(Deserialize)]
struct SetupReq {
    host: String,
    code: String,
    username: String,
}

/// Become the first (admin) account on a new host, with its setup code.
async fn setup(State(app): State<Shared>, Json(r): Json<SetupReq>) -> R {
    let host = lodestone_comms_client::discover::discover(&r.host).await.map_err(Fail)?;
    let c = app.hosts.open(&host).map_err(Fail)?;
    if c.username().is_some() {
        return Err(Fail("You already have an account on that host".into()));
    }
    c.register(&r.username, &r.code).await?;
    Ok(Json(json!({ "host": c.host.url })))
}

#[derive(Deserialize)]
struct SpaceReq {
    host: String,
    name: String,
}

async fn create_space(State(app): State<Shared>, Json(r): Json<SpaceReq>) -> R {
    let c = client(&app, &r.host)?;
    ensure_signed_in(&c).await?;
    let id = c.create_space(&r.name).await?;
    let _ = app.events.send(json!({ "type": "state" }).to_string());
    Ok(Json(json!({ "space": id })))
}

#[derive(Deserialize)]
struct ChannelReq {
    host: String,
    space: String,
    name: String,
}

async fn create_channel(State(app): State<Shared>, Json(r): Json<ChannelReq>) -> R {
    let c = client(&app, &r.host)?;
    ensure_signed_in(&c).await?;
    let id = c.create_channel(&r.space, &r.name).await?;
    let _ = app.events.send(json!({ "type": "state" }).to_string());
    Ok(Json(json!({ "channel": id })))
}

#[derive(Deserialize)]
struct InviteReq {
    host: String,
    space: String,
    #[serde(default = "five")]
    max_uses: i64,
}

fn five() -> i64 {
    5
}

async fn create_invite(State(app): State<Shared>, Json(r): Json<InviteReq>) -> R {
    let c = client(&app, &r.host)?;
    ensure_signed_in(&c).await?;
    Ok(Json(c.create_invite(&r.space, r.max_uses.clamp(1, 100), 7 * 24 * 3600).await?))
}

#[derive(Deserialize)]
struct MessagesQuery {
    host: String,
    channel: String,
}

async fn messages(State(app): State<Shared>, Query(q): Query<MessagesQuery>) -> R {
    let c = client(&app, &q.host)?;
    let reactions: Vec<Value> = c.reactions(&q.channel)?.into_iter().map(|(seq, emoji, users)| json!({ "seq": seq, "emoji": emoji, "users": users })).collect();
    Ok(Json(json!({ "messages": c.messages(&q.channel, 300)?, "reactions": reactions, "me": c.username() })))
}

#[derive(Deserialize)]
struct SendReq {
    host: String,
    channel: String,
    text: String,
}

async fn send(State(app): State<Shared>, Json(r): Json<SendReq>) -> R {
    let text = r.text.trim();
    if text.is_empty() || text.len() > 8000 {
        return Err(Fail("Messages need 1 to 8000 characters".into()));
    }
    let c = client(&app, &r.host)?;
    ensure_signed_in(&c).await?;
    let m = c.send_text(&r.channel, text).await?;
    let _ = app.events.send(json!({ "type": "messages", "host": r.host, "channels": [r.channel] }).to_string());
    Ok(Json(json!(m)))
}

#[derive(Deserialize)]
struct ReactReq {
    host: String,
    channel: String,
    seq: i64,
    emoji: String,
    on: bool,
}

async fn react(State(app): State<Shared>, Json(r): Json<ReactReq>) -> R {
    let c = client(&app, &r.host)?;
    ensure_signed_in(&c).await?;
    c.react(&r.channel, r.seq, &r.emoji, r.on).await?;
    let _ = app.events.send(json!({ "type": "messages", "host": r.host, "channels": [r.channel] }).to_string());
    Ok(Json(json!({ "ok": true })))
}

/// Your Steam and Epic friends, through the bridge (needs the friends:read permission in Lodestone).
async fn friends(State(app): State<Shared>) -> R {
    Ok(Json(json!({ "friends": app.bridge.request("friends", json!({})).await.map_err(Fail)? })))
}

#[derive(Deserialize)]
struct InviteFriendReq {
    host: String,
    space: String,
    store: String,
    id: String,
}

/// Make a single-use invite and hand it to Lodestone, which copies it and opens that friend's chat.
async fn invite_friend(State(app): State<Shared>, Json(r): Json<InviteFriendReq>) -> R {
    let c = client(&app, &r.host)?;
    ensure_signed_in(&c).await?;
    let inv = c.create_invite(&r.space, 1, 7 * 24 * 3600).await?;
    let link = inv["link"].as_str().unwrap_or_default();
    let text = format!("Join me on Lodestone Comms: {link}");
    app.bridge.request("open_chat", json!({ "store": r.store, "id": r.id, "text": text })).await.map_err(Fail)?;
    Ok(Json(json!({ "link": link })))
}

#[derive(Deserialize)]
struct ManageReq {
    host: String,
    method: String,
    path: String,
    #[serde(default)]
    body: Option<Value>,
}

/// Server settings calls (profile, channels, roles, bans, invites), passed to the host with this
/// device's session. Only these host paths are allowed, and the host checks each permission itself.
async fn manage(State(app): State<Shared>, Json(r): Json<ManageReq>) -> R {
    let ok_path = (r.path.starts_with("/v1/spaces/") || r.path.starts_with("/v1/channels/")) && !r.path.contains("..") && !r.path.contains("/messages");
    if !ok_path || !["GET", "PATCH", "PUT", "POST", "DELETE"].contains(&r.method.as_str()) {
        return Err(Fail("Not a settings request".into()));
    }
    let c = client(&app, &r.host)?;
    ensure_signed_in(&c).await?;
    let v = c.host.call(&r.method, &r.path, r.body).await?;
    let _ = app.events.send(json!({ "type": "state" }).to_string());
    Ok(Json(v))
}

#[derive(Deserialize)]
struct IconQuery {
    host: String,
    space: String,
}

/// A server's icon, fetched from its host for the page.
async fn icon(State(app): State<Shared>, Query(q): Query<IconQuery>) -> Response {
    let Ok(c) = client(&app, &q.host) else { return StatusCode::NOT_FOUND.into_response() };
    if ensure_signed_in(&c).await.is_err() || !q.space.bytes().all(|b| b.is_ascii_hexdigit()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    match c.host.get_raw(&format!("/v1/spaces/{}/icon", q.space)).await {
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
    host: String,
    space: String,
    /// The image, base64.
    data: String,
}

async fn set_icon(State(app): State<Shared>, Json(r): Json<SetIcon>) -> R {
    let c = client(&app, &r.host)?;
    ensure_signed_in(&c).await?;
    if !r.space.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Fail("Bad server".into()));
    }
    let bytes = lodestone_comms_client::host::unb64(&r.data).ok_or_else(|| Fail("Bad image".into()))?;
    c.host.put_raw(&format!("/v1/spaces/{}/icon", r.space), bytes).await?;
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
        assert_eq!(super::parse_link("https://comms.invalid/join/abc123"), Some(("https://comms.invalid".into(), "abc123".into())));
        assert_eq!(super::parse_link(" http://127.0.0.1:8080/join/xyz/ "), Some(("http://127.0.0.1:8080".into(), "xyz".into())));
        assert_eq!(super::parse_link("https://comms.invalid/"), None);
        assert_eq!(super::parse_link("https://x/join/a/b"), None);
    }
}
