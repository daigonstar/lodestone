//! Calls in voice channels: one at a time. Joins through the host's relay with the call engine
//! (`lodestone_comms_media::call`), sound from GStreamer (`audio`), and keeps the call's keys current
//! from the channel's encrypted group while you're in it.
//!
//! Device and microphone settings are kept in `voice.json` beside the app's other preferences.

use crate::Shared;
use lodestone_comms_client::Client;
use lodestone_comms_media::audio::{self, Gate, GstAudio, Levels, Options};
use lodestone_comms_media::call::{self, Call, Controls};
use lodestone_comms_media::video::{self, Capture, Picture, Quality, Viewer};
use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Your sound settings.
#[derive(Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub mic: String,
    #[serde(default)]
    pub speakers: String,
    /// "voice" (voice activity, the default), "ptt" (push to talk) or "open".
    #[serde(default = "voice")]
    pub mode: String,
    /// Voice activity threshold in dB.
    #[serde(default = "threshold")]
    pub threshold: f64,
    /// Your camera's id (from the camera list); the first camera if empty.
    #[serde(default)]
    pub camera: String,
    /// Speaker volume in percent (100 = as received; up to 200).
    #[serde(default = "hundred")]
    pub volume: f64,
    /// Each person's volume in percent, by "host|username" (100 when not set).
    #[serde(default)]
    pub user_volumes: HashMap<String, f64>,
    /// Screen share quality: "1080p" (the default) or "720p".
    #[serde(default = "full")]
    pub share_quality: String,
}

fn hundred() -> f64 {
    100.0
}

fn full() -> String {
    "1080p".into()
}

fn voice() -> String {
    "voice".into()
}
fn threshold() -> f64 {
    -45.0
}

impl Default for Settings {
    fn default() -> Self {
        Self { mic: String::new(), speakers: String::new(), mode: voice(), threshold: threshold(), camera: String::new(), volume: hundred(), user_volumes: HashMap::new(), share_quality: full() }
    }
}

impl Settings {
    fn options(&self) -> Options {
        Options { mic: Some(self.mic.clone()).filter(|s| !s.is_empty()), speakers: Some(self.speakers.clone()).filter(|s| !s.is_empty()), silent: false }
    }
}

struct Session {
    host: String,
    channel: String,
    client: Arc<Client>,
    call: Call,
    /// Who's in the call (from the host), refreshed every few seconds.
    participants: Arc<Mutex<Value>>,
    slot: u64,
    video: Arc<Video>,
}

/// Your screen share and camera, the shares you're watching and the cameras you see.
#[derive(Default)]
struct Video {
    share: Mutex<Option<Capture>>,
    viewers: Mutex<HashMap<usize, Viewer>>,
    camera: Mutex<Option<Capture>>,
    /// Cameras are coming in (your call view is open).
    cams_on: AtomicBool,
    cams: Mutex<HashMap<usize, Viewer>>,
}

struct MicTest {
    stop: Arc<AtomicBool>,
}

pub struct Voice {
    pub levels: Arc<Levels>,
    session: tokio::sync::Mutex<Option<Session>>,
    test: Mutex<Option<MicTest>>,
    settings: Mutex<Settings>,
    file: std::path::PathBuf,
    devices: Mutex<Option<Vec<audio::Device>>>,
    listing: AtomicBool,
}

impl Voice {
    pub fn load(file: std::path::PathBuf) -> Self {
        let settings: Settings = std::fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let levels = Arc::new(Levels::default());
        apply(&levels, &settings);
        Self { levels, session: Default::default(), test: Mutex::new(None), settings: Mutex::new(settings), file, devices: Mutex::new(None), listing: AtomicBool::new(false) }
    }

    pub fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    /// Save new settings. Mode and threshold apply at once; devices from the next call (or mic test).
    pub fn set_settings(&self, mut s: Settings) -> Result<(), String> {
        s.volume = if s.volume.is_finite() { s.volume.clamp(0.0, 200.0) } else { 100.0 };
        s.threshold = if s.threshold.is_finite() { s.threshold.clamp(-80.0, 0.0) } else { -45.0 };
        s.user_volumes.retain(|k, v| k.len() <= 300 && v.is_finite());
        for v in s.user_volumes.values_mut() {
            *v = v.clamp(0.0, 200.0);
        }
        if s.user_volumes.len() > 500 {
            return Err("Too many saved volumes".into());
        }
        if Gate::parse(&s.mode).is_none() {
            return Err("Unknown microphone mode".into());
        }
        if !audio::valid_device(&s.camera) {
            return Err("That isn't a camera id".into());
        }
        if Quality::parse(&s.share_quality).is_none() {
            return Err("Unknown screen share quality".into());
        }
        for d in [&s.mic, &s.speakers] {
            if !audio::valid_device(d) {
                return Err("That isn't a sound device name".into());
            }
        }
        apply(&self.levels, &s);
        std::fs::write(&self.file, serde_json::to_string(&s).unwrap_or_default()).map_err(|e| e.to_string())?;
        *self.settings.lock().unwrap() = s;
        Ok(())
    }
}

fn apply(levels: &Levels, s: &Settings) {
    levels.set_gate(Gate::parse(&s.mode).unwrap_or(Gate::VoiceActivity));
    levels.set_threshold(s.threshold);
    levels.set_volume(s.volume);
}

/// Sound devices, from a list kept up to date in the background: opening the settings never waits on
/// the sound system (a slow or stuck one used to freeze the dialog). Listing is given up on after 5 s.
pub async fn devices(app: &Shared) -> Result<Vec<audio::Device>, String> {
    let cached = app.voice.devices.lock().unwrap().clone();
    let refresh = refresh_devices(app.clone());
    match cached {
        Some(list) => {
            tokio::spawn(refresh);
            Ok(list)
        }
        None => refresh.await,
    }
}

/// List sound devices again (at most one listing at a time), and keep the result.
pub async fn refresh_devices(app: Shared) -> Result<Vec<audio::Device>, String> {
    if app.voice.listing.swap(true, Ordering::Relaxed) {
        return app.voice.devices.lock().unwrap().clone().ok_or_else(|| "Still looking for your sound devices; try again in a moment".to_string());
    }
    let job = tokio::task::spawn_blocking(audio::devices);
    let got = match tokio::time::timeout(Duration::from_secs(5), job).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("Your sound system didn't answer, so the device list may be out of date".into()),
    };
    // A listing that timed out may still finish; until then, don't start another.
    if got.is_ok() || !matches!(&got, Err(e) if e.starts_with("Your sound system")) {
        app.voice.listing.store(false, Ordering::Relaxed);
    }
    if let Ok(list) = &got {
        *app.voice.devices.lock().unwrap() = Some(list.clone());
    }
    got
}

/// Start or stop the microphone test (the level meter in settings). Nothing is sent or kept.
pub fn mic_test(app: &Shared, on: bool) -> Result<(), String> {
    let v = &app.voice;
    if let Some(t) = v.test.lock().unwrap().take() {
        t.stop.store(true, Ordering::Relaxed);
    }
    if !on {
        return Ok(());
    }
    let stop = Arc::new(AtomicBool::new(false));
    let (opts, levels, s) = (v.settings().options(), v.levels.clone(), stop.clone());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("comms-mictest".into())
        .spawn(move || {
            let mut a = match GstAudio::mic_test(&opts, levels) {
                Ok(a) => {
                    let _ = tx.send(Ok(()));
                    a
                }
                Err(e) => {
                    let _ = tx.send(Err(e));
                    return;
                }
            };
            // Stops by itself after two minutes, in case the page forgets.
            let until = std::time::Instant::now() + Duration::from_secs(120);
            while !s.load(Ordering::Relaxed) && std::time::Instant::now() < until {
                let _ = call::Audio::take_captured(&mut a);
                std::thread::sleep(Duration::from_millis(20));
            }
        })
        .map_err(|e| e.to_string())?;
    rx.recv_timeout(Duration::from_secs(5)).map_err(|_| "The microphone didn't start".to_string())??;
    *v.test.lock().unwrap() = Some(MicTest { stop });
    Ok(())
}

/// Join a voice channel (leaving any call you're in).
pub async fn join(app: &Shared, c: Arc<Client>, host: &str, channel: &str, muted: bool, deafened: bool) -> Result<Value, String> {
    leave(app).await;
    let _ = mic_test(app, false);
    // Make sure this device has the channel's newest keys before deriving the call's.
    c.sync_channel(channel).await.map_err(|e| e.to_string())?;
    let info = c.voice_info(channel).await.map_err(|e| e.to_string())?;
    if info["enabled"] == json!(false) {
        return Err("This host doesn't have calls turned on".into());
    }
    let mut relays: Vec<SocketAddr> = info["relays"].as_array().into_iter().flatten().filter_map(|r| r.as_str()?.parse().ok()).collect();
    if relays.is_empty() {
        relays.extend(info["relay"].as_str().and_then(|r| r.parse::<SocketAddr>().ok()));
    }
    if relays.is_empty() {
        return Err("The host didn't say where its call relay is".into());
    }
    order_relays(&mut relays);
    let (epoch, leaf, base) = c.voice_secret(channel).map_err(|e| e.to_string())?;
    app.voice.levels.clear_error();

    // Try the relay's addresses in turn until one connects (a LAN address only works on the host's
    // own network; a public one from inside it only if the router loops back).
    let mut last_err = String::from("Couldn't reach the call relay");
    let mut started = None;
    for (i, relay) in relays.iter().enumerate() {
        let last = i + 1 == relays.len();
        let (pending, offer) = match call::prepare(*relay) {
            Ok(x) => x,
            Err(e) => {
                last_err = format!("Couldn't reach the call relay: {e}");
                continue;
            }
        };
        let joined = c.voice_join(channel, &relay.to_string(), offer, muted, deafened).await.map_err(|e| e.to_string())?;
        let controls = Arc::new(Controls::default());
        controls.keys.lock().unwrap().set(epoch, leaf, base);
        controls.muted.store(muted, Ordering::Relaxed);
        controls.deafened.store(deafened, Ordering::Relaxed);
        let audio = match GstAudio::call(&app.voice.settings().options(), app.voice.levels.clone()) {
            Ok(a) => a,
            Err(e) => {
                let _ = c.voice_leave(channel).await;
                return Err(e);
            }
        };
        let call = match pending.start(joined["answer"].clone(), audio, controls.clone()) {
            Ok(call) => call,
            Err(e) => {
                let _ = c.voice_leave(channel).await;
                return Err(e);
            }
        };
        if last {
            started = Some((call, joined, controls));
            break;
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(4);
        while !controls.connected.load(Ordering::Relaxed) && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if controls.connected.load(Ordering::Relaxed) {
            started = Some((call, joined, controls));
            break;
        }
        let _ = tokio::task::spawn_blocking(move || call.stop()).await;
        last_err = format!("The call relay at {relay} didn't answer");
    }
    let Some((call, joined, controls)) = started else {
        let _ = c.voice_leave(channel).await;
        return Err(last_err);
    };
    let participants = Arc::new(Mutex::new(joined["participants"].clone()));
    let video = Arc::new(Video::default());
    let v2 = video.clone();
    *controls.video_in.lock().unwrap() = Some(Box::new(move |slot, cam, frame| {
        if !cam {
            if let Some(view) = v2.viewers.lock().unwrap().get_mut(&slot) {
                view.push(frame);
            }
            return;
        }
        // Cameras: a decoder per person, made when their first frame arrives.
        if !v2.cams_on.load(Ordering::Relaxed) {
            return;
        }
        let mut cams = v2.cams.lock().unwrap();
        if !cams.contains_key(&slot) {
            match Viewer::new() {
                Ok(v) => {
                    cams.insert(slot, v);
                }
                Err(_) => return,
            }
        }
        if let Some(view) = cams.get_mut(&slot) {
            view.push(frame);
        }
    }));
    let session = Session {
        video: video.clone(),
        host: host.to_string(),
        channel: channel.to_string(),
        client: c.clone(),
        call,
        participants: participants.clone(),
        slot: joined["slot"].as_u64().unwrap_or(0),
    };
    *app.voice.session.lock().await = Some(session);

    // While in the call: follow the channel's key changes (people joining or leaving its group) and
    // who's in the call.
    let (ch, ctl) = (channel.to_string(), controls);
    // Keyframes: the relay asks for one of your share; a viewer here asks for one of someone's.
    let (ctl2, app2) = (ctl.clone(), app.clone());
    tokio::spawn(async move {
        let mut last_ask: HashMap<(usize, bool), std::time::Instant> = HashMap::new();
        while !ctl2.stop.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let mut ended = false;
            if let Some(sh) = video.share.lock().unwrap().as_ref() {
                if ctl2.keyframe_wanted.swap(false, Ordering::Relaxed) {
                    sh.keyframe();
                }
                ended = sh.ended.load(Ordering::Relaxed);
            }
            if ended {
                let _ = stop_share(&app2).await;
            }
            let mut cam_ended = false;
            if let Some(cam) = video.camera.lock().unwrap().as_ref() {
                if ctl2.cam_keyframe_wanted.swap(false, Ordering::Relaxed) {
                    cam.keyframe();
                }
                cam_ended = cam.ended.load(Ordering::Relaxed);
            }
            if cam_ended {
                let _ = set_camera(&app2, false).await;
            }
            let mut ask = Vec::new();
            for (cam, list) in [(false, &video.viewers), (true, &video.cams)] {
                for (slot, view) in list.lock().unwrap().iter() {
                    let key = (*slot, cam);
                    let recent = last_ask.get(&key).is_some_and(|t| t.elapsed() < Duration::from_millis(500));
                    if !recent && view.wants_keyframe.swap(false, Ordering::Relaxed) {
                        ask.push(key);
                        last_ask.insert(key, std::time::Instant::now());
                    }
                }
            }
            ctl2.keyframe_from.lock().unwrap().extend(ask);
        }
    });
    let (app3, host3, slot3) = (app.clone(), host.to_string(), joined["slot"].as_u64().unwrap_or(0));
    tokio::spawn(async move {
        let mut tick = 0u32;
        while !ctl.stop.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_secs(1)).await;
            tick += 1;
            if c.sync_channel(&ch).await.is_ok() {
                if let Ok((e, l, b)) = c.voice_secret(&ch) {
                    ctl.keys.lock().unwrap().set(e, l, b);
                }
            }
            if tick % 2 == 0 {
                if let Ok(info) = c.voice_info(&ch).await {
                    update_people(&app3, &host3, slot3, &participants, info["participants"].clone());
                }
            }
        }
    });
    app.voice.levels.chime(true);
    {
        let g = app.voice.session.lock().await;
        if let Some(sess) = g.as_ref() {
            let people = sess.participants.lock().unwrap().clone();
            let vols = app.voice.settings().user_volumes;
            for p in people.as_array().into_iter().flatten() {
                if let (Some(slot), Some(user)) = (p["slot"].as_u64(), p["username"].as_str()) {
                    app.voice.levels.set_slot_volume(slot as usize, vols.get(&format!("{host}|{user}")).copied().unwrap_or(100.0));
                }
            }
        }
    }
    Ok(status(app).await)
}

/// Put relay addresses in the order to try: a private (LAN) address on the same network as the route
/// this PC would use to reach it first, then public ones, then other private ones.
fn order_relays(relays: &mut [SocketAddr]) {
    fn private(ip: std::net::IpAddr) -> bool {
        match ip {
            std::net::IpAddr::V4(v) => v.is_private() || v.is_loopback(),
            std::net::IpAddr::V6(v) => v.is_loopback() || (v.segments()[0] & 0xfe00) == 0xfc00,
        }
    }
    fn same_lan(relay: SocketAddr) -> bool {
        let local = std::net::UdpSocket::bind(if relay.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" }).and_then(|s| s.connect(relay).and_then(|_| s.local_addr()));
        match (local.map(|l| l.ip()), relay.ip()) {
            // Same /24, or same /16 inside 10.0.0.0/8 (home labs often split that into several /24s).
            (Ok(std::net::IpAddr::V4(l)), std::net::IpAddr::V4(r)) => {
                let n = if r.octets()[0] == 10 { 2 } else { 3 };
                r.is_loopback() || l.octets()[..n] == r.octets()[..n]
            }
            (Ok(l), r) => l == r,
            _ => false,
        }
    }
    relays.sort_by_key(|r| match (private(r.ip()), same_lan(*r)) {
        (true, true) => 0,
        (false, _) => 1,
        (true, false) => 2,
    });
}

/// New list of who's in your call: play the join/leave chime for anyone who came or went (not you),
/// and set each person's volume on their slot.
fn update_people(app: &Shared, host: &str, my_slot: u64, store: &Mutex<Value>, people: Value) {
    let others = |v: &Value| -> std::collections::HashSet<String> {
        v.as_array().into_iter().flatten().filter(|p| p["slot"].as_u64() != Some(my_slot)).filter_map(|p| p["device"].as_str().map(str::to_string)).collect()
    };
    let before = others(&store.lock().unwrap());
    let after = others(&people);
    if after.difference(&before).next().is_some() {
        app.voice.levels.chime(true);
    } else if before.difference(&after).next().is_some() {
        app.voice.levels.chime(false);
    }
    let vols = app.voice.settings().user_volumes;
    for p in people.as_array().into_iter().flatten() {
        if let (Some(slot), Some(user)) = (p["slot"].as_u64(), p["username"].as_str()) {
            let v = vols.get(&format!("{host}|{user}")).copied().unwrap_or(100.0);
            app.voice.levels.set_slot_volume(slot as usize, v);
        }
    }
    *store.lock().unwrap() = people;
}

/// Set one person's volume (0 to 200 percent); it applies at once if they're in your call.
pub async fn set_user_volume(app: &Shared, host: &str, username: &str, volume: f64) -> Result<(), String> {
    let mut s = app.voice.settings();
    let key = format!("{host}|{username}");
    if (volume - 100.0).abs() < 0.5 {
        s.user_volumes.remove(&key);
    } else {
        s.user_volumes.insert(key, volume);
    }
    app.voice.set_settings(s)?;
    let g = app.voice.session.lock().await;
    if let Some(sess) = g.as_ref().filter(|x| x.host == host) {
        let people = sess.participants.lock().unwrap().clone();
        drop(g);
        let g = app.voice.session.lock().await;
        if let Some(sess) = g.as_ref() {
            update_people(app, host, sess.slot, &sess.participants, people);
        }
    }
    Ok(())
}

/// A host said a call changed: if it's yours, fetch who's there now (instead of at the next refresh).
pub fn refresh_soon(app: &Shared, channel: &str) {
    let (app, channel) = (app.clone(), channel.to_string());
    tokio::spawn(async move {
        let found = {
            let g = app.voice.session.lock().await;
            g.as_ref().filter(|s| s.channel == channel).map(|s| (s.client.clone(), s.participants.clone(), s.host.clone(), s.slot))
        };
        if let Some((c, participants, host, slot)) = found {
            if let Ok(info) = c.voice_info(&channel).await {
                update_people(&app, &host, slot, &participants, info["participants"].clone());
            }
        }
    });
}

/// Leave the call you're in, if any.
pub async fn leave(app: &Shared) {
    let Some(s) = app.voice.session.lock().await.take() else { return };
    // The "left" chime, heard before the sound closes.
    app.voice.levels.chime(false);
    tokio::time::sleep(Duration::from_millis(250)).await;
    let _ = tokio::task::spawn_blocking(move || s.call.stop()).await;
    let _ = s.client.voice_leave(&s.channel).await;
}

/// Mute or deafen in the call (deafening also mutes, as in most chat apps).
pub async fn set_state(app: &Shared, muted: bool, deafened: bool) -> Result<(), String> {
    let g = app.voice.session.lock().await;
    let s = g.as_ref().ok_or("You're not in a call")?;
    let muted = muted || deafened;
    s.call.controls.muted.store(muted, Ordering::Relaxed);
    s.call.controls.deafened.store(deafened, Ordering::Relaxed);
    s.client.voice_state(&s.channel, muted, deafened).await.map_err(|e| e.to_string())
}

/// The call you're in: who's there, who's speaking, your mute state, the mic level, and how it's going.
pub async fn status(app: &Shared) -> Value {
    let v = &app.voice;
    let s = v.settings();
    let base = json!({
        "mic_db": v.levels.mic_db(),
        "testing": v.test.lock().unwrap().is_some(),
        "settings": s,
        "error": v.levels.error(),
    });
    let g = v.session.lock().await;
    let Some(sess) = g.as_ref() else { return json!({ "call": null, "audio": base }) };
    let ctl = &sess.call.controls;
    let speaking = v.levels.speaking_slots();
    let muted = ctl.muted.load(Ordering::Relaxed);
    let mut people = sess.participants.lock().unwrap().clone();
    let vols = s.user_volumes.clone();
    for p in people.as_array_mut().into_iter().flatten() {
        let slot = p["slot"].as_u64().unwrap_or(u64::MAX);
        let me = slot == sess.slot;
        let talking = if me { v.levels.talking() && !muted } else { speaking.contains(&(slot as usize)) };
        p["speaking"] = json!(talking);
        p["me"] = json!(me);
        let user = p["username"].as_str().unwrap_or_default().to_string();
        p["volume"] = json!(vols.get(&format!("{}|{user}", sess.host)).copied().unwrap_or(100.0));
    }
    json!({
        "call": {
            "host": sess.host,
            "channel": sess.channel,
            "connected": ctl.connected.load(Ordering::Relaxed),
            "muted": muted,
            "deafened": ctl.deafened.load(Ordering::Relaxed),
            "participants": people,
            "sent": ctl.sent.load(Ordering::Relaxed),
            "received": ctl.received.load(Ordering::Relaxed),
            "undecryptable": ctl.undecryptable.load(Ordering::Relaxed),
            "sharing": sess.video.share.lock().unwrap().as_ref().map(|s| json!({ "width": s.size.0, "height": s.size.1, "frames": s.frames.load(Ordering::Relaxed) })),
            "watching": sess.video.viewers.lock().unwrap().keys().collect::<Vec<_>>(),
            "camera": sess.video.camera.lock().unwrap().is_some(),
            "cameras": sess.video.cams.lock().unwrap().keys().collect::<Vec<_>>(),
            "slot": sess.slot,
            "video_sent": ctl.video_sent.load(Ordering::Relaxed),
            "video_received": ctl.video_received.load(Ordering::Relaxed),
        },
        "audio": base,
    })
}

/// Share your screen in the call. On Linux the desktop asks which screen or window; on Windows the
/// page has already asked (`source`, from `screen_sources`).
pub async fn start_share(app: &Shared, source: Option<String>) -> Result<(), String> {
    let (client, channel, controls, video) = {
        let g = app.voice.session.lock().await;
        let s = g.as_ref().ok_or("Join a voice channel first")?;
        if s.video.share.lock().unwrap().is_some() {
            return Ok(());
        }
        (s.client.clone(), s.channel.clone(), s.call.controls.clone(), s.video.clone())
    };
    let quality = Quality::parse(&app.voice.settings().share_quality).unwrap_or(Quality::Full);
    let ctl = controls.clone();
    let share = Capture::start(quality, source.as_deref(), move |frame| {
        let mut q = ctl.video_out.lock().unwrap();
        // If sending falls behind, drop the oldest rather than build up delay.
        while q.len() >= 4 {
            q.pop_front();
        }
        q.push_back(frame);
    })
    .await?;
    share.keyframe();
    *video.share.lock().unwrap() = Some(share);
    client.voice_share(&channel, true).await.map_err(|e| e.to_string())
}

/// What you can share (Windows: monitors and windows; empty on Linux, where the desktop asks).
pub fn screen_sources() -> Vec<video::ScreenSource> {
    video::screen_sources()
}

/// Stop sharing your screen.
pub async fn stop_share(app: &Shared) -> Result<(), String> {
    let (client, channel, video) = {
        let g = app.voice.session.lock().await;
        let s = g.as_ref().ok_or("You're not in a call")?;
        (s.client.clone(), s.channel.clone(), s.video.clone())
    };
    let old = video.share.lock().unwrap().take();
    if old.is_none() {
        return Ok(());
    }
    drop(old);
    client.voice_share(&channel, false).await.map_err(|e| e.to_string())
}

/// Start or stop watching the share of the participant in `slot`.
pub async fn watch(app: &Shared, slot: usize, on: bool) -> Result<(), String> {
    let (client, channel, video) = {
        let g = app.voice.session.lock().await;
        let s = g.as_ref().ok_or("You're not in a call")?;
        (s.client.clone(), s.channel.clone(), s.video.clone())
    };
    if slot >= call::SLOTS {
        return Err("No such participant".into());
    }
    if on {
        if !video.viewers.lock().unwrap().contains_key(&slot) {
            let v = Viewer::new()?;
            video.viewers.lock().unwrap().insert(slot, v);
        }
    } else {
        video.viewers.lock().unwrap().remove(&slot);
    }
    let r = client.voice_watch(&channel, slot as u64, on).await.map_err(|e| e.to_string());
    if r.is_err() && on {
        video.viewers.lock().unwrap().remove(&slot);
    }
    r
}

/// The newest picture of a share you're watching (or of someone's camera, with `cam`).
pub async fn picture(app: &Shared, slot: usize, cam: bool) -> Option<Arc<Picture>> {
    let g = app.voice.session.lock().await;
    let s = g.as_ref()?;
    // Your own slot: the preview of what you're sharing, or of your camera.
    if slot as u64 == s.slot {
        let own = if cam { &s.video.camera } else { &s.video.share };
        return own.lock().unwrap().as_ref().map(|c| c.preview.clone());
    }
    if cam {
        return s.video.cams.lock().unwrap().get(&slot).map(|v| v.picture.clone());
    }
    let v = s.video.viewers.lock().unwrap();
    v.get(&slot).map(|v| v.picture.clone())
}

/// Turn your camera on or off in the call.
pub async fn set_camera(app: &Shared, on: bool) -> Result<(), String> {
    let (client, channel, controls, video) = {
        let g = app.voice.session.lock().await;
        let s = g.as_ref().ok_or("Join a voice channel first")?;
        (s.client.clone(), s.channel.clone(), s.call.controls.clone(), s.video.clone())
    };
    if !on {
        let old = video.camera.lock().unwrap().take();
        if old.is_none() {
            return Ok(());
        }
        drop(old);
        return client.voice_camera(&channel, false).await.map_err(|e| e.to_string());
    }
    if video.camera.lock().unwrap().is_some() {
        return Ok(());
    }
    let which = Some(app.voice.settings().camera).filter(|c| !c.is_empty());
    let ctl = controls.clone();
    let cam = Capture::camera(which.as_deref(), move |frame| {
        let mut q = ctl.cam_out.lock().unwrap();
        while q.len() >= 4 {
            q.pop_front();
        }
        q.push_back(frame);
    })
    .await?;
    cam.keyframe();
    *video.camera.lock().unwrap() = Some(cam);
    client.voice_camera(&channel, true).await.map_err(|e| e.to_string())
}

/// Start or stop receiving everyone's cameras (the page does this while the call view is open).
pub async fn see_cameras(app: &Shared, on: bool) -> Result<(), String> {
    let (client, channel, video) = {
        let g = app.voice.session.lock().await;
        let s = g.as_ref().ok_or("You're not in a call")?;
        (s.client.clone(), s.channel.clone(), s.video.clone())
    };
    video.cams_on.store(on, Ordering::Relaxed);
    if !on {
        video.cams.lock().unwrap().clear();
    }
    client.voice_cameras(&channel, on).await.map_err(|e| e.to_string())
}

/// The cameras you can choose from.
pub async fn cameras() -> Result<Vec<video::CameraInfo>, String> {
    video::cameras().await
}

/// Hold or release push to talk (from the page while it has focus; a desktop-wide key comes later).
pub fn ptt(app: &Shared, down: bool) {
    app.voice.levels.ptt.store(down, Ordering::Relaxed);
}
