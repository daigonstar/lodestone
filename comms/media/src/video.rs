//! Screen shares and cameras, through GStreamer.
//!
//! - Sharing on Linux: the desktop's screen-cast portal asks you which screen or window (Comms never
//!   sees anything you didn't pick) and PipeWire hands over the picture. On Windows, Comms lists your
//!   monitors and windows for you to pick, and Windows Graphics Capture provides the picture (Windows
//!   shows its yellow border around what's captured). Then the picture is and it's scaled to fit 1920x1080 (or 1280x720), 30 frames a
//!   second, and encoded as VP8. Encoded frames go to the call engine, which encrypts them.
//! - Watching: decrypted VP8 frames from a slot are decoded and turned into JPEG pictures for the Comms
//!   page to show. If the decoder loses track (a lost packet), it asks the sharer for a keyframe.
//!
//! Nothing is recorded or kept: each picture is replaced by the next.

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::{AppSink, AppSinkCallbacks, AppSrc};
#[cfg(target_os = "linux")]
use std::os::fd::IntoRawFd;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Share quality.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    /// 1920x1080 at 30 frames a second, about 5 Mbps (the default).
    Full,
    /// 1280x720 at 30 frames a second, about 2.5 Mbps.
    Light,
}

impl Quality {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "1080p" => Some(Self::Full),
            "720p" => Some(Self::Light),
            _ => None,
        }
    }
    fn bounds(self) -> (i32, i32, u32) {
        match self {
            Self::Full => (1920, 1080, 5_000_000),
            Self::Light => (1280, 720, 2_500_000),
        }
    }
}

/// Fit a picture into the quality's bounds, keeping its shape, never enlarging it, even sizes.
fn fit(size: Option<(i32, i32)>, q: Quality) -> (i32, i32) {
    let (mw, mh, _) = q.bounds();
    let (w, h) = size.filter(|(w, h)| *w > 0 && *h > 0).unwrap_or((mw, mh));
    let scale = (mw as f64 / w as f64).min(mh as f64 / h as f64).min(1.0);
    let even = |x: f64| ((x as i32) / 2 * 2).max(2);
    (even(w as f64 * scale), even(h as f64 * scale))
}

/// Cameras: 1280x720 so they stay sharp in the call view's big tiles; about 1.5 Mbps.
const CAMERA_SIZE: (i32, i32) = (1280, 720);
const CAMERA_BITRATE: u32 = 1_500_000;

fn pipeline(desc: &str) -> Result<gst::Pipeline, String> {
    gst::parse::launch(desc).map_err(|e| format!("video setup failed: {e}"))?.downcast::<gst::Pipeline>().map_err(|_| "video setup failed".to_string())
}

/// Your screen share, while it lasts.
pub struct Capture {
    pipeline: gst::Pipeline,
    encoder: gst::Element,
    /// Ended by itself (you stopped it from the desktop, or the window closed).
    pub ended: Arc<AtomicBool>,
    pub frames: Arc<AtomicU64>,
    pub size: (i32, i32),
    /// What you're sharing, small, for your own preview.
    pub preview: Arc<Picture>,
    close: Option<tokio::sync::oneshot::Sender<()>>,
}

#[cfg(target_os = "linux")]
impl Capture {
    /// Ask the desktop which screen or window to share, then start encoding. Each encoded frame goes
    /// to `out` (for the call engine). Fails if you cancel the desktop's dialog. (`_source` is for
    /// Windows, where Comms does the picking.)
    pub async fn start(quality: Quality, _source: Option<&str>, out: impl Fn(Vec<u8>) + Send + Sync + 'static) -> Result<Self, String> {
        use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
        use ashpd::desktop::PersistMode;
        gst::init().map_err(|e| format!("GStreamer didn't start: {e}"))?;
        let proxy = Screencast::new().await.map_err(|e| format!("The desktop's screen-sharing portal isn't available: {e}"))?;
        let session = proxy.create_session(Default::default()).await.map_err(|e| e.to_string())?;
        proxy
            .select_sources(
                &session,
                SelectSourcesOptions::default()
                    .set_cursor_mode(CursorMode::EMBEDDED)
                    .set_sources(SourceType::MONITOR | SourceType::WINDOW)
                    .set_multiple(false)
                    .set_persist_mode(PersistMode::DoNot),
            )
            .await
            .map_err(|e| e.to_string())?;
        let streams = proxy.start(&session, None, Default::default()).await.map_err(|e| e.to_string())?.response().map_err(|_| "Sharing was cancelled".to_string())?;
        let stream = streams.streams().first().ok_or("The desktop didn't share anything")?.clone();
        let fd = proxy.open_pipe_wire_remote(&session, Default::default()).await.map_err(|e| e.to_string())?.into_raw_fd();

        // Keep the portal session open while sharing; close it when the share stops.
        let (close, closed) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            let _ = closed.await;
            let _ = session.close().await;
            drop(proxy);
        });

        let (w, h) = fit(stream.size(), quality);
        let source = format!("pipewiresrc fd={fd} path={} do-timestamp=true keepalive-time=1000 always-copy=true", stream.pipe_wire_node_id());
        Self::encode(&source, &[], (w, h), quality.bounds().2, out, Some(close))
    }

    /// Your camera, through the desktop's camera portal (it may ask you to allow it): 1280x720, 30
    /// frames a second, about 1.5 Mbps. `which` picks a camera by its id from `cameras()`; the first
    /// otherwise. Cameras differ in what they offer, so a few formats are tried in turn.
    pub async fn camera(which: Option<&str>, out: impl Fn(Vec<u8>) + Send + Sync + Clone + 'static) -> Result<Self, String> {
        let (remote, list) = camera_remote().await?;
        let target = which.filter(|w| list.iter().any(|c| c.id == *w)).map(str::to_string).or_else(|| list.first().map(|c| c.id.clone()));
        let target = target.map(|t| format!(" target-object={t}")).unwrap_or_default();
        let mut last = String::from("No camera found");
        for caps in ["image/jpeg,width=1280,height=720 ! jpegdec", "image/jpeg,width=640,height=480 ! jpegdec", "video/x-raw,width=640,height=480", "video/x-raw"] {
            let fd = remote.try_clone().map_err(|e| e.to_string())?.into_raw_fd();
            let source = format!("pipewiresrc fd={fd}{target} do-timestamp=true ! {caps}");
            match Self::encode(&source, &[], CAMERA_SIZE, CAMERA_BITRATE, out.clone(), None) {
                Ok(c) => {
                    // Some formats are refused only once the camera starts: give it a moment.
                    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
                    if !c.ended.load(Ordering::Relaxed) {
                        return Ok(c);
                    }
                    last = "The camera didn't start".into();
                }
                Err(e) => last = e,
            }
        }
        Err(last)
    }
}

impl Capture {
    /// The encoder fed a moving test pattern instead of your screen (for the media check tool).
    pub fn test_pattern(quality: Quality, out: impl Fn(Vec<u8>) + Send + Sync + 'static) -> Result<Self, String> {
        gst::init().map_err(|e| format!("GStreamer didn't start: {e}"))?;
        let (w, h) = fit(None, quality);
        Self::encode("videotestsrc is-live=true pattern=ball", &[], (w, h), quality.bounds().2, out, None)
    }

    /// Start the encoder on `source` (a pipeline description), after setting `props` (element name,
    /// property, value) on its elements: device ids are set this way, never written into the text.
    fn encode(source: &str, props: &[(&str, &str, String)], (w, h): (i32, i32), bitrate: u32, out: impl Fn(Vec<u8>) + Send + Sync + 'static, close: Option<tokio::sync::oneshot::Sender<()>>) -> Result<Self, String> {
        // Your own preview: half size, 10 pictures a second (it only shows you what you're sharing).
        let (pw, ph) = ((w / 4 * 2).max(2), (h / 4 * 2).max(2));
        let p = pipeline(&format!(
            "{source} ! videoconvert ! videoscale add-borders=true ! \
             video/x-raw,width={w},height={h},pixel-aspect-ratio=1/1 ! videorate drop-only=true ! video/x-raw,framerate=30/1 ! tee name=t \
             t. ! queue leaky=downstream max-size-buffers=1 ! videorate drop-only=true ! video/x-raw,framerate=10/1 ! videoscale ! \
             video/x-raw,width={pw},height={ph} ! jpegenc quality=75 ! appsink name=preview sync=false max-buffers=1 drop=true \
             t. ! queue leaky=downstream max-size-buffers=2 ! \
             vp8enc name=enc deadline=1 cpu-used=8 threads=4 end-usage=cbr target-bitrate={bitrate} keyframe-max-dist=300 lag-in-frames=0 error-resilient=default ! \
             appsink name=out sync=false max-buffers=4 drop=true"
        ))?;
        for (el, prop, value) in props {
            p.by_name(el).ok_or("video setup failed")?.set_property_from_str(prop, value);
        }
        let encoder = p.by_name("enc").ok_or("video setup failed")?;
        let sink = p.by_name("out").and_then(|e| e.downcast::<AppSink>().ok()).ok_or("video setup failed")?;
        let preview = Arc::new(Picture::default());
        let (pv, pic) = (p.by_name("preview").and_then(|e| e.downcast::<AppSink>().ok()).ok_or("video setup failed")?, preview.clone());
        let mut count = 0u64;
        pv.set_callbacks(
            AppSinkCallbacks::builder()
                .new_sample(move |s| {
                    let sample = s.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    if let Some(map) = sample.buffer().and_then(|b| b.map_readable().ok()) {
                        count += 1;
                        *pic.jpeg.lock().unwrap() = Some((count, Arc::new(map.to_vec())));
                        pic.changed.notify_waiters();
                    }
                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );
        let frames = Arc::new(AtomicU64::new(0));
        let n = frames.clone();
        sink.set_callbacks(
            AppSinkCallbacks::builder()
                .new_sample(move |s| {
                    let sample = s.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    if let Some(map) = sample.buffer().and_then(|b| b.map_readable().ok()) {
                        out(map.to_vec());
                        n.fetch_add(1, Ordering::Relaxed);
                    }
                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );
        let ended = Arc::new(AtomicBool::new(false));
        let bus = p.bus().ok_or("video setup failed")?;
        // The share ends by itself on end-of-stream or an error (you stopped it from the desktop).
        let e2 = ended.clone();
        let b2 = bus.clone();
        std::thread::Builder::new()
            .name("comms-share-bus".into())
            .spawn(move || {
                while !e2.load(Ordering::Relaxed) {
                    if b2.timed_pop_filtered(gst::ClockTime::from_mseconds(250), &[gst::MessageType::Eos, gst::MessageType::Error]).is_some() {
                        e2.store(true, Ordering::Relaxed);
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        p.set_state(gst::State::Playing).map_err(|_| "Couldn't start the screen share".to_string())?;
        Ok(Self { pipeline: p, encoder, ended, frames, size: (w, h), preview, close })
    }

    /// Make the next frame a keyframe (someone started watching, or lost a frame).
    pub fn keyframe(&self) {
        let ev = gst::event::CustomUpstream::new(gst::Structure::builder("GstForceKeyUnit").field("all-headers", true).build());
        if let Some(pad) = self.encoder.static_pad("src") {
            pad.send_event(ev);
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.ended.store(true, Ordering::Relaxed);
        let _ = self.pipeline.set_state(gst::State::Null);
        if let Some(c) = self.close.take() {
            let _ = c.send(());
        }
    }
}

/// A camera the desktop offers: its name, and the id to choose it by.
#[derive(Clone, Debug, serde::Serialize)]
pub struct CameraInfo {
    pub label: String,
    pub id: String,
}

/// Access to the cameras through the desktop's portal, and the list of them.
#[cfg(target_os = "linux")]
async fn camera_remote() -> Result<(std::os::fd::OwnedFd, Vec<CameraInfo>), String> {
    use ashpd::desktop::camera::Camera;
    gst::init().map_err(|e| format!("GStreamer didn't start: {e}"))?;
    let cam = Camera::new().await.map_err(|e| format!("The desktop's camera portal isn't available: {e}"))?;
    if !cam.is_present().await.unwrap_or(false) {
        return Err("No camera found".into());
    }
    cam.request_access(Default::default()).await.map_err(|e| e.to_string())?.response().map_err(|_| "Camera access wasn't allowed".to_string())?;
    let remote = cam.open_pipe_wire_remote(Default::default()).await.map_err(|e| e.to_string())?;
    let mut list = Vec::new();
    if let Some(provider) = gst::DeviceProviderFactory::by_name("pipewiredeviceprovider") {
        if provider.has_property("fd") {
            provider.set_property("fd", remote.try_clone().map_err(|e| e.to_string())?.into_raw_fd());
            if provider.start().is_ok() {
                for d in provider.devices().into_iter().filter(|d| d.has_classes("Video/Source")) {
                    let props = d.properties();
                    let id = props.as_ref().and_then(|p| p.get::<String>("object.serial").ok().or_else(|| p.get::<u64>("object.serial").ok().map(|x| x.to_string())).or_else(|| p.get::<i32>("object.serial").ok().map(|x| x.to_string())));
                    if let Some(id) = id.filter(|i| i.chars().all(|c| c.is_ascii_digit())) {
                        if !list.iter().any(|c: &CameraInfo| c.id == id) {
                            list.push(CameraInfo { label: d.display_name().to_string(), id });
                        }
                    }
                }
                provider.stop();
            }
        }
    }
    Ok((remote, list))
}

/// The cameras you can choose from (asks the desktop for camera access first, if it hasn't been given).
#[cfg(target_os = "linux")]
pub async fn cameras() -> Result<Vec<CameraInfo>, String> {
    Ok(camera_remote().await?.1)
}

/// Something you can share: a monitor or a window. On Linux the desktop's portal does the picking, so
/// the list is empty there.
#[derive(Clone, Debug, serde::Serialize)]
pub struct ScreenSource {
    pub id: String,
    pub label: String,
    pub kind: &'static str,
}

#[cfg(not(windows))]
pub fn screen_sources() -> Vec<ScreenSource> {
    Vec::new()
}

#[cfg(windows)]
pub use win::{cameras, screen_sources};

/// Windows: monitors and windows to share (Windows Graphics Capture), and cameras (Media Foundation).
#[cfg(windows)]
mod win {
    use super::*;
    use windows_sys::core::BOOL;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
    use windows_sys::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW};
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindow, GetWindowLongPtrW, GetWindowTextLengthW, GetWindowTextW, IsIconic, IsWindowVisible, GWL_EXSTYLE, GW_OWNER, WS_EX_TOOLWINDOW};

    unsafe extern "system" fn on_monitor(m: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        let list = &mut *(data as *mut Vec<ScreenSource>);
        let mut info: MONITORINFOEXW = std::mem::zeroed();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        let (mut w, mut h) = (0, 0);
        if GetMonitorInfoW(m, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO) != 0 {
            let r = info.monitorInfo.rcMonitor;
            (w, h) = (r.right - r.left, r.bottom - r.top);
        }
        let primary = info.monitorInfo.dwFlags & 1 != 0;
        let n = list.iter().filter(|s| s.kind == "monitor").count() + 1;
        list.push(ScreenSource { id: format!("m:{}", m as usize), label: format!("Screen {n}{} ({w}x{h})", if primary { ", main" } else { "" }), kind: "monitor" });
        1
    }

    unsafe extern "system" fn on_window(hwnd: HWND, data: LPARAM) -> BOOL {
        let list = &mut *(data as *mut Vec<ScreenSource>);
        if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 || !GetWindow(hwnd, GW_OWNER).is_null() {
            return 1;
        }
        if GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW != 0 {
            return 1;
        }
        let mut cloaked: u32 = 0;
        if DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED as u32, &mut cloaked as *mut u32 as *mut _, 4) == 0 && cloaked != 0 {
            return 1;
        }
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return 1;
        }
        let mut buf = vec![0u16; len as usize + 1];
        let got = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        let title = String::from_utf16_lossy(&buf[..got.max(0) as usize]);
        if !title.trim().is_empty() && title != "Program Manager" {
            list.push(ScreenSource { id: format!("w:{}", hwnd as usize), label: title, kind: "window" });
        }
        1
    }

    /// Your monitors, then your open windows.
    pub fn screen_sources() -> Vec<ScreenSource> {
        let mut list: Vec<ScreenSource> = Vec::new();
        unsafe {
            EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(on_monitor), &mut list as *mut _ as LPARAM);
            EnumWindows(Some(on_window), &mut list as *mut _ as LPARAM);
        }
        list
    }

    /// Cameras Media Foundation offers.
    pub async fn cameras() -> Result<Vec<CameraInfo>, String> {
        gst::init().map_err(|e| format!("GStreamer didn't start: {e}"))?;
        // Media Foundation's own lister only (not DirectShow, kernel streaming and the rest).
        let provider = gst::DeviceProviderFactory::by_name("mfdeviceprovider").ok_or("Cameras can't be listed here")?;
        let found = provider.devices();
        let list = found
            .iter()
            .filter(|d| d.has_classes("Video/Source") && d.has_property("device-path"))
            .filter_map(|d| Some(CameraInfo { label: d.display_name().to_string(), id: d.property_value("device-path").get::<String>().ok()? }))
            .collect();
        Ok(list)
    }

    impl Capture {
        /// Share the monitor or window `source` (an id from `screen_sources()`).
        pub async fn start(quality: Quality, source: Option<&str>, out: impl Fn(Vec<u8>) + Send + Sync + 'static) -> Result<Self, String> {
            gst::init().map_err(|e| format!("GStreamer didn't start: {e}"))?;
            let source = source.ok_or("Pick a screen or window to share")?;
            let known = screen_sources();
            if !known.iter().any(|s| s.id == source) {
                return Err("That screen or window isn't there any more".into());
            }
            let (kind, handle) = source.split_once(':').ok_or("Pick a screen or window to share")?;
            let handle: u64 = handle.parse().map_err(|_| "Pick a screen or window to share")?;
            let prop = if kind == "w" { "window-handle" } else { "monitor-handle" };
            let props = [("cap", prop, handle.to_string()), ("cap", "capture-api", "wgc".to_string())];
            let (w, h) = fit(None, quality);
            Self::encode("d3d11screencapturesrc name=cap show-cursor=true ! d3d11download", &props, (w, h), quality.bounds().2, out, None)
        }

        /// Your camera, through Media Foundation: 1280x720, 30 frames a second, about 1.5 Mbps.
        pub async fn camera(which: Option<&str>, out: impl Fn(Vec<u8>) + Send + Sync + Clone + 'static) -> Result<Self, String> {
            let list = cameras().await?;
            let id = which.filter(|w| list.iter().any(|c| c.id == *w)).map(str::to_string).or_else(|| list.first().map(|c| c.id.clone())).ok_or("No camera found")?;
            let mut last = String::from("No camera found");
            for caps in ["image/jpeg,width=1280,height=720 ! jpegdec", "video/x-raw,width=1280,height=720", "video/x-raw,width=640,height=480", "video/x-raw"] {
                let props = [("cam", "device-path", id.clone())];
                match Self::encode(&format!("mfvideosrc name=cam ! {caps}"), &props, CAMERA_SIZE, CAMERA_BITRATE, out.clone(), None) {
                    Ok(c) => {
                        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
                        if !c.ended.load(Ordering::Relaxed) {
                            return Ok(c);
                        }
                        last = "The camera didn't start".into();
                    }
                    Err(e) => last = e,
                }
            }
            Err(last)
        }
    }
}

/// The newest picture of a share you're watching, for the page.
#[derive(Default)]
pub struct Picture {
    pub jpeg: Mutex<Option<(u64, Arc<Vec<u8>>)>>,
    pub changed: tokio::sync::Notify,
}

/// Decodes one share (one slot) into JPEG pictures.
pub struct Viewer {
    pipeline: gst::Pipeline,
    src: AppSrc,
    pub picture: Arc<Picture>,
    /// Set when the decoder needs a keyframe (the app passes it to the call).
    pub wants_keyframe: Arc<AtomicBool>,
    waiting_for_key: bool,
}

impl Viewer {
    pub fn new() -> Result<Self, String> {
        gst::init().map_err(|e| format!("GStreamer didn't start: {e}"))?;
        let p = pipeline(
            "appsrc name=src format=time is-live=true do-timestamp=true caps=video/x-vp8 ! vp8dec ! videoconvert ! \
             queue leaky=downstream max-size-buffers=2 ! jpegenc quality=85 ! appsink name=out sync=false max-buffers=2 drop=true",
        )?;
        let src = p.by_name("src").and_then(|e| e.downcast::<AppSrc>().ok()).ok_or("video setup failed")?;
        let sink = p.by_name("out").and_then(|e| e.downcast::<AppSink>().ok()).ok_or("video setup failed")?;
        let picture = Arc::new(Picture::default());
        let pic = picture.clone();
        let mut n = 0u64;
        sink.set_callbacks(
            AppSinkCallbacks::builder()
                .new_sample(move |s| {
                    let sample = s.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    if let Some(map) = sample.buffer().and_then(|b| b.map_readable().ok()) {
                        n += 1;
                        *pic.jpeg.lock().unwrap() = Some((n, Arc::new(map.to_vec())));
                        pic.changed.notify_waiters();
                    }
                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );
        let wants_keyframe = Arc::new(AtomicBool::new(true));
        // Decoder trouble (frames missing): ask for a keyframe.
        let (bus, wk) = (p.bus().ok_or("video setup failed")?, wants_keyframe.clone());
        let weak = p.downgrade();
        std::thread::Builder::new()
            .name("comms-view-bus".into())
            .spawn(move || loop {
                let Some(p) = weak.upgrade() else { break };
                drop(p);
                if bus.timed_pop_filtered(gst::ClockTime::from_mseconds(250), &[gst::MessageType::Warning, gst::MessageType::Error]).is_some() {
                    wk.store(true, Ordering::Relaxed);
                }
            })
            .map_err(|e| e.to_string())?;
        p.set_state(gst::State::Playing).map_err(|_| "Couldn't start the video decoder".to_string())?;
        Ok(Self { pipeline: p, src, picture, wants_keyframe, waiting_for_key: true })
    }

    /// A decrypted VP8 frame. Frames before the first keyframe are skipped (they can't be decoded).
    pub fn push(&mut self, frame: Vec<u8>) {
        let key = frame.first().is_some_and(|b| b & 1 == 0);
        if self.waiting_for_key && !key {
            self.wants_keyframe.store(true, Ordering::Relaxed);
            return;
        }
        if key {
            self.waiting_for_key = false;
        }
        let _ = self.src.push_buffer(gst::Buffer::from_mut_slice(frame));
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}
