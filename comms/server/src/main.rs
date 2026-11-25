//! The Lodestone Comms host program. Settings are described in lib.rs.

use lodestone_comms_server::{app, api, db, Config};

#[tokio::main]
async fn main() {
    let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
    let data = std::path::PathBuf::from(env("COMMS_DATA", "./data"));
    let mb = |k: &str, d: u64| env(k, &d.to_string()).parse::<u64>().unwrap_or(d) * 1024 * 1024;
    let cfg = Config {
        public_url: env("COMMS_PUBLIC_URL", "http://localhost:8080").trim_end_matches('/').to_string(),
        admin_only_spaces: env("COMMS_ADMIN_ONLY_SPACES", "false") == "true",
        blob_dir: data.join("blobs"),
        max_file: mb("COMMS_MAX_FILE_MB", 10),
        space_quota: mb("COMMS_SPACE_QUOTA_MB", 2048),
        media_addr: std::env::var("COMMS_MEDIA_ADDR").ok().filter(|a| !a.trim().is_empty()),
        media_bind: env("COMMS_MEDIA_BIND", "0.0.0.0:7881"),
    };
    std::fs::create_dir_all(&data).expect("can't create the data folder");
    let db = db::Db::open(&data.join("comms.db")).expect("can't open the database");
    if let Some(code) = db.setup_code_if_new().expect("database error") {
        println!("Lodestone Comms: no accounts yet. Register the first (admin) account with this setup code:");
        println!("    {code}");
        println!("It works once. (A restart before anyone registers makes a new one.)");
    }
    let bind = env("COMMS_BIND", "0.0.0.0:8080");
    println!("Lodestone Comms listening on {bind}, public address {}", cfg.public_url);
    let state = app(cfg, db);
    lodestone_comms_server::blobs::sweep(&state);
    let listener = tokio::net::TcpListener::bind(&bind).await.expect("can't listen on COMMS_BIND");
    axum::serve(listener, api::router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .expect("server error");
}
