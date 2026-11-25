//! Server (space) settings: profile and icon, channels, roles, ownership, bans and invites.
//! Owners and admins manage most of it; only the owner changes roles, transfers ownership or deletes
//! the server. Everything here is metadata the host already sees (names, topics, who's a member).

use crate::api::{require_admin, require_member};
use crate::auth::Auth;
use crate::util::{clean_name, now, ApiError, ApiResult};
use crate::Shared;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::Json;
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

const MAX_ICON: usize = 256 * 1024;

fn clean_text(t: &str, max: usize, what: &str) -> ApiResult<String> {
    let t = t.trim();
    if t.chars().count() > max || t.chars().any(|c| c.is_control() && c != '\n') {
        return Err(ApiError::bad(format!("{what} can have at most {max} characters")));
    }
    Ok(t.to_string())
}

fn updated(app: &Shared, space: &str) {
    app.live.to_space(app, space, json!({ "type": "space_updated", "space": space }));
}

// ---------- profile ----------

#[derive(Deserialize)]
pub struct SpacePatch {
    name: Option<String>,
    description: Option<String>,
}

pub async fn update_space(State(app): State<Shared>, me: Auth, Path(space): Path<String>, Json(r): Json<SpacePatch>) -> ApiResult<Json<Value>> {
    require_admin(&app, &space, &me)?;
    let name = r.name.as_deref().map(clean_name).transpose()?;
    let description = r.description.as_deref().map(|d| clean_text(d, 500, "The description")).transpose()?;
    app.db.with(|c| {
        if let Some(n) = &name {
            c.execute("UPDATE spaces SET name = ?2 WHERE id = ?1", params![space, n])?;
        }
        if let Some(d) = &description {
            c.execute("UPDATE spaces SET description = ?2 WHERE id = ?1", params![space, d])?;
        }
        Ok(())
    })?;
    updated(&app, &space);
    Ok(Json(json!({ "ok": true })))
}

/// The image type from its first bytes; only these four are accepted.
fn image_type(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if b.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if b.len() > 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// `PUT /v1/spaces/{space}/icon` with the image as the body (PNG, JPEG, GIF or WebP, up to 256 KiB).
pub async fn put_icon(State(app): State<Shared>, me: Auth, Path(space): Path<String>, body: Bytes) -> ApiResult<Json<Value>> {
    require_admin(&app, &space, &me)?;
    if body.len() > MAX_ICON {
        return Err(ApiError::bad("Server icons can be at most 256 KiB"));
    }
    let kind = image_type(&body).ok_or_else(|| ApiError::bad("Server icons must be PNG, JPEG, GIF or WebP"))?;
    app.db.with(|c| c.execute("UPDATE spaces SET icon = ?2, icon_type = ?3 WHERE id = ?1", params![space, body.to_vec(), kind]))?;
    updated(&app, &space);
    Ok(Json(json!({ "ok": true })))
}

pub async fn delete_icon(State(app): State<Shared>, me: Auth, Path(space): Path<String>) -> ApiResult<Json<Value>> {
    require_admin(&app, &space, &me)?;
    app.db.with(|c| c.execute("UPDATE spaces SET icon = NULL, icon_type = NULL WHERE id = ?1", [&space]))?;
    updated(&app, &space);
    Ok(Json(json!({ "ok": true })))
}

/// The icon, for members. Served as an image that browsers won't treat as anything else.
pub async fn get_icon(State(app): State<Shared>, me: Auth, Path(space): Path<String>) -> ApiResult<Response> {
    require_member(&app, &space, &me)?;
    let icon: Option<(Vec<u8>, String)> = app.db.with(|c| {
        c.query_row("SELECT icon, icon_type FROM spaces WHERE id = ?1 AND icon IS NOT NULL", [&space], |r| Ok((r.get(0)?, r.get(1)?))).optional()
    })?;
    let (bytes, kind) = icon.ok_or_else(|| ApiError::not_found("No icon"))?;
    let mut h = HeaderMap::new();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_str(&kind).unwrap_or(HeaderValue::from_static("application/octet-stream")));
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'none'"));
    Ok((h, bytes).into_response())
}

/// Delete the server, its channels, messages and invites (owner only).
pub async fn delete_space(State(app): State<Shared>, me: Auth, Path(space): Path<String>) -> ApiResult<Json<Value>> {
    if require_member(&app, &space, &me)? != "owner" {
        return Err(ApiError::forbidden("Only the owner can delete the server"));
    }
    // Tell members first: after the delete there's no member list to send to.
    app.live.to_space(&app, &space, json!({ "type": "space_deleted", "space": space }));
    app.db.with(|c| c.execute("DELETE FROM spaces WHERE id = ?1", [&space]))?;
    crate::blobs::sweep(&app);
    Ok(Json(json!({ "ok": true })))
}

// ---------- channels ----------

fn channel_admin(app: &Shared, channel: &str, me: &Auth) -> ApiResult<String> {
    let space: Option<String> = app.db.with(|c| c.query_row("SELECT space FROM channels WHERE id = ?1", [channel], |r| r.get(0)).optional())?;
    let space = space.ok_or_else(|| ApiError::not_found("No such channel"))?;
    require_member(app, &space, me).map_err(|_| ApiError::not_found("No such channel"))?;
    require_admin(app, &space, me)?;
    Ok(space)
}

#[derive(Deserialize)]
pub struct ChannelPatch {
    name: Option<String>,
    topic: Option<String>,
}

pub async fn update_channel(State(app): State<Shared>, me: Auth, Path(channel): Path<String>, Json(r): Json<ChannelPatch>) -> ApiResult<Json<Value>> {
    let space = channel_admin(&app, &channel, &me)?;
    let name = r.name.as_deref().map(clean_name).transpose()?;
    let topic = r.topic.as_deref().map(|t| clean_text(t, 300, "A topic")).transpose()?;
    app.db.with(|c| {
        if let Some(n) = &name {
            c.execute("UPDATE channels SET name = ?2 WHERE id = ?1", params![channel, n])?;
        }
        if let Some(t) = &topic {
            c.execute("UPDATE channels SET topic = ?2 WHERE id = ?1", params![channel, t])?;
        }
        Ok(())
    })?;
    updated(&app, &space);
    Ok(Json(json!({ "ok": true })))
}

/// Delete a channel and its stored (encrypted) messages. A server keeps at least one channel.
pub async fn delete_channel(State(app): State<Shared>, me: Auth, Path(channel): Path<String>) -> ApiResult<Json<Value>> {
    let space = channel_admin(&app, &channel, &me)?;
    let count: i64 = app.db.with(|c| c.query_row("SELECT COUNT(*) FROM channels WHERE space = ?1", [&space], |r| r.get(0)))?;
    if count <= 1 {
        return Err(ApiError::bad("A server needs at least one channel"));
    }
    app.db.with(|c| c.execute("DELETE FROM channels WHERE id = ?1", [&channel]))?;
    crate::blobs::sweep(&app);
    app.live.to_space(&app, &space, json!({ "type": "channel_deleted", "space": space, "channel": channel }));
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct Order {
    channels: Vec<String>,
}

/// Set the channel order: the list must name every channel of the server exactly once.
pub async fn reorder_channels(State(app): State<Shared>, me: Auth, Path(space): Path<String>, Json(r): Json<Order>) -> ApiResult<Json<Value>> {
    require_admin(&app, &space, &me)?;
    app.db.with(|c| {
        let mut current: Vec<String> = c.prepare("SELECT id FROM channels WHERE space = ?1")?.query_map([&space], |r| r.get(0))?.collect::<Result<_, _>>()?;
        let mut wanted = r.channels.clone();
        current.sort();
        wanted.sort();
        if current != wanted {
            return Ok(Err(ApiError::bad("The order must list every channel of this server once")));
        }
        let tx = c.transaction()?;
        for (i, id) in r.channels.iter().enumerate() {
            tx.execute("UPDATE channels SET position = ?2 WHERE id = ?1", params![id, i as i64])?;
        }
        tx.commit()?;
        Ok(Ok(()))
    })??;
    updated(&app, &space);
    Ok(Json(json!({ "ok": true })))
}

// ---------- roles and ownership ----------

fn account_in(app: &Shared, space: &str, username: &str) -> ApiResult<(i64, String)> {
    app.db
        .with(|c| {
            c.query_row(
                "SELECT a.id, m.role FROM accounts a JOIN members m ON m.account = a.id WHERE m.space = ?1 AND a.username = ?2",
                params![space, username.to_ascii_lowercase()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
        })?
        .ok_or_else(|| ApiError::not_found("No such member"))
}

#[derive(Deserialize)]
pub struct RoleReq {
    role: String,
}

/// Make someone an admin or a member again (owner only).
pub async fn set_role(State(app): State<Shared>, me: Auth, Path((space, username)): Path<(String, String)>, Json(r): Json<RoleReq>) -> ApiResult<Json<Value>> {
    if require_member(&app, &space, &me)? != "owner" {
        return Err(ApiError::forbidden("Only the owner can change roles"));
    }
    if !["admin", "member"].contains(&r.role.as_str()) {
        return Err(ApiError::bad("role must be admin or member (use transfer to change the owner)"));
    }
    let (account, role) = account_in(&app, &space, &username)?;
    if role == "owner" {
        return Err(ApiError::bad("Transfer ownership instead"));
    }
    app.db.with(|c| c.execute("UPDATE members SET role = ?3 WHERE space = ?1 AND account = ?2", params![space, account, r.role]))?;
    updated(&app, &space);
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct Transfer {
    username: String,
}

/// Hand the server to another member (owner only); the old owner becomes an admin.
pub async fn transfer(State(app): State<Shared>, me: Auth, Path(space): Path<String>, Json(r): Json<Transfer>) -> ApiResult<Json<Value>> {
    if require_member(&app, &space, &me)? != "owner" {
        return Err(ApiError::forbidden("Only the owner can transfer the server"));
    }
    let (account, _) = account_in(&app, &space, &r.username)?;
    if account == me.account {
        return Err(ApiError::bad("You already own it"));
    }
    app.db.with(|c| {
        let tx = c.transaction()?;
        tx.execute("UPDATE members SET role = 'admin' WHERE space = ?1 AND account = ?2", params![space, me.account])?;
        tx.execute("UPDATE members SET role = 'owner' WHERE space = ?1 AND account = ?2", params![space, account])?;
        tx.execute("UPDATE spaces SET owner = ?2 WHERE id = ?1", params![space, account])?;
        tx.commit()
    })?;
    updated(&app, &space);
    Ok(Json(json!({ "ok": true })))
}

// ---------- bans ----------

pub async fn list_bans(State(app): State<Shared>, me: Auth, Path(space): Path<String>) -> ApiResult<Json<Value>> {
    require_admin(&app, &space, &me)?;
    let names: Vec<String> = app.db.with(|c| {
        c.prepare("SELECT a.username FROM bans b JOIN accounts a ON a.id = b.account WHERE b.space = ?1 ORDER BY a.username")?.query_map([&space], |r| r.get(0))?.collect()
    })?;
    Ok(Json(json!({ "bans": names })))
}

pub async fn unban(State(app): State<Shared>, me: Auth, Path((space, username)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    require_admin(&app, &space, &me)?;
    let n = app.db.with(|c| {
        c.execute("DELETE FROM bans WHERE space = ?1 AND account = (SELECT id FROM accounts WHERE username = ?2)", params![space, username.to_ascii_lowercase()])
    })?;
    if n == 0 {
        return Err(ApiError::not_found("That user isn't banned here"));
    }
    Ok(Json(json!({ "ok": true })))
}

// ---------- invites ----------

/// Active invites. The host keeps only a hash of each code, so links can't be shown again; each invite
/// has a short ID to revoke it by. Admins see all invites, members their own.
pub async fn list_invites(State(app): State<Shared>, me: Auth, Path(space): Path<String>) -> ApiResult<Json<Value>> {
    let role = require_member(&app, &space, &me)?;
    let all = role == "owner" || role == "admin";
    let list: Vec<Value> = app.db.with(|c| {
        c.execute("DELETE FROM invites WHERE expires <= ?1 OR uses >= max_uses", [now()])?;
        c.prepare(
            "SELECT substr(i.code_hash, 1, 12), a.username, i.uses, i.max_uses, i.expires FROM invites i JOIN accounts a ON a.id = i.created_by
             WHERE i.space = ?1 AND (?2 OR i.created_by = ?3) ORDER BY i.expires",
        )?
        .query_map(params![space, all, me.account], |r| {
            Ok(json!({ "id": r.get::<_, String>(0)?, "created_by": r.get::<_, String>(1)?, "uses": r.get::<_, i64>(2)?, "max_uses": r.get::<_, i64>(3)?, "expires": r.get::<_, i64>(4)? }))
        })?
        .collect()
    })?;
    Ok(Json(json!({ "invites": list })))
}

pub async fn revoke_invite(State(app): State<Shared>, me: Auth, Path((space, id)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let role = require_member(&app, &space, &me)?;
    let all = role == "owner" || role == "admin";
    if id.len() != 12 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ApiError::bad("Not an invite ID"));
    }
    let n = app.db.with(|c| {
        c.execute(
            "DELETE FROM invites WHERE space = ?1 AND substr(code_hash, 1, 12) = ?2 AND (?3 OR created_by = ?4)",
            params![space, id, all, me.account],
        )
    })?;
    if n == 0 {
        return Err(ApiError::not_found("No such invite"));
    }
    Ok(Json(json!({ "ok": true })))
}
