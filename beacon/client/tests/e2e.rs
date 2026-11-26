//! End-to-end: real server in-process, several clients, encrypted chat through it.

use lodestone_beacon_client::Client;
use lodestone_beacon_server::{app, db::Db, serve, Config};
use std::path::Path;

async fn start_server(dir: &Path) -> (String, String) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let db = Db::open(&dir.join("server.db")).unwrap();
    let code = db.setup_code_if_new().unwrap().unwrap();
    let state = app(Config { public_url: url.clone(), admin_only_communities: false, blob_dir: dir.join("blobs"), max_file: 10 << 20, community_quota: 100 << 20, media_addr: None, media_bind: String::new() }, db);
    tokio::spawn(serve(listener, state));
    (url, code)
}

fn bodies(c: &Client, channel: &str) -> Vec<String> {
    c.messages(channel, 100).unwrap().into_iter().map(|m| format!("{}: {}", m.sender, m.body)).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn encrypted_chat_end_to_end() {
    let tmp = tempfile::tempdir().unwrap();
    let (url, setup) = start_server(tmp.path()).await;

    // A client given the server's address finds its exact public address.
    assert_eq!(lodestone_beacon_client::discover::discover(url.trim_start_matches("http://")).await.unwrap(), url);

    // Alice sets up the server and a community.
    let alice = Client::open(&tmp.path().join("alice"), &url).unwrap();
    alice.register("alice", &setup, None).await.unwrap();
    let community = alice.create_community("Raid night").await.unwrap();
    let general = alice.channels().unwrap()[0].0.clone();
    let invite = alice.create_invite(&community, 5, 3600).await.unwrap();
    let code = invite["code"].as_str().unwrap().to_string();

    // Bob joins with the invite; Alice's client adds him to the channel's encrypted group.
    let bob = Client::open(&tmp.path().join("bob"), &url).unwrap();
    bob.register("bob", &code, None).await.unwrap();
    alice.sync().await.unwrap();
    bob.sync().await.unwrap();
    assert_eq!(bob.channels().unwrap().len(), 1, "Bob joined the channel's group from the welcome");

    alice.send_text(&general, "hello bob").await.unwrap();
    let got = bob.sync().await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!((got[0].sender.as_str(), got[0].body.as_str()), ("alice", "hello bob"));
    bob.send_text(&general, "hi alice").await.unwrap();
    alice.sync().await.unwrap();
    assert_eq!(bodies(&alice, &general), vec!["alice: hello bob", "bob: hi alice"]);

    // Reactions travel encrypted too, and only you can take back your own.
    let hello_seq = bob.messages(&general, 10).unwrap()[0].seq;
    alice.react(&general, hello_seq, "👍", true).await.unwrap();
    bob.react(&general, hello_seq, "👍", true).await.unwrap();
    bob.sync().await.unwrap();
    alice.sync().await.unwrap();
    let r = alice.reactions(&general).unwrap();
    assert_eq!(r, vec![(hello_seq, "👍".to_string(), vec!["alice".to_string(), "bob".to_string()])]);
    assert!(bob.take_touched().contains(&general));
    alice.react(&general, hello_seq, "👍", false).await.unwrap();
    bob.sync().await.unwrap();
    assert_eq!(bob.reactions(&general).unwrap(), vec![(hello_seq, "👍".to_string(), vec!["bob".to_string()])]);
    assert!(alice.react(&general, hello_seq, "not an emoji", true).await.is_err());

    // Attachments: encrypted on Alice's device, readable by Bob, opaque on the server.
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend(b"secret raid map pixels");
    let sent = alice.send_file(&general, "../../raid map.png", &png, "the plan").await.unwrap();
    let att = sent.attachment.clone().unwrap();
    assert_eq!((att["name"].as_str(), att["mime"].as_str()), (Some("raid map.png"), Some("image/png")));
    assert!(att.get("key").is_none(), "the key isn't handed out with the message");
    let got = bob.sync().await.unwrap();
    let file_msg = got.iter().find(|m| m.attachment.is_some()).expect("Bob got the file message");
    assert_eq!(file_msg.body, "the plan");
    let (bytes, mime, name) = bob.open_file(&general, file_msg.seq).await.unwrap();
    assert_eq!((bytes, mime.as_str(), name.as_str()), (png.clone(), "image/png", "raid map.png"));
    let blob = std::fs::read_dir(tmp.path().join("blobs")).unwrap().flatten().next().unwrap().path();
    let stored = std::fs::read(&blob).unwrap();
    assert!(!stored.windows(16).any(|w| w == &png[8..24]), "the server stored the file in the clear");
    // A file changed on the server is refused, not shown.
    let mut bad = stored.clone();
    bad[20] ^= 1;
    std::fs::write(&blob, &bad).unwrap();
    std::fs::remove_dir_all(tmp.path().join("bob/files")).unwrap();
    assert!(bob.open_file(&general, file_msg.seq).await.is_err());
    std::fs::write(&blob, &stored).unwrap();

    // The server only ever stored ciphertext.
    let raw = std::fs::read(tmp.path().join("server.db")).unwrap();
    let wal = std::fs::read(tmp.path().join("server.db-wal")).unwrap_or_default();
    for text in ["hello bob", "hi alice"] {
        assert!(!raw.windows(text.len()).any(|w| w == text.as_bytes()) && !wal.windows(text.len()).any(|w| w == text.as_bytes()), "the server stored plaintext");
    }

    // Profiles: encrypted like messages, and only ever your own.
    let mut face = b"\x89PNG\r\n\x1a\n".to_vec();
    face.extend(b"alice's face");
    alice.set_profile("Alice ⚔", "#ff8800", Some(&face)).await.unwrap();
    bob.set_profile("Bobby", "", None).await.unwrap();
    bob.sync().await.unwrap();
    alice.sync().await.unwrap();
    let find = |c: &Client, who: &str| c.profiles().unwrap().into_iter().find(|p| p.0 == who);
    assert_eq!(find(&bob, "alice"), Some(("alice".into(), "Alice ⚔".into(), "#ff8800".into(), true)));
    assert_eq!(bob.avatar("alice").await.unwrap(), (face.clone(), "image/png".to_string()));
    assert_eq!(find(&alice, "bob").map(|p| p.1), Some("Bobby".to_string()));
    assert!(alice.set_profile("x".repeat(33).as_str(), "", None).await.is_err());
    assert!(alice.set_profile("ok", "orange", None).await.is_err());
    assert!(alice.set_profile("ok", "", Some(b"not an image")).await.is_err());
    let server_raw = std::fs::read(tmp.path().join("server.db-wal")).unwrap_or_default();
    assert!(!server_raw.windows("Bobby".len()).any(|w| w == b"Bobby"), "the server saw a display name");

    // A new channel: its creator makes the group, members are added on sync.
    let raids = alice.create_channel(&community, "raids").await.unwrap();
    alice.sync().await.unwrap();
    bob.sync().await.unwrap();
    bob.send_text(&raids, "tuesday?").await.unwrap();
    alice.sync().await.unwrap();
    assert_eq!(bodies(&alice, &raids), vec!["bob: tuesday?"]);

    // Carol joins later: she can read what's sent after she's added, not before.
    let carol = Client::open(&tmp.path().join("carol"), &url).unwrap();
    carol.register("carol", &code, None).await.unwrap();
    bob.sync().await.unwrap(); // any member can add her
    alice.sync().await.unwrap();
    carol.sync().await.unwrap();
    assert!(bodies(&carol, &general).is_empty(), "older messages were sent before Carol had keys");
    // But she gets everyone's profile: their apps re-send it when someone joins.
    alice.sync().await.unwrap();
    bob.sync().await.unwrap();
    carol.sync().await.unwrap();
    assert_eq!(find(&carol, "alice").map(|p| (p.1, p.3)), Some(("Alice ⚔".to_string(), true)));
    assert_eq!(carol.avatar("alice").await.unwrap().0, face);
    assert_eq!(find(&carol, "bob").map(|p| p.1), Some("Bobby".to_string()));
    alice.send_text(&general, "welcome carol").await.unwrap();
    carol.sync().await.unwrap();
    assert_eq!(bodies(&carol, &general), vec!["alice: welcome carol"]);

    // Alice pairs a second device: a code, then approval on the first, matching fingerprints.
    let desktop = Client::open(&tmp.path().join("alice-desktop"), &url).unwrap();
    let (pcode, _) = alice.pair_start().await.unwrap();
    let (user, fp) = desktop.pair_claim(&pcode).await.unwrap();
    assert_eq!(user, "alice");
    assert_eq!(desktop.pair_check().await.unwrap().0, "waiting");
    let waiting = alice.pair_waiting().await.unwrap();
    assert_eq!(waiting, vec![(desktop.device_key().to_string(), fp.clone())]);
    // The QR code's key lets Alice's laptop leave the desktop her message history (the server only sees ciphertext).
    let key = [7u8; 32];
    let sent = alice.pair_send_history(desktop.device_key(), &key).await.unwrap();
    assert!(sent >= 3, "Alice's history was sent");
    alice.pair_decide(desktop.device_key(), true, None).await.unwrap();
    assert_eq!(desktop.pair_check().await.unwrap().0, "approved");
    assert_eq!(desktop.username().as_deref(), Some("alice"));
    // The desktop collects it once, with the right key, and sees Alice's older messages straight away.
    let got = desktop.pair_fetch_history(&key).await.unwrap();
    assert_eq!(got, sent);
    assert!(bodies(&desktop, &general).contains(&"alice: welcome carol".to_string()));
    assert_eq!(desktop.pair_fetch_history(&key).await.unwrap(), 0, "the server hands the history over once");
    // Alice's laptop (or any member) adds the new device to the channels.
    alice.sync().await.unwrap();
    desktop.sync().await.unwrap();
    assert!(!desktop.channels().unwrap().is_empty(), "the desktop joined the channels");
    desktop.send_text(&general, "from my desktop").await.unwrap();
    carol.sync().await.unwrap();
    assert_eq!(bodies(&carol, &general).last().unwrap(), "alice: from my desktop");
    // The desktop learns Alice's profile from her laptop's encrypted profile message.
    alice.sync().await.unwrap();
    desktop.sync().await.unwrap();
    assert_eq!(desktop.my_profile().0, "Alice ⚔");
    let devices = alice.devices().await.unwrap();
    assert_eq!(devices.len(), 2);
    assert!(devices.iter().any(|d| d.0 == desktop.device_key() && !d.2));
    // Removing the desktop takes it out of the channels: it reads nothing new.
    alice.remove_device(desktop.device_key()).await.unwrap();
    alice.sync().await.unwrap();
    carol.sync().await.unwrap();
    alice.send_text(&general, "desktop is gone").await.unwrap();
    assert!(desktop.sync().await.is_err() || !bodies(&desktop, &general).iter().any(|b| b.contains("desktop is gone")));
    carol.sync().await.unwrap();
    assert_eq!(bodies(&carol, &general).last().unwrap(), "alice: desktop is gone");

    // Alice removes Bob: the server stops serving him and the groups drop his device.
    alice.server.call("DELETE", &format!("/v1/communities/{community}/members/bob"), None).await.unwrap();
    alice.sync().await.unwrap();
    carol.sync().await.unwrap();
    alice.send_text(&general, "bob is gone").await.unwrap();
    carol.sync().await.unwrap();
    assert_eq!(bodies(&carol, &general).last().unwrap(), "alice: bob is gone");
    assert!(bob.sync().await.unwrap().is_empty());
    assert!(!bodies(&bob, &general).iter().any(|b| b.contains("bob is gone")));

    // State survives a restart: reopen Alice from her folder, sign in, keep chatting.
    drop(alice);
    let alice = Client::open(&tmp.path().join("alice"), &url).unwrap();
    alice.sign_in().await.unwrap();
    alice.send_text(&general, "still here").await.unwrap();
    carol.sync().await.unwrap();
    assert_eq!(bodies(&carol, &general).last().unwrap(), "alice: still here");

    // The client keeps its data private.
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(tmp.path().join("alice/beacon.db")).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[tokio::test(flavor = "multi_thread")]
async fn what_youre_playing_reaches_your_server_encrypted() {
    let tmp = tempfile::tempdir().unwrap();
    let (url, setup) = start_server(tmp.path()).await;
    let alice = Client::open(&tmp.path().join("alice"), &url).unwrap();
    alice.register("alice", &setup, None).await.unwrap();
    let community = alice.create_community("Raid night").await.unwrap();
    let invite = alice.create_invite(&community, 1, 3600).await.unwrap();
    let bob = Client::open(&tmp.path().join("bob"), &url).unwrap();
    bob.register("bob", invite["code"].as_str().unwrap(), None).await.unwrap();
    alice.sync().await.unwrap();
    bob.sync().await.unwrap();

    alice.set_playing(Some("Hades II")).await.unwrap();
    bob.sync().await.unwrap();
    assert_eq!(bob.activities().get("alice").map(String::as_str), Some("Hades II"));
    // Not shown as a chat message.
    let general = bob.channels().unwrap()[0].0.clone();
    assert!(bob.messages(&general, 50).unwrap().iter().all(|m| !m.body.contains("Hades")));

    alice.set_playing(None).await.unwrap();
    bob.sync().await.unwrap();
    assert_eq!(bob.activities().get("alice"), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn one_identity_across_servers_and_recovery_on_a_new_pc() {
    use lodestone_beacon_client::identity::Identity;
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("a")).unwrap();
    std::fs::create_dir_all(tmp.path().join("b")).unwrap();
    let (url_a, setup_a) = start_server(&tmp.path().join("a")).await;
    let (url_b, setup_b) = start_server(&tmp.path().join("b")).await;
    let me = Identity::generate();

    // The same identity (and username) on two servers.
    let pc_a = Client::open(&tmp.path().join("pc-a"), &url_a).unwrap();
    pc_a.register("robb", &setup_a, Some(&me)).await.unwrap();
    let pc_b = Client::open(&tmp.path().join("pc-b"), &url_b).unwrap();
    pc_b.register("robb", &setup_b, Some(&me)).await.unwrap();
    let me_a = pc_a.server.get("/v1/me").await.unwrap();
    let me_b = pc_b.server.get("/v1/me").await.unwrap();
    assert_eq!((me_a["identity"].as_str(), me_b["identity"].as_str()), (Some(me.public().as_str()), Some(me.public().as_str())));

    // A new PC restores the identity from the 24 words and gets back into both accounts.
    let restored = Identity::from_phrase(&me.phrase()).unwrap();
    let new_a = Client::open(&tmp.path().join("new-a"), &url_a).unwrap();
    assert_eq!(new_a.recover(&restored).await.unwrap(), "robb");
    let new_b = Client::open(&tmp.path().join("new-b"), &url_b).unwrap();
    assert_eq!(new_b.recover(&restored).await.unwrap(), "robb");
    assert_eq!(new_a.devices().await.unwrap().len(), 2);

    // Someone else's identity can't recover into the account.
    let thief = Client::open(&tmp.path().join("thief"), &url_a).unwrap();
    assert!(thief.recover(&Identity::generate()).await.is_err());
    assert_eq!(new_a.devices().await.unwrap().len(), 2);

    // Pairing a phone can hand the identity over, sealed with a key that only the QR code carries.
    let phone = Client::open(&tmp.path().join("phone"), &url_a).unwrap();
    let (pcode, _) = pc_a.pair_start().await.unwrap();
    phone.pair_claim(&pcode).await.unwrap();
    let key = [9u8; 32];
    pc_a.pair_decide(phone.device_key(), true, Some(me.seal(&key))).await.unwrap();
    let (status, blob) = phone.pair_check().await.unwrap();
    assert_eq!(status, "approved");
    assert_eq!(Identity::open(&blob.unwrap(), &key).unwrap().public(), me.public());
}

#[tokio::test(flavor = "multi_thread")]
async fn private_channels_only_reach_the_people_on_their_list() {
    let tmp = tempfile::tempdir().unwrap();
    let (url, setup) = start_server(tmp.path()).await;
    let alice = Client::open(&tmp.path().join("alice"), &url).unwrap();
    alice.register("alice", &setup, None).await.unwrap();
    let community = alice.create_community("Guild").await.unwrap();
    let invite = alice.create_invite(&community, 5, 3600).await.unwrap();
    let code = invite["code"].as_str().unwrap().to_string();
    let bob = Client::open(&tmp.path().join("bob"), &url).unwrap();
    bob.register("bob", &code, None).await.unwrap();
    let carol = Client::open(&tmp.path().join("carol"), &url).unwrap();
    carol.register("carol", &code, None).await.unwrap();
    for c in [&alice, &bob, &carol, &alice] {
        c.sync().await.unwrap();
    }

    // Alice makes a private channel for herself and Bob.
    let secret = alice.create_private_channel(&community, "officers", "text", &["bob".to_string()]).await.unwrap();
    alice.sync().await.unwrap();
    bob.sync().await.unwrap();
    carol.sync().await.unwrap();
    alice.send_text(&secret, "plans for friday").await.unwrap();
    bob.sync().await.unwrap();
    assert_eq!(bodies(&bob, &secret), vec!["alice: plans for friday"]);

    // Carol doesn't see it at all: not in her community listing, no group, and the server refuses her.
    let listing = carol.server.get(&format!("/v1/communities/{community}")).await.unwrap();
    assert!(listing["channels"].as_array().unwrap().iter().all(|c| c["id"] != secret.as_str()));
    carol.sync().await.unwrap();
    assert!(carol.channels().unwrap().iter().all(|c| c.0 != secret));
    assert!(carol.server.get(&format!("/v1/channels/{secret}/messages")).await.is_err());

    // Alice adds Carol to the list: she's added to the group and reads from then on, not before.
    alice.server.call("PATCH", &format!("/v1/channels/{secret}"), Some(serde_json::json!({ "access": ["alice", "bob", "carol"] }))).await.unwrap();
    alice.sync().await.unwrap();
    carol.sync().await.unwrap();
    assert!(carol.channels().unwrap().iter().any(|c| c.0 == secret), "Carol was welcomed");
    alice.send_text(&secret, "welcome carol").await.unwrap();
    carol.sync().await.unwrap();
    assert_eq!(bodies(&carol, &secret), vec!["alice: welcome carol"]);

    // Alice takes Bob off the list: he loses the channel and can't read what's said next.
    alice.server.call("PATCH", &format!("/v1/channels/{secret}"), Some(serde_json::json!({ "access": ["alice", "carol"] }))).await.unwrap();
    alice.sync().await.unwrap();
    carol.sync().await.unwrap();
    alice.send_text(&secret, "bob is out").await.unwrap();
    carol.sync().await.unwrap();
    assert_eq!(bodies(&carol, &secret).last().unwrap(), "alice: bob is out");
    bob.sync().await.unwrap();
    assert!(!bodies(&bob, &secret).iter().any(|b| b.contains("bob is out")));
    assert!(bob.server.get(&format!("/v1/channels/{secret}/messages")).await.is_err());

    // Making it public again brings Bob back in.
    alice.server.call("PATCH", &format!("/v1/channels/{secret}"), Some(serde_json::json!({ "private": false }))).await.unwrap();
    alice.sync().await.unwrap();
    bob.sync().await.unwrap();
    alice.send_text(&secret, "everyone now").await.unwrap();
    bob.sync().await.unwrap();
    assert_eq!(bodies(&bob, &secret).last().unwrap(), "alice: everyone now");
}
