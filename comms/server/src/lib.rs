//! Lodestone Comms host: stores and relays end-to-end encrypted messages for self-hosted communities.
//! The protocol is in comms/PROTOCOL.md. This program never sees message text: it handles accounts,
//! spaces (servers), channels, invites, MLS key packages and the ordered delivery of MLS messages.
//!
//! Settings (environment):
//! - `COMMS_PUBLIC_URL`: the address clients use, e.g. `https://comms.example.com` (signed into every
//!   sign-in, so it must match what clients connect to). Default `http://localhost:8080`.
//! - `COMMS_BIND`: where to listen. Default `0.0.0.0:8080` (put HTTPS in front, see compose.yml).
//! - `COMMS_DATA`: folder for the database. Default `./data`.
//! - `COMMS_ADMIN_ONLY_SPACES`: `true` to let only host admins create spaces. Default `false`.

pub mod api;
pub mod auth;
pub mod db;
pub mod live;
pub mod settings;
pub mod util;
#[cfg(test)]
mod tests;

use std::sync::Arc;

pub struct Config {
    pub public_url: String,
    pub admin_only_spaces: bool,
}

pub struct App {
    pub cfg: Config,
    pub db: db::Db,
    pub live: live::Hub,
    pub limits: util::Limits,
}

pub type Shared = Arc<App>;

pub fn app(cfg: Config, db: db::Db) -> Shared {
    Arc::new(App { cfg, db, live: live::Hub::default(), limits: util::Limits::default() })
}

/// Serve a host on an already-bound listener (used by the program and by tests).
pub async fn serve(listener: tokio::net::TcpListener, state: Shared) -> std::io::Result<()> {
    axum::serve(listener, api::router(state)).await
}
