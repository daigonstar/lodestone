//! Lodestone Comms client core: this device's identity, end-to-end encryption with MLS (openmls) and the
//! host connection. Message text only exists here, on members' devices: the host gets MLS ciphertext.
//!
//! - Each text channel is an MLS group whose group ID is the channel ID.
//! - Each device's MLS credential is `<username>:<device key>`; its MLS signature key *is* the device
//!   key the host knows (one Ed25519 key; host sign-ins and MLS signatures sign different strings).
//! - Members' clients keep each channel's group in step with the host's member list: they add new
//!   members' devices (and send them Welcomes) and remove people who left. Two clients doing it at once
//!   is fine: the host accepts one commit per epoch, the other client catches up and finds nothing to do.

pub mod discover;
pub mod host;
pub mod store;

use ed25519_dalek::{Signer as _, SigningKey};
use host::{b64, unb64, Host, HostError};
use openmls::prelude::*;
use openmls::prelude::tls_codec::{Deserialize as _, Serialize as _};
use openmls_basic_credential::SignatureKeyPair;
use openmls_memory_storage::MemoryStorage;
use openmls_rust_crypto::RustCrypto;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
pub use store::Message;
use store::Store;

pub const CIPHERSUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;
const KEY_PACKAGE_TARGET: i64 = 20;

#[derive(Debug)]
pub enum Error {
    Host(HostError),
    Local(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Host(e) => write!(f, "{e}"),
            Error::Local(e) => write!(f, "{e}"),
        }
    }
}

impl From<HostError> for Error {
    fn from(e: HostError) -> Self {
        Error::Host(e)
    }
}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error::Local(format!("local database: {e}"))
    }
}

fn local(e: impl std::fmt::Debug) -> Error {
    Error::Local(format!("{e:?}"))
}

pub type Result<T> = std::result::Result<T, Error>;

/// openmls's crypto plus a key store that this client saves to its database.
pub struct Provider {
    crypto: RustCrypto,
    storage: MemoryStorage,
}

impl OpenMlsProvider for Provider {
    type CryptoProvider = RustCrypto;
    type RandProvider = RustCrypto;
    type StorageProvider = MemoryStorage;

    fn storage(&self) -> &MemoryStorage {
        &self.storage
    }
    fn crypto(&self) -> &RustCrypto {
        &self.crypto
    }
    fn rand(&self) -> &RustCrypto {
        &self.crypto
    }
}

/// `<username>:<device key>` from an MLS credential.
fn identity_of(c: &Credential) -> Option<(String, String)> {
    let basic = BasicCredential::try_from(c.clone()).ok()?;
    let s = String::from_utf8(basic.identity().to_vec()).ok()?;
    let (u, d) = s.split_once(':')?;
    Some((u.to_string(), d.to_string()))
}

/// What's inside an encrypted application message.
/// What's inside an encrypted application message: `text` (a chat message) or `react` (add or remove
/// an emoji on the message with sequence number `target`). Clients ignore kinds they don't know.
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Payload {
    t: String,
    body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    target: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    on: Option<bool>,
    /// `file`: { id, name, mime, size, key, nonce } of an encrypted attachment on the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    file: Option<Value>,
    /// `profile`: the name colour, as #rrggbb.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    color: Option<String>,
}

/// Largest profile picture.
pub const MAX_AVATAR: usize = 256 * 1024;

fn valid_color(c: &str) -> bool {
    c.len() == 7 && c.starts_with('#') && c[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

fn clean_display_name(n: &str) -> Option<String> {
    let n = n.trim();
    (n.chars().count() <= 32 && !n.chars().any(char::is_control)).then(|| n.to_string())
}

/// Largest attachment this client sends (the host may allow less).
pub const MAX_FILE: usize = 10 * 1024 * 1024;

/// The image type from a file's first bytes; anything else is sent as a plain file.
pub fn sniff_image(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if b.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if b.len() > 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// A file name that's only a name: no folders, no control characters, at most 128 characters.
fn clean_file_name(n: &str) -> String {
    let base = n.rsplit(['/', '\\']).next().unwrap_or("").trim();
    let clean: String = base.chars().filter(|c| !c.is_control()).take(128).collect();
    if clean.is_empty() || clean == "." || clean == ".." { "file".into() } else { clean }
}

/// An attachment reference from a message: checked before it's stored.
fn valid_file(f: &Value) -> bool {
    let id_ok = f["id"].as_str().is_some_and(|i| i.len() == 24 && i.bytes().all(|b| b.is_ascii_hexdigit()));
    let key_ok = f["key"].as_str().and_then(unb64).is_some_and(|k| k.len() == 32);
    let nonce_ok = f["nonce"].as_str().and_then(unb64).is_some_and(|k| k.len() == 12);
    let size_ok = f["size"].as_u64().is_some_and(|s| s as usize <= MAX_FILE);
    id_ok && key_ok && nonce_ok && size_ok && f["name"].is_string() && f["mime"].is_string()
}

/// A reaction is one emoji (possibly several code points, like 👍🏽 or 🏳️‍🌈) or a custom `:name:`.
pub fn valid_reaction(e: &str) -> bool {
    !e.is_empty() && e.len() <= 64 && !e.chars().any(|c| c.is_control() || c.is_whitespace())
}

pub struct Client {
    store: Store,
    provider: Provider,
    pub host: Host,
    key: SigningKey,
    signer: SignatureKeyPair,
    device: String,
    dir: std::path::PathBuf,
    op: tokio::sync::Mutex<()>,
    touched: std::sync::Mutex<Vec<String>>,
    /// Channels where our profile should be (re-)sent: new members can't read anything older.
    announce: std::sync::Mutex<std::collections::BTreeSet<String>>,
}

impl Client {
    /// Open (or create) this device's client for a host. One data folder per host.
    pub fn open(dir: &Path, host_url: &str) -> Result<Self> {
        let store = Store::open(dir)?;
        let host_url = host_url.trim_end_matches('/');
        match store.get("host") {
            Some(h) if h != host_url => return Err(Error::Local(format!("This data folder belongs to {h}"))),
            Some(_) => {}
            None => store.set("host", host_url)?,
        }
        let key = match store.get("device_seed").and_then(|s| unb64(&s)).and_then(|b| <[u8; 32]>::try_from(b).ok()) {
            Some(seed) => SigningKey::from_bytes(&seed),
            None => {
                let k = SigningKey::generate(&mut rand::rngs::OsRng);
                store.set("device_seed", &b64(&k.to_bytes()))?;
                k
            }
        };
        let public = key.verifying_key().to_bytes().to_vec();
        let signer = SignatureKeyPair::from_raw(SignatureScheme::ED25519, key.to_bytes().to_vec(), public.clone());
        let storage = MemoryStorage::default();
        *storage.values.write().unwrap() = store.load_mls()?;
        Ok(Self { store, provider: Provider { crypto: RustCrypto::default(), storage }, host: Host::new(host_url), key, signer, device: b64(&public), dir: dir.to_path_buf(), op: Default::default(), touched: Default::default(), announce: Default::default() })
    }

    pub fn device_key(&self) -> &str {
        &self.device
    }

    pub fn username(&self) -> Option<String> {
        self.store.get("username")
    }

    fn persist(&self) -> Result<()> {
        Ok(self.store.save_mls(&self.provider.storage.values.read().unwrap())?)
    }

    fn sign(&self, msg: &str) -> String {
        b64(&self.key.sign(msg.as_bytes()).to_bytes())
    }

    fn credential(&self) -> Result<CredentialWithKey> {
        let user = self.username().ok_or_else(|| Error::Local("Not registered yet".into()))?;
        Ok(CredentialWithKey {
            credential: BasicCredential::new(format!("{user}:{}", self.device).into_bytes()).into(),
            signature_key: self.signer.public().into(),
        })
    }

    fn create_config() -> MlsGroupCreateConfig {
        MlsGroupCreateConfig::builder()
            .ciphersuite(CIPHERSUITE)
            .use_ratchet_tree_extension(true)
            .wire_format_policy(PURE_CIPHERTEXT_WIRE_FORMAT_POLICY)
            .max_past_epochs(5)
            .build()
    }

    fn join_config() -> MlsGroupJoinConfig {
        MlsGroupJoinConfig::builder().use_ratchet_tree_extension(true).wire_format_policy(PURE_CIPHERTEXT_WIRE_FORMAT_POLICY).max_past_epochs(5).build()
    }

    fn group(&self, channel: &str) -> Result<Option<MlsGroup>> {
        MlsGroup::load(self.provider.storage(), &GroupId::from_slice(channel.as_bytes())).map_err(local)
    }

    // ---------- account ----------

    /// Make an account on the host with this device, using an invite or the host's setup code.
    pub async fn register(&self, username: &str, invite: &str) -> Result<Value> {
        let username = username.trim().to_ascii_lowercase();
        let sig = self.sign(&format!("lodestone-comms/v1 register {} {username} {}", self.host.url, self.device));
        let r = self.host.post("/v1/register", json!({ "username": username, "device_key": self.device, "signature": sig, "invite": invite.trim() })).await?;
        self.store.set("username", &username)?;
        self.sign_in().await?;
        self.top_up_key_packages().await?;
        Ok(r)
    }

    /// Sign this device in (challenge and signature).
    pub async fn sign_in(&self) -> Result<()> {
        let c = self.host.post("/v1/challenge", json!({ "device_key": self.device })).await?;
        let nonce = c["nonce"].as_str().unwrap_or_default();
        let sig = self.sign(&format!("lodestone-comms/v1 login {} {nonce}", self.host.url));
        let s = self.host.post("/v1/session", json!({ "device_key": self.device, "nonce": nonce, "signature": sig })).await?;
        self.host.set_token(s["token"].as_str().map(String::from));
        Ok(())
    }

    /// Keep enough single-use key packages on the host for others to add this device to channels.
    pub async fn top_up_key_packages(&self) -> Result<usize> {
        let waiting = self.host.get("/v1/keypackages").await?["waiting"].as_i64().unwrap_or(0);
        if waiting >= KEY_PACKAGE_TARGET / 2 {
            return Ok(0);
        }
        let cred = self.credential()?;
        let mut out = Vec::new();
        for _ in waiting..KEY_PACKAGE_TARGET {
            let bundle = KeyPackage::builder().build(CIPHERSUITE, &self.provider, &self.signer, cred.clone()).map_err(local)?;
            out.push(b64(&bundle.key_package().tls_serialize_detached().map_err(local)?));
        }
        self.persist()?;
        self.host.post("/v1/keypackages", json!({ "key_packages": out })).await?;
        Ok(out.len())
    }

    // ---------- calls ----------

    /// The voice channel's call key material for the current epoch: (epoch, this device's leaf index,
    /// 32-byte base from the MLS exporter). Everyone in the channel's group derives the same base.
    pub fn voice_secret(&self, channel: &str) -> Result<(u64, u32, [u8; 32])> {
        let group = self.group(channel)?.ok_or_else(|| Error::Local("You aren't in this channel's encrypted group yet".into()))?;
        let secret = group.export_secret(self.provider.crypto(), "lodestone-comms voice v1", b"", 32).map_err(local)?;
        let base: [u8; 32] = secret.try_into().map_err(|_| Error::Local("bad exporter length".into()))?;
        Ok((group.epoch().as_u64(), group.own_leaf_index().u32(), base))
    }

    /// Who's in a voice channel, and the relay's address (`relay`, ip:port).
    pub async fn voice_info(&self, channel: &str) -> Result<Value> {
        Ok(self.host.get(&format!("/v1/channels/{channel}/voice")).await?)
    }

    /// Join a call with an offer from the call engine; returns the relay's answer, your slot and who's in.
    /// `relay` is the relay address (from `voice_info`'s `relays`) this device connects to.
    pub async fn voice_join(&self, channel: &str, relay: &str, offer: Value, muted: bool, deafened: bool) -> Result<Value> {
        Ok(self.host.post(&format!("/v1/channels/{channel}/voice/join"), json!({ "offer": offer, "relay": relay, "muted": muted, "deafened": deafened })).await?)
    }

    pub async fn voice_state(&self, channel: &str, muted: bool, deafened: bool) -> Result<()> {
        self.host.post(&format!("/v1/channels/{channel}/voice/state"), json!({ "muted": muted, "deafened": deafened })).await?;
        Ok(())
    }

    pub async fn voice_leave(&self, channel: &str) -> Result<()> {
        self.host.post(&format!("/v1/channels/{channel}/voice/leave"), json!({})).await?;
        Ok(())
    }

    /// Catch up one channel now (calls do this often, so new members' keys arrive quickly).
    pub async fn sync_channel(&self, channel: &str) -> Result<()> {
        let _g = self.op.lock().await;
        let devices = self.space_devices_for(channel).await?;
        self.catch_up(channel, &devices).await?;
        self.reconcile(channel, &devices).await?;
        Ok(())
    }

    // ---------- devices and pairing ----------

    /// This device's fingerprint: the same short code the host and your other devices show for it.
    pub fn fingerprint(&self) -> String {
        use sha2::Digest;
        let d = sha2::Sha256::digest(self.device.as_bytes());
        d[..8].chunks(2).map(|c| format!("{:02x}{:02x}", c[0], c[1])).collect::<Vec<_>>().join("-")
    }

    /// On a device you already use: a one-time code (10 minutes) for adding another device.
    pub async fn pair_start(&self) -> Result<(String, i64)> {
        let r = self.host.post("/v1/pairing", json!({})).await?;
        Ok((r["code"].as_str().unwrap_or_default().to_string(), r["expires"].as_i64().unwrap_or(0)))
    }

    /// Devices that used one of your codes and are waiting for approval: (device key, fingerprint).
    pub async fn pair_waiting(&self) -> Result<Vec<(String, String)>> {
        let r = self.host.get("/v1/pairing").await?;
        Ok(r["waiting"].as_array().into_iter().flatten().map(|w| (w["device_key"].as_str().unwrap_or_default().to_string(), w["fingerprint"].as_str().unwrap_or_default().to_string())).collect())
    }

    pub async fn pair_decide(&self, device_key: &str, approve: bool) -> Result<()> {
        self.host.post(if approve { "/v1/pairing/approve" } else { "/v1/pairing/refuse" }, json!({ "device_key": device_key })).await?;
        Ok(())
    }

    /// On the new device: offer this device for a code. Returns (username, this device's fingerprint);
    /// then wait for `pair_check` to say it was approved.
    pub async fn pair_claim(&self, code: &str) -> Result<(String, String)> {
        let code = code.trim().to_ascii_lowercase();
        let sig = self.sign(&format!("lodestone-comms/v1 pair {} {code} {}", self.host.url, self.device));
        let r = self.host.post("/v1/pairing/claim", json!({ "code": code, "device_key": self.device, "signature": sig })).await?;
        let user = r["username"].as_str().unwrap_or_default().to_string();
        self.store.set("pairing", &json!({ "code": code, "username": user }).to_string())?;
        Ok((user, self.fingerprint()))
    }

    /// On the new device: "waiting", "approved" (now signed in and ready) or "refused".
    pub async fn pair_check(&self) -> Result<String> {
        let p: Value = self.store.get("pairing").and_then(|t| serde_json::from_str(&t).ok()).ok_or_else(|| Error::Local("Not pairing".into()))?;
        let r = self.host.post("/v1/pairing/status", json!({ "code": p["code"], "device_key": self.device })).await?;
        let status = r["status"].as_str().unwrap_or("waiting").to_string();
        if status == "approved" {
            self.store.set("username", p["username"].as_str().unwrap_or_default())?;
            self.store.with(|c| c.execute("DELETE FROM settings WHERE k = 'pairing'", []).map(|_| ()))?;
            self.sign_in().await?;
            self.top_up_key_packages().await?;
        } else if status == "refused" {
            self.store.with(|c| c.execute("DELETE FROM settings WHERE k = 'pairing'", []).map(|_| ()))?;
        }
        Ok(status)
    }

    /// Whether this device is waiting for approval (and for which username).
    pub fn pairing(&self) -> Option<String> {
        self.store.get("pairing").and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|p| p["username"].as_str().map(String::from))
    }

    /// Your devices: (device key, fingerprint, is this one).
    pub async fn devices(&self) -> Result<Vec<(String, String, bool)>> {
        let me = self.host.get("/v1/me").await?;
        Ok(me["devices"].as_array().into_iter().flatten().map(|d| (d["key"].as_str().unwrap_or_default().to_string(), d["fingerprint"].as_str().unwrap_or_default().to_string(), d["this"].as_bool().unwrap_or(false))).collect())
    }

    /// Remove one of your other devices; your apps then take it out of every channel.
    pub async fn remove_device(&self, device_key: &str) -> Result<()> {
        self.host.post("/v1/devices/remove", json!({ "device_key": device_key })).await?;
        Ok(())
    }

    // ---------- spaces and channels ----------

    /// Create a server (space) with its `general` channel, and its encrypted group.
    pub async fn create_space(&self, name: &str) -> Result<String> {
        let _g = self.op.lock().await;
        let s = self.host.post("/v1/spaces", json!({ "name": name })).await?;
        let space = s["id"].as_str().unwrap_or_default().to_string();
        for ch in s["channels"].as_array().into_iter().flatten() {
            self.new_group(ch["id"].as_str().unwrap_or_default(), &space, ch["name"].as_str().unwrap_or_default())?;
        }
        Ok(space)
    }

    /// Add a text channel (owner or admin). Other members are added to its group on the next sync.
    pub async fn create_channel(&self, space: &str, name: &str) -> Result<String> {
        self.create_channel_of(space, name, "text").await
    }

    /// Add a voice channel: also an MLS group, whose keys protect the call's audio.
    pub async fn create_voice_channel(&self, space: &str, name: &str) -> Result<String> {
        self.create_channel_of(space, name, "voice").await
    }

    async fn create_channel_of(&self, space: &str, name: &str, kind: &str) -> Result<String> {
        let _g = self.op.lock().await;
        let ch = self.host.post(&format!("/v1/spaces/{space}/channels"), json!({ "name": name, "kind": kind })).await?;
        let id = ch["id"].as_str().unwrap_or_default().to_string();
        self.new_group(&id, space, name)?;
        Ok(id)
    }

    fn new_group(&self, channel: &str, space: &str, name: &str) -> Result<()> {
        MlsGroup::new_with_group_id(&self.provider, &self.signer, &Self::create_config(), GroupId::from_slice(channel.as_bytes()), self.credential()?).map_err(local)?;
        self.persist()?;
        self.store.add_channel(channel, space, name, 0)?;
        Ok(())
    }

    pub async fn create_invite(&self, space: &str, max_uses: i64, expires_in: i64) -> Result<Value> {
        Ok(self.host.post(&format!("/v1/spaces/{space}/invites"), json!({ "max_uses": max_uses, "expires_in": expires_in })).await?)
    }

    pub async fn accept_invite(&self, code: &str) -> Result<String> {
        let r = self.host.post(&format!("/v1/invites/{}/accept", code.trim()), json!({})).await?;
        Ok(r["space"].as_str().unwrap_or_default().to_string())
    }

    /// Channels whose encrypted group you're in: (channel ID, space ID, name).
    pub fn channels(&self) -> Result<Vec<(String, String, String)>> {
        Ok(self.store.channels()?.into_iter().map(|c| (c.0, c.1, c.2)).collect())
    }

    pub fn messages(&self, channel: &str, limit: i64) -> Result<Vec<Message>> {
        Ok(self.store.messages(channel, limit)?)
    }

    // ---------- sending ----------

    /// Encrypt and send a text message. Catches up with the channel first, so it goes out in the
    /// current epoch.
    pub async fn send_text(&self, channel: &str, text: &str) -> Result<Message> {
        let _g = self.op.lock().await;
        let devices = self.space_devices_for(channel).await?;
        self.catch_up(channel, &devices).await?;
        let mut group = self.group(channel)?.ok_or_else(|| Error::Local("You aren't in this channel's encrypted group yet; someone in it has to add you (it happens automatically when they're online)".into()))?;
        let payload = serde_json::to_vec(&Payload { t: "text".into(), body: text.to_string(), ..Default::default() }).map_err(local)?;
        let out = group.create_message(&self.provider, &self.signer, &payload).map_err(local)?;
        self.persist()?;
        let epoch = group.epoch().as_u64();
        let seq = self.host.send(channel, "application", epoch, &out.tls_serialize_detached().map_err(local)?).await?.ok_or_else(|| Error::Local("The host refused the message".into()))?;
        let m = Message { channel: channel.into(), seq, sender: self.username().unwrap_or_default(), sender_device: self.device.clone(), body: text.into(), sent_at: now(), attachment: None };
        // The read position isn't moved here: catching up skips our own messages, and moving it could
        // jump over someone else's message that arrived just before ours.
        self.store.add_message(&m)?;
        Ok(m)
    }

    /// Encrypt a file on this device (AES-256-GCM, a new random key per file), upload the ciphertext,
    /// and send the key inside an end-to-end encrypted message. Images are recognised by their content.
    pub async fn send_file(&self, channel: &str, name: &str, bytes: &[u8], caption: &str) -> Result<Message> {
        use aes_gcm::aead::{Aead, KeyInit};
        if bytes.is_empty() || bytes.len() > MAX_FILE {
            return Err(Error::Local(format!("Files can be at most {} MB", MAX_FILE / 1024 / 1024)));
        }
        let mime = sniff_image(bytes).unwrap_or("application/octet-stream");
        let mut key = [0u8; 32];
        let mut nonce = [0u8; 12];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut key);
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut nonce);
        let cipher = aes_gcm::Aes256Gcm::new_from_slice(&key).map_err(local)?;
        let sealed = cipher.encrypt(aes_gcm::Nonce::from_slice(&nonce), bytes).map_err(local)?;
        let up = self.host.post_raw(&format!("/v1/channels/{channel}/blobs"), sealed.clone()).await?;
        let id = up["id"].as_str().unwrap_or_default().to_string();
        let file = json!({ "id": id, "name": clean_file_name(name), "mime": mime, "size": bytes.len(), "key": b64(&key), "nonce": b64(&nonce) });
        let _ = std::fs::create_dir_all(self.dir.join("files"));
        let _ = std::fs::write(self.dir.join("files").join(&id), &sealed);
        let _g = self.op.lock().await;
        let devices = self.space_devices_for(channel).await?;
        self.catch_up(channel, &devices).await?;
        let mut group = self.group(channel)?.ok_or_else(|| Error::Local("You aren't in this channel's encrypted group yet".into()))?;
        let payload = serde_json::to_vec(&Payload { t: "file".into(), body: caption.to_string(), file: Some(file.clone()), ..Default::default() }).map_err(local)?;
        let out = group.create_message(&self.provider, &self.signer, &payload).map_err(local)?;
        self.persist()?;
        let seq = self.host.send(channel, "application", group.epoch().as_u64(), &out.tls_serialize_detached().map_err(local)?).await?.ok_or_else(|| Error::Local("The host refused the message".into()))?;
        let m = Message { channel: channel.into(), seq, sender: self.username().unwrap_or_default(), sender_device: self.device.clone(), body: caption.into(), sent_at: now(), attachment: Some(file) };
        // The read position isn't moved here: catching up skips our own messages, and moving it could
        // jump over someone else's message that arrived just before ours.
        self.store.add_message(&m)?;
        Ok(self.store.messages(channel, 1)?.into_iter().find(|x| x.seq == seq).unwrap_or(m))
    }

    /// Your profile: display name (up to 32 characters, empty for your username), name colour
    /// (#rrggbb or empty) and picture (PNG, JPEG, GIF or WebP up to 256 KiB; `None` keeps the current
    /// one, `Some(empty)` removes it). It's sent to your servers end-to-end encrypted.
    pub async fn set_profile(&self, name: &str, color: &str, avatar: Option<&[u8]>) -> Result<()> {
        let name = clean_display_name(name).ok_or_else(|| Error::Local("Display names can have at most 32 characters".into()))?;
        if !color.is_empty() && !valid_color(color) {
            return Err(Error::Local("Colours look like #7c5cff".into()));
        }
        if let Some(a) = avatar {
            if !a.is_empty() && (a.len() > MAX_AVATAR || sniff_image(a).is_none()) {
                return Err(Error::Local("Profile pictures must be PNG, JPEG, GIF or WebP, up to 256 KiB".into()));
            }
            self.store.set("avatar", &b64(a))?;
            // Each server gets a fresh upload of the new picture.
            self.store.with(|c| c.execute("DELETE FROM settings WHERE k LIKE 'avatar_ref:%'", []).map(|_| ()))?;
        }
        self.store.set("profile", &json!({ "name": name, "color": color }).to_string())?;
        // Show it on this device straight away, and send it to every channel we're in.
        let me = self.username().unwrap_or_default();
        self.store.set_profile(&me, &name, color, None, i64::MAX)?;
        let all: Vec<String> = self.store.channels()?.into_iter().map(|c| c.0).collect();
        self.announce.lock().unwrap().extend(all);
        let _g = self.op.lock().await;
        self.announce_profile().await
    }

    /// Your own profile as stored on this device: (display name, colour, has a picture).
    pub fn my_profile(&self) -> (String, String, bool) {
        let Some(p) = self.store.get("profile").and_then(|t| serde_json::from_str::<Value>(&t).ok()) else {
            // Not set on this device: use the one your other device sent, if any.
            let me = self.username().unwrap_or_default();
            return self.store.profiles().ok().and_then(|l| l.into_iter().find(|x| x.0 == me)).map(|x| (x.1, x.2, x.3.is_some())).unwrap_or_default();
        };
        let has = self.store.get("avatar").is_some_and(|a| !a.is_empty());
        (p["name"].as_str().unwrap_or_default().to_string(), p["color"].as_str().unwrap_or_default().to_string(), has)
    }

    /// Everyone's profiles this device knows: (username, display name, colour, has a picture).
    pub fn profiles(&self) -> Result<Vec<(String, String, String, bool)>> {
        let me = self.username().unwrap_or_default();
        Ok(self
            .store
            .profiles()?
            .into_iter()
            .map(|(u, n, c, a)| {
                let has = if u == me && self.store.get("profile").is_some() { self.my_profile().2 } else { a.is_some() };
                (u, n, c, has)
            })
            .collect())
    }

    /// Someone's profile picture, decrypted (yours comes from this device).
    pub async fn avatar(&self, user: &str) -> Result<(Vec<u8>, String)> {
        if Some(user.to_string()) == self.username() {
            if let Some(a) = self.store.get("avatar").and_then(|a| unb64(&a)).filter(|a| !a.is_empty()) {
                let mime = sniff_image(&a).unwrap_or("application/octet-stream").to_string();
                return Ok((a, mime));
            }
        }
        let f = self.store.profiles()?.into_iter().find(|p| p.0 == user).and_then(|p| p.3).ok_or_else(|| Error::Local("No picture".into()))?;
        let (bytes, mime) = self.decrypt_ref(&f).await?;
        if !mime.starts_with("image/") {
            return Err(Error::Local("Not a picture".into()));
        }
        Ok((bytes, mime))
    }

    /// Send our profile into every channel waiting for it. Called with the op lock held.
    async fn announce_profile(&self) -> Result<()> {
        let Some(profile) = self.store.get("profile").and_then(|t| serde_json::from_str::<Value>(&t).ok()) else {
            self.announce.lock().unwrap().clear();
            return Ok(()); // no profile set: nothing to share
        };
        let channels: Vec<String> = std::mem::take(&mut *self.announce.lock().unwrap()).into_iter().collect();
        let known = self.store.channels()?;
        for channel in channels {
            let Some(space) = known.iter().find(|c| c.0 == channel).map(|c| c.1.clone()) else { continue };
            let Some(mut group) = self.group(&channel)? else { continue };
            let avatar = match self.avatar_ref(&space, &channel).await {
                Ok(a) => a,
                Err(e) => {
                    eprintln!("comms: couldn't upload our profile picture: {e}");
                    None
                }
            };
            let payload = Payload {
                t: "profile".into(),
                body: profile["name"].as_str().unwrap_or_default().to_string(),
                color: profile["color"].as_str().filter(|c| !c.is_empty()).map(String::from),
                file: avatar,
                ..Default::default()
            };
            let out = group.create_message(&self.provider, &self.signer, &serde_json::to_vec(&payload).map_err(local)?).map_err(local)?;
            self.persist()?;
            match self.host.send(&channel, "application", group.epoch().as_u64(), &out.tls_serialize_detached().map_err(local)?).await {
                Ok(_) => {}
                Err(e) => eprintln!("comms: couldn't send our profile to a channel: {e}"),
            }
        }
        Ok(())
    }

    /// Our encrypted picture for a server, uploaded once per server (files belong to a server).
    async fn avatar_ref(&self, space: &str, channel: &str) -> Result<Option<Value>> {
        use aes_gcm::aead::{Aead, KeyInit};
        let Some(bytes) = self.store.get("avatar").and_then(|a| unb64(&a)).filter(|a| !a.is_empty()) else { return Ok(None) };
        if let Some(r) = self.store.get(&format!("avatar_ref:{space}")).and_then(|t| serde_json::from_str(&t).ok()) {
            return Ok(Some(r));
        }
        let mut key = [0u8; 32];
        let mut nonce = [0u8; 12];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut key);
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut nonce);
        let sealed = aes_gcm::Aes256Gcm::new_from_slice(&key).map_err(local)?.encrypt(aes_gcm::Nonce::from_slice(&nonce), bytes.as_ref()).map_err(local)?;
        let up = self.host.post_raw(&format!("/v1/channels/{channel}/blobs"), sealed).await?;
        let r = json!({ "id": up["id"], "name": "avatar", "mime": sniff_image(&bytes).unwrap_or(""), "size": bytes.len(), "key": b64(&key), "nonce": b64(&nonce) });
        self.store.set(&format!("avatar_ref:{space}"), &r.to_string())?;
        Ok(Some(r))
    }

    /// Download (or read from the local cache) and decrypt a file reference: (bytes, mime by content).
    async fn decrypt_ref(&self, f: &Value) -> Result<(Vec<u8>, String)> {
        use aes_gcm::aead::{Aead, KeyInit};
        let id = f["id"].as_str().unwrap_or_default().to_string();
        if !(id.len() == 24 && id.bytes().all(|b| b.is_ascii_hexdigit())) {
            return Err(Error::Local("Bad file reference".into()));
        }
        let cache = self.dir.join("files").join(&id);
        let sealed = match std::fs::read(&cache) {
            Ok(b) => b,
            Err(_) => {
                let (b, _) = self.host.get_raw(&format!("/v1/blobs/{id}")).await?;
                let _ = std::fs::create_dir_all(self.dir.join("files"));
                let _ = std::fs::write(&cache, &b);
                b
            }
        };
        let key = f["key"].as_str().and_then(unb64).ok_or_else(|| Error::Local("Bad file key".into()))?;
        let nonce = f["nonce"].as_str().and_then(unb64).filter(|n| n.len() == 12).ok_or_else(|| Error::Local("Bad file key".into()))?;
        let plain = aes_gcm::Aes256Gcm::new_from_slice(&key).map_err(local)?.decrypt(aes_gcm::Nonce::from_slice(&nonce), sealed.as_ref()).map_err(|_| {
            let _ = std::fs::remove_file(&cache);
            Error::Local("The file doesn't match its key (damaged or swapped on the host)".into())
        })?;
        let mime = sniff_image(&plain).unwrap_or("application/octet-stream").to_string();
        Ok((plain, mime))
    }

    /// A message's attachment, decrypted: (bytes, mime type, file name). The encrypted copy is kept in
    /// this device's folder so it isn't downloaded twice.
    pub async fn open_file(&self, channel: &str, seq: i64) -> Result<(Vec<u8>, String, String)> {
        let f = self.store.attachment(channel, seq).ok_or_else(|| Error::Local("No file on that message".into()))?;
        let (plain, mime) = self.decrypt_ref(&f).await?;
        Ok((plain, mime, clean_file_name(f["name"].as_str().unwrap_or("file"))))
    }

    /// Add (`on`) or remove your emoji reaction on a message, end-to-end encrypted like a message.
    pub async fn react(&self, channel: &str, target: i64, emoji: &str, on: bool) -> Result<()> {
        if !valid_reaction(emoji) {
            return Err(Error::Local("Not a reaction".into()));
        }
        let _g = self.op.lock().await;
        let devices = self.space_devices_for(channel).await?;
        self.catch_up(channel, &devices).await?;
        let mut group = self.group(channel)?.ok_or_else(|| Error::Local("You aren't in this channel's encrypted group yet".into()))?;
        let payload = serde_json::to_vec(&Payload { t: "react".into(), body: emoji.into(), target: Some(target), on: Some(on), file: None, color: None }).map_err(local)?;
        let out = group.create_message(&self.provider, &self.signer, &payload).map_err(local)?;
        self.persist()?;
        self.host.send(channel, "application", group.epoch().as_u64(), &out.tls_serialize_detached().map_err(local)?).await?.ok_or_else(|| Error::Local("The host refused it".into()))?;
        self.store.set_reaction(channel, target, emoji, &self.username().unwrap_or_default(), on)?;
        Ok(())
    }

    /// A channel's reactions: (message seq, emoji, who), as this device knows them.
    pub fn reactions(&self, channel: &str) -> Result<Vec<(i64, String, Vec<String>)>> {
        Ok(self.store.reactions(channel)?)
    }

    /// Channels whose reactions changed since the last call (for live updates).
    pub fn take_touched(&self) -> Vec<String> {
        std::mem::take(&mut *self.touched.lock().unwrap())
    }

    // ---------- syncing ----------

    /// Catch up with everything: join channels you were welcomed into, read new messages, keep channel
    /// groups in step with each server's members, and top up key packages. Returns new messages.
    pub async fn sync(&self) -> Result<Vec<Message>> {
        let _g = self.op.lock().await;
        self.take_welcomes().await?;
        let me = self.host.get("/v1/me").await?;
        let mut new = Vec::new();
        for s in me["spaces"].as_array().into_iter().flatten() {
            let space_id = s["id"].as_str().unwrap_or_default();
            let space = self.host.get(&format!("/v1/spaces/{space_id}")).await?;
            let devices = devices_of(&space);
            for ch in space["channels"].as_array().into_iter().flatten() {
                let channel = ch["id"].as_str().unwrap_or_default();
                if self.group(channel)?.is_none() {
                    continue; // waiting for a member to add us
                }
                self.store.add_channel(channel, space_id, ch["name"].as_str().unwrap_or_default(), 0)?;
                new.extend(self.catch_up(channel, &devices).await?);
                self.reconcile(channel, &devices).await?;
            }
        }
        self.announce_profile().await?;
        self.top_up_key_packages().await?;
        Ok(new)
    }

    /// Device key -> username for every member of the channel's space, from the host.
    async fn space_devices_for(&self, channel: &str) -> Result<HashMap<String, String>> {
        let space = self.store.channels()?.into_iter().find(|c| c.0 == channel).map(|c| c.1).ok_or_else(|| Error::Local("Unknown channel".into()))?;
        Ok(devices_of(&self.host.get(&format!("/v1/spaces/{space}")).await?))
    }

    async fn take_welcomes(&self) -> Result<()> {
        let w = self.host.get("/v1/welcomes").await?;
        for item in w["welcomes"].as_array().into_iter().flatten() {
            let channel = item["channel"].as_str().unwrap_or_default();
            let Some(bytes) = item["body"].as_str().and_then(unb64) else { continue };
            let Ok(msg) = MlsMessageIn::tls_deserialize_exact(&bytes) else { continue };
            let MlsMessageBodyIn::Welcome(welcome) = msg.extract() else { continue };
            match StagedWelcome::new_from_welcome(&self.provider, &Self::join_config(), welcome, None).and_then(|s| s.into_group(&self.provider)) {
                Ok(group) => {
                    // The group ID must be the channel the host delivered it for.
                    if group.group_id().as_slice() != channel.as_bytes() {
                        continue;
                    }
                    let epoch = group.epoch().as_u64();
                    self.persist()?;
                    let space = self.space_of_channel(channel).await?;
                    self.store.add_channel(channel, &space.0, &space.1, epoch)?;
                    self.announce.lock().unwrap().insert(channel.to_string());
                }
                Err(e) => eprintln!("comms: couldn't use a welcome for a channel: {e:?}"),
            }
        }
        Ok(())
    }

    /// (space ID, channel name) for a channel, from the host.
    async fn space_of_channel(&self, channel: &str) -> Result<(String, String)> {
        let me = self.host.get("/v1/me").await?;
        for s in me["spaces"].as_array().into_iter().flatten() {
            let id = s["id"].as_str().unwrap_or_default();
            let space = self.host.get(&format!("/v1/spaces/{id}")).await?;
            if let Some(ch) = space["channels"].as_array().into_iter().flatten().find(|c| c["id"] == channel) {
                return Ok((id.to_string(), ch["name"].as_str().unwrap_or_default().to_string()));
            }
        }
        Err(Error::Local("Channel not found in your servers".into()))
    }

    /// Fetch and process a channel's new messages, in the host's order.
    async fn catch_up(&self, channel: &str, devices: &HashMap<String, String>) -> Result<Vec<Message>> {
        let Some(mut group) = self.group(channel)? else { return Ok(vec![]) };
        let joined_epoch = self.store.channels()?.into_iter().find(|c| c.0 == channel).map(|c| c.3).unwrap_or(0);
        let mut out = Vec::new();
        loop {
            let after = self.store.last_seq(channel);
            let page = self.host.get(&format!("/v1/channels/{channel}/messages?after={after}&limit=200")).await?;
            let msgs = page["messages"].as_array().cloned().unwrap_or_default();
            if msgs.is_empty() {
                break;
            }
            for m in &msgs {
                let seq = m["seq"].as_i64().unwrap_or(0);
                let epoch = m["epoch"].as_u64().unwrap_or(0);
                // Our own messages were handled when we sent them; messages from before we joined can't be read.
                if m["sender"] != self.device.as_str() && epoch >= joined_epoch {
                    if let Some(msg) = self.process(&mut group, channel, m, devices)? {
                        out.push(msg);
                    }
                }
                self.store.set_last_seq(channel, seq)?;
            }
            if msgs.len() < 200 {
                break;
            }
        }
        Ok(out)
    }

    /// Process one message from the host. Returns it if it's a readable chat message.
    fn process(&self, group: &mut MlsGroup, channel: &str, m: &Value, devices: &HashMap<String, String>) -> Result<Option<Message>> {
        let Some(bytes) = m["body"].as_str().and_then(unb64) else { return Ok(None) };
        let Ok(msg) = MlsMessageIn::tls_deserialize_exact(&bytes) else { return Ok(None) };
        let Ok(protocol) = msg.try_into_protocol_message() else { return Ok(None) };
        if protocol.group_id().as_slice() != channel.as_bytes() {
            return Ok(None);
        }
        let processed = match group.process_message(&self.provider, protocol) {
            Ok(p) => p,
            Err(e) => {
                // Usually an epoch we no longer (or never) had keys for; nothing else to do with it.
                eprintln!("comms: skipped a message in a channel: {e:?}");
                self.persist()?;
                return Ok(None);
            }
        };
        let sender = match processed.sender() {
            Sender::Member(leaf) => group.member_at(*leaf),
            _ => None,
        };
        let out = match processed.into_content() {
            ProcessedMessageContent::ApplicationMessage(app) => {
                let payload: Option<Payload> = serde_json::from_slice(&app.into_bytes()).ok();
                let (name, device) = match &sender {
                    Some(member) => verified_sender(member, devices),
                    None => ("⚠ unknown".to_string(), String::new()),
                };
                match payload {
                    Some(p) if p.t == "text" || (p.t == "file" && p.file.as_ref().is_some_and(valid_file)) => Some(Message {
                        channel: channel.into(),
                        seq: m["seq"].as_i64().unwrap_or(0),
                        sender: name,
                        sender_device: device,
                        body: p.body,
                        sent_at: m["sent_at"].as_i64().unwrap_or(0),
                        attachment: p.file.map(|mut f| {
                            f["name"] = json!(clean_file_name(f["name"].as_str().unwrap_or_default()));
                            f
                        }),
                    }),
                    Some(p) if p.t == "profile" => {
                                        // Only a verified sender's own profile is kept (names marked ⚠ don't count).
                        let ok_avatar = p.file.as_ref().is_none_or(|f| valid_file(f) && f["size"].as_u64().is_some_and(|s| s as usize <= MAX_AVATAR));
                        let color = p.color.clone().filter(|c| valid_color(c)).unwrap_or_default();
                        if let (false, Some(display), true) = (name.starts_with('⚠'), clean_display_name(&p.body), ok_avatar) {
                            if self.store.set_profile(&name, &display, &color, p.file.as_ref(), m["seq"].as_i64().unwrap_or(0) + m["sent_at"].as_i64().unwrap_or(0) * 1_000_000)? {
                                let mut t = self.touched.lock().unwrap();
                                if !t.iter().any(|c| c == "profiles") {
                                    t.push("profiles".to_string());
                                }
                            }
                        }
                        None
                    }
                    Some(p) if p.t == "react" && valid_reaction(&p.body) => {
                        if let Some(target) = p.target {
                            self.store.set_reaction(channel, target, &p.body, &name, p.on.unwrap_or(true))?;
                            let mut t = self.touched.lock().unwrap();
                            if !t.iter().any(|c| c == channel) {
                                t.push(channel.to_string());
                            }
                        }
                        None
                    }
                    _ => None,
                }
            }
            ProcessedMessageContent::StagedCommitMessage(staged) => {
                for add in staged.add_proposals() {
                    let leaf = add.add_proposal().key_package().leaf_node();
                    if !credential_matches(leaf.credential(), leaf.signature_key().as_slice(), devices) {
                        eprintln!("comms: a member added a device the host doesn't list for that user");
                    }
                }
                let added = staged.add_proposals().count() > 0;
                group.merge_staged_commit(&self.provider, *staged).map_err(local)?;
                if added {
                    self.announce.lock().unwrap().insert(channel.to_string());
                }
                None
            }
            ProcessedMessageContent::ProposalMessage(p) => {
                group.store_pending_proposal(self.provider.storage(), *p).map_err(local)?;
                None
            }
            _ => None,
        };
        self.persist()?;
        if let Some(msg) = &out {
            self.store.add_message(msg)?;
        }
        Ok(out)
    }

    /// Make the channel's group match the host's member list: add devices that are missing, remove ones
    /// whose accounts left. Any member may do this; the host's one-commit-per-epoch rule settles races.
    async fn reconcile(&self, channel: &str, devices: &HashMap<String, String>) -> Result<()> {
        let Some(mut group) = self.group(channel)? else { return Ok(()) };
        let in_group: BTreeMap<String, LeafNodeIndex> = group.members().filter_map(|m| identity_of(&m.credential).map(|(_, d)| (d, m.index))).collect();
        let remove: Vec<LeafNodeIndex> = in_group.iter().filter(|(d, _)| !devices.contains_key(*d) && **d != self.device).map(|(_, i)| *i).collect();
        let mut users: Vec<&String> = devices.iter().filter(|(d, _)| !in_group.contains_key(*d)).map(|(_, u)| u).collect();
        users.sort();
        users.dedup();
        let mut add = Vec::new();
        for user in users {
            let claimed = self.host.post("/v1/keypackages/claim", json!({ "username": user })).await?;
            for d in claimed["devices"].as_array().into_iter().flatten() {
                let device = d["device_key"].as_str().unwrap_or_default();
                if in_group.contains_key(device) {
                    continue;
                }
                let Some(kp) = d["key_package"].as_str().and_then(unb64) else { continue };
                let Ok(kp) = KeyPackageIn::tls_deserialize_exact(&kp) else { continue };
                let Ok(kp) = kp.validate(self.provider.crypto(), ProtocolVersion::Mls10) else { continue };
                // The key package must be this exact device of this user.
                let leaf = kp.leaf_node();
                if credential_matches(leaf.credential(), leaf.signature_key().as_slice(), devices) && identity_of(leaf.credential()).is_some_and(|(u, d)| &u == user && d == device) {
                    add.push((device.to_string(), kp));
                }
            }
        }
        if !remove.is_empty() {
            let epoch = group.epoch().as_u64();
            let (commit, _, _) = group.remove_members(&self.provider, &self.signer, &remove).map_err(local)?;
            if self.commit(&mut group, channel, epoch, &commit).await? {
                // A removal changes the epoch; reload before adding anyone.
                group = self.group(channel)?.ok_or_else(|| Error::Local("group vanished".into()))?;
            } else {
                return Ok(());
            }
        }
        if !add.is_empty() {
            let epoch = group.epoch().as_u64();
            let kps: Vec<KeyPackage> = add.iter().map(|(_, k)| k.clone()).collect();
            let (commit, welcome, _) = group.add_members(&self.provider, &self.signer, &kps).map_err(local)?;
            if self.commit(&mut group, channel, epoch, &commit).await? {
                self.announce.lock().unwrap().insert(channel.to_string());
                let to: Vec<&String> = add.iter().map(|(d, _)| d).collect();
                self.host.post("/v1/welcomes", json!({ "channel": channel, "to": to, "body": b64(&welcome.tls_serialize_detached().map_err(local)?) })).await?;
            }
        }
        Ok(())
    }

    /// Send a commit; merge it if the host accepted it, drop it if someone else's commit got in first.
    async fn commit(&self, group: &mut MlsGroup, channel: &str, epoch: u64, commit: &MlsMessageOut) -> Result<bool> {
        self.persist()?;
        match self.host.send(channel, "commit", epoch, &commit.tls_serialize_detached().map_err(local)?).await? {
            Some(_) => {
                group.merge_pending_commit(&self.provider).map_err(local)?;
                self.persist()?;
                Ok(true)
            }
            None => {
                group.clear_pending_commit(self.provider.storage()).map_err(local)?;
                self.persist()?;
                Ok(false)
            }
        }
    }
}

/// Device key -> username for a space's members (from `GET /v1/spaces/{id}`).
fn devices_of(space: &Value) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for m in space["members"].as_array().into_iter().flatten() {
        let user = m["username"].as_str().unwrap_or_default();
        for d in m["devices"].as_array().into_iter().flatten() {
            if let Some(d) = d.as_str() {
                out.insert(d.to_string(), user.to_string());
            }
        }
    }
    out
}

/// A credential says `<user>:<device>`, its signature key is that device key, and the host lists that
/// device for that user.
fn credential_matches(cred: &Credential, signature_key: &[u8], devices: &HashMap<String, String>) -> bool {
    identity_of(cred).is_some_and(|(u, d)| unb64(&d).as_deref() == Some(signature_key) && devices.get(&d) == Some(&u))
}

/// The sender's name, marked ⚠ if their MLS credential doesn't match what the host lists.
fn verified_sender(member: &Member, devices: &HashMap<String, String>) -> (String, String) {
    match identity_of(&member.credential) {
        Some((u, d)) if credential_matches(&member.credential, &member.signature_key, devices) => (u, d),
        Some((u, d)) => (format!("⚠ {u}"), d),
        None => ("⚠ unknown".into(), String::new()),
    }
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}
