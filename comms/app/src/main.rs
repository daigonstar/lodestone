//! Lodestone Comms, the plug-in: runs the encrypted chat client for each host you've joined, serves its
//! interface on 127.0.0.1 for Lodestone to show, and talks to Lodestone over the bridge (PLUGIN.md).
//!
//! Started by Lodestone (with `--lodestone-bridge`), its standard input and output are the bridge. Run on its own (for testing) it
//! prints the interface address and works without the bridge (no friends list).

mod bridge;
mod hosts;
mod voice;
mod web;

use std::sync::Arc;

pub struct App {
    pub hosts: hosts::Hosts,
    pub bridge: bridge::Bridge,
    /// Per-launch secret the interface sends with every request.
    pub token: String,
    /// "127.0.0.1:<port>": requests with any other Host header are refused (stops DNS-rebinding pages).
    pub authority: String,
    pub events: tokio::sync::broadcast::Sender<String>,
    pub voice: voice::Voice,
}

pub type Shared = Arc<App>;

fn data_dir() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME").map(std::path::PathBuf::from).unwrap_or_else(|| {
        std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
    });
    base.join("lodestone-comms")
}

#[tokio::main]
async fn main() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("can't listen on 127.0.0.1");
    let port = listener.local_addr().unwrap().port();
    let mut secret = [0u8; 32];
    getrandom::getrandom(&mut secret).expect("no system randomness");
    let token = base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, secret);
    let (events, _) = tokio::sync::broadcast::channel(256);
    let hosts = hosts::Hosts::load(data_dir());
    let voice = voice::Voice::load(hosts.prefs_file().with_file_name("voice.json"));
    let app: Shared = Arc::new(App {
        hosts,
        voice,
        bridge: bridge::Bridge::default(),
        token: token.clone(),
        authority: format!("127.0.0.1:{port}"),
        events,
    });
    let ui = format!("http://127.0.0.1:{port}/#t={token}");
    let bridged = bridge::start(app.clone(), &ui);
    if !bridged {
        eprintln!("Lodestone Comms (no Lodestone bridge): open {ui}");
    }
    hosts::start_sync(app.clone());
    let server = axum::serve(listener, web::router(app.clone()));
    tokio::select! {
        r = server => { if let Err(e) = r { eprintln!("comms: server stopped: {e}"); } }
        // Lodestone closed the bridge: it's quitting, so we do too.
        _ = app.bridge.closed() => {}
    }
}
