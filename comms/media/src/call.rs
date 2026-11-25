//! The call engine: one WebRTC connection to the host's relay, end-to-end encrypted frames, slots.
//!
//! - `prepare(relay)` binds a UDP socket towards the relay and makes the offer (one send-only audio
//!   track, `SLOTS` receive-only ones, as the relay expects).
//! - `Pending::start(answer, audio, controls)` runs the call on its own thread until stopped.
//! - Captured frames (from `Audio`) are encrypted with this device's key for the current epoch
//!   (`frames::seal`) and sent; frames arriving in a slot are decrypted with the sender's key
//!   (`frames::open`, which picks the epoch's base from the key ring) and handed to `Audio::play`.
//! - Screen shares: encoded VP8 frames the app puts in `Controls::video_out` are encrypted
//!   (`frames::seal_video`) and sent on the video track; video arriving in a slot is decrypted and handed
//!   to `Controls::video_in`. The relay only sends video for shares you chose to watch.
//! - The app keeps `Controls::keys` current from the voice channel's MLS group (and keeps a few older
//!   epochs, for frames sent just before someone joined or left).

use crate::frames;
use std::net::{SocketAddr, UdpSocket};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use str0m::change::{SdpAnswer, SdpPendingOffer};
use str0m::media::{Direction, Frequency, KeyframeRequestKind, MediaKind, MediaTime, Mid};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, Input, Output, Rtc};

/// Receive tracks offered: the relay's slots.
pub const SLOTS: usize = 16;

/// Where the call's sound comes from and goes to (GStreamer in Comms, a stand-in in tests).
pub trait Audio: Send + 'static {
    /// Encoded 20 ms frames captured since the last call.
    fn take_captured(&mut self) -> Vec<Vec<u8>>;
    /// Play an encoded frame from the participant in `slot`.
    fn play(&mut self, slot: usize, frame: &[u8]);
}

/// Call keys: the current epoch's (epoch, own leaf, base), and a few older bases for late frames.
#[derive(Default)]
pub struct KeyRing {
    current: Option<(u32, u32, [u8; 32])>,
    old: Vec<(u32, [u8; 32])>,
}

impl KeyRing {
    pub fn set(&mut self, epoch: u64, leaf: u32, base: [u8; 32]) {
        let epoch = epoch as u32;
        if let Some((e, _, b)) = self.current {
            if e == epoch && b == base {
                self.current = Some((epoch, leaf, base));
                return;
            }
            self.old.push((e, b));
            if self.old.len() > 4 {
                self.old.remove(0);
            }
        }
        self.current = Some((epoch, leaf, base));
    }

    pub fn base_for(&self, epoch: u32) -> Option<[u8; 32]> {
        match self.current {
            Some((e, _, b)) if e == epoch => Some(b),
            _ => self.old.iter().rev().find(|(e, _)| *e == epoch).map(|(_, b)| *b),
        }
    }
}

/// Shared between the app and the call thread.
#[derive(Default)]
pub struct Controls {
    pub muted: AtomicBool,
    pub deafened: AtomicBool,
    pub stop: AtomicBool,
    pub connected: AtomicBool,
    pub keys: Mutex<KeyRing>,
    pub sent: AtomicU64,
    pub received: AtomicU64,
    /// Frames that arrived but couldn't be decrypted (usually: their epoch's key isn't here yet).
    pub undecryptable: AtomicU64,
    /// Encoded VP8 frames to send (your screen share). Kept short by the app.
    pub video_out: Mutex<VecDeque<Vec<u8>>>,
    /// Where decrypted VP8 frames from a slot go.
    pub video_in: Mutex<Option<Box<dyn FnMut(usize, Vec<u8>) + Send>>>,
    /// The relay asked for a keyframe of your share (someone started watching, or lost a frame).
    pub keyframe_wanted: AtomicBool,
    /// Slots whose share needs a keyframe here (the decoder lost track); the call asks the relay.
    pub keyframe_from: Mutex<Vec<usize>>,
    pub video_sent: AtomicU64,
    pub video_received: AtomicU64,
}

pub struct Pending {
    rtc: Rtc,
    socket: UdpSocket,
    pending: SdpPendingOffer,
    send: Mid,
    recv: Vec<Mid>,
    send_video: Mid,
    recv_video: Vec<Mid>,
}

/// Bind a socket towards the relay and make the offer to send to `POST .../voice/join`.
pub fn prepare(relay: SocketAddr) -> std::io::Result<(Pending, serde_json::Value)> {
    let any: SocketAddr = if relay.is_ipv4() { "0.0.0.0:0".parse().unwrap() } else { "[::]:0".parse().unwrap() };
    let socket = UdpSocket::bind(any)?;
    // Connecting picks the local address that routes to the relay; it's our only candidate.
    socket.connect(relay)?;
    let local = socket.local_addr()?;
    let mut rtc = Rtc::builder().clear_codecs().enable_opus(true, false).enable_vp8(true).build(Instant::now());
    let candidate = Candidate::host(local, "udp").map_err(|e| std::io::Error::other(e.to_string()))?;
    rtc.add_local_candidate(candidate);
    let mut api = rtc.sdp_api();
    let send = api.add_media(MediaKind::Audio, Direction::SendOnly, None, None, None);
    let recv: Vec<Mid> = (0..SLOTS).map(|_| api.add_media(MediaKind::Audio, Direction::RecvOnly, None, None, None)).collect();
    let send_video = api.add_media(MediaKind::Video, Direction::SendOnly, None, None, None);
    let recv_video: Vec<Mid> = (0..SLOTS).map(|_| api.add_media(MediaKind::Video, Direction::RecvOnly, None, None, None)).collect();
    let (offer, pending) = api.apply().ok_or_else(|| std::io::Error::other("no offer"))?;
    let json = serde_json::to_value(&offer).map_err(std::io::Error::other)?;
    Ok((Pending { rtc, socket, pending, send, recv, send_video, recv_video }, json))
}

pub struct Call {
    pub controls: Arc<Controls>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Call {
    /// End the call (the thread closes the connection and exits).
    pub fn stop(mut self) {
        self.controls.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Call {
    fn drop(&mut self) {
        self.controls.stop.store(true, Ordering::Relaxed);
    }
}

impl Pending {
    pub fn start(mut self, answer: serde_json::Value, audio: impl Audio, controls: Arc<Controls>) -> Result<Call, String> {
        let answer: SdpAnswer = serde_json::from_value(answer).map_err(|_| "not a WebRTC answer".to_string())?;
        self.rtc.sdp_api().accept_answer(self.pending, answer).map_err(|e| e.to_string())?;
        let c = controls.clone();
        let tracks = Tracks { send: self.send, recv: self.recv, send_video: self.send_video, recv_video: self.recv_video };
        let (rtc, socket) = (self.rtc, self.socket);
        let thread = std::thread::Builder::new().name("comms-call".into()).spawn(move || run(rtc, socket, tracks, audio, c)).map_err(|e| e.to_string())?;
        Ok(Call { controls, thread: Some(thread) })
    }
}

struct Tracks {
    send: Mid,
    recv: Vec<Mid>,
    send_video: Mid,
    recv_video: Vec<Mid>,
}

fn run(mut rtc: Rtc, socket: UdpSocket, t: Tracks, mut audio: impl Audio, c: Arc<Controls>) {
    let Tracks { send, recv, send_video, recv_video } = t;
    let mut buf = vec![0u8; 2000];
    let local = socket.local_addr().ok();
    // One counter for audio and video: both use this device's key, so nonces must never repeat.
    let mut counter = 0u64;
    let mut rtp_frames = 0u64;
    let started = Instant::now();
    while !c.stop.load(Ordering::Relaxed) && rtc.is_alive() {
        let mut timeout = Instant::now() + Duration::from_millis(10);
        loop {
            match rtc.poll_output() {
                Ok(Output::Timeout(t)) => {
                    timeout = timeout.min(t);
                    break;
                }
                Ok(Output::Transmit(t)) => {
                    let _ = socket.send(&t.contents);
                }
                Ok(Output::Event(Event::Connected)) => c.connected.store(true, Ordering::Relaxed),
                Ok(Output::Event(Event::KeyframeRequest(k))) if k.mid == send_video => c.keyframe_wanted.store(true, Ordering::Relaxed),
                Ok(Output::Event(Event::MediaData(d))) if recv_video.contains(&d.mid) => {
                    let slot = recv_video.iter().position(|m| *m == d.mid).unwrap_or(0);
                    let opened = {
                        let keys = c.keys.lock().unwrap();
                        frames::open_video(|e| keys.base_for(e), &d.data)
                    };
                    match opened {
                        Some((_, frame)) => {
                            c.video_received.fetch_add(1, Ordering::Relaxed);
                            if let Some(sink) = c.video_in.lock().unwrap().as_mut() {
                                sink(slot, frame);
                            }
                        }
                        None => {
                            c.undecryptable.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
                Ok(Output::Event(Event::MediaData(d))) => {
                    let Some(slot) = recv.iter().position(|m| *m == d.mid) else { continue };
                    if c.deafened.load(Ordering::Relaxed) {
                        continue;
                    }
                    let opened = {
                        let keys = c.keys.lock().unwrap();
                        frames::open(|e| keys.base_for(e), &d.data)
                    };
                    match opened {
                        Some((_, frame)) => {
                            c.received.fetch_add(1, Ordering::Relaxed);
                            audio.play(slot, &frame);
                        }
                        None => {
                            c.undecryptable.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
                Ok(Output::Event(_)) => {}
                Err(_) => {
                    rtc.disconnect();
                    break;
                }
            }
        }

        // Keyframe requests for shares this device's decoder lost track of.
        for slot in std::mem::take(&mut *c.keyframe_from.lock().unwrap()) {
            if let Some(mut w) = recv_video.get(slot).and_then(|m| rtc.writer(*m)) {
                let _ = w.request_keyframe(None, KeyframeRequestKind::Pli);
            }
        }

        // Your screen share, if any: encrypted frame by frame.
        let video: Vec<Vec<u8>> = c.video_out.lock().unwrap().drain(..).collect();
        if !video.is_empty() && rtc.is_connected() {
            let key = c.keys.lock().unwrap().current;
            let pt = rtc.writer(send_video).and_then(|w| w.payload_params().next().map(|p| p.pt()));
            if let (Some((epoch, leaf, base)), Some(pt)) = (key, pt) {
                let time = MediaTime::new((started.elapsed().as_micros() as u64 * 9) / 100, Frequency::NINETY_KHZ);
                for frame in video {
                    let sealed = frames::seal_video(&base, epoch, leaf, counter, &frame);
                    counter += 1;
                    if let Some(w) = rtc.writer(send_video) {
                        if w.write(pt, Instant::now(), time, sealed).is_ok() {
                            c.video_sent.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
        }

        // Send what was captured: encrypted, unless muted (frames are still taken, so a long mute
        // doesn't release a burst of old audio when you unmute).
        let captured = audio.take_captured();
        let key = c.keys.lock().unwrap().current;
        if !captured.is_empty() && rtc.is_connected() && !c.muted.load(Ordering::Relaxed) {
            let pt = rtc.writer(send).and_then(|w| w.payload_params().next().map(|p| p.pt()));
            if let (Some((epoch, leaf, base)), Some(pt)) = (key, pt) {
                for frame in captured {
                    let sealed = frames::seal(&base, epoch, leaf, counter, &frame);
                    counter += 1;
                    let time = MediaTime::new(rtp_frames * 960, Frequency::FORTY_EIGHT_KHZ);
                    rtp_frames += 1;
                    if let Some(w) = rtc.writer(send) {
                        if w.write(pt, Instant::now(), time, sealed).is_ok() {
                            c.sent.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
                continue; // transmit before waiting
            }
        }

        let wait = timeout.saturating_duration_since(Instant::now()).clamp(Duration::from_millis(1), Duration::from_millis(10));
        let _ = socket.set_read_timeout(Some(wait));
        if let (Ok((n, source)), Some(local)) = (socket.recv_from(&mut buf), local) {
            if let Ok(contents) = buf[..n].try_into() {
                let input = Input::Receive(Instant::now(), Receive { proto: Protocol::Udp, source, destination: local, contents });
                if rtc.handle_input(input).is_err() {
                    rtc.disconnect();
                }
            }
        }
        if rtc.handle_input(Input::Timeout(Instant::now())).is_err() {
            rtc.disconnect();
        }
    }
    rtc.disconnect();
    c.connected.store(false, Ordering::Relaxed);
}
