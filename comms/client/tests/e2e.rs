//! End-to-end: real host in-process, several clients, encrypted chat through it.

use lodestone_comms_client::Client;
use lodestone_comms_server::{app, db::Db, serve, Config};
use std::path::Path;

async fn start_host(dir: &Path) -> (String, String) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let db = Db::open(&dir.join("host.db")).unwrap();
    let code = db.setup_code_if_new().unwrap().unwrap();
    let state = app(Config { public_url: url.clone(), admin_only_spaces: false }, db);
    tokio::spawn(serve(listener, state));
    (url, code)
}

fn bodies(c: &Client, channel: &str) -> Vec<String> {
    c.messages(channel, 100).unwrap().into_iter().map(|m| format!("{}: {}", m.sender, m.body)).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn encrypted_chat_end_to_end() {
    let tmp = tempfile::tempdir().unwrap();
    let (url, setup) = start_host(tmp.path()).await;

    // A client given the host's address finds its exact public address.
    assert_eq!(lodestone_comms_client::discover::discover(url.trim_start_matches("http://")).await.unwrap(), url);

    // Alice sets up the host and a server.
    let alice = Client::open(&tmp.path().join("alice"), &url).unwrap();
    alice.register("alice", &setup).await.unwrap();
    let space = alice.create_space("Raid night").await.unwrap();
    let general = alice.channels().unwrap()[0].0.clone();
    let invite = alice.create_invite(&space, 5, 3600).await.unwrap();
    let code = invite["code"].as_str().unwrap().to_string();

    // Bob joins with the invite; Alice's client adds him to the channel's encrypted group.
    let bob = Client::open(&tmp.path().join("bob"), &url).unwrap();
    bob.register("bob", &code).await.unwrap();
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

    // The host only ever stored ciphertext.
    let raw = std::fs::read(tmp.path().join("host.db")).unwrap();
    let wal = std::fs::read(tmp.path().join("host.db-wal")).unwrap_or_default();
    for text in ["hello bob", "hi alice"] {
        assert!(!raw.windows(text.len()).any(|w| w == text.as_bytes()) && !wal.windows(text.len()).any(|w| w == text.as_bytes()), "the host stored plaintext");
    }

    // A new channel: its creator makes the group, members are added on sync.
    let raids = alice.create_channel(&space, "raids").await.unwrap();
    alice.sync().await.unwrap();
    bob.sync().await.unwrap();
    bob.send_text(&raids, "tuesday?").await.unwrap();
    alice.sync().await.unwrap();
    assert_eq!(bodies(&alice, &raids), vec!["bob: tuesday?"]);

    // Carol joins later: she can read what's sent after she's added, not before.
    let carol = Client::open(&tmp.path().join("carol"), &url).unwrap();
    carol.register("carol", &code).await.unwrap();
    bob.sync().await.unwrap(); // any member can add her
    alice.sync().await.unwrap();
    carol.sync().await.unwrap();
    assert!(bodies(&carol, &general).is_empty(), "older messages were sent before Carol had keys");
    alice.send_text(&general, "welcome carol").await.unwrap();
    carol.sync().await.unwrap();
    assert_eq!(bodies(&carol, &general), vec!["alice: welcome carol"]);

    // Alice removes Bob: the host stops serving him and the groups drop his device.
    alice.host.call("DELETE", &format!("/v1/spaces/{space}/members/bob"), None).await.unwrap();
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
    let mode = std::fs::metadata(tmp.path().join("alice/comms.db")).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}
