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

mod api;
mod auth;
mod db;
mod live;
mod util;
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

#[tokio::main]
async fn main() {
    let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
    let cfg = Config {
        public_url: env("COMMS_PUBLIC_URL", "http://localhost:8080").trim_end_matches('/').to_string(),
        admin_only_spaces: env("COMMS_ADMIN_ONLY_SPACES", "false") == "true",
    };
    let data = std::path::PathBuf::from(env("COMMS_DATA", "./data"));
    std::fs::create_dir_all(&data).expect("can't create the data folder");
    let db = db::Db::open(&data.join("comms.db")).expect("can't open the database");
    if let Some(code) = db.setup_code_if_new().expect("database error") {
        println!("Lodestone Comms: no accounts yet. Register the first (admin) account with this setup code:");
        println!("    {code}");
        println!("It is shown only once and works once.");
    }
    let bind = env("COMMS_BIND", "0.0.0.0:8080");
    println!("Lodestone Comms listening on {bind}, public address {}", cfg.public_url);
    let state = app(cfg, db);
    let listener = tokio::net::TcpListener::bind(&bind).await.expect("can't listen on COMMS_BIND");
    axum::serve(listener, api::router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .expect("server error");
}
