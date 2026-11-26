//! Adding a device to your account with a one-time code, approved on a device you already have.
//!
//! 1. A signed-in device asks for a code (valid 10 minutes, single use).
//! 2. The new device claims it with its key and a signature, and gets the username back.
//! 3. The existing device sees the waiting device (and a fingerprint of its key to compare on both
//!    screens) and approves or refuses it. Only then is the device added; members' apps then add it
//!    to every channel. A leaked code alone isn't enough to join someone's account.

use crate::auth::{verify, Auth, PREFIX};
use crate::util::{hash, new_code, now, ApiError, ApiResult};
use crate::Shared;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::Duration;

const VALID_FOR: i64 = 600;

/// A short fingerprint of a device key, shown on both screens: "3f9a-02c1-77de-b410".
pub fn fingerprint(device_key: &str) -> String {
    let d = Sha256::digest(device_key.as_bytes());
    d[..8].chunks(2).map(|c| format!("{:02x}{:02x}", c[0], c[1])).collect::<Vec<_>>().join("-")
}

/// `POST /v1/pairing`: a new one-time code for adding a device to your account.
pub async fn start(State(app): State<Shared>, me: Auth) -> ApiResult<Json<Value>> {
    let code = new_code();
    let expires = now() + VALID_FOR;
    let ok = app.db.with(|c| {
        c.execute("DELETE FROM pairings WHERE expires < ?1", [now()])?;
        let active: i64 = c.query_row("SELECT COUNT(*) FROM pairings WHERE account = ?1", [me.account], |r| r.get(0))?;
        if active >= 3 {
            return Ok(false);
        }
        c.execute("INSERT INTO pairings (code_hash, account, expires) VALUES (?1, ?2, ?3)", params![hash(&code), me.account, expires])?;
        Ok(true)
    })?;
    if !ok {
        return Err(ApiError::bad("You already have three pairing codes waiting; use or let one expire first"));
    }
    Ok(Json(json!({ "code": code, "expires": expires })))
}

#[derive(Deserialize)]
pub struct Claim {
    code: String,
    device_key: String,
    signature: String,
}

/// `POST /v1/pairing/claim` (no sign-in): the new device offers its key for a code. Signed over
/// `lodestone-beacon/v1 pair <server> <code> <device_key>`. Returns the username; the device still has to be
/// approved.
pub async fn claim(State(app): State<Shared>, Json(r): Json<Claim>) -> ApiResult<Json<Value>> {
    if !app.limits.allow("pair-claims", 120, Duration::from_secs(60)) {
        return Err(ApiError(StatusCode::TOO_MANY_REQUESTS, "Too many attempts; try again in a minute".into()));
    }
    let code = r.code.trim().to_ascii_lowercase();
    verify(&r.device_key, &format!("{PREFIX} pair {} {code} {}", app.cfg.public_url, r.device_key), &r.signature)?;
    let res = app.db.with(|c| {
        let tx = c.transaction()?;
        if tx.query_row("SELECT 1 FROM devices WHERE key = ?1", [&r.device_key], |_| Ok(())).optional()?.is_some() {
            return Ok(Err(ApiError::conflict("This device is already registered")));
        }
        let row: Option<(i64, Option<String>)> = tx
            .query_row("SELECT account, device_key FROM pairings WHERE code_hash = ?1 AND expires >= ?2", params![hash(&code), now()], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        let Some((account, claimed)) = row else { return Ok(Err(ApiError::not_found("That code isn't valid (it may have expired)"))) };
        match claimed {
            Some(k) if k != r.device_key => return Ok(Err(ApiError::conflict("Another device already used that code"))),
            Some(_) => {}
            None => {
                tx.execute("UPDATE pairings SET device_key = ?2 WHERE code_hash = ?1", params![hash(&code), r.device_key])?;
            }
        }
        let username: String = tx.query_row("SELECT username FROM accounts WHERE id = ?1", [account], |r| r.get(0))?;
        tx.commit()?;
        Ok(Ok(username))
    })??;
    Ok(Json(json!({ "username": res, "fingerprint": fingerprint(&r.device_key), "status": "waiting" })))
}

#[derive(Deserialize)]
pub struct StatusReq {
    code: String,
    device_key: String,
}

/// `POST /v1/pairing/status` (no sign-in): the new device asks whether it's been approved yet.
pub async fn status(State(app): State<Shared>, Json(r): Json<StatusReq>) -> ApiResult<Json<Value>> {
    let code = r.code.trim().to_ascii_lowercase();
    let (registered, waiting) = app.db.with(|c| {
        let registered = c.query_row("SELECT 1 FROM devices WHERE key = ?1", [&r.device_key], |_| Ok(())).optional()?.is_some();
        let waiting = c
            .query_row("SELECT 1 FROM pairings WHERE code_hash = ?1 AND device_key = ?2 AND expires >= ?3", params![hash(&code), r.device_key, now()], |_| Ok(()))
            .optional()?
            .is_some();
        Ok((registered, waiting))
    })?;
    let status = if registered { "approved" } else if waiting { "waiting" } else { "refused" };
    // The hand-over data (if any) is given once, to the device it was meant for.
    let blob: Option<String> = if registered {
        app.db.with(|c| {
            let blob = c.query_row("SELECT blob FROM pairing_blobs WHERE device_key = ?1", [&r.device_key], |r| r.get(0)).optional()?;
            c.execute("DELETE FROM pairing_blobs WHERE device_key = ?1", [&r.device_key])?;
            Ok(blob)
        })?
    } else {
        None
    };
    // A device that was added by scanning its QR code (`adopt`) never saw a username; tell it once it's in.
    let username: Option<String> = if registered {
        app.db.with(|c| c.query_row("SELECT a.username FROM devices d JOIN accounts a ON a.id = d.account WHERE d.key = ?1", [&r.device_key], |r| r.get(0)).optional())?
    } else {
        None
    };
    Ok(Json(json!({ "status": status, "blob": blob, "username": username })))
}

/// `GET /v1/pairing`: devices waiting for your approval, with their fingerprints.
pub async fn waiting(State(app): State<Shared>, me: Auth) -> ApiResult<Json<Value>> {
    let keys: Vec<String> = app.db.with(|c| {
        c.prepare("SELECT device_key FROM pairings WHERE account = ?1 AND device_key IS NOT NULL AND expires >= ?2")?.query_map(params![me.account, now()], |r| r.get(0))?.collect()
    })?;
    let list: Vec<Value> = keys.iter().map(|k| json!({ "device_key": k, "fingerprint": fingerprint(k) })).collect();
    Ok(Json(json!({ "waiting": list })))
}

#[derive(Deserialize)]
pub struct Decide {
    device_key: String,
    /// On approve: something for the new device, encrypted on the approving device with a key the server never
    /// sees (it travels in the QR code), such as the person's identity. Up to 1 KiB; handed over once.
    #[serde(default)]
    blob: Option<String>,
}

/// `POST /v1/pairing/approve`: add the waiting device to your account.
pub async fn approve(State(app): State<Shared>, me: Auth, Json(r): Json<Decide>) -> ApiResult<Json<Value>> {
    app.db.with(|c| {
        let tx = c.transaction()?;
        let n = tx.execute("DELETE FROM pairings WHERE account = ?1 AND device_key = ?2 AND expires >= ?3", params![me.account, r.device_key, now()])?;
        if n == 0 {
            return Ok(Err(ApiError::not_found("No such device waiting (the code may have expired)")));
        }
        let count: i64 = tx.query_row("SELECT COUNT(*) FROM devices WHERE account = ?1", [me.account], |r| r.get(0))?;
        if count >= 10 {
            return Ok(Err(ApiError::bad("An account can have at most 10 devices; remove one first")));
        }
        tx.execute("INSERT INTO devices (key, account, created) VALUES (?1, ?2, ?3)", params![r.device_key, me.account, now()])?;
        if let Some(blob) = &r.blob {
            if blob.len() > 1024 {
                return Ok(Err(ApiError::bad("The hand-over data can be at most 1 KiB")));
            }
            tx.execute("INSERT OR REPLACE INTO pairing_blobs (device_key, blob) VALUES (?1, ?2)", params![r.device_key, blob])?;
        }
        tx.commit()?;
        Ok(Ok(()))
    })??;
    Ok(Json(json!({ "ok": true, "fingerprint": fingerprint(&r.device_key) })))
}

/// `POST /v1/pairing/refuse`: turn the waiting device away (its code stops working).
pub async fn refuse(State(app): State<Shared>, me: Auth, Json(r): Json<Decide>) -> ApiResult<Json<Value>> {
    app.db.with(|c| c.execute("DELETE FROM pairings WHERE account = ?1 AND device_key = ?2", params![me.account, r.device_key]))?;
    Ok(Json(json!({ "ok": true })))
}

/// `POST /v1/devices/remove`: remove one of your other devices (a lost or old one). Its sessions and key
/// packages go with it, and members' apps take it out of every channel.
pub async fn remove_device(State(app): State<Shared>, me: Auth, Json(r): Json<Decide>) -> ApiResult<Json<Value>> {
    if r.device_key == me.device {
        return Err(ApiError::bad("You can't remove the device you're using"));
    }
    let n = app.db.with(|c| c.execute("DELETE FROM devices WHERE key = ?1 AND account = ?2", params![r.device_key, me.account]))?;
    if n == 0 {
        return Err(ApiError::not_found("No such device on your account"));
    }
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct Adopt {
    device_key: String,
    /// When the new device made its QR code (seconds since 1970); a QR code is good for 10 minutes.
    ts: i64,
    /// The new device's signature over `lodestone-beacon/v1 adopt <server> <ts> <device_key>`: proof that the key in
    /// the QR code belongs to a device that is showing it.
    signature: String,
    /// The identity (or other data) sealed with a key that only travels in the QR code. Up to 1 KiB.
    #[serde(default)]
    blob: Option<String>,
}

/// `POST /v1/pairing/adopt`: add a new device to your account because you scanned its QR code. The other way round
/// from `pairing`: the new device (a PC with no camera, say) shows a QR code, and a signed-in device (your phone)
/// scans it and approves. Your phone has already shown you the new device's fingerprint to compare.
pub async fn adopt(State(app): State<Shared>, me: Auth, Json(r): Json<Adopt>) -> ApiResult<Json<Value>> {
    if !app.limits.allow("pair-adopt", 30, Duration::from_secs(60)) {
        return Err(ApiError(StatusCode::TOO_MANY_REQUESTS, "Too many attempts; try again in a minute".into()));
    }
    if (now() - r.ts).abs() > VALID_FOR {
        return Err(ApiError::bad("That QR code has expired; make a new one on the other device"));
    }
    verify(&r.device_key, &format!("{PREFIX} adopt {} {} {}", app.cfg.public_url, r.ts, r.device_key), &r.signature)?;
    if r.blob.as_ref().is_some_and(|b| b.len() > 1024) {
        return Err(ApiError::bad("The hand-over data can be at most 1 KiB"));
    }
    app.db.with(|c| {
        let tx = c.transaction()?;
        if tx.query_row("SELECT 1 FROM devices WHERE key = ?1", [&r.device_key], |_| Ok(())).optional()?.is_some() {
            return Ok(Err(ApiError::conflict("This device is already registered")));
        }
        let count: i64 = tx.query_row("SELECT COUNT(*) FROM devices WHERE account = ?1", [me.account], |r| r.get(0))?;
        if count >= 10 {
            return Ok(Err(ApiError::bad("An account can have at most 10 devices; remove one first")));
        }
        tx.execute("INSERT INTO devices (key, account, created) VALUES (?1, ?2, ?3)", params![r.device_key, me.account, now()])?;
        if let Some(blob) = &r.blob {
            tx.execute("INSERT OR REPLACE INTO pairing_blobs (device_key, blob) VALUES (?1, ?2)", params![r.device_key, blob])?;
        }
        tx.commit()?;
        Ok(Ok(()))
    })??;
    Ok(Json(json!({ "ok": true, "fingerprint": fingerprint(&r.device_key), "username": me.username })))
}
