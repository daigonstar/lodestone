//! Accounts and sign-in with device keys (no passwords). See "Signing in" in PROTOCOL.md.

use crate::util::{b64_decode, hash, new_code, new_token, now, valid_username, ApiError, ApiResult};
use crate::Shared;
use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::Json;
use ed25519_dalek::{Signature, VerifyingKey};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

pub const PREFIX: &str = "lodestone-comms/v1";
const CHALLENGE_SECS: i64 = 120;
const SESSION_SECS: i64 = 30 * 24 * 3600;

/// Check an Ed25519 signature by a device key (both base64) over `message`.
pub fn verify(device_key: &str, message: &str, signature: &str) -> ApiResult<()> {
    let key: [u8; 32] = b64_decode(device_key).and_then(|k| k.try_into().ok()).ok_or_else(|| ApiError::bad("device_key must be a base64 Ed25519 public key"))?;
    let key = VerifyingKey::from_bytes(&key).map_err(|_| ApiError::bad("device_key isn't a valid Ed25519 key"))?;
    let sig = b64_decode(signature).and_then(|s| Signature::from_slice(&s).ok()).ok_or_else(|| ApiError::bad("signature must be a base64 Ed25519 signature"))?;
    key.verify_strict(message.as_bytes(), &sig).map_err(|_| ApiError::unauthorized("The signature doesn't match"))
}

/// The signed-in device, from `Authorization: Bearer <token>`.
#[derive(Clone, Debug)]
pub struct Auth {
    pub account: i64,
    pub username: String,
    pub device: String,
    pub host_admin: bool,
}

/// Look up a session token (used by the Authorization header and the live connection).
pub fn session(app: &Shared, token: &str) -> ApiResult<Auth> {
    let found = app.db.with(|c| {
        c.query_row(
            "SELECT a.id, a.username, d.key, a.host_admin FROM sessions s JOIN devices d ON d.key = s.device_key JOIN accounts a ON a.id = d.account
             WHERE s.token_hash = ?1 AND s.expires > ?2",
            params![hash(token), now()],
            |r| Ok(Auth { account: r.get(0)?, username: r.get(1)?, device: r.get(2)?, host_admin: r.get::<_, i64>(3)? == 1 }),
        )
        .optional()
    })?;
    found.ok_or_else(|| ApiError::unauthorized("Sign in again"))
}

impl FromRequestParts<Shared> for Auth {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, app: &Shared) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or_else(|| ApiError::unauthorized("Sign in first"))?;
        session(app, token.trim())
    }
}

#[derive(Deserialize)]
pub struct Register {
    username: String,
    device_key: String,
    signature: String,
    invite: String,
}

/// Make an account with its first device. Needs an invite code or the host's setup code.
pub async fn register(State(app): State<Shared>, Json(r): Json<Register>) -> ApiResult<Json<Value>> {
    let username = r.username.trim().to_ascii_lowercase();
    if !valid_username(&username) {
        return Err(ApiError::bad("Usernames need 2 to 32 characters: a-z, 0-9, _ . -"));
    }
    if !app.limits.allow(&format!("register:{}", r.device_key), 5, Duration::from_secs(600)) {
        return Err(ApiError(axum::http::StatusCode::TOO_MANY_REQUESTS, "Too many attempts; try again later".into()));
    }
    verify(&r.device_key, &format!("{PREFIX} register {} {username} {}", app.cfg.public_url, r.device_key), &r.signature)?;
    let code = r.invite.trim().to_string();
    let joined = app.db.with(|c| {
        let tx = c.transaction()?;
        let setup = tx.query_row("SELECT 1 FROM setup WHERE code_hash = ?1", [hash(&code)], |_| Ok(())).optional()?.is_some();
        let invite_space = if setup { None } else { crate::api::usable_invite(&tx, &code)? };
        if !setup && invite_space.is_none() {
            return Ok(Err(ApiError::forbidden("That invite or setup code isn't valid (it may have expired or been used up)")));
        }
        if tx.query_row("SELECT 1 FROM accounts WHERE username = ?1", [&username], |_| Ok(())).optional()?.is_some() {
            return Ok(Err(ApiError::conflict("That username is taken on this host")));
        }
        if tx.query_row("SELECT 1 FROM devices WHERE key = ?1", [&r.device_key], |_| Ok(())).optional()?.is_some() {
            return Ok(Err(ApiError::conflict("This device is already registered")));
        }
        tx.execute("INSERT INTO accounts (username, host_admin, created) VALUES (?1, ?2, ?3)", params![username, setup as i64, now()])?;
        let account = tx.last_insert_rowid();
        tx.execute("INSERT INTO devices (key, account, created) VALUES (?1, ?2, ?3)", params![r.device_key, account, now()])?;
        if setup {
            tx.execute("DELETE FROM setup", [])?;
        }
        if let Some(space) = &invite_space {
            crate::api::add_member(&tx, space, account, &code)?;
        }
        tx.commit()?;
        Ok(Ok(invite_space))
    })??;
    if let Some(space) = &joined {
        app.live.member_joined(&app, space, &username);
    }
    Ok(Json(json!({ "username": username, "host_admin": joined.is_none(), "joined_space": joined })))
}

#[derive(Deserialize)]
pub struct ChallengeReq {
    device_key: String,
}

/// Step 1 of signing in: a single-use nonce for this device to sign.
pub async fn challenge(State(app): State<Shared>, Json(r): Json<ChallengeReq>) -> ApiResult<Json<Value>> {
    let nonce = new_code() + &new_code();
    let ok = app.db.with(|c| {
        c.execute("DELETE FROM challenges WHERE expires < ?1", [now()])?;
        if c.query_row("SELECT 1 FROM devices WHERE key = ?1", [&r.device_key], |_| Ok(())).optional()?.is_none() {
            return Ok(false);
        }
        let waiting: i64 = c.query_row("SELECT COUNT(*) FROM challenges WHERE device_key = ?1", [&r.device_key], |r| r.get(0))?;
        if waiting >= 5 {
            return Ok(false);
        }
        c.execute("INSERT INTO challenges (nonce, device_key, expires) VALUES (?1, ?2, ?3)", params![nonce, r.device_key, now() + CHALLENGE_SECS])?;
        Ok(true)
    })?;
    // Same answer for unknown devices and for too many waiting challenges: don't reveal which keys exist.
    if !ok {
        return Err(ApiError::unauthorized("Can't sign in this device"));
    }
    Ok(Json(json!({ "nonce": nonce })))
}

#[derive(Deserialize)]
pub struct SessionReq {
    device_key: String,
    nonce: String,
    signature: String,
}

/// Step 2: the signed nonce for a session token.
pub async fn new_session(State(app): State<Shared>, Json(r): Json<SessionReq>) -> ApiResult<Json<Value>> {
    let used = app.db.with(|c| {
        c.execute("DELETE FROM challenges WHERE nonce = ?1 AND device_key = ?2 AND expires >= ?3", params![r.nonce, r.device_key, now()])
    })?;
    if used != 1 {
        return Err(ApiError::unauthorized("That sign-in challenge expired; start again"));
    }
    verify(&r.device_key, &format!("{PREFIX} login {} {}", app.cfg.public_url, r.nonce), &r.signature)?;
    let token = new_token();
    let expires = now() + SESSION_SECS;
    app.db.with(|c| {
        c.execute("DELETE FROM sessions WHERE expires < ?1", [now()])?;
        c.execute("INSERT INTO sessions (token_hash, device_key, expires) VALUES (?1, ?2, ?3)", params![hash(&token), r.device_key, expires])
    })?;
    Ok(Json(json!({ "token": token, "expires": expires })))
}

#[derive(Deserialize)]
pub struct AddDevice {
    device_key: String,
    signature: String,
}

/// Add another device to your account. The new device signs, proving it holds its key.
pub async fn add_device(State(app): State<Shared>, me: Auth, Json(r): Json<AddDevice>) -> ApiResult<Json<Value>> {
    verify(&r.device_key, &format!("{PREFIX} add-device {} {} {}", app.cfg.public_url, me.username, r.device_key), &r.signature)?;
    let added = app.db.with(|c| {
        let count: i64 = c.query_row("SELECT COUNT(*) FROM devices WHERE account = ?1", [me.account], |r| r.get(0))?;
        if count >= 10 {
            return Ok(Err(ApiError::bad("An account can have at most 10 devices")));
        }
        match c.execute("INSERT INTO devices (key, account, created) VALUES (?1, ?2, ?3)", params![r.device_key, me.account, now()]) {
            Ok(_) => Ok(Ok(())),
            Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == rusqlite::ErrorCode::ConstraintViolation => Ok(Err(ApiError::conflict("This device is already registered"))),
            Err(e) => Err(e),
        }
    })?;
    added?;
    Ok(Json(json!({ "ok": true })))
}

/// Sign out this device (ends the session).
pub async fn sign_out(State(app): State<Shared>, me: Auth) -> ApiResult<Json<Value>> {
    app.db.with(|c| c.execute("DELETE FROM sessions WHERE device_key = ?1", [&me.device]))?;
    Ok(Json(json!({ "ok": true })))
}
