//! Real sound for calls, through GStreamer: the call engine's `Audio`.
//!
//! - Capture: your microphone -> echo cancellation, noise suppression and gain control (`webrtcdsp`) ->
//!   raw 20 ms frames here, where the level is measured and the voice gate decides what's sent (voice
//!   activity, push-to-talk or always on) -> Opus.
//! - Playback: each slot's Opus frames -> a small jitter buffer (timestamps here) -> `opusdec` with packet
//!   loss concealment -> mixed (`audiomixer`) -> the echo canceller's reference (`webrtcechoprobe`) -> your
//!   speakers.
//!
//! Nothing is recorded or kept: frames live only as long as it takes to send or play them.
//! The device elements are the only platform-specific part (PulseAudio/PipeWire on Linux; WASAPI for the
//! later Windows version).

use crate::call::{Audio, SLOTS};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::{AppSink, AppSrc};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(not(windows))]
const MIC: &str = "pulsesrc";
#[cfg(not(windows))]
const SPEAKERS: &str = "pulsesink";
#[cfg(windows)]
const MIC: &str = "wasapi2src";
#[cfg(windows)]
const SPEAKERS: &str = "wasapi2sink";

const RATE: usize = 48_000;
/// Samples in a 20 ms frame (mono).
const FRAME: usize = RATE / 50;
/// The voice gate stays open this many frames after the last loud one (400 ms), so words aren't clipped.
const HANGOVER: u32 = 20;
/// Frames kept from before the gate opens (40 ms), so the start of a word isn't clipped either.
const PREROLL: usize = 2;
/// Playback delay that absorbs network jitter.
const JITTER: Duration = Duration::from_millis(60);
/// If a slot's queue gets this far ahead of real time, it's reset (stops delay building up).
const MAX_AHEAD: Duration = Duration::from_millis(300);

/// How the microphone decides when you're talking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    /// Sends while your level is above the threshold (the default).
    VoiceActivity = 0,
    /// Sends while the push-to-talk key is held.
    PushToTalk = 1,
    /// Always sends.
    Open = 2,
}

impl Gate {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "voice" => Some(Self::VoiceActivity),
            "ptt" => Some(Self::PushToTalk),
            "open" => Some(Self::Open),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::VoiceActivity => "voice",
            Self::PushToTalk => "ptt",
            Self::Open => "open",
        }
    }
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::PushToTalk,
            2 => Self::Open,
            _ => Self::VoiceActivity,
        }
    }
}

/// Shared with the app: settings it can change live, and what the interface shows.
pub struct Levels {
    gate: AtomicU8,
    /// Voice-activity threshold, in tenths of a dB (default -45 dB).
    threshold: AtomicI32,
    /// Held push-to-talk key.
    pub ptt: AtomicBool,
    /// Your microphone's level after processing, in tenths of a dB.
    mic: AtomicI32,
    /// The gate is open (you're being sent, unless muted).
    talking: AtomicBool,
    heard: Mutex<[Option<Instant>; SLOTS]>,
    /// Speaker volume in percent (100 = as received, up to 200).
    volume: AtomicI32,
    /// Each slot's volume in percent (per person: the app maps people to slots).
    slot_volume: Mutex<[i32; SLOTS]>,
    /// A sound to play: 1 = someone joined, 2 = someone left.
    tone: AtomicU8,
    error: Mutex<Option<String>>,
    pub frames_in: AtomicU64,
}

impl Default for Levels {
    fn default() -> Self {
        Self {
            gate: AtomicU8::new(Gate::VoiceActivity as u8),
            threshold: AtomicI32::new(-450),
            ptt: AtomicBool::new(false),
            mic: AtomicI32::new(-1000),
            talking: AtomicBool::new(false),
            heard: Mutex::new([None; SLOTS]),
            volume: AtomicI32::new(100),
            slot_volume: Mutex::new([100; SLOTS]),
            tone: AtomicU8::new(0),
            error: Mutex::new(None),
            frames_in: AtomicU64::new(0),
        }
    }
}

impl Levels {
    pub fn set_gate(&self, g: Gate) {
        self.gate.store(g as u8, Ordering::Relaxed);
    }
    pub fn gate(&self) -> Gate {
        Gate::from_u8(self.gate.load(Ordering::Relaxed))
    }
    /// Threshold in dB (clamped to -80..0).
    pub fn set_threshold(&self, db: f64) {
        self.threshold.store((db.clamp(-80.0, 0.0) * 10.0) as i32, Ordering::Relaxed);
    }
    pub fn threshold(&self) -> f64 {
        self.threshold.load(Ordering::Relaxed) as f64 / 10.0
    }
    /// The microphone's current level in dB (-100 when silent or not running).
    pub fn mic_db(&self) -> f64 {
        self.mic.load(Ordering::Relaxed) as f64 / 10.0
    }
    /// Speaker volume, 0 to 200 percent. Applies at once, also during a call.
    pub fn set_volume(&self, percent: f64) {
        self.volume.store(percent.clamp(0.0, 200.0) as i32, Ordering::Relaxed);
    }
    /// One slot's volume, 0 to 200 percent (the person in it).
    pub fn set_slot_volume(&self, slot: usize, percent: f64) {
        if let Some(v) = self.slot_volume.lock().unwrap().get_mut(slot) {
            *v = percent.clamp(0.0, 200.0) as i32;
        }
    }
    /// Play the "someone joined" (true) or "someone left" (false) sound.
    pub fn chime(&self, joined: bool) {
        self.tone.store(if joined { 1 } else { 2 }, Ordering::Relaxed);
    }
    pub fn talking(&self) -> bool {
        self.talking.load(Ordering::Relaxed)
    }
    /// Slots that sent audio in the last 300 ms (people send only while talking).
    pub fn speaking_slots(&self) -> Vec<usize> {
        let heard = self.heard.lock().unwrap();
        heard.iter().enumerate().filter(|(_, t)| t.is_some_and(|t| t.elapsed() < Duration::from_millis(300))).map(|(i, _)| i).collect()
    }
    /// A device or pipeline failure, if one happened.
    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }
    pub fn clear_error(&self) {
        *self.error.lock().unwrap() = None;
    }
}

/// A microphone or speakers, with the name to choose it by.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Device {
    pub kind: &'static str,
    pub label: String,
    pub name: String,
}

/// Microphones and speakers the system offers (monitors of outputs left out).
/// Only the system's sound server is asked (PulseAudio/PipeWire, or WASAPI): asking every lister
/// GStreamer has also pokes raw hardware (ALSA, OSS), which can hang while a call has the mic open.
#[cfg(not(windows))]
const SOUND_LISTER: &str = "pulsedeviceprovider";
#[cfg(windows)]
const SOUND_LISTER: &str = "wasapi2deviceprovider";

pub fn devices() -> Result<Vec<Device>, String> {
    gst::init().map_err(|e| format!("GStreamer didn't start: {e}"))?;
    let provider = gst::DeviceProviderFactory::by_name(SOUND_LISTER).ok_or("This system's sound devices can't be listed")?;
    // Without starting it, the lister probes once and answers (some, like WASAPI's, only fill in
    // later when started).
    let found = provider.devices();
    let mut list: Vec<Device> = found
        .iter()
        .filter(|d| d.device_class() == "Audio/Source" || d.device_class() == "Audio/Sink")
        // Windows also offers each speaker as a "loopback" recording of what it plays, and the
        // default devices again: neither belongs in the list (System default is its own choice).
        .filter(|d| {
            let props = d.properties();
            let flag = |k: &str| props.as_ref().and_then(|p| p.get::<bool>(k).ok()).unwrap_or(false);
            !flag("wasapi2.device.loopback") && !(flag("device.default") && props.as_ref().and_then(|p| p.get::<String>("device.api").ok()).as_deref() == Some("wasapi2"))
        })
        .map(|d| Device {
            kind: if d.device_class() == "Audio/Source" { "microphone" } else { "speakers" },
            label: d.display_name().to_string(),
            // PulseAudio/PipeWire names its devices; WASAPI gives an endpoint id.
            name: ["internal-name", "device"].iter().find(|p| d.has_property(p)).and_then(|p| d.property_value(p).get::<String>().ok()).unwrap_or_default(),
        })
        .filter(|d| !d.name.is_empty() && !d.name.ends_with(".monitor"))
        .collect();
    list.sort_by(|a, b| (a.kind, &a.label).cmp(&(b.kind, &b.label)));
    Ok(list)
}

/// A sound device id: names on Linux, endpoint ids (with `\\ ? # { }`) on Windows. It's set on the
/// element as a property, never written into a pipeline description.
pub fn valid_device(name: &str) -> bool {
    name.len() <= 512 && name.chars().all(|c| c.is_ascii_graphic() && c != '"' && c != '\'' || c == ' ')
}

/// Choose a device on the element called `el`, if one was picked.
fn set_device(p: &gst::Pipeline, el: &str, name: Option<&str>) -> Result<(), String> {
    match name.filter(|n| !n.is_empty()) {
        None => Ok(()),
        Some(n) if valid_device(n) => {
            let e = p.by_name(el).ok_or("sound setup failed")?;
            e.set_property_from_str("device", n);
            Ok(())
        }
        Some(_) => Err("That isn't a sound device name".into()),
    }
}

fn pipeline(desc: &str) -> Result<gst::Pipeline, String> {
    gst::parse::launch(desc).map_err(|e| format!("sound setup failed: {e}"))?.downcast::<gst::Pipeline>().map_err(|_| "sound setup failed".to_string())
}

fn by_name<T: IsA<gst::Element>>(p: &gst::Pipeline, name: &str) -> Result<T, String> {
    p.by_name(name).and_then(|e| e.downcast::<T>().ok()).ok_or_else(|| format!("sound setup failed: no {name}"))
}

/// Which devices to use. `None` means the system default.
#[derive(Clone, Debug, Default)]
pub struct Options {
    pub mic: Option<String>,
    pub speakers: Option<String>,
    /// Play into nothing instead of the speakers (for checks).
    pub silent: bool,
}

static NEXT: AtomicU64 = AtomicU64::new(1);

/// GStreamer sound for one call (or, without playback, the microphone test in settings).
pub struct GstAudio {
    capture: gst::Pipeline,
    pcm: AppSink,
    encoder: Option<(gst::Pipeline, AppSrc, AppSink)>,
    playback: Option<(gst::Pipeline, Vec<AppSrc>)>,
    outvol: Option<gst::Element>,
    applied_volume: i32,
    slot_vols: Vec<gst::Element>,
    applied_slots: [i32; SLOTS],
    tone_src: Option<AppSrc>,
    levels: Arc<Levels>,
    pending: Vec<i16>,
    preroll: VecDeque<Vec<i16>>,
    open_for: u32,
    encoded: u64,
    next_pts: [Option<gst::ClockTime>; SLOTS],
}

impl GstAudio {
    /// Sound for a call: microphone, encoder and playback.
    pub fn call(opts: &Options, levels: Arc<Levels>) -> Result<Self, String> {
        Self::build(opts, levels, true)
    }

    /// Just the microphone and its level, for the device settings (nothing is encoded or sent).
    pub fn mic_test(opts: &Options, levels: Arc<Levels>) -> Result<Self, String> {
        Self::build(opts, levels, false)
    }

    fn build(opts: &Options, levels: Arc<Levels>, full: bool) -> Result<Self, String> {
        gst::init().map_err(|e| format!("GStreamer didn't start: {e}"))?;
        let probe = format!("commsprobe{}", NEXT.fetch_add(1, Ordering::Relaxed));
        let raw = format!("audio/x-raw,format=S16LE,rate={RATE},channels=1,layout=interleaved");

        // Playback first: the echo canceller looks its reference (the probe) up by name when it starts.
        let playback = if full {
            let mut desc = format!(
                "audiomixer name=mix latency=20000000 ! audioconvert ! audioresample ! {raw} ! volume name=outvol ! webrtcechoprobe name={probe} ! audioconvert ! audioresample ! {} \
                 audiotestsrc wave=silence is-live=true samplesperbuffer=960 ! {raw} ! mix. \
                 appsrc name=tone format=time is-live=true do-timestamp=true caps=\"{raw}\" ! queue ! mix. ",
                if opts.silent { "fakesink sync=true".to_string() } else { format!("{SPEAKERS} name=speakers") }
            );
            for i in 0..SLOTS {
                desc.push_str(&format!(
                    "appsrc name=s{i} format=time is-live=true do-timestamp=false min-latency=0 \
                     caps=\"audio/x-opus,channel-mapping-family=(int)0,channels=(int)1,rate=(int)48000\" ! \
                     opusdec plc=true ! audioconvert ! audioresample ! {raw} ! volume name=v{i} ! queue max-size-time=500000000 ! mix. "
                ));
            }
            let p = pipeline(&desc)?;
            if !opts.silent {
                set_device(&p, "speakers", opts.speakers.as_deref())?;
            }
            let srcs = (0..SLOTS).map(|i| by_name::<AppSrc>(&p, &format!("s{i}"))).collect::<Result<Vec<_>, _>>()?;
            p.set_state(gst::State::Playing).map_err(|_| "Couldn't open your speakers".to_string())?;
            Some((p, srcs))
        } else {
            None
        };

        let dsp = if full {
            format!("webrtcdsp probe={probe} echo-cancel=true noise-suppression=true noise-suppression-level=high gain-control=true")
        } else {
            "webrtcdsp echo-cancel=false noise-suppression=true noise-suppression-level=high gain-control=true".to_string()
        };
        let capture = pipeline(&format!(
            "{MIC} name=mic ! queue ! audioconvert ! audioresample ! {raw} ! {dsp} ! audioconvert ! {raw} ! appsink name=pcm sync=false max-buffers=100 drop=true"
        ))?;
        set_device(&capture, "mic", opts.mic.as_deref())?;
        let pcm = by_name::<AppSink>(&capture, "pcm")?;
        if capture.set_state(gst::State::Playing).is_err() {
            if let Some((p, _)) = &playback {
                let _ = p.set_state(gst::State::Null);
            }
            return Err("Couldn't open your microphone".into());
        }

        let encoder = if full {
            let p = pipeline(&format!(
                "appsrc name=src format=time caps=\"{raw}\" ! opusenc bitrate=32000 frame-size=20 audio-type=voice ! appsink name=out sync=false"
            ))?;
            let (src, out) = (by_name::<AppSrc>(&p, "src")?, by_name::<AppSink>(&p, "out")?);
            p.set_state(gst::State::Playing).map_err(|_| "Couldn't start the voice encoder".to_string())?;
            Some((p, src, out))
        } else {
            None
        };

        let outvol = playback.as_ref().and_then(|(p, _)| p.by_name("outvol"));
        let slot_vols: Vec<gst::Element> = playback.as_ref().map(|(p, _)| (0..SLOTS).filter_map(|i| p.by_name(&format!("v{i}"))).collect()).unwrap_or_default();
        let tone_src = playback.as_ref().and_then(|(p, _)| p.by_name("tone")).and_then(|e| e.downcast::<AppSrc>().ok());
        Ok(Self { capture, pcm, encoder, playback, outvol, applied_volume: -1, slot_vols, applied_slots: [100; SLOTS], tone_src, levels, pending: Vec::new(), preroll: VecDeque::new(), open_for: 0, encoded: 0, next_pts: [None; SLOTS] })
    }

    /// Note the first pipeline error for the interface.
    fn check_bus(&self) {
        let mut pipes = vec![&self.capture];
        pipes.extend(self.playback.as_ref().map(|p| &p.0));
        pipes.extend(self.encoder.as_ref().map(|e| &e.0));
        for p in pipes {
            let Some(bus) = p.bus() else { continue };
            while let Some(msg) = bus.pop_filtered(&[gst::MessageType::Error]) {
                if let gst::MessageView::Error(e) = msg.view() {
                    let mut err = self.levels.error.lock().unwrap();
                    err.get_or_insert_with(|| e.error().to_string());
                }
            }
        }
    }

    /// Pull what the microphone captured and run the level and voice gate over it, frame by frame.
    /// Returns the frames to send.
    fn gate_frames(&mut self) -> Vec<Vec<i16>> {
        while let Some(sample) = self.pcm.try_pull_sample(gst::ClockTime::ZERO) {
            if let Some(map) = sample.buffer().and_then(|b| b.map_readable().ok()) {
                self.pending.extend(map.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])));
            }
        }
        let mut out = Vec::new();
        while self.pending.len() >= FRAME {
            let frame: Vec<i16> = self.pending.drain(..FRAME).collect();
            let power = frame.iter().map(|&s| (s as f64) * (s as f64)).sum::<f64>() / FRAME as f64;
            let db = if power > 0.0 { 10.0 * (power.sqrt() / 32768.0).powi(2).log10() } else { -100.0 }.max(-100.0);
            self.levels.mic.store((db * 10.0) as i32, Ordering::Relaxed);
            let open = match self.levels.gate() {
                Gate::Open => true,
                Gate::PushToTalk => self.levels.ptt.load(Ordering::Relaxed),
                Gate::VoiceActivity => {
                    if db >= self.levels.threshold() {
                        self.open_for = HANGOVER;
                    } else {
                        self.open_for = self.open_for.saturating_sub(1);
                    }
                    self.open_for > 0
                }
            };
            let was = self.levels.talking.swap(open, Ordering::Relaxed);
            if open {
                if !was {
                    out.extend(self.preroll.drain(..));
                }
                out.push(frame);
            } else {
                self.preroll.push_back(frame);
                while self.preroll.len() > PREROLL {
                    self.preroll.pop_front();
                }
            }
        }
        out
    }
}

impl Audio for GstAudio {
    fn take_captured(&mut self) -> Vec<Vec<u8>> {
        self.check_bus();
        let v = self.levels.volume.load(Ordering::Relaxed);
        let wanted = *self.levels.slot_volume.lock().unwrap();
        if wanted != self.applied_slots {
            for (i, e) in self.slot_vols.iter().enumerate() {
                if wanted[i] != self.applied_slots[i] {
                    e.set_property("volume", wanted[i] as f64 / 100.0);
                }
            }
            self.applied_slots = wanted;
        }
        match self.levels.tone.swap(0, Ordering::Relaxed) {
            0 => {}
            t => {
                if let Some(src) = &self.tone_src {
                    let _ = src.push_buffer(chime(t == 1));
                }
            }
        }
        if v != self.applied_volume {
            if let Some(e) = &self.outvol {
                e.set_property("volume", v as f64 / 100.0);
            }
            self.applied_volume = v;
        }
        let frames = self.gate_frames();
        let Some((_, src, out)) = &self.encoder else { return Vec::new() };
        for f in frames {
            let mut buf = gst::Buffer::with_size(FRAME * 2).expect("buffer");
            {
                let b = buf.get_mut().unwrap();
                // Continuous timestamps for the encoder (it doesn't care about the gaps between bursts).
                b.set_pts(gst::ClockTime::from_mseconds(self.encoded * 20));
                b.set_duration(gst::ClockTime::from_mseconds(20));
                let mut map = b.map_writable().expect("map");
                for (d, s) in map.chunks_exact_mut(2).zip(&f) {
                    d.copy_from_slice(&s.to_le_bytes());
                }
            }
            self.encoded += 1;
            let _ = src.push_buffer(buf);
        }
        let mut encoded = Vec::new();
        while let Some(sample) = out.try_pull_sample(gst::ClockTime::ZERO) {
            if let Some(map) = sample.buffer().and_then(|b| b.map_readable().ok()) {
                encoded.push(map.to_vec());
            }
        }
        encoded
    }

    fn play(&mut self, slot: usize, frame: &[u8]) {
        let Some((p, srcs)) = &self.playback else { return };
        let Some(src) = srcs.get(slot) else { return };
        self.levels.heard.lock().unwrap()[slot] = Some(Instant::now());
        self.levels.frames_in.fetch_add(1, Ordering::Relaxed);
        let Some(now) = p.current_running_time() else { return };
        // A small jitter buffer: queue frames back to back, starting JITTER from now; start again
        // from now after a pause, or if the queue has run too far ahead.
        let earliest = now + gst::ClockTime::from_nseconds(JITTER.as_nanos() as u64);
        let pts = match self.next_pts[slot] {
            Some(t) if t >= now && t <= now + gst::ClockTime::from_nseconds(MAX_AHEAD.as_nanos() as u64) => t,
            _ => earliest,
        };
        self.next_pts[slot] = Some(pts + gst::ClockTime::from_mseconds(20));
        let mut buf = gst::Buffer::from_slice(frame.to_vec());
        {
            let b = buf.get_mut().unwrap();
            b.set_pts(pts);
            b.set_duration(gst::ClockTime::from_mseconds(20));
        }
        let _ = src.push_buffer(buf);
    }
}

impl Drop for GstAudio {
    fn drop(&mut self) {
        let _ = self.capture.set_state(gst::State::Null);
        if let Some((p, _, _)) = &self.encoder {
            let _ = p.set_state(gst::State::Null);
        }
        if let Some((p, _)) = &self.playback {
            let _ = p.set_state(gst::State::Null);
        }
        self.levels.talking.store(false, Ordering::Relaxed);
        self.levels.mic.store(-1000, Ordering::Relaxed);
    }
}

/// A short two-note chime: rising when someone joins, falling when someone leaves. Made here (no sound
/// files), quiet, with soft edges so it doesn't click.
fn chime(joined: bool) -> gst::Buffer {
    let notes: [f64; 2] = if joined { [660.0, 880.0] } else { [880.0, 587.0] };
    let per = RATE / 10; // 100 ms a note
    let mut pcm: Vec<u8> = Vec::with_capacity(per * 2 * 2);
    for (n, f) in notes.iter().enumerate() {
        for i in 0..per {
            let t = i as f64 / RATE as f64;
            let fade = (i.min(per - i) as f64 / (RATE as f64 * 0.01)).min(1.0);
            let s = (2.0 * std::f64::consts::PI * f * t).sin() * 0.18 * fade * if n == 1 { 0.9 } else { 1.0 };
            pcm.extend_from_slice(&((s * 32767.0) as i16).to_le_bytes());
        }
    }
    gst::Buffer::from_mut_slice(pcm)
}
