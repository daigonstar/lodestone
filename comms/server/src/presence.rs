//! Who's around: online and offline from when each account's app last checked in, plus the status
//! people choose (away, busy, invisible) and a short status message. Invisible looks offline to others.

use crate::auth::Auth;
use crate::util::{ApiError, ApiResult};
use crate::Shared;
use axum::extract::State;
use axum::Json;
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

/// An app that hasn't checked in for this long counts as offline (apps check in every few seconds).
pub const ONLINE_FOR: Duration = Duration::from_secs(45);

pub const STATUSES: &[&str] = &["online", "away", "busy", "invisible"];

/// What others see for an account: online, away, busy or offline.
pub fn shown(app: &Shared, account: i64, status: &str) -> &'static str {
    let recent = app.seen.lock().unwrap().get(&account).is_some_and(|t| t.elapsed() < ONLINE_FOR);
    match (recent, status) {
        (false, _) | (_, "invisible") => "offline",
        (_, "away") => "away",
        (_, "busy") => "busy",
        _ => "online",
    }
}

#[derive(Deserialize)]
pub struct StatusReq {
    status: String,
    #[serde(default)]
    text: String,
}

/// `PUT /v1/me/status`: your chosen status and an optional message (up to 80 characters).
pub async fn set_status(State(app): State<Shared>, me: Auth, Json(r): Json<StatusReq>) -> ApiResult<Json<Value>> {
    if !STATUSES.contains(&r.status.as_str()) {
        return Err(ApiError::bad("status must be online, away, busy or invisible"));
    }
    let text = r.text.trim();
    if text.chars().count() > 80 || text.chars().any(char::is_control) {
        return Err(ApiError::bad("A status message can have at most 80 characters"));
    }
    let before: String = app.db.with(|c| c.query_row("SELECT status FROM accounts WHERE id = ?1", [me.account], |r| r.get(0)))?;
    app.db.with(|c| c.execute("UPDATE accounts SET status = ?2, status_text = ?3 WHERE id = ?1", params![me.account, r.status, text]))?;
    // Tell the people you share spaces with. Going invisible is announced (you appear offline);
    // staying invisible isn't.
    if !(before == "invisible" && r.status == "invisible") {
        app.live.announce(&app, me.account, true);
    }
    Ok(Json(json!({ "status": r.status, "text": text })))
}
