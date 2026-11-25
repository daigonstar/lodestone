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
#[derive(serde::Serialize, serde::Deserialize)]
struct Payload {
    t: String,
    body: String,
}

pub struct Client {
    store: Store,
    provider: Provider,
    pub host: Host,
    key: SigningKey,
    signer: SignatureKeyPair,
    device: String,
    op: tokio::sync::Mutex<()>,
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
        Ok(Self { store, provider: Provider { crypto: RustCrypto::default(), storage }, host: Host::new(host_url), key, signer, device: b64(&public), op: Default::default() })
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
        let _g = self.op.lock().await;
        let ch = self.host.post(&format!("/v1/spaces/{space}/channels"), json!({ "name": name })).await?;
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
        let payload = serde_json::to_vec(&Payload { t: "text".into(), body: text.to_string() }).map_err(local)?;
        let out = group.create_message(&self.provider, &self.signer, &payload).map_err(local)?;
        self.persist()?;
        let epoch = group.epoch().as_u64();
        let seq = self.host.send(channel, "application", epoch, &out.tls_serialize_detached().map_err(local)?).await?.ok_or_else(|| Error::Local("The host refused the message".into()))?;
        let m = Message { channel: channel.into(), seq, sender: self.username().unwrap_or_default(), sender_device: self.device.clone(), body: text.into(), sent_at: now() };
        self.store.add_message(&m)?;
        self.store.set_last_seq(channel, seq)?;
        Ok(m)
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
                payload.filter(|p| p.t == "text").map(|p| Message {
                    channel: channel.into(),
                    seq: m["seq"].as_i64().unwrap_or(0),
                    sender: name,
                    sender_device: device,
                    body: p.body,
                    sent_at: m["sent_at"].as_i64().unwrap_or(0),
                })
            }
            ProcessedMessageContent::StagedCommitMessage(staged) => {
                for add in staged.add_proposals() {
                    let leaf = add.add_proposal().key_package().leaf_node();
                    if !credential_matches(leaf.credential(), leaf.signature_key().as_slice(), devices) {
                        eprintln!("comms: a member added a device the host doesn't list for that user");
                    }
                }
                group.merge_staged_commit(&self.provider, *staged).map_err(local)?;
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
            Some(seq) => {
                group.merge_pending_commit(&self.provider).map_err(local)?;
                self.persist()?;
                self.store.set_last_seq(channel, seq)?;
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
