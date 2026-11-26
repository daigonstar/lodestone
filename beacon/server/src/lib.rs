//! Lodestone Beacon server: stores and relays end-to-end encrypted messages for self-hosted communities.
//! The protocol is in beacon/PROTOCOL.md. This program never sees message text: it handles accounts,
//! communities (communities), channels, invites, MLS key packages and the ordered delivery of MLS messages.
//!
//! Settings (environment):
//! - `BEACON_PUBLIC_URL`: the address clients use, e.g. `https://beacon.yourdomain.example` (signed into every
//!   sign-in, so it must match what clients connect to). Default `http://localhost:8080`.
//! - `BEACON_BIND`: where to listen. Default `0.0.0.0:8080` (put HTTPS in front, see compose.yml).
//! - `BEACON_DATA`: folder for the database. Default `./data`.
//! - `BEACON_ADMIN_ONLY_COMMUNITIES`: `true` to let only server admins create communities. Default `false`.
//! - `BEACON_MAX_FILE_MB`: largest attachment. Default 10.
//! - `BEACON_COMMUNITY_QUOTA_MB`: attachment space per community. Default 2048.
//! - `BEACON_MEDIA_ADDR`: the voice relay's address as clients reach it (`server:port`, UDP). Voice is off
//!   without it.
//! - `BEACON_MEDIA_BIND`: where the voice relay listens. Default `0.0.0.0:7881`.

pub mod api;
pub mod auth;
pub mod db;
pub mod blobs;
pub mod live;
pub mod pairing;
pub mod presence;
pub mod settings;
pub mod util;
pub mod voice;
#[cfg(test)]
mod tests;

use std::sync::Arc;

pub struct Config {
    pub public_url: String,
    pub admin_only_communities: bool,
    /// Where encrypted attachments are kept (`<BEACON_DATA>/blobs`).
    pub blob_dir: std::path::PathBuf,
    /// Largest attachment, in bytes (`BEACON_MAX_FILE_MB`, default 10).
    pub max_file: u64,
    /// Attachment community per community, in bytes (`BEACON_COMMUNITY_QUOTA_MB`, default 2048).
    pub community_quota: u64,
    /// The voice relay's address as clients reach it (`BEACON_MEDIA_ADDR`, `server:port`); voice is off without it.
    pub media_addr: Option<String>,
    /// Where the voice relay listens (`BEACON_MEDIA_BIND`, default `0.0.0.0:7881`).
    pub media_bind: String,
}

pub struct App {
    pub cfg: Config,
    pub db: db::Db,
    pub live: live::Hub,
    pub limits: util::Limits,
    pub voice: voice::Voice,
    /// When each account was last heard from (any signed-in request), for online/offline. Kept in
    /// memory only: after a restart everyone shows offline until their app next checks in.
    pub seen: std::sync::Mutex<std::collections::HashMap<i64, std::time::Instant>>,
}

pub type Shared = Arc<App>;

pub fn app(cfg: Config, db: db::Db) -> Shared {
    let voice = voice::Voice::start(cfg.media_addr.clone(), &cfg.media_bind);
    Arc::new(App { cfg, db, live: live::Hub::default(), limits: util::Limits::default(), seen: Default::default(), voice })
}

/// Serve a server on an already-bound listener (used by the program and by tests).
pub async fn serve(listener: tokio::net::TcpListener, state: Shared) -> std::io::Result<()> {
    axum::serve(listener, api::router(state)).await
}
