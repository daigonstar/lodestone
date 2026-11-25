//! Lodestone Comms, the plug-in: runs the encrypted chat client for each host you've joined, serves its
//! interface on 127.0.0.1 for Lodestone to show, and talks to Lodestone over the bridge (PLUGIN.md).
//!
//! Started by Lodestone (with `--lodestone-bridge`), its standard input and output are the bridge. Run on its own (for testing) it
//! prints the interface address and works without the bridge (no friends list).
//!
//! On Windows there's no Lodestone: Comms opens its own window (WebView2) showing the same page.

// No console window on Windows.
#![cfg_attr(windows, windows_subsystem = "windows")]

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
    /// Wakes the background sync early (a host said something changed).
    pub wake: tokio::sync::Notify,
    /// Unread mentions of you, by "host|channel" (worked out here after decrypting; the host never
    /// knows who was mentioned).
    pub mentions: std::sync::Mutex<std::collections::HashMap<String, u32>>,
}

pub type Shared = Arc<App>;

#[cfg(windows)]
fn data_dir() -> std::path::PathBuf {
    // %APPDATA% is per user and private to them.
    std::path::PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join("Lodestone Comms")
}

#[cfg(not(windows))]
fn data_dir() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME").map(std::path::PathBuf::from).unwrap_or_else(|| {
        std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
    });
    base.join("lodestone-comms")
}

#[cfg(not(windows))]
#[tokio::main]
async fn main() {
    run(None).await;
}

/// Windows: the Comms server runs on a background thread; the main thread shows the window.
#[cfg(windows)]
fn main() {
    // Started by Lodestone: Comms shows inside Lodestone, so no window of its own.
    if std::env::args().any(|a| a == "--lodestone-bridge") {
        let rt = tokio::runtime::Runtime::new().expect("can't start");
        rt.block_on(run(None));
        return;
    }
    use tao::event::{Event, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoop};
    use tao::window::WindowBuilder;
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("can't start");
        rt.block_on(run(Some(tx)));
    });
    let Ok(ui) = rx.recv() else { return };
    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title("Lodestone Comms (test build)")
        .with_inner_size(tao::dpi::LogicalSize::new(1280.0, 800.0))
        .with_min_inner_size(tao::dpi::LogicalSize::new(800.0, 500.0))
        .build(&event_loop)
        .expect("can't open a window");
    // The page's own storage lives with Comms' data, not beside the program.
    let mut context = wry::WebContext::new(Some(data_dir().join("webview")));
    let _view = wry::WebViewBuilder::new_with_web_context(&mut context).with_url(&ui).build(&window).expect("can't show the page (is WebView2 installed?)");
    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::Wait;
        if let Event::WindowEvent { event: WindowEvent::CloseRequested, .. } = event {
            *flow = ControlFlow::Exit;
        }
    });
}

/// Start Comms. `ready` (Windows) receives the page's address for the window.
async fn run(ready: Option<std::sync::mpsc::Sender<String>>) {
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
        wake: tokio::sync::Notify::new(),
        mentions: Default::default(),
        bridge: bridge::Bridge::default(),
        token: token.clone(),
        authority: format!("127.0.0.1:{port}"),
        events,
    });
    let ui = format!("http://127.0.0.1:{port}/#t={token}");
    let bridged = bridge::start(app.clone(), &ui);
    if let Some(tx) = ready {
        let _ = tx.send(ui.clone());
    } else if !bridged {
        eprintln!("Lodestone Comms (no Lodestone bridge): open {ui}");
    }
    hosts::start_sync(app.clone());
    hosts::start_live(app.clone());
    hosts::start_playing(app.clone());
    // Have the sound device list ready before anyone opens the settings.
    tokio::spawn(voice::refresh_devices(app.clone()));
    let server = axum::serve(listener, web::router(app.clone()));
    tokio::select! {
        r = server => { if let Err(e) = r { eprintln!("comms: server stopped: {e}"); } }
        // Lodestone closed the bridge: it's quitting, so we do too.
        _ = app.bridge.closed() => {}
    }
}
