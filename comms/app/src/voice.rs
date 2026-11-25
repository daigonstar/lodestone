//! Calls in voice channels: one at a time. Joins through the host's relay with the call engine
//! (`lodestone_comms_media::call`), sound from GStreamer (`audio`), and keeps the call's keys current
//! from the channel's encrypted group while you're in it.
//!
//! Device and microphone settings are kept in `voice.json` beside the app's other preferences.

use crate::Shared;
use lodestone_comms_client::Client;
use lodestone_comms_media::audio::{self, Gate, GstAudio, Levels, Options};
use lodestone_comms_media::call::{self, Call, Controls};
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
}

fn voice() -> String {
    "voice".into()
}
fn threshold() -> f64 {
    -45.0
}

impl Default for Settings {
    fn default() -> Self {
        Self { mic: String::new(), speakers: String::new(), mode: voice(), threshold: threshold() }
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
}

impl Voice {
    pub fn load(file: std::path::PathBuf) -> Self {
        let settings: Settings = std::fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let levels = Arc::new(Levels::default());
        apply(&levels, &settings);
        Self { levels, session: Default::default(), test: Mutex::new(None), settings: Mutex::new(settings), file }
    }

    pub fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    /// Save new settings. Mode and threshold apply at once; devices from the next call (or mic test).
    pub fn set_settings(&self, s: Settings) -> Result<(), String> {
        if Gate::parse(&s.mode).is_none() {
            return Err("Unknown microphone mode".into());
        }
        for d in [&s.mic, &s.speakers] {
            if !d.chars().all(|c| c.is_ascii_alphanumeric() || "._-:@".contains(c)) {
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
}

pub fn devices() -> Result<Vec<audio::Device>, String> {
    audio::devices()
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
    let relay: SocketAddr = info["relay"].as_str().and_then(|r| r.parse().ok()).ok_or("The host didn't say where its call relay is")?;
    let (epoch, leaf, base) = c.voice_secret(channel).map_err(|e| e.to_string())?;
    let (pending, offer) = call::prepare(relay).map_err(|e| format!("Couldn't reach the call relay: {e}"))?;
    let joined = c.voice_join(channel, offer, muted, deafened).await.map_err(|e| e.to_string())?;

    let controls = Arc::new(Controls::default());
    controls.keys.lock().unwrap().set(epoch, leaf, base);
    controls.muted.store(muted, Ordering::Relaxed);
    controls.deafened.store(deafened, Ordering::Relaxed);
    app.voice.levels.clear_error();
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
    let participants = Arc::new(Mutex::new(joined["participants"].clone()));
    let session = Session {
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
                    *participants.lock().unwrap() = info["participants"].clone();
                }
            }
        }
    });
    Ok(status(app).await)
}

/// Leave the call you're in, if any.
pub async fn leave(app: &Shared) {
    let Some(s) = app.voice.session.lock().await.take() else { return };
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
    for p in people.as_array_mut().into_iter().flatten() {
        let slot = p["slot"].as_u64().unwrap_or(u64::MAX);
        let me = slot == sess.slot;
        let talking = if me { v.levels.talking() && !muted } else { speaking.contains(&(slot as usize)) };
        p["speaking"] = json!(talking);
        p["me"] = json!(me);
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
        },
        "audio": base,
    })
}

/// Hold or release push to talk (from the page while it has focus; a desktop-wide key comes later).
pub fn ptt(app: &Shared, down: bool) {
    app.voice.levels.ptt.store(down, Ordering::Relaxed);
}
