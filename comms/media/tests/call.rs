//! The call engine end to end: real host and relay, real MLS groups for the keys, two engines over UDP.
//! Audio is a stand-in (the engine doesn't care where sound comes from).

use lodestone_comms_client::Client;
use lodestone_comms_media::call::{self, Audio, Controls};
use lodestone_comms_server::{app, db::Db, serve, Config};
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
    let db = Db::open(&tmp.path().join("host.db")).unwrap();
    let setup = db.setup_code_if_new().unwrap().unwrap();
    let media = format!("127.0.0.1:{port}");
    let state = app(Config { public_url: url.clone(), admin_only_spaces: false, blob_dir: tmp.path().join("blobs"), max_file: 1 << 20, space_quota: 1 << 20, media_addr: Some(media.clone()), media_bind: media }, db);
    tokio::spawn(serve(listener, state));

    let alice = Client::open(&tmp.path().join("alice"), &url).unwrap();
    alice.register("alice", &setup).await.unwrap();
    let space = alice.create_space("Raid night").await.unwrap();
    let voice = alice.create_voice_channel(&space, "Lounge").await.unwrap();
    let invite = alice.create_invite(&space, 1, 3600).await.unwrap();
    let bob = Client::open(&tmp.path().join("bob"), &url).unwrap();
    bob.register("bob", invite["code"].as_str().unwrap()).await.unwrap();
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
