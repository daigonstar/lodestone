//! In-app store browser: the store's own website shown inside Lodestone's main window as a child
//! webview (Tauri multi-webview). Store pages get no access to Lodestone's commands (remote content
//! isn't granted any capability). Steam's install/play links are handed to the Steam client.

use crate::launcher;
use crate::stores;
use tauri::webview::PageLoadEvent;
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Url, WebviewBuilder, WebviewUrl};

const LABEL: &str = "shop";

pub fn home(store: &str) -> Option<&'static str> {
    Some(match store {
        "steam" => "https://store.steampowered.com/",
        "epic" => "https://store.epicgames.com/",
        "gog" => "https://www.gog.com/en/games",
        "battlenet" => "https://shop.battle.net/",
        _ => return None,
    })
}

/// Links a store page hands to its desktop client (e.g. Steam's "Install" button).
fn hand_off(url: &Url) -> bool {
    match url.scheme() {
        "steam" => {
            if let Some(spec) = stores::steam::open_url(url.as_str()) {
                let _ = launcher::spawn(&spec, launcher::log_path("steam-url"));
            }
            true
        }
        // Epic's "launch in the Epic launcher" links: nothing to hand them to.
        "com.epicgames.launcher" | "goggalaxy" | "battlenet" => true,
        _ => false,
    }
}

/// On Linux, Tauri packs child webviews into the window's vertical GTK box (stacked, ignoring
/// position). Move Lodestone's UI and the store into a GtkOverlay instead and place the store
/// with margins, so it sits exactly over the store area of the page.
#[cfg(target_os = "linux")]
mod overlay {
    use gtk::prelude::*;

    pub fn place(store: &gtk::Widget, x: i32, y: i32, w: i32, h: i32) {
        let Some(parent) = store.parent() else { return };
        let overlay = match parent.clone().downcast::<gtk::Overlay>() {
            Ok(o) => o,
            Err(_) => {
                let Ok(vbox) = parent.downcast::<gtk::Box>() else { return };
                let Some(ui) = vbox.children().into_iter().find(|c| c != store && c.is::<webkit2gtk::WebView>()) else { return };
                let overlay = gtk::Overlay::new();
                vbox.remove(&ui);
                vbox.remove(store);
                overlay.add(&ui);
                overlay.add_overlay(store);
                vbox.pack_start(&overlay, true, true, 0);
                overlay.show_all();
                overlay
            }
        };
        // The overlay may not be laid out yet on first use; its parent (the window's box) is.
        let area = overlay.parent().unwrap_or_else(|| overlay.clone().upcast());
        let (ow, oh) = (area.allocated_width(), area.allocated_height());
        store.set_size_request(1, 1);
        store.set_halign(gtk::Align::Fill);
        store.set_valign(gtk::Align::Fill);
        store.set_margin_start(x.max(0));
        store.set_margin_top(y.max(0));
        store.set_margin_end((ow - x - w).max(0));
        store.set_margin_bottom((oh - y - h).max(0));
        store.queue_resize();
    }
}

fn place(wv: &tauri::Webview, x: f64, y: f64, w: f64, h: f64) {
    #[cfg(target_os = "linux")]
    {
        let (x, y, w, h) = (x.round() as i32, y.round() as i32, w.round() as i32, h.round() as i32);
        let _ = wv.with_webview(move |p| {
            use gtk::prelude::Cast;
            let widget: gtk::Widget = p.inner().upcast();
            overlay::place(&widget, x, y, w, h);
        });
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = wv.set_position(LogicalPosition::new(x, y));
        let _ = wv.set_size(LogicalSize::new(w.max(1.0), h.max(1.0)));
    }
}

/// Show the store browser at the given spot in the window (logical px), creating it if needed.
pub fn show(app: &AppHandle, store: &str, x: f64, y: f64, w: f64, h: f64) -> Result<(), String> {
    let url = home(store).ok_or("unknown store")?;
    let pos = LogicalPosition::new(x, y);
    let size = LogicalSize::new(w.max(1.0), h.max(1.0));
    if let Some(wv) = app.get_webview(LABEL) {
        place(&wv, x, y, w, h);
        // Switching stores: go to the new store's home page.
        let on_store = wv.url().ok().and_then(|u| u.host_str().map(String::from));
        let want = Url::parse(url).ok().and_then(|u| u.host_str().map(String::from));
        if on_store != want {
            wv.navigate(Url::parse(url).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        }
        wv.show().map_err(|e| e.to_string())?;
        return Ok(());
    }
    let window = app.get_window("main").ok_or("main window not found")?;
    let handle = app.clone();
    let builder = WebviewBuilder::new(LABEL, WebviewUrl::External(Url::parse(url).map_err(|e| e.to_string())?))
        .on_navigation(|u| !hand_off(u))
        .on_page_load(move |wv, p| {
            if matches!(p.event(), PageLoadEvent::Finished) {
                let _ = handle.emit("shop-page", serde_json::json!({ "url": wv.url().map(|u| u.to_string()).unwrap_or_default() }));
            }
        });
    let wv = window.add_child(builder, pos, size).map_err(|e| e.to_string())?;
    place(&wv, x, y, w, h);
    Ok(())
}

pub fn hide(app: &AppHandle) {
    if let Some(wv) = app.get_webview(LABEL) {
        let _ = wv.hide();
    }
}

pub fn bounds(app: &AppHandle, x: f64, y: f64, w: f64, h: f64) {
    if let Some(wv) = app.get_webview(LABEL) {
        place(&wv, x, y, w, h);
    }
}

/// back / forward / reload / home
pub fn nav(app: &AppHandle, store: &str, action: &str) -> Result<(), String> {
    let wv = app.get_webview(LABEL).ok_or("store isn't open")?;
    match action {
        "back" => wv.eval("history.back()"),
        "forward" => wv.eval("history.forward()"),
        "reload" => wv.eval("location.reload()"),
        "home" => wv.navigate(Url::parse(home(store).ok_or("unknown store")?).map_err(|e| e.to_string())?),
        _ => return Err("unknown action".into()),
    }
    .map_err(|e| e.to_string())
}

/// The page currently shown (for "Open in browser").
pub fn current_url(app: &AppHandle) -> Option<String> {
    app.get_webview(LABEL).and_then(|w| w.url().ok()).map(|u| u.to_string())
}
