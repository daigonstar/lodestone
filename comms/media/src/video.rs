//! Screen shares, through GStreamer and the desktop's screen-cast portal.
//!
//! - Sharing: the portal asks you which screen or window (Comms never sees anything you didn't pick),
//!   PipeWire hands over the picture, and it's scaled to fit 1920x1080 (or 1280x720), 30 frames a
//!   second, and encoded as VP8. Encoded frames go to the call engine, which encrypts them.
//! - Watching: decrypted VP8 frames from a slot are decoded and turned into JPEG pictures for the Comms
//!   page to show. If the decoder loses track (a lost packet), it asks the sharer for a keyframe.
//!
//! Nothing is recorded or kept: each picture is replaced by the next.

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::{AppSink, AppSinkCallbacks, AppSrc};
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

fn pipeline(desc: &str) -> Result<gst::Pipeline, String> {
    gst::parse::launch(desc).map_err(|e| format!("video setup failed: {e}"))?.downcast::<gst::Pipeline>().map_err(|_| "video setup failed".to_string())
}

/// Your screen share, while it lasts.
pub struct ScreenShare {
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

impl ScreenShare {
    /// Ask the desktop which screen or window to share, then start encoding. Each encoded frame goes
    /// to `out` (for the call engine). Fails if you cancel the desktop's dialog.
    pub async fn start(quality: Quality, out: impl Fn(Vec<u8>) + Send + Sync + 'static) -> Result<Self, String> {
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
        Self::encode(&source, (w, h), quality, out, Some(close))
    }

    /// The same encoder fed a moving test pattern instead of your screen (for the media check tool).
    pub fn test_pattern(quality: Quality, out: impl Fn(Vec<u8>) + Send + Sync + 'static) -> Result<Self, String> {
        gst::init().map_err(|e| format!("GStreamer didn't start: {e}"))?;
        let (w, h) = fit(None, quality);
        Self::encode("videotestsrc is-live=true pattern=ball", (w, h), quality, out, None)
    }

    fn encode(source: &str, (w, h): (i32, i32), quality: Quality, out: impl Fn(Vec<u8>) + Send + Sync + 'static, close: Option<tokio::sync::oneshot::Sender<()>>) -> Result<Self, String> {
        let (_, _, bitrate) = quality.bounds();
        // Your own preview: half size, 10 pictures a second (it only shows you what you're sharing).
        let (pw, ph) = ((w / 4 * 2).max(2), (h / 4 * 2).max(2));
        let p = pipeline(&format!(
            "{source} ! videoconvert ! videoscale ! \
             video/x-raw,width={w},height={h},pixel-aspect-ratio=1/1 ! videorate drop-only=true ! video/x-raw,framerate=30/1 ! tee name=t \
             t. ! queue leaky=downstream max-size-buffers=1 ! videorate drop-only=true ! video/x-raw,framerate=10/1 ! videoscale ! \
             video/x-raw,width={pw},height={ph} ! jpegenc quality=75 ! appsink name=preview sync=false max-buffers=1 drop=true \
             t. ! queue leaky=downstream max-size-buffers=2 ! \
             vp8enc name=enc deadline=1 cpu-used=8 threads=4 end-usage=cbr target-bitrate={bitrate} keyframe-max-dist=300 lag-in-frames=0 error-resilient=default ! \
             appsink name=out sync=false max-buffers=4 drop=true"
        ))?;
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

impl Drop for ScreenShare {
    fn drop(&mut self) {
        self.ended.store(true, Ordering::Relaxed);
        let _ = self.pipeline.set_state(gst::State::Null);
        if let Some(c) = self.close.take() {
            let _ = c.send(());
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
