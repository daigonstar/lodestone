//! The call engine end to end: real server and relay, real MLS groups for the keys, two engines over UDP.
//! Audio is a stand-in (the engine doesn't care where sound comes from).

use lodestone_beacon_client::Client;
use lodestone_beacon_media::call::{self, Audio, Controls};
use lodestone_beacon_server::{app, db::Db, serve, Config};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Makes a labelled "frame" every 20 ms and records what it's asked to play.
struct FakeAudio {
    label: &'static str,
    n: u64,
    last: Instant,
    played: Arc<Mutex<Vec<(usize, Vec<u8>)>>>,
}

impl Audio for FakeAudio {
    fn take_captured(&mut self) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        while self.last.elapsed() >= Duration::from_millis(20) {
            self.last += Duration::from_millis(20);
            out.push(format!("{}-{}", self.label, self.n).into_bytes());
            self.n += 1;
        }
        out
    }
    fn play(&mut self, slot: usize, frame: &[u8]) {
        self.played.lock().unwrap().push((slot, frame.to_vec()));
    }
    fn play_share(&mut self, slot: usize, frame: &[u8]) {
        // Share sound is recorded with its slot plus 100, to tell it apart from voices.
        self.played.lock().unwrap().push((slot + 100, frame.to_vec()));
    }
}

async fn join(c: &Client, channel: &str, label: &'static str) -> (call::Call, Arc<Mutex<Vec<(usize, Vec<u8>)>>>, u64) {
    let (epoch, leaf, base) = c.voice_secret(channel).unwrap();
    let controls = Arc::new(Controls::default());
    controls.keys.lock().unwrap().set(epoch, leaf, base);
    let info = c.voice_info(channel).await.unwrap();
    let relay = info["relay"].as_str().unwrap().parse().unwrap();
    let (pending, offer) = call::prepare(relay).unwrap();
    let joined = c.voice_join(channel, &relay.to_string(), offer, false, false).await.unwrap();
    let played = Arc::new(Mutex::new(Vec::new()));
    let audio = FakeAudio { label, n: 0, last: Instant::now(), played: played.clone() };
    let call = pending.start(joined["answer"].clone(), audio, controls).unwrap();
    (call, played, joined["slot"].as_u64().unwrap())
}

fn heard(played: &Mutex<Vec<(usize, Vec<u8>)>>, slot: u64, who: &str) -> usize {
    played.lock().unwrap().iter().filter(|(s, f)| *s as u64 == slot && String::from_utf8_lossy(f).starts_with(who)).count()
}

#[tokio::test(flavor = "multi_thread")]
async fn two_engines_talk_through_the_relay_encrypted() {
    let tmp = tempfile::tempdir().unwrap();
    let port = std::net::UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let db = Db::open(&tmp.path().join("server.db")).unwrap();
    let setup = db.setup_code_if_new().unwrap().unwrap();
    let media = format!("127.0.0.1:{port}");
    let state = app(Config { public_url: url.clone(), admin_only_communities: false, blob_dir: tmp.path().join("blobs"), max_file: 1 << 20, community_quota: 1 << 20, media_addr: Some(media.clone()), media_bind: media }, db);
    tokio::spawn(serve(listener, state));

    let alice = Client::open(&tmp.path().join("alice"), &url).unwrap();
    alice.register("alice", &setup, None).await.unwrap();
    let community = alice.create_community("Raid night").await.unwrap();
    let voice = alice.create_voice_channel(&community, "Lounge").await.unwrap();
    let invite = alice.create_invite(&community, 1, 3600).await.unwrap();
    let bob = Client::open(&tmp.path().join("bob"), &url).unwrap();
    bob.register("bob", invite["code"].as_str().unwrap(), None).await.unwrap();
    alice.sync().await.unwrap(); // adds Bob to the voice channel's group
    bob.sync().await.unwrap();
    // Both derive the same call base from the group; the relay never has it.
    assert_eq!(alice.voice_secret(&voice).unwrap().2, bob.voice_secret(&voice).unwrap().2);

    let (call_a, played_a, slot_a) = join(&alice, &voice, "alice").await;
    let (call_b, played_b, slot_b) = join(&bob, &voice, "bob").await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(heard(&played_b, slot_a, "alice-") > 50, "Bob heard Alice: {}", heard(&played_b, slot_a, "alice-"));
    assert!(heard(&played_a, slot_b, "bob-") > 50, "Alice heard Bob: {}", heard(&played_a, slot_b, "bob-"));
    assert_eq!(call_b.controls.undecryptable.load(Ordering::Relaxed), 0);

    // Muted: Alice's frames stop reaching Bob.
    call_a.controls.muted.store(true, Ordering::Relaxed);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let before = heard(&played_b, slot_a, "alice-");
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(heard(&played_b, slot_a, "alice-"), before, "nothing new after muting");
    call_a.controls.muted.store(false, Ordering::Relaxed);

    // Deafened: Bob plays nothing.
    call_b.controls.deafened.store(true, Ordering::Relaxed);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let before = played_b.lock().unwrap().len();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(played_b.lock().unwrap().len(), before, "nothing played while deafened");
    call_b.controls.deafened.store(false, Ordering::Relaxed);

    // With the wrong key, frames are refused, not played.
    *call_b.controls.keys.lock().unwrap() = Default::default();
    call_b.controls.keys.lock().unwrap().set(9999, 0, [1u8; 32]);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let before = played_b.lock().unwrap().len();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(played_b.lock().unwrap().len(), before, "wrong key: nothing plays");
    assert!(call_b.controls.undecryptable.load(Ordering::Relaxed) > 20);

    call_a.stop();
    call_b.stop();
    alice.voice_leave(&voice).await.unwrap();
    bob.voice_leave(&voice).await.unwrap();
    assert_eq!(alice.voice_info(&voice).await.unwrap()["participants"].as_array().unwrap().len(), 0);
}

/// A stand-in VP8 frame: byte 0 has the keyframe bit (0 = keyframe), then a label.
fn fake_vp8(n: u64, key: bool) -> Vec<u8> {
    let mut f = vec![if key { 0x10 } else { 0x11 }];
    f.extend_from_slice(format!("frame-{n}-").as_bytes());
    f.resize(1200 + (n as usize % 7) * 300, 0xab); // more than one packet
    f
}

#[tokio::test(flavor = "multi_thread")]
async fn screen_share_reaches_only_watchers_encrypted() {
    let tmp = tempfile::tempdir().unwrap();
    let port = std::net::UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let db = Db::open(&tmp.path().join("server.db")).unwrap();
    let setup = db.setup_code_if_new().unwrap().unwrap();
    let media = format!("127.0.0.1:{port}");
    let state = app(Config { public_url: url.clone(), admin_only_communities: false, blob_dir: tmp.path().join("blobs"), max_file: 1 << 20, community_quota: 1 << 20, media_addr: Some(media.clone()), media_bind: media }, db);
    tokio::spawn(serve(listener, state));

    let alice = Client::open(&tmp.path().join("alice"), &url).unwrap();
    alice.register("alice", &setup, None).await.unwrap();
    let community = alice.create_community("Raid night").await.unwrap();
    let voice = alice.create_voice_channel(&community, "Lounge").await.unwrap();
    let invite = alice.create_invite(&community, 2, 3600).await.unwrap();
    let bob = Client::open(&tmp.path().join("bob"), &url).unwrap();
    bob.register("bob", invite["code"].as_str().unwrap(), None).await.unwrap();
    let carol = Client::open(&tmp.path().join("carol"), &url).unwrap();
    carol.register("carol", invite["code"].as_str().unwrap(), None).await.unwrap();
    alice.sync().await.unwrap();
    bob.sync().await.unwrap();
    carol.sync().await.unwrap();

    let (call_a, _, slot_a) = join(&alice, &voice, "alice").await;
    let (call_b, played_b, _) = join(&bob, &voice, "bob").await;
    let (call_c, played_c, _) = join(&carol, &voice, "carol").await;
    let got_b: Arc<Mutex<Vec<(usize, Vec<u8>)>>> = Default::default();
    let got_c: Arc<Mutex<Vec<(usize, Vec<u8>)>>> = Default::default();
    for (call, got) in [(&call_b, got_b.clone()), (&call_c, got_c.clone())] {
        *call.controls.video_in.lock().unwrap() = Some(Box::new(move |slot, cam, f| {
            if !cam {
                got.lock().unwrap().push((slot, f));
            }
        }));
    }
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Watching needs someone sharing in that slot.
    assert!(bob.voice_watch(&voice, slot_a, true).await.is_err());
    alice.voice_share(&voice, true).await.unwrap();
    assert_eq!(alice.voice_info(&voice).await.unwrap()["participants"].as_array().unwrap().iter().filter(|p| p["sharing"] == true).count(), 1);
    bob.voice_watch(&voice, slot_a, true).await.unwrap();

    // Alice "shares" 30 frames a second; a keyframe when the relay asks for one (Bob started watching).
    let sent: Arc<Mutex<Vec<Vec<u8>>>> = Default::default();
    let mut keyframes = 0;
    for n in 0..60u64 {
        let key = n == 0 || call_a.controls.keyframe_wanted.swap(false, Ordering::Relaxed);
        keyframes += key as u32;
        let f = fake_vp8(n, key);
        sent.lock().unwrap().push(f.clone());
        call_a.controls.video_out.lock().unwrap().push_back(f);
        call_a.controls.share_sound_out.lock().unwrap().push_back(format!("sound-{n}").into_bytes());
        tokio::time::sleep(Duration::from_millis(33)).await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let sound = |p: &Mutex<Vec<(usize, Vec<u8>)>>| p.lock().unwrap().iter().filter(|(s, f)| *s as u64 == slot_a + 100 && f.starts_with(b"sound-")).count();
    assert!(sound(&played_b) > 40, "Bob hears the share's sound: {}", sound(&played_b));
    assert_eq!(sound(&played_c), 0, "Carol isn't watching, so she gets no share sound");

    let b = got_b.lock().unwrap().clone();
    assert!(b.len() > 40, "Bob saw Alice's share: {} frames", b.len());
    assert!(b.iter().all(|(slot, _)| *slot as u64 == slot_a), "in Alice's slot");
    let sent = sent.lock().unwrap();
    assert!(b.iter().all(|(_, f)| sent.contains(f)), "frames arrive exactly as sent (decrypted)");
    assert!(keyframes >= 2, "the relay asked Alice for a keyframe when Bob started watching");
    assert!(got_c.lock().unwrap().is_empty(), "Carol isn't watching, so she gets no video");
    assert_eq!(call_b.controls.undecryptable.load(Ordering::Relaxed), 0);

    // Bob stops watching: nothing more arrives.
    bob.voice_watch(&voice, slot_a, false).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let before = got_b.lock().unwrap().len();
    for n in 60..75u64 {
        call_a.controls.video_out.lock().unwrap().push_back(fake_vp8(n, false));
        tokio::time::sleep(Duration::from_millis(33)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(got_b.lock().unwrap().len(), before, "nothing after unwatching");

    for c in [call_a, call_b, call_c] {
        c.stop();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cameras_reach_people_with_the_call_view_open() {
    let tmp = tempfile::tempdir().unwrap();
    let port = std::net::UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let db = Db::open(&tmp.path().join("server.db")).unwrap();
    let setup = db.setup_code_if_new().unwrap().unwrap();
    let media = format!("127.0.0.1:{port}");
    let state = app(Config { public_url: url.clone(), admin_only_communities: false, blob_dir: tmp.path().join("blobs"), max_file: 1 << 20, community_quota: 1 << 20, media_addr: Some(media.clone()), media_bind: media }, db);
    tokio::spawn(serve(listener, state));
    let alice = Client::open(&tmp.path().join("alice"), &url).unwrap();
    alice.register("alice", &setup, None).await.unwrap();
    let community = alice.create_community("Raid night").await.unwrap();
    let voice = alice.create_voice_channel(&community, "Lounge").await.unwrap();
    let invite = alice.create_invite(&community, 2, 3600).await.unwrap();
    let bob = Client::open(&tmp.path().join("bob"), &url).unwrap();
    bob.register("bob", invite["code"].as_str().unwrap(), None).await.unwrap();
    let carol = Client::open(&tmp.path().join("carol"), &url).unwrap();
    carol.register("carol", invite["code"].as_str().unwrap(), None).await.unwrap();
    alice.sync().await.unwrap();
    bob.sync().await.unwrap();
    carol.sync().await.unwrap();

    let (call_a, _, slot_a) = join(&alice, &voice, "alice").await;
    let (call_b, _, _) = join(&bob, &voice, "bob").await;
    let (call_c, _, _) = join(&carol, &voice, "carol").await;
    let got: Vec<Arc<Mutex<Vec<(usize, bool, Vec<u8>)>>>> = vec![Default::default(), Default::default()];
    for (call, g) in [(&call_b, got[0].clone()), (&call_c, got[1].clone())] {
        *call.controls.video_in.lock().unwrap() = Some(Box::new(move |slot, cam, f| g.lock().unwrap().push((slot, cam, f))));
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    alice.voice_camera(&voice, true).await.unwrap();
    bob.voice_cameras(&voice, true).await.unwrap(); // Bob's call view is open; Carol's isn't.
    let mut keyframes = 0;
    for n in 0..45u64 {
        let key = n == 0 || call_a.controls.cam_keyframe_wanted.swap(false, Ordering::Relaxed);
        keyframes += key as u32;
        call_a.controls.cam_out.lock().unwrap().push_back(fake_vp8(n, key));
        tokio::time::sleep(Duration::from_millis(33)).await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let b = got[0].lock().unwrap().clone();
    assert!(b.len() > 30, "Bob sees Alice's camera: {}", b.len());
    assert!(b.iter().all(|(slot, cam, _)| *cam && *slot as u64 == slot_a), "as a camera, in Alice's slot");
    assert!(keyframes >= 2, "the relay asked Alice's camera for a keyframe when Bob opened the call view");
    assert!(got[1].lock().unwrap().is_empty(), "Carol's call view is closed: no cameras");
    assert_eq!(alice.voice_info(&voice).await.unwrap()["participants"].as_array().unwrap().iter().filter(|p| p["camera"] == true).count(), 1);
    for c in [call_a, call_b, call_c] {
        c.stop();
    }
}
