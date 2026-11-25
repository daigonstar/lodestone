//! Errors, random IDs and codes, hashing and simple rate limits.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// An error answer: `{ "error": "..." }` with a status code.
#[derive(Debug)]
pub struct ApiError(pub StatusCode, pub String);

pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    pub fn bad(msg: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, msg.into())
    }
    pub fn unauthorized(msg: impl Into<String>) -> Self {
        Self(StatusCode::UNAUTHORIZED, msg.into())
    }
    pub fn forbidden(msg: impl Into<String>) -> Self {
        Self(StatusCode::FORBIDDEN, msg.into())
    }
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self(StatusCode::NOT_FOUND, msg.into())
    }
    pub fn conflict(msg: impl Into<String>) -> Self {
        Self(StatusCode::CONFLICT, msg.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, axum::Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}

/// Database errors become a plain 500; details go to the host's log, not to the client.
impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> Self {
        eprintln!("database error: {e}");
        Self(StatusCode::INTERNAL_SERVER_ERROR, "database error".into())
    }
}

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::getrandom(&mut b).expect("no system randomness");
    b
}

/// An ID for spaces and channels: 12 random bytes as hex.
pub fn new_id() -> String {
    random_bytes::<12>().iter().map(|b| format!("{b:02x}")).collect()
}

/// A session token: 32 random bytes, base64url without padding.
pub fn new_token() -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random_bytes::<32>())
}

/// A code people can type: 12 characters from an alphabet without look-alikes (about 60 bits).
pub fn new_code() -> String {
    const ALPHABET: &[u8] = b"23456789abcdefghjkmnpqrstuvwxyz";
    random_bytes::<12>().iter().map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char).collect()
}

/// Codes and tokens are stored as SHA-256, so a copied database can't be used to sign in or join.
pub fn hash(secret: &str) -> String {
    Sha256::digest(secret.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD.decode(s).ok()
}

pub fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Usernames: 2 to 32 of a-z 0-9 _ . - (stored lower-case).
pub fn valid_username(u: &str) -> bool {
    (2..=32).contains(&u.len()) && u.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_.-".contains(&c))
}

/// Space and channel names: 1 to 64 characters, no control characters, trimmed.
pub fn clean_name(name: &str) -> ApiResult<String> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > 64 || n.chars().any(char::is_control) {
        return Err(ApiError::bad("Names need 1 to 64 characters, without control characters"));
    }
    Ok(n.to_string())
}

/// Per-device sliding-window limits, kept in memory.
#[derive(Default)]
pub struct Limits {
    sent: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl Limits {
    /// Allow at most `max` events per `window` for this key.
    pub fn allow(&self, key: &str, max: usize, window: Duration) -> bool {
        let mut map = self.sent.lock().unwrap();
        let q = map.entry(key.to_string()).or_default();
        let now = Instant::now();
        while q.front().is_some_and(|t| now.duration_since(*t) > window) {
            q.pop_front();
        }
        if q.len() >= max {
            return false;
        }
        q.push_back(now);
        true
    }
}
