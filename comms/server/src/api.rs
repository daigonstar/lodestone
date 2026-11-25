//! The web API: spaces (servers), channels, invites, MLS key packages, encrypted messages and welcomes.
//! See PROTOCOL.md. Every handler checks membership before reading or writing anything in a space.

use crate::auth::{self, Auth};
use crate::util::{b64, b64_decode, clean_name, hash, new_code, new_id, now, ApiError, ApiResult};
use crate::Shared;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

const MAX_BODY: usize = 64 * 1024;

pub fn router(app: Shared) -> Router {
    Router::new()
        .route("/v1/register", post(auth::register))
        .route("/v1/challenge", post(auth::challenge))
        .route("/v1/session", post(auth::new_session).delete(auth::sign_out))
        .route("/v1/devices", post(auth::add_device))
        .route("/v1/me", get(me))
        .route("/v1/spaces", post(create_space))
        .route("/v1/spaces/{space}", get(get_space))
        .route("/v1/spaces/{space}/channels", post(create_channel))
        .route("/v1/spaces/{space}/invites", post(create_invite))
        .route("/v1/spaces/{space}/members/{username}", delete(remove_member))
        .route("/v1/invites/{code}", get(preview_invite))
        .route("/v1/invites/{code}/accept", post(accept_invite))
        .route("/v1/keypackages", post(upload_keypackages))
        .route("/v1/keypackages/claim", post(claim_keypackages))
        .route("/v1/channels/{channel}/messages", post(send_message).get(list_messages))
        .route("/v1/welcomes", post(send_welcome).get(take_welcomes))
        .route("/v1/live", get(crate::live::connect))
        .route("/join/{code}", get(join_page))
        // Bodies are capped well above the 64 KiB message limit (base64 and JSON overhead, key package batches).
        .layer(axum::extract::DefaultBodyLimit::max(1024 * 1024))
        .with_state(app)
}

// ---------- helpers shared with auth.rs ----------

/// The space an invite code is for, if it's still usable.
pub fn usable_invite(c: &Connection, code: &str) -> rusqlite::Result<Option<String>> {
    c.query_row(
        "SELECT space FROM invites WHERE code_hash = ?1 AND expires > ?2 AND uses < max_uses",
        params![hash(code), now()],
        |r| r.get(0),
    )
    .optional()
}

/// Add an account to a space through an invite (counts one use). Banned accounts and existing members are
/// left as they are; returns whether they were added.
pub fn add_member(c: &Connection, space: &str, account: i64, code: &str) -> rusqlite::Result<bool> {
    let banned = c.query_row("SELECT 1 FROM bans WHERE space = ?1 AND account = ?2", params![space, account], |_| Ok(())).optional()?.is_some();
    if banned {
        return Ok(false);
    }
    let added = c.execute("INSERT OR IGNORE INTO members (space, account, role, joined) VALUES (?1, ?2, 'member', ?3)", params![space, account, now()])?;
    if added == 1 {
        c.execute("UPDATE invites SET uses = uses + 1 WHERE code_hash = ?1", [hash(code)])?;
    }
    Ok(added == 1)
}

fn role_of(c: &Connection, space: &str, account: i64) -> rusqlite::Result<Option<String>> {
    c.query_row("SELECT role FROM members WHERE space = ?1 AND account = ?2", params![space, account], |r| r.get(0)).optional()
}

/// Your role in a space, or 404 (the same answer whether the space exists or you aren't in it).
fn require_member(app: &Shared, space: &str, me: &Auth) -> ApiResult<String> {
    app.db.with(|c| role_of(c, space, me.account))?.ok_or_else(|| ApiError::not_found("No such space"))
}

fn require_admin(app: &Shared, space: &str, me: &Auth) -> ApiResult<String> {
    let role = require_member(app, space, me)?;
    if role == "owner" || role == "admin" {
        Ok(role)
    } else {
        Err(ApiError::forbidden("Only the space's owner and admins can do that"))
    }
}

/// The space a channel belongs to, if you're a member of it.
fn channel_space(app: &Shared, channel: &str, me: &Auth) -> ApiResult<String> {
    let space: Option<String> = app.db.with(|c| c.query_row("SELECT space FROM channels WHERE id = ?1", [channel], |r| r.get(0)).optional())?;
    let space = space.ok_or_else(|| ApiError::not_found("No such channel"))?;
    require_member(app, &space, me).map_err(|_| ApiError::not_found("No such channel"))?;
    Ok(space)
}

// ---------- you ----------

async fn me(State(app): State<Shared>, me: Auth) -> ApiResult<Json<Value>> {
    let (devices, spaces) = app.db.with(|c| {
        let devices: Vec<String> = c.prepare("SELECT key FROM devices WHERE account = ?1 ORDER BY created")?.query_map([me.account], |r| r.get(0))?.collect::<Result<_, _>>()?;
        let spaces: Vec<Value> = c
            .prepare("SELECT s.id, s.name, m.role FROM members m JOIN spaces s ON s.id = m.space WHERE m.account = ?1 ORDER BY m.joined")?
            .query_map([me.account], |r| Ok(json!({ "id": r.get::<_, String>(0)?, "name": r.get::<_, String>(1)?, "role": r.get::<_, String>(2)? })))?
            .collect::<Result<_, _>>()?;
        Ok((devices, spaces))
    })?;
    Ok(Json(json!({ "username": me.username, "device": me.device, "host_admin": me.host_admin, "devices": devices, "spaces": spaces })))
}

// ---------- spaces and channels ----------

#[derive(Deserialize)]
struct NameReq {
    name: String,
}

async fn create_space(State(app): State<Shared>, me: Auth, Json(r): Json<NameReq>) -> ApiResult<Json<Value>> {
    if app.cfg.admin_only_spaces && !me.host_admin {
        return Err(ApiError::forbidden("On this host only its admins can create servers"));
    }
    let name = clean_name(&r.name)?;
    let (space, channel) = (new_id(), new_id());
    app.db.with(|c| {
        let tx = c.transaction()?;
        tx.execute("INSERT INTO spaces (id, name, owner, created) VALUES (?1, ?2, ?3, ?4)", params![space, name, me.account, now()])?;
        tx.execute("INSERT INTO members (space, account, role, joined) VALUES (?1, ?2, 'owner', ?3)", params![space, me.account, now()])?;
        tx.execute("INSERT INTO channels (id, space, name, created) VALUES (?1, ?2, 'general', ?3)", params![channel, space, now()])?;
        tx.commit()
    })?;
    Ok(Json(json!({ "id": space, "name": name, "channels": [{ "id": channel, "name": "general", "epoch": 0 }] })))
}

async fn get_space(State(app): State<Shared>, me: Auth, Path(space): Path<String>) -> ApiResult<Json<Value>> {
    let role = require_member(&app, &space, &me)?;
    let out = app.db.with(|c| {
        let name: String = c.query_row("SELECT name FROM spaces WHERE id = ?1", [&space], |r| r.get(0))?;
        let channels: Vec<Value> = c
            .prepare("SELECT id, name, epoch FROM channels WHERE space = ?1 ORDER BY created, name")?
            .query_map([&space], |r| Ok(json!({ "id": r.get::<_, String>(0)?, "name": r.get::<_, String>(1)?, "epoch": r.get::<_, i64>(2)? })))?
            .collect::<Result<_, _>>()?;
        // Members with their device keys: clients check MLS credentials against these.
        let mut members: Vec<Value> = Vec::new();
        let mut stmt = c.prepare("SELECT a.id, a.username, m.role FROM members m JOIN accounts a ON a.id = m.account WHERE m.space = ?1 ORDER BY a.username")?;
        let rows: Vec<(i64, String, String)> = stmt.query_map([&space], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
        for (id, username, role) in rows {
            let devices: Vec<String> = c.prepare("SELECT key FROM devices WHERE account = ?1 ORDER BY created")?.query_map([id], |r| r.get(0))?.collect::<Result<_, _>>()?;
            members.push(json!({ "username": username, "role": role, "devices": devices }));
        }
        Ok(json!({ "id": space, "name": name, "your_role": role, "channels": channels, "members": members }))
    })?;
    Ok(Json(out))
}

async fn create_channel(State(app): State<Shared>, me: Auth, Path(space): Path<String>, Json(r): Json<NameReq>) -> ApiResult<Json<Value>> {
    require_admin(&app, &space, &me)?;
    let name = clean_name(&r.name)?;
    let id = new_id();
    let count: i64 = app.db.with(|c| c.query_row("SELECT COUNT(*) FROM channels WHERE space = ?1", [&space], |r| r.get(0)))?;
    if count >= 200 {
        return Err(ApiError::bad("A server can have at most 200 channels"));
    }
    app.db.with(|c| c.execute("INSERT INTO channels (id, space, name, created) VALUES (?1, ?2, ?3, ?4)", params![id, space, name, now()]))?;
    app.live.to_space(&app, &space, json!({ "type": "channel_created", "space": space, "channel": id, "name": name }));
    Ok(Json(json!({ "id": id, "name": name, "epoch": 0 })))
}

// ---------- invites and members ----------

#[derive(Deserialize)]
struct InviteReq {
    max_uses: i64,
    expires_in: i64,
}

async fn create_invite(State(app): State<Shared>, me: Auth, Path(space): Path<String>, Json(r): Json<InviteReq>) -> ApiResult<Json<Value>> {
    require_member(&app, &space, &me)?;
    if !(1..=100).contains(&r.max_uses) || !(60..=30 * 24 * 3600).contains(&r.expires_in) {
        return Err(ApiError::bad("Invites allow 1 to 100 uses and last from a minute to 30 days"));
    }
    let code = new_code();
    let expires = now() + r.expires_in;
    app.db.with(|c| {
        c.execute(
            "INSERT INTO invites (code_hash, space, created_by, max_uses, expires) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![hash(&code), space, me.account, r.max_uses, expires],
        )
    })?;
    Ok(Json(json!({ "code": code, "link": format!("{}/join/{code}", app.cfg.public_url), "expires": expires })))
}

async fn preview_invite(State(app): State<Shared>, Path(code): Path<String>) -> ApiResult<Json<Value>> {
    // Codes have about 60 bits of randomness, and lookups are capped host-wide, so guessing one is hopeless.
    if !app.limits.allow("invite-lookups", 600, Duration::from_secs(60)) {
        return Err(ApiError(StatusCode::TOO_MANY_REQUESTS, "Too many invite lookups; try again in a minute".into()));
    }
    let found = app.db.with(|c| {
        let Some(space) = usable_invite(c, &code)? else { return Ok(None) };
        let name: String = c.query_row("SELECT name FROM spaces WHERE id = ?1", [&space], |r| r.get(0))?;
        let members: i64 = c.query_row("SELECT COUNT(*) FROM members WHERE space = ?1", [&space], |r| r.get(0))?;
        Ok(Some(json!({ "space": space, "name": name, "members": members })))
    })?;
    found.map(Json).ok_or_else(|| ApiError::not_found("That invite isn't valid (it may have expired or been used up)"))
}

async fn accept_invite(State(app): State<Shared>, me: Auth, Path(code): Path<String>) -> ApiResult<Json<Value>> {
    let result = app.db.with(|c| {
        let tx = c.transaction()?;
        let Some(space) = usable_invite(&tx, &code)? else { return Ok(Err(ApiError::not_found("That invite isn't valid (it may have expired or been used up)"))) };
        if role_of(&tx, &space, me.account)?.is_some() {
            return Ok(Ok((space, false)));
        }
        if !add_member(&tx, &space, me.account, &code)? {
            return Ok(Err(ApiError::forbidden("You can't join this server")));
        }
        tx.commit()?;
        Ok(Ok((space, true)))
    })??;
    let (space, new) = result;
    if new {
        app.live.member_joined(&app, &space, &me.username);
    }
    Ok(Json(json!({ "space": space, "joined": new })))
}

#[derive(Deserialize)]
struct RemoveQuery {
    #[serde(default)]
    ban: bool,
}

async fn remove_member(State(app): State<Shared>, me: Auth, Path((space, username)): Path<(String, String)>, Query(q): Query<RemoveQuery>) -> ApiResult<Json<Value>> {
    let my_role = require_member(&app, &space, &me)?;
    let target = app.db.with(|c| {
        c.query_row(
            "SELECT a.id, m.role FROM accounts a JOIN members m ON m.account = a.id WHERE m.space = ?1 AND a.username = ?2",
            params![space, username.to_ascii_lowercase()],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
        )
        .optional()
    })?;
    let (target, target_role) = target.ok_or_else(|| ApiError::not_found("No such member"))?;
    let leaving = target == me.account;
    if leaving && my_role == "owner" {
        return Err(ApiError::bad("The owner can't leave their own server"));
    }
    if !leaving {
        let allowed = match my_role.as_str() {
            "owner" => true,
            "admin" => target_role == "member",
            _ => false,
        };
        if !allowed {
            return Err(ApiError::forbidden("You can't remove this member"));
        }
    }
    app.db.with(|c| {
        c.execute("DELETE FROM members WHERE space = ?1 AND account = ?2", params![space, target])?;
        if q.ban && !leaving {
            c.execute("INSERT OR IGNORE INTO bans (space, account) VALUES (?1, ?2)", params![space, target])?;
        }
        Ok(())
    })?;
    // Their live connection stops getting this space's events at once (the hub checks membership on send).
    app.live.to_space(&app, &space, json!({ "type": "member_left", "space": space, "username": username.to_ascii_lowercase() }));
    Ok(Json(json!({ "ok": true })))
}

// ---------- MLS key packages ----------

#[derive(Deserialize)]
struct KeyPackages {
    key_packages: Vec<String>,
}

async fn upload_keypackages(State(app): State<Shared>, me: Auth, Json(r): Json<KeyPackages>) -> ApiResult<Json<Value>> {
    let bodies: Vec<Vec<u8>> = r.key_packages.iter().map(|k| b64_decode(k).filter(|b| !b.is_empty() && b.len() <= 16 * 1024)).collect::<Option<_>>().ok_or_else(|| ApiError::bad("Key packages must be base64, up to 16 KiB each"))?;
    let waiting = app.db.with(|c| {
        let waiting: i64 = c.query_row("SELECT COUNT(*) FROM keypackages WHERE device_key = ?1", [&me.device], |r| r.get(0))?;
        if waiting as usize + bodies.len() > 100 {
            return Ok(Err(ApiError::bad("A device can have at most 100 key packages waiting")));
        }
        let tx = c.transaction()?;
        for b in &bodies {
            tx.execute("INSERT INTO keypackages (device_key, body) VALUES (?1, ?2)", params![me.device, b])?;
        }
        tx.commit()?;
        Ok(Ok(waiting as usize + bodies.len()))
    })??;
    Ok(Json(json!({ "waiting": waiting })))
}

#[derive(Deserialize)]
struct Claim {
    username: String,
}

/// One key package per device of a user who shares a space with you (each is handed out once).
async fn claim_keypackages(State(app): State<Shared>, me: Auth, Json(r): Json<Claim>) -> ApiResult<Json<Value>> {
    let username = r.username.to_ascii_lowercase();
    let out = app.db.with(|c| {
        let shared: Option<i64> = c
            .query_row(
                "SELECT a.id FROM accounts a WHERE a.username = ?1 AND EXISTS (
                     SELECT 1 FROM members m1 JOIN members m2 ON m1.space = m2.space WHERE m1.account = a.id AND m2.account = ?2)",
                params![username, me.account],
                |r| r.get(0),
            )
            .optional()?;
        let Some(account) = shared else { return Ok(Err(ApiError::not_found("No such user in your servers"))) };
        let tx = c.transaction()?;
        let devices: Vec<String> = tx.prepare("SELECT key FROM devices WHERE account = ?1")?.query_map([account], |r| r.get(0))?.collect::<Result<_, _>>()?;
        let mut packages = Vec::new();
        for d in devices {
            let kp: Option<(i64, Vec<u8>)> = tx.query_row("SELECT id, body FROM keypackages WHERE device_key = ?1 ORDER BY id LIMIT 1", [&d], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
            if let Some((id, body)) = kp {
                tx.execute("DELETE FROM keypackages WHERE id = ?1", [id])?;
                packages.push(json!({ "device_key": d, "key_package": b64(&body) }));
            } else {
                packages.push(json!({ "device_key": d, "key_package": null }));
            }
        }
        tx.commit()?;
        Ok(Ok(packages))
    })??;
    Ok(Json(json!({ "username": username, "devices": out })))
}

// ---------- encrypted messages ----------

#[derive(Deserialize)]
struct SendReq {
    kind: String,
    epoch: i64,
    body: String,
}

async fn send_message(State(app): State<Shared>, me: Auth, Path(channel): Path<String>, Json(r): Json<SendReq>) -> ApiResult<Json<Value>> {
    let space = channel_space(&app, &channel, &me)?;
    if !["commit", "proposal", "application"].contains(&r.kind.as_str()) {
        return Err(ApiError::bad("kind must be commit, proposal or application"));
    }
    let body = b64_decode(&r.body).filter(|b| !b.is_empty() && b.len() <= MAX_BODY).ok_or_else(|| ApiError::bad("body must be base64, up to 64 KiB"))?;
    if !app.limits.allow(&format!("send:{}", me.device), 60, Duration::from_secs(60)) {
        return Err(ApiError(StatusCode::TOO_MANY_REQUESTS, "Slow down: at most 60 messages a minute".into()));
    }
    let sent_at = now();
    let seq = app.db.with(|c| {
        let tx = c.transaction()?;
        let (epoch, seq): (i64, i64) = tx.query_row("SELECT epoch, next_seq FROM channels WHERE id = ?1", [&channel], |r| Ok((r.get(0)?, r.get(1)?)))?;
        // The host orders commits: only one commit per epoch is accepted, so everyone shares one history.
        if r.kind == "commit" {
            if r.epoch != epoch {
                return Ok(Err(ApiError::conflict(format!("The channel is at epoch {epoch}; catch up and try again"))));
            }
            tx.execute("UPDATE channels SET epoch = epoch + 1 WHERE id = ?1", [&channel])?;
        }
        tx.execute("UPDATE channels SET next_seq = next_seq + 1 WHERE id = ?1", [&channel])?;
        tx.execute(
            "INSERT INTO messages (channel, seq, kind, epoch, sender, body, sent_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![channel, seq, r.kind, r.epoch, me.device, body, sent_at],
        )?;
        tx.commit()?;
        Ok(Ok(seq))
    })??;
    app.live.to_space(
        &app,
        &space,
        json!({ "type": "message", "channel": channel, "seq": seq, "kind": r.kind, "epoch": r.epoch, "sender": me.device, "sender_name": me.username, "body": r.body, "sent_at": sent_at }),
    );
    Ok(Json(json!({ "seq": seq, "sent_at": sent_at })))
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default)]
    after: i64,
    limit: Option<i64>,
}

async fn list_messages(State(app): State<Shared>, me: Auth, Path(channel): Path<String>, Query(q): Query<ListQuery>) -> ApiResult<Json<Value>> {
    channel_space(&app, &channel, &me)?;
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let messages: Vec<Value> = app.db.with(|c| {
        c.prepare(
            "SELECT m.seq, m.kind, m.epoch, m.sender, COALESCE(a.username, ''), m.body, m.sent_at FROM messages m
             LEFT JOIN devices d ON d.key = m.sender LEFT JOIN accounts a ON a.id = d.account
             WHERE m.channel = ?1 AND m.seq > ?2 ORDER BY m.seq LIMIT ?3",
        )?
        .query_map(params![channel, q.after, limit], |r| {
            Ok(json!({ "seq": r.get::<_, i64>(0)?, "kind": r.get::<_, String>(1)?, "epoch": r.get::<_, i64>(2)?, "sender": r.get::<_, String>(3)?,
                       "sender_name": r.get::<_, String>(4)?, "body": b64(&r.get::<_, Vec<u8>>(5)?), "sent_at": r.get::<_, i64>(6)? }))
        })?
        .collect()
    })?;
    Ok(Json(json!({ "channel": channel, "messages": messages })))
}

// ---------- MLS welcomes ----------

#[derive(Deserialize)]
struct WelcomeReq {
    channel: String,
    to: Vec<String>,
    body: String,
}

/// Deliver an MLS Welcome to new members' devices (only devices of members of the channel's space).
async fn send_welcome(State(app): State<Shared>, me: Auth, Json(r): Json<WelcomeReq>) -> ApiResult<Json<Value>> {
    let space = channel_space(&app, &r.channel, &me)?;
    let body = b64_decode(&r.body).filter(|b| !b.is_empty() && b.len() <= MAX_BODY).ok_or_else(|| ApiError::bad("body must be base64, up to 64 KiB"))?;
    if r.to.is_empty() || r.to.len() > 100 {
        return Err(ApiError::bad("Send a welcome to 1 to 100 devices"));
    }
    let delivered = app.db.with(|c| {
        let tx = c.transaction()?;
        let mut n = Vec::new();
        for d in &r.to {
            let in_space = tx
                .query_row("SELECT 1 FROM devices dv JOIN members m ON m.account = dv.account WHERE dv.key = ?1 AND m.space = ?2", params![d, space], |_| Ok(()))
                .optional()?
                .is_some();
            if in_space {
                tx.execute("INSERT INTO welcomes (device_key, channel, body, created) VALUES (?1, ?2, ?3, ?4)", params![d, r.channel, body, now()])?;
                n.push(d.clone());
            }
        }
        tx.commit()?;
        Ok(n)
    })?;
    for d in &delivered {
        app.live.to_device(d, json!({ "type": "welcome", "channel": r.channel }));
    }
    Ok(Json(json!({ "delivered": delivered.len() })))
}

/// Your device's waiting welcomes; handed out once.
async fn take_welcomes(State(app): State<Shared>, me: Auth) -> ApiResult<Json<Value>> {
    let list = app.db.with(|c| {
        let tx = c.transaction()?;
        let rows: Vec<(i64, String, Vec<u8>)> = tx.prepare("SELECT id, channel, body FROM welcomes WHERE device_key = ?1 ORDER BY id")?.query_map([&me.device], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
        tx.execute("DELETE FROM welcomes WHERE device_key = ?1", [&me.device])?;
        tx.commit()?;
        Ok(rows.into_iter().map(|(_, ch, b)| json!({ "channel": ch, "body": b64(&b) })).collect::<Vec<_>>())
    })?;
    Ok(Json(json!({ "welcomes": list })))
}

// ---------- invite links ----------

/// What an invite link shows in a browser: how to join with Lodestone. The code isn't echoed into the page.
async fn join_page() -> axum::response::Html<&'static str> {
    axum::response::Html(
        "<!doctype html><meta charset=utf-8><title>Lodestone Comms invite</title>\
         <body style=\"font-family:sans-serif;max-width:36em;margin:4em auto;line-height:1.5\">\
         <h1>You've been invited to a Lodestone Comms server</h1>\
         <p>Open Lodestone, go to <b>Comms</b>, choose <b>Join a server</b> and paste this page's address.</p>\
         <p>Lodestone is a free, open-source game launcher for Linux: <a href=\"https://github.com/daigonstar/lodestone\">github.com/daigonstar/lodestone</a>.</p>",
    )
}
