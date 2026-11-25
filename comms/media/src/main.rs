//! `lodestone-comms-mediacheck`: phase 1 checks for Comms calls (see the voice and video plan).
//! Run inside the Comms Flatpak:  flatpak run --command=lodestone-comms-mediacheck io.github.daigonstar.Lodestone.Comms [mode]
//!
//! - `check` (default): every GStreamer element calls need is present; Opus encode/decode speed with
//!   echo cancellation and noise suppression; the microphone's *level* for two seconds (nothing is
//!   recorded or kept).
//! - `devices`: your microphones and speakers, with the names to pass to `--mic` and `--speakers`.
//! - `loopback [seconds] [--mic NAME] [--speakers NAME]`: hear yourself through the full chain (mic ->
//!   echo cancellation and noise suppression -> Opus -> back to your speakers), with a live level meter.
//!   Without --mic/--speakers it uses your system's defaults. Use headphones to judge it.
//! - `screen [seconds]`: share a screen or window through the desktop's portal (it asks you which),
//!   then measure capture and VP8 encoding speed. Nothing is saved or sent anywhere.

use gstreamer as gst;
use gstreamer::prelude::*;
use std::os::fd::IntoRawFd;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

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
    ("jpegenc", "video preview in the Comms page"),
    ("level", "speaking indicator"),
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
    let p = pipeline("pulsesrc ! audioconvert ! level interval=200000000 post-messages=true ! fakesink sync=false")?;
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
fn device_arg(name: Option<&String>) -> Result<String, String> {
    match name {
        None => Ok(String::new()),
        Some(n) if !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || "._-:@".contains(c)) => Ok(format!(" device=\"{n}\"")),
        Some(n) => Err(format!("not a device name: {n}")),
    }
}

/// Microphones and speakers PulseAudio/PipeWire offers inside this sandbox.
fn devices() -> Result<(), String> {
    let monitor = gst::DeviceMonitor::new();
    monitor.add_filter(Some("Audio/Source"), None);
    monitor.add_filter(Some("Audio/Sink"), None);
    monitor.start().map_err(|e| format!("can't list devices: {e}"))?;
    let mut list: Vec<(String, String, String)> = monitor
        .devices()
        .iter()
        .map(|d| {
            let kind = if d.has_classes("Audio/Source") { "microphone" } else { "speakers" };
            let name = d.property_value("internal-name").get::<String>().unwrap_or_default();
            (kind.to_string(), d.display_name().to_string(), name)
        })
        .filter(|(_, _, n)| !n.ends_with(".monitor"))
        .collect();
    monitor.stop();
    list.sort();
    for (kind, label, name) in list {
        println!("  {kind:<10} {label}\n             --{} {name}", if kind == "microphone" { "mic" } else { "speakers" });
    }
    Ok(())
}

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
        "loopback" => loopback(secs(1, 15), flag("--mic"), flag("--speakers")),
        "screen" => screen(secs(1, 10)).await,
        other => Err(format!("unknown mode {other}: use check, devices, loopback or screen")),
    };
    if let Err(e) = res {
        eprintln!("FAILED: {e}");
        std::process::exit(1);
    }
}
