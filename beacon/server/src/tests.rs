//! End-to-end tests of the API against an in-memory database.

use crate::util::b64;
use crate::{app, db::Db, Config, Shared};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use ed25519_dalek::{Signer, SigningKey};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const SERVER: &str = "https://beacon.test";

fn server() -> (Shared, String) {
    let db = Db::memory();
    let code = db.setup_code_if_new().unwrap().unwrap();
    let blob_dir = std::env::temp_dir().join(format!("beacon-blobs-{}-{}", std::process::id(), crate::util::new_id()));
    (app(Config { public_url: SERVER.into(), admin_only_communities: false, blob_dir, max_file: 64 * 1024, community_quota: 200 * 1024, media_addr: None, media_bind: String::new() }, db), code)
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
        let sig = self.sign(&format!("lodestone-beacon/v1 register {SERVER} {user} {}", self.public()));
        call(app, "POST", "/v1/register", None, json!({ "username": user, "device_key": self.public(), "signature": sig, "invite": invite })).await
    }
    async fn sign_in(&self, app: &Shared) -> String {
        let (s, c) = call(app, "POST", "/v1/challenge", None, json!({ "device_key": self.public() })).await;
        assert_eq!(s, StatusCode::OK, "{c}");
        let nonce = c["nonce"].as_str().unwrap();
        let sig = self.sign(&format!("lodestone-beacon/v1 login {SERVER} {nonce}"));
        let (s, r) = call(app, "POST", "/v1/session", None, json!({ "device_key": self.public(), "nonce": nonce, "signature": sig })).await;
        assert_eq!(s, StatusCode::OK, "{r}");
        r["token"].as_str().unwrap().to_string()
    }
}

#[tokio::test]
async fn setup_code_works_once_and_signatures_are_bound_to_the_server() {
    let (app, code) = server();
    let a = Device::new();
    // A signature made for another server is refused.
    let wrong = a.sign(&format!("lodestone-beacon/v1 register https://other.test alice {}", a.public()));
    let (s, _) = call(&app, "POST", "/v1/register", None, json!({ "username": "alice", "device_key": a.public(), "signature": wrong, "invite": code })).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, r) = a.register(&app, "alice", &code).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["server_admin"], true);
    // The setup code is gone, and no code means no account.
    let (s, _) = Device::new().register(&app, "bob", &code).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = Device::new().register(&app, "bob", "nonsense").await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn sign_in_challenges_are_single_use() {
    let (app, code) = server();
    let a = Device::new();
    a.register(&app, "alice", &code).await;
    let (_, c) = call(&app, "POST", "/v1/challenge", None, json!({ "device_key": a.public() })).await;
    let nonce = c["nonce"].as_str().unwrap();
    let sig = a.sign(&format!("lodestone-beacon/v1 login {SERVER} {nonce}"));
    let body = json!({ "device_key": a.public(), "nonce": nonce, "signature": sig });
    assert_eq!(call(&app, "POST", "/v1/session", None, body.clone()).await.0, StatusCode::OK);
    assert_eq!(call(&app, "POST", "/v1/session", None, body).await.0, StatusCode::UNAUTHORIZED);
    // Unknown devices get the same answer as any refused challenge.
    assert_eq!(call(&app, "POST", "/v1/challenge", None, json!({ "device_key": Device::new().public() })).await.0, StatusCode::UNAUTHORIZED);
    // A bad token gets nothing.
    assert_eq!(call(&app, "GET", "/v1/me", Some("not-a-token"), Value::Null).await.0, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn communities_invites_and_encrypted_messages() {
    let (app, code) = server();
    let (alice, bob, carol) = (Device::new(), Device::new(), Device::new());
    alice.register(&app, "alice", &code).await;
    let ta = alice.sign_in(&app).await;

    // Alice makes a community; it comes with a general channel.
    let (s, community) = call(&app, "POST", "/v1/communities", Some(&ta), json!({ "name": "Friday Raids" })).await;
    assert_eq!(s, StatusCode::OK, "{community}");
    let sid = community["id"].as_str().unwrap().to_string();
    let ch = community["channels"][0]["id"].as_str().unwrap().to_string();

    // An invite brings Bob in at registration.
    let (_, inv) = call(&app, "POST", &format!("/v1/communities/{sid}/invites"), Some(&ta), json!({ "max_uses": 1, "expires_in": 3600 })).await;
    let invite = inv["code"].as_str().unwrap().to_string();
    assert!(inv["link"].as_str().unwrap().starts_with("https://beacon.test/join/"));
    let (s, preview) = call(&app, "GET", &format!("/v1/invites/{invite}"), None, Value::Null).await;
    assert_eq!((s, preview["name"].as_str()), (StatusCode::OK, Some("Friday Raids")));
    let (s, r) = bob.register(&app, "bob", &invite).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["joined_community"], sid);
    let tb = bob.sign_in(&app).await;
    // The invite was single-use.
    assert_eq!(Device::new().register(&app, "dave", &invite).await.0, StatusCode::FORBIDDEN);

    // Carol is on the server (via another community) but not in Alice's.
    let (_, other) = call(&app, "POST", "/v1/communities", Some(&tb), json!({ "name": "Bob's place" })).await;
    let (_, inv2) = call(&app, "POST", &format!("/v1/communities/{}/invites", other["id"].as_str().unwrap()), Some(&tb), json!({ "max_uses": 5, "expires_in": 3600 })).await;
    carol.register(&app, "carol", inv2["code"].as_str().unwrap()).await;
    let tc = carol.sign_in(&app).await;
    assert_eq!(call(&app, "GET", &format!("/v1/communities/{sid}"), Some(&tc), Value::Null).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&app, "GET", &format!("/v1/channels/{ch}/messages"), Some(&tc), Value::Null).await.0, StatusCode::NOT_FOUND);

    // Members see each other's device keys (clients check MLS credentials against them).
    let (_, sp) = call(&app, "GET", &format!("/v1/communities/{sid}"), Some(&ta), Value::Null).await;
    let members = sp["members"].as_array().unwrap();
    assert_eq!(members.len(), 2);
    assert!(members.iter().any(|m| m["username"] == "bob" && m["devices"][0] == bob.public()));

    // Key packages: Bob uploads, Alice claims one per device, and it's used up.
    call(&app, "POST", "/v1/keypackages", Some(&tb), json!({ "key_packages": [b64(b"kp-1"), b64(b"kp-2")] })).await;
    let (_, claim) = call(&app, "POST", "/v1/keypackages/claim", Some(&ta), json!({ "username": "bob" })).await;
    assert_eq!(claim["devices"][0]["key_package"], b64(b"kp-1"));
    let (_, claim) = call(&app, "POST", "/v1/keypackages/claim", Some(&ta), json!({ "username": "bob" })).await;
    assert_eq!(claim["devices"][0]["key_package"], b64(b"kp-2"));
    // Carol shares a community with Bob, but not with Alice: Alice can't fetch Carol's packages.
    assert_eq!(call(&app, "POST", "/v1/keypackages/claim", Some(&ta), json!({ "username": "carol" })).await.0, StatusCode::NOT_FOUND);

    // Commits are ordered: one per epoch.
    let send = |kind: &str, epoch: i64, body: &[u8]| json!({ "kind": kind, "epoch": epoch, "body": b64(body) });
    assert_eq!(call(&app, "POST", &format!("/v1/channels/{ch}/messages"), Some(&ta), send("commit", 0, b"add bob")).await.0, StatusCode::OK);
    assert_eq!(call(&app, "POST", &format!("/v1/channels/{ch}/messages"), Some(&tb), send("commit", 0, b"late commit")).await.0, StatusCode::CONFLICT);
    let (_, sent) = call(&app, "POST", &format!("/v1/channels/{ch}/messages"), Some(&ta), send("application", 1, b"ciphertext")).await;
    assert_eq!(sent["seq"], 2);

    // The welcome reaches Bob's device once.
    let (_, w) = call(&app, "POST", "/v1/welcomes", Some(&ta), json!({ "channel": ch, "to": [bob.public(), carol.public()], "body": b64(b"welcome") })).await;
    assert_eq!(w["delivered"], 1, "Carol isn't in the community, so her device gets nothing");
    let (_, got) = call(&app, "GET", "/v1/welcomes", Some(&tb), Value::Null).await;
    assert_eq!(got["welcomes"][0]["body"], b64(b"welcome"));
    let (_, again) = call(&app, "GET", "/v1/welcomes", Some(&tb), Value::Null).await;
    assert_eq!(again["welcomes"].as_array().unwrap().len(), 0);

    // Bob reads the history: the server stored exactly the bytes sent, in order.
    let (_, hist) = call(&app, "GET", &format!("/v1/channels/{ch}/messages?after=0"), Some(&tb), Value::Null).await;
    let msgs = hist["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 2);
    assert_eq!((msgs[1]["body"].clone(), msgs[1]["sender_name"].clone()), (json!(b64(b"ciphertext")), json!("alice")));

    // Members can't remove the owner; the owner can remove and ban.
    assert_eq!(call(&app, "DELETE", &format!("/v1/communities/{sid}/members/alice"), Some(&tb), Value::Null).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&app, "DELETE", &format!("/v1/communities/{sid}/members/bob?ban=true"), Some(&ta), Value::Null).await.0, StatusCode::OK);
    assert_eq!(call(&app, "GET", &format!("/v1/channels/{ch}/messages"), Some(&tb), Value::Null).await.0, StatusCode::NOT_FOUND);
    let (_, inv3) = call(&app, "POST", &format!("/v1/communities/{sid}/invites"), Some(&ta), json!({ "max_uses": 5, "expires_in": 3600 })).await;
    assert_eq!(call(&app, "POST", &format!("/v1/invites/{}/accept", inv3["code"].as_str().unwrap()), Some(&tb), Value::Null).await.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn rejects_bad_input() {
    let (app, code) = server();
    let a = Device::new();
    assert_eq!(a.register(&app, "No Communities!", &code).await.0, StatusCode::BAD_REQUEST);
    a.register(&app, "alice", &code).await;
    let t = a.sign_in(&app).await;
    assert_eq!(call(&app, "POST", "/v1/communities", Some(&t), json!({ "name": "  " })).await.0, StatusCode::BAD_REQUEST);
    let (_, sp) = call(&app, "POST", "/v1/communities", Some(&t), json!({ "name": "x" })).await;
    let ch = sp["channels"][0]["id"].as_str().unwrap();
    let big = b64(&vec![0u8; 64 * 1024 + 1]);
    assert_eq!(call(&app, "POST", &format!("/v1/channels/{ch}/messages"), Some(&t), json!({ "kind": "application", "epoch": 0, "body": big })).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&app, "POST", &format!("/v1/channels/{ch}/messages"), Some(&t), json!({ "kind": "shout", "epoch": 0, "body": b64(b"x") })).await.0, StatusCode::BAD_REQUEST);
    let sid = sp["id"].as_str().unwrap();
    assert_eq!(call(&app, "POST", &format!("/v1/communities/{sid}/invites"), Some(&t), json!({ "max_uses": 0, "expires_in": 3600 })).await.0, StatusCode::BAD_REQUEST);
}

/// Upload helper for raw bodies (icons).
async fn put_raw(app: &Shared, path: &str, token: &str, body: Vec<u8>) -> StatusCode {
    let req = Request::builder().method("PUT").uri(path).header("authorization", format!("Bearer {token}")).body(Body::from(body)).unwrap();
    crate::api::router(app.clone()).oneshot(req).await.unwrap().status()
}

#[tokio::test]
async fn server_settings_and_permissions() {
    let (app, code) = server();
    let (alice, bob, carol) = (Device::new(), Device::new(), Device::new());
    alice.register(&app, "alice", &code).await;
    let ta = alice.sign_in(&app).await;
    let (_, sp) = call(&app, "POST", "/v1/communities", Some(&ta), json!({ "name": "Raid night" })).await;
    let sid = sp["id"].as_str().unwrap().to_string();
    let general = sp["channels"][0]["id"].as_str().unwrap().to_string();
    let (_, inv) = call(&app, "POST", &format!("/v1/communities/{sid}/invites"), Some(&ta), json!({ "max_uses": 5, "expires_in": 3600 })).await;
    let code5 = inv["code"].as_str().unwrap().to_string();
    bob.register(&app, "bob", &code5).await;
    carol.register(&app, "carol", &code5).await;
    let (tb, tc) = (bob.sign_in(&app).await, carol.sign_in(&app).await);
    let sp_path = format!("/v1/communities/{sid}");

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

    // Banner: same rules as the icon.
    assert_eq!(put_raw(&app, &format!("{sp_path}/banner"), &ta, b"<svg onload=alert(1)>".to_vec()).await, StatusCode::BAD_REQUEST);
    assert_eq!(put_raw(&app, &format!("{sp_path}/banner"), &tb, png.clone()).await, StatusCode::FORBIDDEN);
    assert_eq!(put_raw(&app, &format!("{sp_path}/banner"), &ta, png.clone()).await, StatusCode::OK);
    let req = Request::builder().uri(format!("{sp_path}/banner")).header("authorization", format!("Bearer {tc}")).body(Body::empty()).unwrap();
    let res = crate::api::router(app.clone()).oneshot(req).await.unwrap();
    assert_eq!((res.headers()["content-type"].to_str().unwrap(), res.headers()["x-content-type-options"].to_str().unwrap()), ("image/png", "nosniff"));
    let (_, got) = call(&app, "GET", &sp_path, Some(&tc), Value::Null).await;
    assert_eq!(got["has_banner"].as_bool(), Some(true));
    assert_eq!(call(&app, "DELETE", &format!("{sp_path}/banner"), Some(&tb), Value::Null).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&app, "DELETE", &format!("{sp_path}/banner"), Some(&ta), Value::Null).await.0, StatusCode::OK);

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

    // Ownership: transfer to Bob; Alice becomes an admin and can no longer delete the community.
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
    let dir = std::env::temp_dir().join(format!("beacon-migrate-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("old.db");
    let _ = std::fs::remove_file(&path);
    // A database from before settings existed: no description, icon, topic or position columns.
    let c = rusqlite::Connection::open(&path).unwrap();
    c.execute_batch(
        "CREATE TABLE communities (id TEXT PRIMARY KEY, name TEXT NOT NULL, owner INTEGER NOT NULL, created INTEGER NOT NULL);
         CREATE TABLE channels (id TEXT PRIMARY KEY, community TEXT NOT NULL, name TEXT NOT NULL, epoch INTEGER NOT NULL DEFAULT 0, next_seq INTEGER NOT NULL DEFAULT 1, created INTEGER NOT NULL);
         INSERT INTO communities VALUES ('s1', 'Old community', 1, 0);
         INSERT INTO channels (id, community, name, created) VALUES ('c1', 's1', 'general', 0);",
    )
    .unwrap();
    drop(c);
    let db = Db::open(&path).unwrap();
    let (name, desc, topic): (String, String, String) = db
        .with(|c| c.query_row("SELECT s.name, s.description, ch.topic FROM communities s JOIN channels ch ON ch.community = s.id", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))))
        .unwrap();
    assert_eq!((name.as_str(), desc.as_str(), topic.as_str()), ("Old community", "", ""));
    drop(db);
    let _ = Db::open(&path).unwrap(); // a second open changes nothing
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn attachments_are_members_only_limited_and_cleaned_up() {
    let (app, code) = server();
    let (alice, bob) = (Device::new(), Device::new());
    alice.register(&app, "alice", &code).await;
    let ta = alice.sign_in(&app).await;
    let (_, sp) = call(&app, "POST", "/v1/communities", Some(&ta), json!({ "name": "Raid night" })).await;
    let sid = sp["id"].as_str().unwrap().to_string();
    let ch = sp["channels"][0]["id"].as_str().unwrap().to_string();
    let upload = |token: String, channel: String, body: Vec<u8>| {
        let app = app.clone();
        async move {
            let req = Request::builder().method("POST").uri(format!("/v1/channels/{channel}/blobs")).header("authorization", format!("Bearer {token}")).body(Body::from(body)).unwrap();
            let res = crate::api::router(app).oneshot(req).await.unwrap();
            let status = res.status();
            let bytes = res.into_body().collect().await.unwrap().to_bytes();
            (status, serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null))
        }
    };
    let (s, up) = upload(ta.clone(), ch.clone(), vec![7u8; 1000]).await;
    assert_eq!(s, StatusCode::OK, "{up}");
    let id = up["id"].as_str().unwrap().to_string();

    // Downloads come back byte for byte, as an inert attachment.
    let req = Request::builder().uri(format!("/v1/blobs/{id}")).header("authorization", format!("Bearer {ta}")).body(Body::empty()).unwrap();
    let res = crate::api::router(app.clone()).oneshot(req).await.unwrap();
    assert_eq!(res.headers()["content-type"], "application/octet-stream");
    assert_eq!(res.headers()["content-disposition"], "attachment");
    assert_eq!(res.into_body().collect().await.unwrap().to_bytes().as_ref(), &[7u8; 1000][..]);

    // Outsiders get nothing; too-big files and a full quota are refused.
    let (_, other) = call(&app, "POST", "/v1/communities", Some(&ta), json!({ "name": "Other" })).await;
    let (_, inv) = call(&app, "POST", &format!("/v1/communities/{}/invites", other["id"].as_str().unwrap()), Some(&ta), json!({ "max_uses": 1, "expires_in": 3600 })).await;
    bob.register(&app, "bob", inv["code"].as_str().unwrap()).await;
    let tb = bob.sign_in(&app).await;
    assert_eq!(call(&app, "GET", &format!("/v1/blobs/{id}"), Some(&tb), Value::Null).await.0, StatusCode::NOT_FOUND);
    assert_eq!(upload(tb.clone(), ch.clone(), vec![1u8; 10]).await.0, StatusCode::NOT_FOUND);
    assert_eq!(upload(ta.clone(), ch.clone(), vec![1u8; 64 * 1024 + 1]).await.0, StatusCode::BAD_REQUEST, "over the file limit");
    for _ in 0..3 {
        assert_eq!(upload(ta.clone(), ch.clone(), vec![1u8; 60 * 1024]).await.0, StatusCode::OK);
    }
    assert_eq!(upload(ta.clone(), ch.clone(), vec![1u8; 60 * 1024]).await.0, StatusCode::BAD_REQUEST, "over the community's quota");
    assert_eq!(call(&app, "GET", "/v1/blobs/..%2Fbeacon.db", Some(&ta), Value::Null).await.0, StatusCode::NOT_FOUND);

    // Deleting the community removes its files from disk.
    let files = || std::fs::read_dir(&app.cfg.blob_dir).map(|d| d.count()).unwrap_or(0);
    assert_eq!(files(), 4);
    assert_eq!(call(&app, "DELETE", &format!("/v1/communities/{sid}"), Some(&ta), Value::Null).await.0, StatusCode::OK);
    assert_eq!(files(), 0);
    let _ = std::fs::remove_dir_all(&app.cfg.blob_dir);
}

#[tokio::test]
async fn presence_and_status() {
    let (app, code) = server();
    let (alice, bob) = (Device::new(), Device::new());
    alice.register(&app, "alice", &code).await;
    let ta = alice.sign_in(&app).await;
    let (_, sp) = call(&app, "POST", "/v1/communities", Some(&ta), json!({ "name": "Raid night" })).await;
    let sid = sp["id"].as_str().unwrap().to_string();
    let (_, inv) = call(&app, "POST", &format!("/v1/communities/{sid}/invites"), Some(&ta), json!({ "max_uses": 1, "expires_in": 3600 })).await;
    bob.register(&app, "bob", inv["code"].as_str().unwrap()).await;
    let presence = |community: Value, who: &str| {
        let m = community["members"].as_array().unwrap().iter().find(|m| m["username"] == who).unwrap().clone();
        (m["presence"].as_str().unwrap().to_string(), m["status_text"].as_str().unwrap().to_string())
    };
    let sp_path = format!("/v1/communities/{sid}");

    // Bob registered but his app hasn't checked in since this server started: offline.
    app.seen.lock().unwrap().clear();
    let (_, s) = call(&app, "GET", &sp_path, Some(&ta), Value::Null).await;
    assert_eq!(presence(s.clone(), "bob").0, "offline");
    assert_eq!(presence(s, "alice").0, "online", "Alice just made a request");

    // Bob checks in and picks a status.
    let tb = bob.sign_in(&app).await;
    assert_eq!(call(&app, "PUT", "/v1/me/status", Some(&tb), json!({ "status": "busy", "text": "In a raid" })).await.0, StatusCode::OK);
    let (_, s) = call(&app, "GET", &sp_path, Some(&ta), Value::Null).await;
    assert_eq!(presence(s, "bob"), ("busy".into(), "In a raid".into()));
    let (_, me) = call(&app, "GET", "/v1/me", Some(&tb), Value::Null).await;
    assert_eq!((me["status"].as_str(), me["status_text"].as_str()), (Some("busy"), Some("In a raid")));

    // Invisible looks offline to others, message and all.
    call(&app, "PUT", "/v1/me/status", Some(&tb), json!({ "status": "invisible", "text": "shh" })).await;
    let (_, s) = call(&app, "GET", &sp_path, Some(&ta), Value::Null).await;
    assert_eq!(presence(s, "bob"), ("offline".into(), String::new()));

    // Bad input.
    assert_eq!(call(&app, "PUT", "/v1/me/status", Some(&tb), json!({ "status": "asleep" })).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&app, "PUT", "/v1/me/status", Some(&tb), json!({ "status": "away", "text": "x".repeat(81) })).await.0, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn pairing_needs_a_code_and_approval() {
    let (app, code) = server();
    let (laptop, desktop, stranger) = (Device::new(), Device::new(), Device::new());
    laptop.register(&app, "alice", &code).await;
    let ta = laptop.sign_in(&app).await;
    let claim = |d: &Device, code: &str| json!({ "code": code, "device_key": d.public(), "signature": d.sign(&format!("lodestone-beacon/v1 pair {SERVER} {code} {}", d.public())) });

    let (_, p) = call(&app, "POST", "/v1/pairing", Some(&ta), Value::Null).await;
    let pcode = p["code"].as_str().unwrap().to_string();

    // A signature for another code or server is refused; a wrong code finds nothing.
    let mut forged = claim(&desktop, &pcode);
    forged["signature"] = json!(desktop.sign(&format!("lodestone-beacon/v1 pair https://other.test {pcode} {}", desktop.public())));
    assert_eq!(call(&app, "POST", "/v1/pairing/claim", None, forged).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(call(&app, "POST", "/v1/pairing/claim", None, claim(&desktop, "nope")).await.0, StatusCode::NOT_FOUND);

    // The desktop claims the code; a second device can't take the same code.
    let (s, c) = call(&app, "POST", "/v1/pairing/claim", None, claim(&desktop, &pcode)).await;
    assert_eq!(s, StatusCode::OK, "{c}");
    assert_eq!(c["username"], "alice");
    assert_eq!(call(&app, "POST", "/v1/pairing/claim", None, claim(&stranger, &pcode)).await.0, StatusCode::CONFLICT);

    // Not a device yet: it can't sign in until the laptop approves.
    assert_eq!(call(&app, "POST", "/v1/challenge", None, json!({ "device_key": desktop.public() })).await.0, StatusCode::UNAUTHORIZED);
    let status = |d: &Device| json!({ "code": pcode, "device_key": d.public() });
    assert_eq!(call(&app, "POST", "/v1/pairing/status", None, status(&desktop)).await.1["status"], "waiting");
    let (_, w) = call(&app, "GET", "/v1/pairing", Some(&ta), Value::Null).await;
    assert_eq!(w["waiting"][0]["device_key"], desktop.public());
    assert_eq!(w["waiting"][0]["fingerprint"], c["fingerprint"], "both screens show the same fingerprint");
    assert_eq!(call(&app, "POST", "/v1/pairing/approve", Some(&ta), json!({ "device_key": desktop.public() })).await.0, StatusCode::OK);
    assert_eq!(call(&app, "POST", "/v1/pairing/status", None, status(&desktop)).await.1["status"], "approved");
    let td = desktop.sign_in(&app).await;
    let (_, me) = call(&app, "GET", "/v1/me", Some(&td), Value::Null).await;
    assert_eq!((me["username"].as_str(), me["devices"].as_array().unwrap().len()), (Some("alice"), 2));

    // The code is used up.
    assert_eq!(call(&app, "POST", "/v1/pairing/claim", None, claim(&stranger, &pcode)).await.0, StatusCode::NOT_FOUND);

    // A refused device stays out.
    let (_, p2) = call(&app, "POST", "/v1/pairing", Some(&ta), Value::Null).await;
    let c2 = p2["code"].as_str().unwrap().to_string();
    call(&app, "POST", "/v1/pairing/claim", None, claim(&stranger, &c2)).await;
    call(&app, "POST", "/v1/pairing/refuse", Some(&ta), json!({ "device_key": stranger.public() })).await;
    assert_eq!(call(&app, "POST", "/v1/pairing/status", None, json!({ "code": c2, "device_key": stranger.public() })).await.1["status"], "refused");
    assert_eq!(call(&app, "POST", "/v1/pairing/approve", Some(&ta), json!({ "device_key": stranger.public() })).await.0, StatusCode::NOT_FOUND);

    // Removing: not the device you're on; the other one loses its session.
    assert_eq!(call(&app, "POST", "/v1/devices/remove", Some(&ta), json!({ "device_key": laptop.public() })).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&app, "POST", "/v1/devices/remove", Some(&ta), json!({ "device_key": desktop.public() })).await.0, StatusCode::OK);
    assert_eq!(call(&app, "GET", "/v1/me", Some(&td), Value::Null).await.0, StatusCode::UNAUTHORIZED);
}

// ---------- voice relay ----------

mod voice_test {
    use std::net::UdpSocket;
    use std::time::{Duration, Instant};
    use str0m::change::{SdpAnswer, SdpPendingOffer};
    use str0m::media::{Direction, Frequency, MediaKind, MediaTime, Mid};
    use str0m::net::{Protocol, Receive};
    use str0m::{Candidate, Event, Input, Output, Rtc};

    pub struct Peer {
        pub rtc: Rtc,
        pub socket: UdpSocket,
        pub send: Mid,
        pub recv: Vec<Mid>,
        pub pending: Option<SdpPendingOffer>,
    }

    /// A test client's offer: one send-only audio track and 16 receive-only ones, like Beacon will use.
    pub fn offer() -> (Peer, serde_json::Value) {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut rtc = Rtc::builder().clear_codecs().enable_opus(true, false).build(Instant::now());
        rtc.add_local_candidate(Candidate::host(socket.local_addr().unwrap(), "udp").unwrap());
        let mut api = rtc.sdp_api();
        let send = api.add_media(MediaKind::Audio, Direction::SendOnly, None, None, None);
        let recv: Vec<Mid> = (0..super::super::voice::SLOTS).map(|_| api.add_media(MediaKind::Audio, Direction::RecvOnly, None, None, None)).collect();
        let (offer, pending) = api.apply().unwrap();
        (Peer { rtc, socket, send, recv, pending: Some(pending) }, serde_json::to_value(&offer).unwrap())
    }

    pub fn accept(p: &mut Peer, answer: serde_json::Value) {
        let answer: SdpAnswer = serde_json::from_value(answer).unwrap();
        p.rtc.sdp_api().accept_answer(p.pending.take().unwrap(), answer).unwrap();
    }

    /// Run a client for `secs`: send a frame labelled `label` every 20 ms once connected; return what
    /// arrived as (receive track index, payload).
    pub fn drive(mut p: Peer, label: &'static str, secs: u64) -> Vec<(usize, Vec<u8>)> {
        let start = Instant::now();
        let mut got = Vec::new();
        let mut buf = vec![0u8; 2000];
        let mut n = 0u64;
        let mut next = Instant::now();
        while start.elapsed() < Duration::from_secs(secs) {
            let mut timeout = Instant::now() + Duration::from_millis(20);
            loop {
                match p.rtc.poll_output().unwrap() {
                    Output::Timeout(t) => {
                        timeout = timeout.min(t);
                        break;
                    }
                    Output::Transmit(t) => {
                        let _ = p.socket.send_to(&t.contents, t.destination);
                    }
                    Output::Event(Event::MediaData(d)) => {
                        if let Some(i) = p.recv.iter().position(|m| *m == d.mid) {
                            got.push((i, d.data.to_vec()));
                        }
                    }
                    Output::Event(_) => {}
                }
            }
            if p.rtc.is_connected() && Instant::now() >= next {
                if let Some(w) = p.rtc.writer(p.send) {
                    let pt = w.payload_params().next().map(|x| x.pt());
                    if let Some(pt) = pt {
                        let _ = w.write(pt, Instant::now(), MediaTime::new(n * 960, Frequency::FORTY_EIGHT_KHZ), format!("{label}-{n}").into_bytes());
                        n += 1;
                    }
                }
                next = Instant::now() + Duration::from_millis(20);
                continue;
            }
            let wait = timeout.saturating_duration_since(Instant::now()).max(Duration::from_millis(1));
            p.socket.set_read_timeout(Some(wait)).unwrap();
            if let Ok((len, source)) = p.socket.recv_from(&mut buf) {
                if let Ok(contents) = buf[..len].try_into() {
                    let input = Input::Receive(Instant::now(), Receive { proto: Protocol::Udp, source, destination: p.socket.local_addr().unwrap(), contents });
                    let _ = p.rtc.handle_input(input);
                }
            }
            let _ = p.rtc.handle_input(Input::Timeout(Instant::now()));
        }
        got
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn voice_relay_forwards_between_participants() {
    let port = std::net::UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let db = Db::memory();
    let code = db.setup_code_if_new().unwrap().unwrap();
    let blob_dir = std::env::temp_dir().join(format!("beacon-voice-{}", std::process::id()));
    let addr = format!("127.0.0.1:{port}");
    let app = app(Config { public_url: SERVER.into(), admin_only_communities: false, blob_dir, max_file: 1 << 20, community_quota: 1 << 20, media_addr: Some(addr.clone()), media_bind: addr }, db);
    let (alice, bob, carol) = (Device::new(), Device::new(), Device::new());
    alice.register(&app, "alice", &code).await;
    let ta = alice.sign_in(&app).await;
    let (_, sp) = call(&app, "POST", "/v1/communities", Some(&ta), json!({ "name": "Raid night" })).await;
    let sid = sp["id"].as_str().unwrap().to_string();
    let text = sp["channels"][0]["id"].as_str().unwrap().to_string();
    let (_, vc) = call(&app, "POST", &format!("/v1/communities/{sid}/channels"), Some(&ta), json!({ "name": "Lounge", "kind": "voice" })).await;
    let voice = vc["id"].as_str().unwrap().to_string();
    let (_, inv) = call(&app, "POST", &format!("/v1/communities/{sid}/invites"), Some(&ta), json!({ "max_uses": 1, "expires_in": 3600 })).await;
    bob.register(&app, "bob", inv["code"].as_str().unwrap()).await;
    let tb = bob.sign_in(&app).await;

    // Text channels don't take calls; outsiders can't join.
    let (_, o) = voice_test::offer();
    assert_eq!(call(&app, "POST", &format!("/v1/channels/{text}/voice/join"), Some(&ta), json!({ "offer": o })).await.0, StatusCode::BAD_REQUEST);
    let (_, other) = call(&app, "POST", "/v1/communities", Some(&ta), json!({ "name": "Other" })).await;
    let (_, inv2) = call(&app, "POST", &format!("/v1/communities/{}/invites", other["id"].as_str().unwrap()), Some(&ta), json!({ "max_uses": 1, "expires_in": 3600 })).await;
    carol.register(&app, "carol", inv2["code"].as_str().unwrap()).await;
    let tc = carol.sign_in(&app).await;
    assert_eq!(call(&app, "POST", &format!("/v1/channels/{voice}/voice/join"), Some(&tc), json!({ "offer": o })).await.0, StatusCode::NOT_FOUND);

    // Alice and Bob join and get slots 0 and 1.
    let (mut pa, oa) = voice_test::offer();
    let (s, ja) = call(&app, "POST", &format!("/v1/channels/{voice}/voice/join"), Some(&ta), json!({ "offer": oa })).await;
    assert_eq!(s, StatusCode::OK, "{ja}");
    voice_test::accept(&mut pa, ja["answer"].clone());
    let (mut pb, ob) = voice_test::offer();
    let (_, jb) = call(&app, "POST", &format!("/v1/channels/{voice}/voice/join"), Some(&tb), json!({ "offer": ob })).await;
    voice_test::accept(&mut pb, jb["answer"].clone());
    assert_eq!((ja["slot"].as_u64(), jb["slot"].as_u64()), (Some(0), Some(1)));
    let (_, st) = call(&app, "GET", &format!("/v1/channels/{voice}/voice"), Some(&tb), Value::Null).await;
    assert_eq!(st["participants"].as_array().unwrap().len(), 2);
    let (_, community) = call(&app, "GET", &format!("/v1/communities/{sid}"), Some(&ta), Value::Null).await;
    assert_eq!(community["channels"][1]["in_call"], json!(["alice", "bob"]));

    // Both talk for three seconds through the relay.
    let ha = std::thread::spawn(move || voice_test::drive(pa, "alice", 3));
    let hb = std::thread::spawn(move || voice_test::drive(pb, "bob", 3));
    let (got_a, got_b) = (ha.join().unwrap(), hb.join().unwrap());
    let from = |got: &[(usize, Vec<u8>)], slot: usize, who: &str| got.iter().filter(|(i, d)| *i == slot && String::from_utf8_lossy(d).starts_with(who)).count();
    assert!(from(&got_b, 0, "alice") > 50, "Bob heard Alice in slot 0 ({} frames)", from(&got_b, 0, "alice"));
    assert!(from(&got_a, 1, "bob") > 50, "Alice heard Bob in slot 1 ({} frames)", from(&got_a, 1, "bob"));
    assert_eq!(from(&got_a, 0, "alice"), 0, "nobody hears themselves");

    // Leaving empties the call.
    call(&app, "POST", &format!("/v1/channels/{voice}/voice/leave"), Some(&ta), Value::Null).await;
    call(&app, "POST", &format!("/v1/channels/{voice}/voice/leave"), Some(&tb), Value::Null).await;
    let (_, st) = call(&app, "GET", &format!("/v1/channels/{voice}/voice"), Some(&tb), Value::Null).await;
    assert_eq!(st["participants"].as_array().unwrap().len(), 0);
}
