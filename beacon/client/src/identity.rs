//! The person's identity: one Ed25519 key that belongs to them, not to a server or a device. Every server they
//! join records only its public half, so the same person can be recognised across servers and can get back into
//! their accounts on a new device. It is backed up as a 24-word recovery phrase (the key's 32 bytes as BIP-39
//! words); whoever has the phrase is that person, so it is never sent anywhere, logged or put in an error message.
//!
//! Devices keep their own keys as before; the identity vouches for a device when it registers or recovers
//! (see "Identity" in PROTOCOL.md).

use crate::server::{b64, unb64};
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use ed25519_dalek::{Signer as _, SigningKey};
use rand::RngCore;
use sha2::Digest;
use std::path::Path;

pub struct Identity {
    key: SigningKey,
}

/// A short fingerprint of an identity's public key (base64), shown to people: "3f9a-02c1-77de-b410".
pub fn fingerprint_of(public: &str) -> String {
    let d = sha2::Sha256::digest(public.as_bytes());
    d[..8].chunks(2).map(|c| format!("{:02x}{:02x}", c[0], c[1])).collect::<Vec<_>>().join("-")
}

impl Identity {
    pub fn generate() -> Self {
        Self { key: SigningKey::generate(&mut rand::rngs::OsRng) }
    }

    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self { key: SigningKey::from_bytes(&seed) }
    }

    /// The 24 recovery words.
    pub fn phrase(&self) -> String {
        bip39::Mnemonic::from_entropy(&self.key.to_bytes()).map(|m| m.to_string()).unwrap_or_default()
    }

    /// Rebuild the identity from its recovery words (any case, extra spaces are fine).
    pub fn from_phrase(phrase: &str) -> Result<Self, String> {
        let words = phrase.split_whitespace().map(|w| w.to_lowercase()).collect::<Vec<_>>().join(" ");
        let m = bip39::Mnemonic::parse_normalized(&words).map_err(|_| "Those words aren't a valid recovery phrase. Check the spelling and that all 24 are there.".to_string())?;
        let seed: [u8; 32] = m.to_entropy().try_into().map_err(|_| "A Beacon recovery phrase has 24 words.".to_string())?;
        Ok(Self::from_seed(seed))
    }

    pub fn seed(&self) -> [u8; 32] {
        self.key.to_bytes()
    }

    pub fn public(&self) -> String {
        b64(self.key.verifying_key().as_bytes())
    }

    pub fn fingerprint(&self) -> String {
        fingerprint_of(&self.public())
    }

    pub fn sign(&self, message: &str) -> String {
        b64(&self.key.sign(message.as_bytes()).to_bytes())
    }

    /// Encrypt the identity for another of the person's devices, with a key that travels in the pairing QR code
    /// and never reaches a server: base64 of a random nonce and the AES-256-GCM ciphertext.
    pub fn seal(&self, key: &[u8; 32]) -> String {
        let mut nonce = [0u8; 12];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let ct = Aes256Gcm::new(key.into()).encrypt(Nonce::from_slice(&nonce), self.key.to_bytes().as_slice()).unwrap_or_default();
        b64(&[nonce.as_slice(), &ct].concat())
    }

    pub fn open(sealed: &str, key: &[u8; 32]) -> Option<Self> {
        let raw = unb64(sealed)?;
        if raw.len() < 13 {
            return None;
        }
        let (nonce, ct) = raw.split_at(12);
        let seed = Aes256Gcm::new(key.into()).decrypt(Nonce::from_slice(nonce), ct).ok()?;
        Some(Self::from_seed(seed.try_into().ok()?))
    }

    /// Load the saved identity and its chosen username.
    pub fn load(path: &Path) -> Option<(Self, String)> {
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
        let seed: [u8; 32] = unb64(v["seed"].as_str()?)?.try_into().ok()?;
        Some((Self::from_seed(seed), v["username"].as_str().unwrap_or_default().to_string()))
    }

    /// Save it, readable only by the user (the file holds the key).
    pub fn save(&self, path: &Path, username: &str) -> std::io::Result<()> {
        let text = serde_json::json!({ "seed": b64(&self.key.to_bytes()), "username": username }).to_string();
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
            f.write_all(text.as_bytes())
        }
        #[cfg(not(unix))]
        {
            std::fs::write(path, text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_recovery_phrase_round_trips() {
        let a = Identity::generate();
        let phrase = a.phrase();
        assert_eq!(phrase.split(' ').count(), 24);
        let b = Identity::from_phrase(&format!("  {}  ", phrase.to_uppercase())).unwrap();
        assert_eq!(a.public(), b.public());
        assert!(Identity::from_phrase("not a real phrase").is_err());
        let mut words: Vec<&str> = phrase.split(' ').collect();
        words.swap(0, 1);
        // A wrong order is either rejected (checksum) or a different identity; never the same one.
        assert!(Identity::from_phrase(&words.join(" ")).map(|c| c.public() != a.public()).unwrap_or(true) || words[0] == words[1]);
    }

    #[test]
    fn a_sealed_identity_opens_only_with_its_key() {
        let a = Identity::generate();
        let key = [7u8; 32];
        let sealed = a.seal(&key);
        assert_eq!(Identity::open(&sealed, &key).unwrap().public(), a.public());
        assert!(Identity::open(&sealed, &[8u8; 32]).is_none());
        assert!(Identity::open("garbage", &key).is_none());
    }

    #[test]
    fn the_identity_file_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("identity.json");
        let a = Identity::generate();
        a.save(&path, "robb").unwrap();
        let (b, user) = Identity::load(&path).unwrap();
        assert_eq!((b.public(), user.as_str()), (a.public(), "robb"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }
}
