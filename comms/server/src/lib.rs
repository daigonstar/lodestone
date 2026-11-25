//! Lodestone Comms host: stores and relays end-to-end encrypted messages for self-hosted communities.
//! The protocol is in comms/PROTOCOL.md. This program never sees message text: it handles accounts,
//! spaces (servers), channels, invites, MLS key packages and the ordered delivery of MLS messages.
//!
//! Settings (environment):
//! - `COMMS_PUBLIC_URL`: the address clients use, e.g. `https://comms.yourdomain.example` (signed into every
//!   sign-in, so it must match what clients connect to). Default `http://localhost:8080`.
//! - `COMMS_BIND`: where to listen. Default `0.0.0.0:8080` (put HTTPS in front, see compose.yml).
//! - `COMMS_DATA`: folder for the database. Default `./data`.
//! - `COMMS_ADMIN_ONLY_SPACES`: `true` to let only host admins create spaces. Default `false`.
//! - `COMMS_MAX_FILE_MB`: largest attachment. Default 10.
//! - `COMMS_SPACE_QUOTA_MB`: attachment space per server. Default 2048.

pub mod api;
pub mod auth;
pub mod db;
pub mod blobs;
pub mod live;
pub mod pairing;
pub mod presence;
pub mod settings;
pub mod util;
#[cfg(test)]
mod tests;

use std::sync::Arc;

pub struct Config {
    pub public_url: String,
    pub admin_only_spaces: bool,
    /// Where encrypted attachments are kept (`<COMMS_DATA>/blobs`).
    pub blob_dir: std::path::PathBuf,
    /// Largest attachment, in bytes (`COMMS_MAX_FILE_MB`, default 10).
    pub max_file: u64,
    /// Attachment space per server, in bytes (`COMMS_SPACE_QUOTA_MB`, default 2048).
    pub space_quota: u64,
}

pub struct App {
    pub cfg: Config,
    pub db: db::Db,
    pub live: live::Hub,
    pub limits: util::Limits,
    /// When each account was last heard from (any signed-in request), for online/offline. Kept in
    /// memory only: after a restart everyone shows offline until their app next checks in.
    pub seen: std::sync::Mutex<std::collections::HashMap<i64, std::time::Instant>>,
}

pub type Shared = Arc<App>;

pub fn app(cfg: Config, db: db::Db) -> Shared {
    Arc::new(App { cfg, db, live: live::Hub::default(), limits: util::Limits::default(), seen: Default::default() })
}

/// Serve a host on an already-bound listener (used by the program and by tests).
pub async fn serve(listener: tokio::net::TcpListener, state: Shared) -> std::io::Result<()> {
    axum::serve(listener, api::router(state)).await
}
