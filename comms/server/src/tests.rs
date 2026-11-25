//! End-to-end tests of the API against an in-memory database.

use crate::util::b64;
use crate::{app, db::Db, Config, Shared};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use ed25519_dalek::{Signer, SigningKey};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const HOST: &str = "https://comms.test";

fn host() -> (Shared, String) {
    let db = Db::memory();
    let code = db.setup_code_if_new().unwrap().unwrap();
    (app(Config { public_url: HOST.into(), admin_only_spaces: false }, db), code)
}

async fn call(app: &Shared, method: &str, path: &str, token: Option<&str>, body: Value) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(path).header("content-type", "application/json");
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let body = if body.is_null() { Body::empty() } else { Body::from(body.to_string()) };
    let res = crate::api::router(app.clone()).oneshot(req.body(body).unwrap()).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

struct Device {
    key: SigningKey,
}

impl Device {
    fn new() -> Self {
        Self { key: SigningKey::generate(&mut rand::rngs::OsRng) }
    }
    fn public(&self) -> String {
        b64(self.key.verifying_key().as_bytes())
    }
    fn sign(&self, msg: &str) -> String {
        b64(&self.key.sign(msg.as_bytes()).to_bytes())
    }
    async fn register(&self, app: &Shared, user: &str, invite: &str) -> (StatusCode, Value) {
        let sig = self.sign(&format!("lodestone-comms/v1 register {HOST} {user} {}", self.public()));
        call(app, "POST", "/v1/register", None, json!({ "username": user, "device_key": self.public(), "signature": sig, "invite": invite })).await
    }
    async fn sign_in(&self, app: &Shared) -> String {
        let (s, c) = call(app, "POST", "/v1/challenge", None, json!({ "device_key": self.public() })).await;
        assert_eq!(s, StatusCode::OK, "{c}");
        let nonce = c["nonce"].as_str().unwrap();
        let sig = self.sign(&format!("lodestone-comms/v1 login {HOST} {nonce}"));
        let (s, r) = call(app, "POST", "/v1/session", None, json!({ "device_key": self.public(), "nonce": nonce, "signature": sig })).await;
        assert_eq!(s, StatusCode::OK, "{r}");
        r["token"].as_str().unwrap().to_string()
    }
}

#[tokio::test]
async fn setup_code_works_once_and_signatures_are_bound_to_the_host() {
    let (app, code) = host();
    let a = Device::new();
    // A signature made for another host is refused.
    let wrong = a.sign(&format!("lodestone-comms/v1 register https://other.test alice {}", a.public()));
    let (s, _) = call(&app, "POST", "/v1/register", None, json!({ "username": "alice", "device_key": a.public(), "signature": wrong, "invite": code })).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, r) = a.register(&app, "alice", &code).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["host_admin"], true);
    // The setup code is gone, and no code means no account.
    let (s, _) = Device::new().register(&app, "bob", &code).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = Device::new().register(&app, "bob", "nonsense").await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn sign_in_challenges_are_single_use() {
    let (app, code) = host();
    let a = Device::new();
    a.register(&app, "alice", &code).await;
    let (_, c) = call(&app, "POST", "/v1/challenge", None, json!({ "device_key": a.public() })).await;
    let nonce = c["nonce"].as_str().unwrap();
    let sig = a.sign(&format!("lodestone-comms/v1 login {HOST} {nonce}"));
    let body = json!({ "device_key": a.public(), "nonce": nonce, "signature": sig });
    assert_eq!(call(&app, "POST", "/v1/session", None, body.clone()).await.0, StatusCode::OK);
    assert_eq!(call(&app, "POST", "/v1/session", None, body).await.0, StatusCode::UNAUTHORIZED);
    // Unknown devices get the same answer as any refused challenge.
    assert_eq!(call(&app, "POST", "/v1/challenge", None, json!({ "device_key": Device::new().public() })).await.0, StatusCode::UNAUTHORIZED);
    // A bad token gets nothing.
    assert_eq!(call(&app, "GET", "/v1/me", Some("not-a-token"), Value::Null).await.0, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn spaces_invites_and_encrypted_messages() {
    let (app, code) = host();
    let (alice, bob, carol) = (Device::new(), Device::new(), Device::new());
    alice.register(&app, "alice", &code).await;
    let ta = alice.sign_in(&app).await;

    // Alice makes a server; it comes with a general channel.
    let (s, space) = call(&app, "POST", "/v1/spaces", Some(&ta), json!({ "name": "Friday Raids" })).await;
    assert_eq!(s, StatusCode::OK, "{space}");
    let sid = space["id"].as_str().unwrap().to_string();
    let ch = space["channels"][0]["id"].as_str().unwrap().to_string();

    // An invite brings Bob in at registration.
    let (_, inv) = call(&app, "POST", &format!("/v1/spaces/{sid}/invites"), Some(&ta), json!({ "max_uses": 1, "expires_in": 3600 })).await;
    let invite = inv["code"].as_str().unwrap().to_string();
    assert!(inv["link"].as_str().unwrap().starts_with("https://comms.test/join/"));
    let (s, preview) = call(&app, "GET", &format!("/v1/invites/{invite}"), None, Value::Null).await;
    assert_eq!((s, preview["name"].as_str()), (StatusCode::OK, Some("Friday Raids")));
    let (s, r) = bob.register(&app, "bob", &invite).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["joined_space"], sid);
    let tb = bob.sign_in(&app).await;
    // The invite was single-use.
    assert_eq!(Device::new().register(&app, "dave", &invite).await.0, StatusCode::FORBIDDEN);

    // Carol is on the host (via another server) but not in Alice's.
    let (_, other) = call(&app, "POST", "/v1/spaces", Some(&tb), json!({ "name": "Bob's place" })).await;
    let (_, inv2) = call(&app, "POST", &format!("/v1/spaces/{}/invites", other["id"].as_str().unwrap()), Some(&tb), json!({ "max_uses": 5, "expires_in": 3600 })).await;
    carol.register(&app, "carol", inv2["code"].as_str().unwrap()).await;
    let tc = carol.sign_in(&app).await;
    assert_eq!(call(&app, "GET", &format!("/v1/spaces/{sid}"), Some(&tc), Value::Null).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&app, "GET", &format!("/v1/channels/{ch}/messages"), Some(&tc), Value::Null).await.0, StatusCode::NOT_FOUND);

    // Members see each other's device keys (clients check MLS credentials against them).
    let (_, sp) = call(&app, "GET", &format!("/v1/spaces/{sid}"), Some(&ta), Value::Null).await;
    let members = sp["members"].as_array().unwrap();
    assert_eq!(members.len(), 2);
    assert!(members.iter().any(|m| m["username"] == "bob" && m["devices"][0] == bob.public()));

    // Key packages: Bob uploads, Alice claims one per device, and it's used up.
    call(&app, "POST", "/v1/keypackages", Some(&tb), json!({ "key_packages": [b64(b"kp-1"), b64(b"kp-2")] })).await;
    let (_, claim) = call(&app, "POST", "/v1/keypackages/claim", Some(&ta), json!({ "username": "bob" })).await;
    assert_eq!(claim["devices"][0]["key_package"], b64(b"kp-1"));
    let (_, claim) = call(&app, "POST", "/v1/keypackages/claim", Some(&ta), json!({ "username": "bob" })).await;
    assert_eq!(claim["devices"][0]["key_package"], b64(b"kp-2"));
    // Carol shares a server with Bob, but not with Alice: Alice can't fetch Carol's packages.
    assert_eq!(call(&app, "POST", "/v1/keypackages/claim", Some(&ta), json!({ "username": "carol" })).await.0, StatusCode::NOT_FOUND);

    // Commits are ordered: one per epoch.
    let send = |kind: &str, epoch: i64, body: &[u8]| json!({ "kind": kind, "epoch": epoch, "body": b64(body) });
    assert_eq!(call(&app, "POST", &format!("/v1/channels/{ch}/messages"), Some(&ta), send("commit", 0, b"add bob")).await.0, StatusCode::OK);
    assert_eq!(call(&app, "POST", &format!("/v1/channels/{ch}/messages"), Some(&tb), send("commit", 0, b"late commit")).await.0, StatusCode::CONFLICT);
    let (_, sent) = call(&app, "POST", &format!("/v1/channels/{ch}/messages"), Some(&ta), send("application", 1, b"ciphertext")).await;
    assert_eq!(sent["seq"], 2);

    // The welcome reaches Bob's device once.
    let (_, w) = call(&app, "POST", "/v1/welcomes", Some(&ta), json!({ "channel": ch, "to": [bob.public(), carol.public()], "body": b64(b"welcome") })).await;
    assert_eq!(w["delivered"], 1, "Carol isn't in the space, so her device gets nothing");
    let (_, got) = call(&app, "GET", "/v1/welcomes", Some(&tb), Value::Null).await;
    assert_eq!(got["welcomes"][0]["body"], b64(b"welcome"));
    let (_, again) = call(&app, "GET", "/v1/welcomes", Some(&tb), Value::Null).await;
    assert_eq!(again["welcomes"].as_array().unwrap().len(), 0);

    // Bob reads the history: the host stored exactly the bytes sent, in order.
    let (_, hist) = call(&app, "GET", &format!("/v1/channels/{ch}/messages?after=0"), Some(&tb), Value::Null).await;
    let msgs = hist["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 2);
    assert_eq!((msgs[1]["body"].clone(), msgs[1]["sender_name"].clone()), (json!(b64(b"ciphertext")), json!("alice")));

    // Members can't remove the owner; the owner can remove and ban.
    assert_eq!(call(&app, "DELETE", &format!("/v1/spaces/{sid}/members/alice"), Some(&tb), Value::Null).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&app, "DELETE", &format!("/v1/spaces/{sid}/members/bob?ban=true"), Some(&ta), Value::Null).await.0, StatusCode::OK);
    assert_eq!(call(&app, "GET", &format!("/v1/channels/{ch}/messages"), Some(&tb), Value::Null).await.0, StatusCode::NOT_FOUND);
    let (_, inv3) = call(&app, "POST", &format!("/v1/spaces/{sid}/invites"), Some(&ta), json!({ "max_uses": 5, "expires_in": 3600 })).await;
    assert_eq!(call(&app, "POST", &format!("/v1/invites/{}/accept", inv3["code"].as_str().unwrap()), Some(&tb), Value::Null).await.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn rejects_bad_input() {
    let (app, code) = host();
    let a = Device::new();
    assert_eq!(a.register(&app, "No Spaces!", &code).await.0, StatusCode::BAD_REQUEST);
    a.register(&app, "alice", &code).await;
    let t = a.sign_in(&app).await;
    assert_eq!(call(&app, "POST", "/v1/spaces", Some(&t), json!({ "name": "  " })).await.0, StatusCode::BAD_REQUEST);
    let (_, sp) = call(&app, "POST", "/v1/spaces", Some(&t), json!({ "name": "x" })).await;
    let ch = sp["channels"][0]["id"].as_str().unwrap();
    let big = b64(&vec![0u8; 64 * 1024 + 1]);
    assert_eq!(call(&app, "POST", &format!("/v1/channels/{ch}/messages"), Some(&t), json!({ "kind": "application", "epoch": 0, "body": big })).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&app, "POST", &format!("/v1/channels/{ch}/messages"), Some(&t), json!({ "kind": "shout", "epoch": 0, "body": b64(b"x") })).await.0, StatusCode::BAD_REQUEST);
    let sid = sp["id"].as_str().unwrap();
    assert_eq!(call(&app, "POST", &format!("/v1/spaces/{sid}/invites"), Some(&t), json!({ "max_uses": 0, "expires_in": 3600 })).await.0, StatusCode::BAD_REQUEST);
}

/// Upload helper for raw bodies (icons).
async fn put_raw(app: &Shared, path: &str, token: &str, body: Vec<u8>) -> StatusCode {
    let req = Request::builder().method("PUT").uri(path).header("authorization", format!("Bearer {token}")).body(Body::from(body)).unwrap();
    crate::api::router(app.clone()).oneshot(req).await.unwrap().status()
}

#[tokio::test]
async fn server_settings_and_permissions() {
    let (app, code) = host();
    let (alice, bob, carol) = (Device::new(), Device::new(), Device::new());
    alice.register(&app, "alice", &code).await;
    let ta = alice.sign_in(&app).await;
    let (_, sp) = call(&app, "POST", "/v1/spaces", Some(&ta), json!({ "name": "Raid night" })).await;
    let sid = sp["id"].as_str().unwrap().to_string();
    let general = sp["channels"][0]["id"].as_str().unwrap().to_string();
    let (_, inv) = call(&app, "POST", &format!("/v1/spaces/{sid}/invites"), Some(&ta), json!({ "max_uses": 5, "expires_in": 3600 })).await;
    let code5 = inv["code"].as_str().unwrap().to_string();
    bob.register(&app, "bob", &code5).await;
    carol.register(&app, "carol", &code5).await;
    let (tb, tc) = (bob.sign_in(&app).await, carol.sign_in(&app).await);
    let sp_path = format!("/v1/spaces/{sid}");

    // Profile: members can't change it, the owner can.
    assert_eq!(call(&app, "PATCH", &sp_path, Some(&tb), json!({ "name": "Mine now" })).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&app, "PATCH", &sp_path, Some(&ta), json!({ "name": "Raid Night", "description": "Tuesdays, 8pm" })).await.0, StatusCode::OK);
    let (_, got) = call(&app, "GET", &sp_path, Some(&tc), Value::Null).await;
    assert_eq!((got["name"].as_str(), got["description"].as_str(), got["has_icon"].as_bool()), (Some("Raid Night"), Some("Tuesdays, 8pm"), Some(false)));

    // Icon: only real images, members can fetch it, outsiders can't.
    let png = b"\x89PNG\r\n\x1a\nrest-of-a-png".to_vec();
    assert_eq!(put_raw(&app, &format!("{sp_path}/icon"), &ta, b"<svg onload=alert(1)>".to_vec()).await, StatusCode::BAD_REQUEST);
    assert_eq!(put_raw(&app, &format!("{sp_path}/icon"), &tb, png.clone()).await, StatusCode::FORBIDDEN);
    assert_eq!(put_raw(&app, &format!("{sp_path}/icon"), &ta, png.clone()).await, StatusCode::OK);
    let req = Request::builder().uri(format!("{sp_path}/icon")).header("authorization", format!("Bearer {tc}")).body(Body::empty()).unwrap();
    let res = crate::api::router(app.clone()).oneshot(req).await.unwrap();
    assert_eq!(res.headers()["content-type"], "image/png");
    assert_eq!(res.headers()["x-content-type-options"], "nosniff");

    // Channels: create, rename with a topic, reorder, delete (never the last one).
    let (_, raids) = call(&app, "POST", &format!("{sp_path}/channels"), Some(&ta), json!({ "name": "raids" })).await;
    let raids = raids["id"].as_str().unwrap().to_string();
    assert_eq!(call(&app, "PATCH", &format!("/v1/channels/{raids}"), Some(&tb), json!({ "topic": "x" })).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&app, "PATCH", &format!("/v1/channels/{raids}"), Some(&ta), json!({ "name": "raid-plans", "topic": "Who's in?" })).await.0, StatusCode::OK);
    assert_eq!(call(&app, "PUT", &format!("{sp_path}/channel-order"), Some(&ta), json!({ "channels": [raids] })).await.0, StatusCode::BAD_REQUEST, "must list every channel");
    assert_eq!(call(&app, "PUT", &format!("{sp_path}/channel-order"), Some(&ta), json!({ "channels": [raids, general] })).await.0, StatusCode::OK);
    let (_, got) = call(&app, "GET", &sp_path, Some(&tb), Value::Null).await;
    assert_eq!((got["channels"][0]["name"].as_str(), got["channels"][0]["topic"].as_str()), (Some("raid-plans"), Some("Who's in?")));
    assert_eq!(call(&app, "DELETE", &format!("/v1/channels/{raids}"), Some(&ta), Value::Null).await.0, StatusCode::OK);
    assert_eq!(call(&app, "DELETE", &format!("/v1/channels/{general}"), Some(&ta), Value::Null).await.0, StatusCode::BAD_REQUEST, "last channel stays");

    // Roles: only the owner promotes; an admin can remove members but not other admins or the owner.
    assert_eq!(call(&app, "PUT", &format!("{sp_path}/members/carol/role"), Some(&tb), json!({ "role": "admin" })).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&app, "PUT", &format!("{sp_path}/members/bob/role"), Some(&ta), json!({ "role": "owner" })).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&app, "PUT", &format!("{sp_path}/members/bob/role"), Some(&ta), json!({ "role": "admin" })).await.0, StatusCode::OK);
    assert_eq!(call(&app, "PATCH", &sp_path, Some(&tb), json!({ "description": "Admins can edit" })).await.0, StatusCode::OK);
    assert_eq!(call(&app, "DELETE", &format!("{sp_path}/members/alice"), Some(&tb), Value::Null).await.0, StatusCode::FORBIDDEN);

    // Bans: an admin bans Carol, lists and lifts the ban.
    assert_eq!(call(&app, "DELETE", &format!("{sp_path}/members/carol?ban=true"), Some(&tb), Value::Null).await.0, StatusCode::OK);
    let (_, bans) = call(&app, "GET", &format!("{sp_path}/bans"), Some(&tb), Value::Null).await;
    assert_eq!(bans["bans"], json!(["carol"]));
    assert_eq!(call(&app, "DELETE", &format!("{sp_path}/bans/carol"), Some(&tb), Value::Null).await.0, StatusCode::OK);

    // Invites: listed by short ID, revoked ones stop working.
    let (_, inv2) = call(&app, "POST", &format!("{sp_path}/invites"), Some(&ta), json!({ "max_uses": 3, "expires_in": 3600 })).await;
    let (_, list) = call(&app, "GET", &format!("{sp_path}/invites"), Some(&ta), Value::Null).await;
    let ids: Vec<String> = list["invites"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap().to_string()).collect();
    assert_eq!(ids.len(), 2, "the first invite still has uses left");
    for id in &ids {
        assert_eq!(call(&app, "DELETE", &format!("{sp_path}/invites/{id}"), Some(&ta), Value::Null).await.0, StatusCode::OK);
    }
    assert_eq!(call(&app, "POST", &format!("/v1/invites/{}/accept", inv2["code"].as_str().unwrap()), Some(&tc), Value::Null).await.0, StatusCode::NOT_FOUND);

    // Ownership: transfer to Bob; Alice becomes an admin and can no longer delete the server.
    assert_eq!(call(&app, "POST", &format!("{sp_path}/transfer"), Some(&tb), json!({ "username": "bob" })).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&app, "POST", &format!("{sp_path}/transfer"), Some(&ta), json!({ "username": "bob" })).await.0, StatusCode::OK);
    let (_, got) = call(&app, "GET", &sp_path, Some(&ta), Value::Null).await;
    assert_eq!(got["your_role"], "admin");
    assert_eq!(call(&app, "DELETE", &sp_path, Some(&ta), Value::Null).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&app, "DELETE", &sp_path, Some(&tb), Value::Null).await.0, StatusCode::OK);
    assert_eq!(call(&app, "GET", &sp_path, Some(&tb), Value::Null).await.0, StatusCode::NOT_FOUND);
}

#[test]
fn upgrades_an_existing_database_in_place() {
    let dir = std::env::temp_dir().join(format!("comms-migrate-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("old.db");
    let _ = std::fs::remove_file(&path);
    // A database from before settings existed: no description, icon, topic or position columns.
    let c = rusqlite::Connection::open(&path).unwrap();
    c.execute_batch(
        "CREATE TABLE spaces (id TEXT PRIMARY KEY, name TEXT NOT NULL, owner INTEGER NOT NULL, created INTEGER NOT NULL);
         CREATE TABLE channels (id TEXT PRIMARY KEY, space TEXT NOT NULL, name TEXT NOT NULL, epoch INTEGER NOT NULL DEFAULT 0, next_seq INTEGER NOT NULL DEFAULT 1, created INTEGER NOT NULL);
         INSERT INTO spaces VALUES ('s1', 'Old server', 1, 0);
         INSERT INTO channels (id, space, name, created) VALUES ('c1', 's1', 'general', 0);",
    )
    .unwrap();
    drop(c);
    let db = Db::open(&path).unwrap();
    let (name, desc, topic): (String, String, String) = db
        .with(|c| c.query_row("SELECT s.name, s.description, ch.topic FROM spaces s JOIN channels ch ON ch.space = s.id", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))))
        .unwrap();
    assert_eq!((name.as_str(), desc.as_str(), topic.as_str()), ("Old server", "", ""));
    drop(db);
    let _ = Db::open(&path).unwrap(); // a second open changes nothing
    let _ = std::fs::remove_dir_all(&dir);
}
