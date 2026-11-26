//! Voice channels: a media relay (SFU) built on str0m. It forwards each participant's audio to the
//! others without decoding it: the audio is already end-to-end encrypted frame by frame by the senders
//! (see "Calls" in PROTOCOL.md), so the relay can't hear anything.
//!
//! One UDP socket serves every call. Each participant has one WebRTC connection: one track they send
//! and `SLOTS` tracks they receive. A participant gets a slot in their call; the relay writes their
//! audio into that slot's track on everyone else's connection, so joining and leaving never needs a
//! renegotiation.
//!
//! Screen shares work the same way with video tracks (one sent, `SLOTS` received), except that video is
//! only forwarded to people who chose to watch that share: a share costs upload per viewer, not per
//! person in the call. When someone starts watching, or a viewer's decoder loses track, the relay asks
//! the sharer for a keyframe. Video frames are end-to-end encrypted too (PROTOCOL.md).
//!
//! Webcams have their own video tracks (one sent, `SLOTS` received, after the screen share ones), so
//! someone can share their screen and be on camera at once. A share's sound has audio tracks of its
//! own (after the camera tracks) and, like the picture, only goes to the share's watchers. Cameras go to everyone in the call who has
//! the call view open (`cameras` on), not to people who only have the chat in front of them.
//!
//! Settings: `BEACON_MEDIA_ADDR` (the addresses clients reach, `server:port`, names or IPs, separated by
//! commas: usually the public address first and the LAN address second, for people on the server's own
//! network when the router can't loop back to its public address; voice is off without it) and `BEACON_MEDIA_BIND` (where to listen, default `0.0.0.0:7881`).

use crate::auth::Auth;
use crate::util::{ApiError, ApiResult};
use crate::Shared;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use str0m::change::SdpOffer;
use str0m::media::{Direction, KeyframeRequestKind, MediaKind, Mid};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, Input, Output, Rtc};

/// Receive tracks per connection, and the most people in one voice channel.
pub const SLOTS: usize = 16;

#[derive(Clone, Serialize, Debug)]
pub struct Participant {
    pub username: String,
    pub device: String,
    pub slot: usize,
    pub muted: bool,
    pub deafened: bool,
    /// Sharing their screen (others can choose to watch).
    pub sharing: bool,
    /// Their camera is on.
    pub camera: bool,
    #[serde(skip)]
    pub account: i64,
}

enum Cmd {
    Add(Box<Peer>),
    Remove { channel: String, device: String },
    Deafen { channel: String, device: String, on: bool },
    /// Start or stop sending `device` the video of the participant in `slot`.
    Watch { channel: String, device: String, slot: usize, on: bool },
    /// Start or stop sending `device` everyone's cameras.
    Cameras { channel: String, device: String, on: bool },
}

struct Peer {
    /// The relay address this participant connects to (one of BEACON_MEDIA_ADDR's).
    addr: SocketAddr,
    channel: String,
    device: String,
    slot: usize,
    rtc: Rtc,
    /// The track this participant sends on (receive-only from the relay's side).
    from_client: Option<Mid>,
    /// The tracks this participant receives on, in the order they offered them (slot i = index i).
    to_client: Vec<Mid>,
    /// The video track this participant sends a screen share on.
    from_client_video: Option<Mid>,
    /// The video tracks they receive on (slot i = index i).
    to_client_video: Vec<Mid>,
    /// Slots whose video this participant is watching.
    watching: Vec<usize>,
    /// The video track their camera is sent on, and the ones they receive cameras on.
    from_client_cam: Option<Mid>,
    to_client_cam: Vec<Mid>,
    /// They receive cameras (their call view is open).
    cameras: bool,
    /// The track their share's sound comes on, and the ones they receive shares' sound on.
    from_client_share: Option<Mid>,
    to_client_share: Vec<Mid>,
    deafened: bool,
}

type Calls = Arc<Mutex<HashMap<String, Vec<Participant>>>>;

pub struct Voice {
    /// BEACON_MEDIA_ADDR's addresses, in order.
    addrs: Vec<String>,
    calls: Calls,
    tx: Mutex<Option<mpsc::Sender<Cmd>>>,
}

impl Voice {
    /// Start the relay if `addr` (BEACON_MEDIA_ADDR) is set.
    pub fn start(addr: Option<String>, bind: &str) -> Self {
        let calls: Calls = Default::default();
        let mut tx_slot = None;
        let addrs: Vec<String> = addr.iter().flat_map(|a| a.split(',')).map(|a| a.trim().to_string()).filter(|a| !a.is_empty()).collect();
        if let Some(a) = (!addrs.is_empty()).then(|| addrs.join(", ")) {
            match UdpSocket::bind(bind) {
                Ok(socket) => {
                    let (tx, rx) = mpsc::channel();
                    let calls2 = calls.clone();
                    std::thread::Builder::new()
                        .name("voice-relay".into())
                        .spawn(move || relay(socket, rx, calls2))
                        .expect("voice relay thread");
                    tx_slot = Some(tx);
                    println!("Lodestone Beacon voice relay on udp {bind}, reached at {a}");
                }
                Err(e) => eprintln!("beacon: voice is off: can't listen on udp {bind}: {e}"),
            }
        }
        let addrs = if tx_slot.is_some() { addrs } else { Vec::new() };
        Self { addrs, calls, tx: Mutex::new(tx_slot) }
    }

    pub fn enabled(&self) -> bool {
        !self.addrs.is_empty()
    }

    /// Who's in a voice channel (usernames, each once), for channel lists.
    pub fn usernames(&self, channel: &str) -> Vec<String> {
        let mut v: Vec<String> = self.calls.lock().unwrap().get(channel).map(|p| p.iter().map(|x| x.username.clone()).collect()).unwrap_or_default();
        v.sort();
        v.dedup();
        v
    }

    fn participants(&self, channel: &str) -> Vec<Participant> {
        self.calls.lock().unwrap().get(channel).cloned().unwrap_or_default()
    }

    fn send(&self, cmd: Cmd) {
        if let Some(tx) = self.tx.lock().unwrap().as_ref() {
            let _ = tx.send(cmd);
        }
    }

    /// Take a device out of a call (leaving, or removed from the community).
    pub fn remove(&self, channel: &str, device: &str) {
        if let Some(list) = self.calls.lock().unwrap().get_mut(channel) {
            list.retain(|p| p.device != device);
        }
        self.send(Cmd::Remove { channel: channel.into(), device: device.into() });
    }

    /// Take an account out of every call in these channels (someone removed from a community).
    pub fn remove_account(&self, account: i64, channels: &[String]) {
        let mut gone = Vec::new();
        {
            let mut calls = self.calls.lock().unwrap();
            for ch in channels {
                if let Some(list) = calls.get_mut(ch) {
                    for p in list.iter().filter(|p| p.account == account) {
                        gone.push((ch.clone(), p.device.clone()));
                    }
                    list.retain(|p| p.account != account);
                }
            }
        }
        for (channel, device) in gone {
            self.send(Cmd::Remove { channel, device });
        }
    }
}

/// The relay's address, resolved now (a DDNS name may change).
fn resolve(addr: &str) -> Option<SocketAddr> {
    let all: Vec<SocketAddr> = addr.to_socket_addrs().ok()?.collect();
    all.iter().find(|a| a.is_ipv4()).or(all.first()).copied()
}

/// Tell a community's members that a call changed (someone joined, left, muted, shared, or turned their
/// camera on or off), so their lists update at once.
fn voice_changed(app: &Shared, community: &str, channel: &str) {
    app.live.to_channel(app, community, channel, json!({ "type": "voice", "community": community, "channel": channel }));
}

const AUDIO: u8 = 0;
const SCREEN: u8 = 1;
const CAMERA: u8 = 2;
const SHARE_SOUND: u8 = 3;

/// Ask whoever sends video (their screen, or their camera if `cam`) in `slot` of a call for a keyframe.
fn request_keyframe(peers: &mut [Peer], channel: &str, slot: usize, cam: bool) {
    if let Some(s) = peers.iter_mut().find(|p| p.channel == channel && p.slot == slot) {
        if let Some(mid) = if cam { s.from_client_cam } else { s.from_client_video } {
            if let Some(mut w) = s.rtc.writer(mid) {
                let _ = w.request_keyframe(None, KeyframeRequestKind::Pli);
            }
        }
    }
}

/// The relay loop: one socket, every connection, forwarding between participants of the same call.
fn relay(socket: UdpSocket, rx: mpsc::Receiver<Cmd>, calls: Calls) {
    let mut peers: Vec<Peer> = Vec::new();
    let mut buf = vec![0u8; 2000];
    loop {
        while let Ok(cmd) = rx.try_recv() {
            match cmd {
                Cmd::Add(p) => {
                    peers.retain(|x| !(x.channel == p.channel && x.device == p.device));
                    peers.push(*p);
                }
                Cmd::Remove { channel, device } => peers.retain(|x| !(x.channel == channel && x.device == device)),
                Cmd::Watch { channel, device, slot, on } => {
                    if let Some(p) = peers.iter_mut().find(|x| x.channel == channel && x.device == device) {
                        p.watching.retain(|s| *s != slot);
                        if on {
                            p.watching.push(slot);
                        }
                    }
                    if on {
                        request_keyframe(&mut peers, &channel, slot, false);
                    }
                }
                Cmd::Cameras { channel, device, on } => {
                    if let Some(p) = peers.iter_mut().find(|x| x.channel == channel && x.device == device) {
                        p.cameras = on;
                    }
                    if on {
                        let slots: Vec<usize> = peers.iter().filter(|p| p.channel == channel && p.device != device && p.from_client_cam.is_some()).map(|p| p.slot).collect();
                        for slot in slots {
                            request_keyframe(&mut peers, &channel, slot, true);
                        }
                    }
                }
                Cmd::Deafen { channel, device, on } => {
                    if let Some(p) = peers.iter_mut().find(|x| x.channel == channel && x.device == device) {
                        p.deafened = on;
                    }
                }
            }
        }

        // Drive every connection until it only has a timeout left; collect audio to forward.
        let mut timeout = Instant::now() + Duration::from_millis(50);
        let mut forward = Vec::new();
        let mut keyframes: Vec<(String, usize, bool)> = Vec::new();
        for p in peers.iter_mut() {
            loop {
                match p.rtc.poll_output() {
                    Ok(Output::Timeout(t)) => {
                        timeout = timeout.min(t);
                        break;
                    }
                    Ok(Output::Transmit(t)) => {
                        let _ = socket.send_to(&t.contents, t.destination);
                    }
                    Ok(Output::Event(Event::MediaAdded(m))) => match (m.kind, m.direction) {
                        // Audio tracks: the voice's first, then the share's sound.
                        (MediaKind::Audio, Direction::RecvOnly) if p.from_client.is_none() => p.from_client = Some(m.mid),
                        (MediaKind::Audio, Direction::RecvOnly) => p.from_client_share = Some(m.mid),
                        (MediaKind::Audio, Direction::SendOnly) if p.to_client.len() < SLOTS => p.to_client.push(m.mid),
                        (MediaKind::Audio, Direction::SendOnly) => p.to_client_share.push(m.mid),
                        // Video tracks come in order: the screen share's, then the camera's.
                        (MediaKind::Video, Direction::RecvOnly) if p.from_client_video.is_none() => p.from_client_video = Some(m.mid),
                        (MediaKind::Video, Direction::RecvOnly) => p.from_client_cam = Some(m.mid),
                        (MediaKind::Video, Direction::SendOnly) if p.to_client_video.len() < SLOTS => p.to_client_video.push(m.mid),
                        (MediaKind::Video, Direction::SendOnly) => p.to_client_cam.push(m.mid),
                        _ => {}
                    },
                    Ok(Output::Event(Event::MediaData(d))) if Some(d.mid) == p.from_client => forward.push((p.channel.clone(), p.device.clone(), p.slot, AUDIO, d)),
                    Ok(Output::Event(Event::MediaData(d))) if Some(d.mid) == p.from_client_video => forward.push((p.channel.clone(), p.device.clone(), p.slot, SCREEN, d)),
                    Ok(Output::Event(Event::MediaData(d))) if Some(d.mid) == p.from_client_share => forward.push((p.channel.clone(), p.device.clone(), p.slot, SHARE_SOUND, d)),
                    Ok(Output::Event(Event::MediaData(d))) if Some(d.mid) == p.from_client_cam => forward.push((p.channel.clone(), p.device.clone(), p.slot, CAMERA, d)),
                    // A viewer lost a frame: ask the sharer of that slot for a keyframe.
                    Ok(Output::Event(Event::KeyframeRequest(k))) => {
                        if let Some(slot) = p.to_client_video.iter().position(|m| *m == k.mid) {
                            keyframes.push((p.channel.clone(), slot, false));
                        } else if let Some(slot) = p.to_client_cam.iter().position(|m| *m == k.mid) {
                            keyframes.push((p.channel.clone(), slot, true));
                        }
                    }
                    Ok(Output::Event(_)) => {}
                    Err(_) => {
                        p.rtc.disconnect();
                        break;
                    }
                }
            }
        }
        keyframes.sort();
        keyframes.dedup();
        for (channel, slot, cam) in keyframes {
            request_keyframe(&mut peers, &channel, slot, cam);
        }
        for (channel, from, slot, kind, d) in &forward {
            // Audio to everyone in the call who isn't deafened; a screen share only to people watching
            // it; cameras to people with the call view open.
            let wants = |p: &Peer| match *kind {
                SCREEN | SHARE_SOUND => p.watching.contains(slot),
                CAMERA => p.cameras,
                _ => !p.deafened,
            };
            for p in peers.iter_mut().filter(|p| &p.channel == channel && &p.device != from && wants(p)) {
                let mids = match *kind {
                    SCREEN => &p.to_client_video,
                    CAMERA => &p.to_client_cam,
                    SHARE_SOUND => &p.to_client_share,
                    _ => &p.to_client,
                };
                let Some(mid) = mids.get(*slot).copied() else { continue };
                let Some(w) = p.rtc.writer(mid) else { continue };
                let Some(pt) = w.match_params(d.params) else { continue };
                let _ = w.write(pt, d.network_time, d.time, d.data.clone());
            }
        }
        if !forward.is_empty() {
            continue; // send what was just written before waiting for input
        }

        // Connections that ended (ICE timed out, closed): out of their call.
        let dead: Vec<(String, String)> = peers.iter().filter(|p| !p.rtc.is_alive()).map(|p| (p.channel.clone(), p.device.clone())).collect();
        if !dead.is_empty() {
            peers.retain(|p| p.rtc.is_alive());
            let mut c = calls.lock().unwrap();
            for (ch, dev) in dead {
                if let Some(list) = c.get_mut(&ch) {
                    list.retain(|x| x.device != dev);
                }
            }
        }

        let wait = timeout.saturating_duration_since(Instant::now()).max(Duration::from_millis(1));
        let _ = socket.set_read_timeout(Some(wait));
        match socket.recv_from(&mut buf) {
            Ok((n, source)) => {
                // Each connection sees the packet as sent to the relay address it chose.
                let now = Instant::now();
                for p in peers.iter_mut() {
                    let Ok(contents) = buf[..n].try_into() else { break };
                    let input = Input::Receive(now, Receive { proto: Protocol::Udp, source, destination: p.addr, contents });
                    if p.rtc.accepts(&input) {
                        if p.rtc.handle_input(input).is_err() {
                            p.rtc.disconnect();
                        }
                        break;
                    }
                }
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(e) => eprintln!("beacon: voice socket: {e}"),
        }
        let now = Instant::now();
        for p in peers.iter_mut() {
            if p.rtc.handle_input(Input::Timeout(now)).is_err() {
                p.rtc.disconnect();
            }
        }
    }
}

// ---------- API ----------

/// The voice channel's community, if you're a member and it is a voice channel.
fn voice_channel(app: &Shared, channel: &str, me: &Auth) -> ApiResult<String> {
    let row: Option<(String, String)> = app.db.with(|c| c.query_row("SELECT community, kind FROM channels WHERE id = ?1", [channel], |r| Ok((r.get(0)?, r.get(1)?))).optional())?;
    let (community, kind) = row.ok_or_else(|| ApiError::not_found("No such channel"))?;
    crate::api::require_member(app, &community, me).map_err(|_| ApiError::not_found("No such channel"))?;
    if !app.db.with(|c| crate::api::can_read(c, channel, me.account))? {
        return Err(ApiError::not_found("No such channel"));
    }
    if kind != "voice" {
        return Err(ApiError::bad("That isn't a voice channel"));
    }
    Ok(community)
}

/// `GET /v1/channels/{channel}/voice`: who's in the call (members can look without joining).
pub async fn state(State(app): State<Shared>, me: Auth, Path(channel): Path<String>) -> ApiResult<Json<Value>> {
    voice_channel(&app, &channel, &me)?;
    // The relay's addresses as IPs (resolved now: a DDNS name may change), so a client can choose.
    let addrs = app.voice.addrs.clone();
    let relays: Vec<String> = tokio::task::spawn_blocking(move || addrs.iter().filter_map(|a| resolve(a)).map(|a| a.to_string()).collect()).await.unwrap_or_default();
    Ok(Json(json!({ "enabled": app.voice.enabled(), "relay": relays.first(), "relays": relays, "participants": app.voice.participants(&channel) })))
}

#[derive(Deserialize)]
pub struct JoinReq {
    offer: Value,
    /// Which relay address (from `relays`) the client connects to; the first if left out.
    #[serde(default)]
    relay: Option<String>,
    #[serde(default)]
    muted: bool,
    #[serde(default)]
    deafened: bool,
}

/// `POST /v1/channels/{channel}/voice/join` with a WebRTC offer (one send-only audio track and up to
/// SLOTS receive-only ones). Returns the answer, your slot and who's in the call.
pub async fn join(State(app): State<Shared>, me: Auth, Path(channel): Path<String>, Json(r): Json<JoinReq>) -> ApiResult<Json<Value>> {
    let community = voice_channel(&app, &channel, &me)?;
    if !app.voice.enabled() {
        return Err(ApiError(StatusCode::SERVICE_UNAVAILABLE, "Voice isn't set up on this server yet (BEACON_MEDIA_ADDR)".into()));
    };
    if !app.limits.allow(&format!("voice-join:{}", me.device), 20, Duration::from_secs(60)) {
        return Err(ApiError(StatusCode::TOO_MANY_REQUESTS, "Too many joins; wait a moment".into()));
    }
    let offer: SdpOffer = serde_json::from_value(r.offer).map_err(|_| ApiError::bad("Not a WebRTC offer"))?;
    let addrs = app.voice.addrs.clone();
    let relays: Vec<SocketAddr> = tokio::task::spawn_blocking(move || addrs.iter().filter_map(|a| resolve(a)).collect()).await.unwrap_or_default();
    let public = match &r.relay {
        Some(want) => relays.iter().find(|a| a.to_string() == *want).copied().ok_or_else(|| ApiError::bad("That isn't one of this server's relay addresses"))?,
        None => *relays.first().ok_or_else(|| ApiError(StatusCode::SERVICE_UNAVAILABLE, "The voice relay's address doesn't resolve".into()))?,
    };

    // A slot: the lowest one free in this call (rejoining keeps yours).
    let slot = {
        let mut calls = app.voice.calls.lock().unwrap();
        let list = calls.entry(channel.clone()).or_default();
        list.retain(|p| p.device != me.device);
        let used: Vec<usize> = list.iter().map(|p| p.slot).collect();
        let slot = (0..SLOTS).find(|s| !used.contains(s)).ok_or_else(|| ApiError::bad("This voice channel is full"))?;
        list.push(Participant { username: me.username.clone(), device: me.device.clone(), slot, muted: r.muted, deafened: r.deafened, sharing: false, camera: false, account: me.account });
        slot
    };
    let built = (|| {
        let mut rtc = Rtc::builder().set_ice_lite(true).clear_codecs().enable_opus(true, false).enable_vp8(true).build(Instant::now());
        rtc.add_local_candidate(Candidate::host(public, "udp").map_err(|e| e.to_string())?).ok_or("candidate refused")?;
        let answer = rtc.sdp_api().accept_offer(offer).map_err(|e| e.to_string())?;
        Ok::<_, String>((rtc, answer))
    })();
    let (rtc, answer) = match built {
        Ok(x) => x,
        Err(e) => {
            app.voice.remove(&channel, &me.device);
            return Err(ApiError::bad(format!("Couldn't set up the call: {e}")));
        }
    };
    app.voice.send(Cmd::Add(Box::new(Peer { addr: public, channel: channel.clone(), device: me.device.clone(), slot, rtc, from_client: None, to_client: Vec::new(), from_client_video: None, to_client_video: Vec::new(), watching: Vec::new(), from_client_cam: None, to_client_cam: Vec::new(), cameras: false, from_client_share: None, to_client_share: Vec::new(), deafened: r.deafened })));
    voice_changed(&app, &community, &channel);
    Ok(Json(json!({ "answer": answer, "slot": slot, "participants": app.voice.participants(&channel) })))
}

#[derive(Deserialize)]
pub struct StateReq {
    muted: bool,
    deafened: bool,
}

/// `POST /v1/channels/{channel}/voice/state`: your mute and deafen state (deafened people aren't sent audio).
pub async fn set_state(State(app): State<Shared>, me: Auth, Path(channel): Path<String>, Json(r): Json<StateReq>) -> ApiResult<Json<Value>> {
    let community = voice_channel(&app, &channel, &me)?;
    let found = {
        let mut calls = app.voice.calls.lock().unwrap();
        match calls.get_mut(&channel).and_then(|l| l.iter_mut().find(|p| p.device == me.device)) {
            Some(p) => {
                p.muted = r.muted;
                p.deafened = r.deafened;
                true
            }
            None => false,
        }
    };
    if !found {
        return Err(ApiError::not_found("You aren't in this call"));
    }
    app.voice.send(Cmd::Deafen { channel: channel.clone(), device: me.device, on: r.deafened });
    voice_changed(&app, &community, &channel);
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct ShareReq {
    on: bool,
}

/// `POST /v1/channels/{channel}/voice/share` `{ on }`: say you're sharing your screen (or stopped).
pub async fn share(State(app): State<Shared>, me: Auth, Path(channel): Path<String>, Json(r): Json<ShareReq>) -> ApiResult<Json<Value>> {
    let community = voice_channel(&app, &channel, &me)?;
    let mut calls = app.voice.calls.lock().unwrap();
    let p = calls.get_mut(&channel).and_then(|l| l.iter_mut().find(|p| p.device == me.device)).ok_or_else(|| ApiError::not_found("You aren't in this call"))?;
    p.sharing = r.on;
    voice_changed(&app, &community, &channel);
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct WatchReq {
    slot: usize,
    on: bool,
}

/// `POST /v1/channels/{channel}/voice/watch` `{ slot, on }`: start or stop receiving someone's share.
pub async fn watch(State(app): State<Shared>, me: Auth, Path(channel): Path<String>, Json(r): Json<WatchReq>) -> ApiResult<Json<Value>> {
    voice_channel(&app, &channel, &me)?;
    {
        let calls = app.voice.calls.lock().unwrap();
        let list = calls.get(&channel).ok_or_else(|| ApiError::not_found("You aren't in this call"))?;
        if !list.iter().any(|p| p.device == me.device) {
            return Err(ApiError::not_found("You aren't in this call"));
        }
        if r.on && !list.iter().any(|p| p.slot == r.slot && p.sharing) {
            return Err(ApiError::bad("Nobody is sharing in that slot"));
        }
    }
    app.voice.send(Cmd::Watch { channel, device: me.device, slot: r.slot, on: r.on });
    Ok(Json(json!({ "ok": true })))
}

/// `POST /v1/channels/{channel}/voice/camera` `{ on }`: say your camera is on (or off).
pub async fn camera(State(app): State<Shared>, me: Auth, Path(channel): Path<String>, Json(r): Json<ShareReq>) -> ApiResult<Json<Value>> {
    let community = voice_channel(&app, &channel, &me)?;
    let mut calls = app.voice.calls.lock().unwrap();
    let p = calls.get_mut(&channel).and_then(|l| l.iter_mut().find(|p| p.device == me.device)).ok_or_else(|| ApiError::not_found("You aren't in this call"))?;
    p.camera = r.on;
    voice_changed(&app, &community, &channel);
    Ok(Json(json!({ "ok": true })))
}

/// `POST /v1/channels/{channel}/voice/cameras` `{ on }`: start or stop receiving everyone's cameras
/// (clients turn this on while their call view is open).
pub async fn cameras(State(app): State<Shared>, me: Auth, Path(channel): Path<String>, Json(r): Json<ShareReq>) -> ApiResult<Json<Value>> {
    voice_channel(&app, &channel, &me)?;
    let here = app.voice.calls.lock().unwrap().get(&channel).is_some_and(|l| l.iter().any(|p| p.device == me.device));
    if !here {
        return Err(ApiError::not_found("You aren't in this call"));
    }
    app.voice.send(Cmd::Cameras { channel, device: me.device, on: r.on });
    Ok(Json(json!({ "ok": true })))
}

/// `POST /v1/channels/{channel}/voice/leave`.
pub async fn leave(State(app): State<Shared>, me: Auth, Path(channel): Path<String>) -> ApiResult<Json<Value>> {
    let community = voice_channel(&app, &channel, &me)?;
    app.voice.remove(&channel, &me.device);
    voice_changed(&app, &community, &channel);
    Ok(Json(json!({ "ok": true })))
}

/// The channel IDs of a community (for removing someone from its calls).
pub fn community_channels(app: &Shared, community: &str) -> Vec<String> {
    app.db.with(|c| c.prepare("SELECT id FROM channels WHERE community = ?1")?.query_map([community], |r| r.get(0))?.collect()).unwrap_or_default()
}
