//! A Beacon server on this computer, for trying Beacon out on your own: started from the setup guide's
//! "Start it for me" button, listening only on 127.0.0.1:8080 (so only this computer can reach it), with its
//! data in Beacon's own data folder. It is the same server program as the one friends would use, just run inside
//! this app; voice calls through it are off. Nothing is downloaded.

use lodestone_beacon_server::{api, app, blobs, db, Config};
use std::path::Path;
use std::sync::Mutex;

const ADDRESS: &str = "127.0.0.1:8080";

/// The setup code while the server is running and no account exists yet; `None` before it starts.
static RUNNING: Mutex<Option<Option<String>>> = Mutex::new(None);

/// Start the local server (or report on it if it is already running). Returns the address to connect to and
/// the one-time setup code (only while nobody has registered yet).
pub async fn start(dir: &Path) -> Result<(String, Option<String>), String> {
    if let Some(code) = RUNNING.lock().unwrap().clone() {
        return Ok((ADDRESS.into(), code));
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("Can't create the server's folder: {e}"))?;
    let db = db::Db::open(&dir.join("beacon.db")).map_err(|e| format!("Can't open the server's database: {e}"))?;
    let code = db.setup_code_if_new().map_err(|e| e.to_string())?;
    let listener = tokio::net::TcpListener::bind(ADDRESS).await.map_err(|_| "Port 8080 is already in use on this computer. Is another Beacon server running?".to_string())?;
    let cfg = Config {
        public_url: format!("http://{ADDRESS}"),
        admin_only_communities: false,
        blob_dir: dir.join("blobs"),
        max_file: 10 * 1024 * 1024,
        community_quota: 2048 * 1024 * 1024,
        media_addr: None,
        media_bind: "127.0.0.1:0".into(),
    };
    let state = app(cfg, db);
    blobs::sweep(&state);
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, api::router(state)).await {
            eprintln!("beacon: local server stopped: {e}");
        }
    });
    *RUNNING.lock().unwrap() = Some(code.clone());
    Ok((ADDRESS.into(), code))
}

/// At launch: if a local server was set up before, run it again (it has no accounts to lose its code, so the
/// code is only shown on a first start).
pub async fn resume(dir: &Path) {
    if dir.join("beacon.db").exists() {
        let _ = start(dir).await;
    }
}
