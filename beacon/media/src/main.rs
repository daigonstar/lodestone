//! `lodestone-beacon-mediacheck`: phase 1 checks for Beacon calls (see the voice and video plan).
//! Run inside the Beacon Flatpak:  flatpak run --command=lodestone-beacon-mediacheck io.github.daigonstar.Lodestone.Beacon [mode]
//!
//! - `check` (default): every GStreamer element calls need is present; Opus encode/decode speed with
//!   echo cancellation and noise suppression; the microphone's *level* for two seconds (nothing is
//!   recorded or kept).
//! - `devices`: your microphones and speakers, with the names to pass to `--mic` and `--speakers`.
//! - `loopback [seconds] [--mic NAME] [--speakers NAME]`: hear yourself through the full chain (mic ->
//!   echo cancellation and noise suppression -> Opus -> back to your speakers), with a live level meter.
//!   Without --mic/--speakers it uses your system's defaults. Use headphones to judge it.
//! - `voice [seconds] [--mic NAME] [--speakers NAME] [--open] [--silent]`: the real call sound code
//!   (voice gate, Opus, jitter buffer, mixing, echo cancellation) with your audio played straight back
//!   to you, as one other person in a call would hear it. Shows the level and when the gate is open.
//!   `--open` sends all the time instead of on voice activity; `--silent` plays into nothing.
//! - `share-sound [seconds]`: the call's sound (silently, so the call's voices can be taken out) plus
//!   a screen share's sound from your default output, encoded as it would be sent. Shows frames per
//!   second. Nothing is recorded or sent.
//! - `video [seconds] [720p]`: the screen share encoder (1080p30 VP8 by default) fed a moving test
//!   pattern, end-to-end encrypted and decrypted, then decoded into pictures as a viewer would. Shows
//!   frame rates and sizes. Nothing is captured from your screen.
//! - `camera [seconds]`: ask the desktop for the camera (it may ask you to allow it), list the cameras
//!   it offers, then measure capture and 640x360 VP8 encoding speed. Nothing is saved or sent.
//! - `screen [seconds]`: share a screen or window through the desktop's portal (it asks you which),
//!   then measure capture and VP8 encoding speed. Nothing is saved or sent anywhere.

use gstreamer as gst;
use gstreamer::prelude::*;
#[cfg(target_os = "linux")]
use std::os::fd::IntoRawFd;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[cfg(not(windows))]
const NEEDED: &[(&str, &str)] = &[
    ("pulsesrc", "microphone"),
    ("pulsesink", "speakers"),
    ("webrtcdsp", "echo cancellation, noise suppression"),
    ("webrtcechoprobe", "echo reference for the canceller"),
    ("opusenc", "voice encoding"),
    ("opusdec", "voice decoding"),
    ("pipewiresrc", "screen and camera capture"),
    ("vp8enc", "video encoding (software)"),
    ("vp8dec", "video decoding"),
    ("jpegenc", "video preview in the Beacon page"),
    ("volume", "speaker volume"),
    ("audiomixer", "mixing everyone's voices"),
    ("appsrc", "call audio and video in"),
    ("appsink", "call audio and video out"),
    ("videoconvert", "video conversion"),
    ("videoscale", "video scaling"),
    ("videorate", "video frame rate"),
    ("jpegdec", "camera pictures (MJPEG)"),
    ("level", "speaking indicator"),
];
#[cfg(windows)]
const NEEDED: &[(&str, &str)] = &[
    ("wasapi2src", "microphone"),
    ("wasapi2sink", "speakers"),
    ("webrtcdsp", "echo cancellation, noise suppression"),
    ("webrtcechoprobe", "echo reference for the canceller"),
    ("opusenc", "voice encoding"),
    ("opusdec", "voice decoding"),
    ("d3d11screencapturesrc", "screen capture"),
    ("mfvideosrc", "camera"),
    ("vp8enc", "video encoding (software)"),
    ("vp8dec", "video decoding"),
    ("jpegenc", "video pictures in the Beacon page"),
    ("volume", "speaker volume"),
    ("audiomixer", "mixing everyone's voices"),
    ("appsrc", "call audio and video in"),
    ("appsink", "call audio and video out"),
    ("videoconvert", "video conversion"),
    ("videoscale", "video scaling"),
    ("videorate", "video frame rate"),
    ("jpegdec", "camera pictures (MJPEG)"),
];
const OPTIONAL: &[(&str, &str)] = &[("vah264enc", "hardware H.264 encoding (VA-API)"), ("vaav1enc", "hardware AV1 encoding (VA-API)"), ("dav1ddec", "AV1 decoding")];

fn pipeline(desc: &str) -> Result<gst::Pipeline, String> {
    gst::parse::launch(desc).map_err(|e| format!("can't build `{desc}`: {e}"))?.downcast::<gst::Pipeline>().map_err(|_| "not a pipeline".into())
}

/// Run a pipeline until end-of-stream, an error, or `limit`; calls `on_msg` for element messages.
fn run(p: &gst::Pipeline, limit: Duration, mut on_msg: impl FnMut(&gst::StructureRef)) -> Result<Duration, String> {
    let bus = p.bus().ok_or("no bus")?;
    p.set_state(gst::State::Playing).map_err(|e| format!("can't start: {e:?}"))?;
    let t = Instant::now();
    let mut result = Ok(());
    while t.elapsed() < limit {
        let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(100)) else { continue };
        match msg.view() {
            gst::MessageView::Eos(_) => break,
            gst::MessageView::Error(e) => {
                result = Err(format!("{} ({})", e.error(), e.debug().unwrap_or_default()));
                break;
            }
            gst::MessageView::Element(el) => {
                if let Some(s) = el.structure() {
                    on_msg(s);
                }
            }
            _ => {}
        }
    }
    let took = t.elapsed();
    let _ = p.set_state(gst::State::Null);
    result.map(|_| took)
}

fn check() -> Result<(), String> {
    println!("GStreamer {}", gst::version_string());
    let mut missing = 0;
    for (e, what) in NEEDED {
        let ok = gst::ElementFactory::find(e).is_some();
        missing += usize::from(!ok);
        println!("  {} {e:<16} {what}", if ok { "ok " } else { "MISSING" });
    }
    for (e, what) in OPTIONAL {
        println!("  {} {e:<16} {what}", if gst::ElementFactory::find(e).is_some() { "ok " } else { "-- " });
    }

    // 10 seconds of 48 kHz mono speech-band audio through the processing and codec chain.
    let p = pipeline(
        "audiotestsrc wave=pink-noise num-buffers=1000 samplesperbuffer=480 ! audio/x-raw,rate=48000,channels=1 ! audioconvert ! \
         webrtcdsp echo-cancel=false noise-suppression=true ! opusenc bitrate=32000 frame-size=20 ! opusdec ! fakesink sync=false",
    )?;
    let took = run(&p, Duration::from_secs(30), |_| {})?;
    println!("  Opus + noise suppression: 10 s of audio processed in {:.0} ms ({:.0}x real time)", took.as_secs_f64() * 1000.0, 10.0 / took.as_secs_f64());

    // The microphone's level for two seconds: proves capture works in this sandbox. Nothing is kept.
    #[cfg(not(windows))]
    let mic = "pulsesrc";
    #[cfg(windows)]
    let mic = "wasapi2src";
    let p = pipeline(&format!("{mic} ! audioconvert ! level interval=200000000 post-messages=true ! fakesink sync=false"))?;
    let mut peak = f64::MIN;
    let res = run(&p, Duration::from_secs(2), |s| {
        if s.name() == "level" {
            if let Ok(v) = s.get::<gst::glib::ValueArray>("peak") {
                for x in v.iter() {
                    if let Ok(db) = x.get::<f64>() {
                        peak = peak.max(db);
                    }
                }
            }
        }
    });
    match res {
        Ok(_) if peak > f64::MIN => println!("  microphone: working (loudest {peak:.0} dB; talk to see it rise)"),
        Ok(_) => println!("  microphone: opened, but no level readings"),
        Err(e) => println!("  microphone: FAILED: {e}"),
    }
    if missing > 0 {
        return Err(format!("{missing} element(s) missing"));
    }
    Ok(())
}

/// A device name is passed to GStreamer inside quotes, so it may only contain safe characters.
#[cfg(target_os = "linux")]
fn device_arg(name: Option<&String>) -> Result<String, String> {
    match name {
        None => Ok(String::new()),
        Some(n) if !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || "._-:@".contains(c)) => Ok(format!(" device=\"{n}\"")),
        Some(n) => Err(format!("not a device name: {n}")),
    }
}

/// Microphones and speakers the system offers, with the names to choose them by.
fn devices() -> Result<(), String> {
    for d in lodestone_beacon_media::audio::devices()? {
        println!("  {:<10} {}\n             --{} {}", d.kind, d.label, if d.kind == "microphone" { "mic" } else { "speakers" }, d.name);
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn loopback(secs: u64, mic: Option<&String>, speakers: Option<&String>) -> Result<(), String> {
    println!("Speak: you'll hear yourself for {secs} s through echo cancellation, noise suppression and Opus.");
    println!("(microphone: {}; speakers: {})", mic.map(String::as_str).unwrap_or("system default"), speakers.map(String::as_str).unwrap_or("system default"));
    let p = pipeline(&format!(
        "pulsesrc{} ! queue ! audioconvert ! audioresample ! audio/x-raw,rate=48000,channels=1 ! \
         level interval=500000000 post-messages=true ! \
         webrtcdsp echo-cancel=true noise-suppression=true noise-suppression-level=high gain-control=true ! \
         opusenc bitrate=32000 frame-size=20 ! opusdec ! audioconvert ! audioresample ! webrtcechoprobe ! pulsesink{}",
        device_arg(mic)?,
        device_arg(speakers)?
    ))?;
    run(&p, Duration::from_secs(secs), |s| {
        if s.name() == "level" {
            if let Ok(v) = s.get::<gst::glib::ValueArray>("peak") {
                let db = v.iter().filter_map(|x| x.get::<f64>().ok()).fold(f64::MIN, f64::max);
                let bars = (((db + 60.0) / 60.0).clamp(0.0, 1.0) * 30.0) as usize;
                println!("  mic {:>5.0} dB |{}{}|", db, "#".repeat(bars), " ".repeat(30 - bars));
            }
        }
    })?;
    println!("Done. If the meter moved but you heard nothing, try another --speakers; if it didn't move, another --mic.");
    Ok(())
}

fn voice(secs: u64, mic: Option<&String>, speakers: Option<&String>, open: bool, silent: bool) -> Result<(), String> {
    use lodestone_beacon_media::audio::{Gate, GstAudio, Levels, Options};
    use lodestone_beacon_media::call::Audio;
    let levels = Arc::new(Levels::default());
    if open {
        levels.set_gate(Gate::Open);
    }
    let opts = Options { mic: mic.cloned(), speakers: speakers.cloned(), silent };
    let mut a = GstAudio::call(&opts, levels.clone())?;
    println!("Speak: you'll hear yourself as a caller would ({}), for {secs} s.", if open { "always sending" } else { "voice activity" });
    let t = Instant::now();
    let (mut sent, mut last) = (0u64, Instant::now());
    while t.elapsed() < Duration::from_secs(secs) {
        for f in a.take_captured() {
            sent += 1;
            a.play(0, &f);
        }
        if last.elapsed() >= Duration::from_millis(500) {
            last = Instant::now();
            let db = levels.mic_db();
            let bars = (((db + 60.0) / 60.0).clamp(0.0, 1.0) * 30.0) as usize;
            println!("  mic {:>5.0} dB |{}{}| {}  frames {sent}", db, "#".repeat(bars), " ".repeat(30 - bars), if levels.talking() { "SENDING" } else { "quiet  " });
        }
        if let Some(e) = levels.error() {
            return Err(e);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    println!("Done: {sent} Opus frames went through the call sound path.");
    Ok(())
}

fn share_sound(secs: u64) -> Result<(), String> {
    use lodestone_beacon_media::audio::{GstAudio, Levels, Options, ShareSound, ShareSoundFrom};
    use lodestone_beacon_media::call::Audio;
    let levels = Arc::new(Levels::default());
    // The call's sound (silent) gives the echo reference that takes the call out on Linux; if no
    // microphone opens, the share's sound is still checked on its own.
    let mut call = match GstAudio::call(&Options { silent: true, ..Default::default() }, levels.clone()) {
        Ok(c) => Some(c),
        Err(e) => {
            println!("  (call sound not started: {e}; checking the share's sound on its own)");
            None
        }
    };
    let frames = Arc::new(AtomicU64::new(0));
    let (f, bytes) = (frames.clone(), Arc::new(AtomicU64::new(0)));
    let b = bytes.clone();
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    let _share = ShareSound::start(ShareSoundFrom::Default, &levels, move |frame| {
        f.fetch_add(1, Ordering::Relaxed);
        b.fetch_add(frame.len() as u64, Ordering::Relaxed);
        let _ = tx.send(frame);
    })?;
    println!("Capturing your default output's sound for {secs} s (play something to see the size grow)...");
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(secs) {
        if let Some(call) = call.as_mut() {
            let _ = call.take_captured();
            // Played back as a watcher would hear it (into nothing: this check is silent).
            while let Ok(frame) = rx.try_recv() {
                call.play_share(0, &frame);
            }
        }
        std::thread::sleep(Duration::from_millis(10));
        if let Some(e) = levels.error() {
            return Err(e);
        }
    }
    let s = secs as f64;
    if call.is_some() {
        println!("  played back with no errors as a watcher would hear it");
    }
    println!("  {:.1} Opus frames/s (50 expected), {:.0} kbps", frames.load(Ordering::Relaxed) as f64 / s, bytes.load(Ordering::Relaxed) as f64 * 8.0 / s / 1000.0);
    if frames.load(Ordering::Relaxed) == 0 {
        return Err("no sound frames came out".into());
    }
    Ok(())
}

fn video(secs: u64, quality: &str) -> Result<(), String> {
    use lodestone_beacon_media::frames;
    use lodestone_beacon_media::video::{Quality, Capture, Viewer};
    let q = Quality::parse(quality).unwrap_or(Quality::Full);
    let viewer = Arc::new(std::sync::Mutex::new(Viewer::new()?));
    let (v, base) = (viewer.clone(), [3u8; 32]);
    let bytes = Arc::new(AtomicU64::new(0));
    let (b, counter) = (bytes.clone(), Arc::new(AtomicU64::new(0)));
    let share = Capture::test_pattern(q, move |frame| {
        // Through the same encryption a call uses, and back.
        let sealed = frames::seal_video(&base, 1, 0, counter.fetch_add(1, Ordering::Relaxed), &frame);
        b.fetch_add(sealed.len() as u64, Ordering::Relaxed);
        if let Some((_, f)) = frames::open_video(|_| Some(base), &sealed) {
            v.lock().unwrap().push(f);
        }
    })?;
    println!("Encoding a {}x{} test pattern for {secs} s...", share.size.0, share.size.1);
    std::thread::sleep(Duration::from_secs(secs));
    let pictures = viewer.lock().unwrap().picture.jpeg.lock().unwrap().as_ref().map(|(n, j)| (*n, j.len())).unwrap_or((0, 0));
    let s = secs as f64;
    println!("  encoded {:.1} frames/s, {:.1} Mbps (target {})", share.frames.load(Ordering::Relaxed) as f64 / s, bytes.load(Ordering::Relaxed) as f64 * 8.0 / s / 1e6, if q == Quality::Full { "5" } else { "2.5" });
    println!("  decoded into {:.1} pictures/s for the page (last picture {} KB)", pictures.0 as f64 / s, pictures.1 / 1024);
    if pictures.0 == 0 {
        return Err("no pictures came out of the decoder".into());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
async fn camera(secs: u64) -> Result<(), String> {
    use ashpd::desktop::camera::Camera;
    let cam = Camera::new().await.map_err(|e| format!("camera portal: {e}"))?;
    println!("camera present: {:?}", cam.is_present().await);
    cam.request_access(Default::default()).await.map_err(|e| e.to_string())?.response().map_err(|e| format!("camera access refused: {e}"))?;
    let remote = cam.open_pipe_wire_remote(Default::default()).await.map_err(|e| e.to_string())?;
    // The device provider and the capture each get their own copy of the connection.
    let list_fd = remote.try_clone().map_err(|e| e.to_string())?.into_raw_fd();
    let fd = remote.into_raw_fd();
    // List cameras through PipeWire's device provider on the portal's connection.
    let provider = gst::DeviceProviderFactory::by_name("pipewiredeviceprovider").ok_or("no pipewire device provider")?;
    if provider.has_property("fd") {
        provider.set_property("fd", list_fd);
    } else {
        println!("  (the device provider can't take the portal's connection; using the default camera)");
    }
    provider.start().map_err(|e| format!("can't list cameras: {e}"))?;
    let cams: Vec<gst::Device> = provider.devices().into_iter().filter(|d| d.has_classes("Video/Source")).collect();
    provider.stop();
    for d in &cams {
        let props = d.properties();
        let serial = props.as_ref().and_then(|p| p.get::<String>("object.serial").ok().or_else(|| p.get::<u64>("object.serial").ok().map(|x| x.to_string())).or_else(|| p.get::<i32>("object.serial").ok().map(|x| x.to_string())));
        println!("  camera: {}  (serial {:?})", d.display_name(), serial);
    }
    let target = cams.first().and_then(|d| d.properties()).and_then(|p| p.get::<String>("object.serial").ok().or_else(|| p.get::<u64>("object.serial").ok().map(|x| x.to_string())).or_else(|| p.get::<i32>("object.serial").ok().map(|x| x.to_string())));
    let captured = Arc::new(AtomicU64::new(0));
    let encoded = Arc::new(AtomicU64::new(0));
    let p = pipeline(&format!(
        "pipewiresrc fd={fd}{} do-timestamp=true ! image/jpeg,width=1280,height=720 ! jpegdec ! tee name=t \
         t. ! queue leaky=downstream max-size-buffers=2 ! fakesink name=raw sync=false signal-handoffs=true \
         t. ! queue leaky=downstream max-size-buffers=2 ! videoconvert ! videoscale ! video/x-raw,width=640,height=360,pixel-aspect-ratio=1/1 ! videorate drop-only=true ! video/x-raw,framerate=30/1 ! \
         vp8enc deadline=1 cpu-used=8 target-bitrate=800000 ! fakesink name=enc sync=false signal-handoffs=true",
        target.map(|t| format!(" target-object={t}")).unwrap_or_default()
    ))?;
    for (name, counter) in [("raw", captured.clone()), ("enc", encoded.clone())] {
        p.by_name(name).ok_or("missing sink")?.connect("handoff", false, move |_| {
            counter.fetch_add(1, Ordering::Relaxed);
            None
        });
    }
    let took = run(&p, Duration::from_secs(secs), |_| {})?;
    let s = took.as_secs_f64();
    println!("  captured {:.1} frames/s; VP8 640x360 encoded {:.1} frames/s", captured.load(Ordering::Relaxed) as f64 / s, encoded.load(Ordering::Relaxed) as f64 / s);
    Ok(())
}

#[cfg(target_os = "linux")]
async fn screen(secs: u64) -> Result<(), String> {
    use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
    use ashpd::desktop::PersistMode;
    println!("Your desktop will ask which screen or window to share...");
    let proxy = Screencast::new().await.map_err(|e| format!("screen-sharing portal: {e}"))?;
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
    let streams = proxy.start(&session, None, Default::default()).await.map_err(|e| e.to_string())?.response().map_err(|e| format!("sharing was cancelled or refused: {e}"))?;
    let stream = streams.streams().first().ok_or("no stream")?.clone();
    let fd = proxy.open_pipe_wire_remote(&session, Default::default()).await.map_err(|e| e.to_string())?.into_raw_fd();
    println!("Sharing node {} ({:?}); measuring for {secs} s...", stream.pipe_wire_node_id(), stream.size());

    let captured = Arc::new(AtomicU64::new(0));
    let encoded = Arc::new(AtomicU64::new(0));
    let p = pipeline(&format!(
        "pipewiresrc fd={fd} path={} do-timestamp=true keepalive-time=1000 ! tee name=t \
         t. ! queue leaky=downstream max-size-buffers=2 ! fakesink name=raw sync=false signal-handoffs=true \
         t. ! queue leaky=downstream max-size-buffers=2 ! videoconvert ! videoscale ! video/x-raw,width=1280,height=720 ! videorate ! video/x-raw,framerate=30/1 ! \
         vp8enc deadline=1 cpu-used=8 target-bitrate=2500000 ! fakesink name=enc sync=false signal-handoffs=true",
        stream.pipe_wire_node_id()
    ))?;
    for (name, counter) in [("raw", captured.clone()), ("enc", encoded.clone())] {
        p.by_name(name).ok_or("missing sink")?.connect("handoff", false, move |_| {
            counter.fetch_add(1, Ordering::Relaxed);
            None
        });
    }
    let took = run(&p, Duration::from_secs(secs), |_| {})?;
    let s = took.as_secs_f64();
    println!(
        "  captured {:.1} frames/s; VP8 720p encoded {:.1} frames/s (target 30)",
        captured.load(Ordering::Relaxed) as f64 / s,
        encoded.load(Ordering::Relaxed) as f64 / s
    );
    println!("  (the capture rate follows what changes on screen; move a window to see it rise)");
    Ok(())
}

#[tokio::main]
async fn main() {
    gst::init().expect("GStreamer didn't start");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let secs = |i: usize, d: u64| args.get(i).and_then(|s| s.parse().ok()).unwrap_or(d).clamp(1, 120);
    let flag = |f: &str| args.iter().position(|a| a == f).and_then(|i| args.get(i + 1));
    let res = match args.first().map(String::as_str).unwrap_or("check") {
        "check" => check(),
        "devices" => devices(),
        #[cfg(target_os = "linux")]
        "loopback" => loopback(secs(1, 15), flag("--mic"), flag("--speakers")),
        "voice" => voice(secs(1, 15), flag("--mic"), flag("--speakers"), args.iter().any(|a| a == "--open"), args.iter().any(|a| a == "--silent")),
        "share-sound" => share_sound(secs(1, 4)),
        "video" => video(secs(1, 5), args.get(2).map(String::as_str).unwrap_or("1080p")),
        #[cfg(target_os = "linux")]
        "camera" => camera(secs(1, 5)).await,
        #[cfg(target_os = "linux")]
        "screen" => screen(secs(1, 10)).await,
        other => Err(format!("unknown mode {other}: use check, devices, loopback, voice, video, camera or screen")),
    };
    if let Err(e) = res {
        eprintln!("FAILED: {e}");
        std::process::exit(1);
    }
}
