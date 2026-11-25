//! End-to-end encryption of call frames (SFrame-style). The relay forwards these without being able
//! to read them.
//!
//! - `base`: 32 bytes from the voice channel's MLS group for the current epoch (the MLS exporter,
//!   label "lodestone-comms voice v1"). Everyone in the call derives the same base; the relay can't.
//! - Each sender's key: HKDF-SHA256(base, info = "lodestone-comms voice sender" + leaf index), so
//!   senders never share a key and nonces can't collide across senders.
//! - A frame: header `[version 1][epoch u32][leaf u32][counter u64]` (17 bytes, authenticated) then
//!   AES-256-GCM of the encoded audio, with the counter as the nonce. The leaf in the header picks the
//!   key: claiming to be someone else just fails to decrypt.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};

pub const VERSION: u8 = 1;
pub const HEADER: usize = 17;
pub const EXPORT_LABEL: &str = "lodestone-comms voice v1";

/// A sender's key for one epoch.
pub fn sender_key(base: &[u8; 32], leaf: u32) -> [u8; 32] {
    let hk = hkdf::Hkdf::<sha2::Sha256>::new(None, base);
    let mut info = b"lodestone-comms voice sender".to_vec();
    info.extend_from_slice(&leaf.to_be_bytes());
    let mut out = [0u8; 32];
    hk.expand(&info, &mut out).expect("32 bytes is a valid HKDF length");
    out
}

fn nonce(counter: u64) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[4..].copy_from_slice(&counter.to_be_bytes());
    n
}

/// Encrypt one encoded frame.
pub fn seal(base: &[u8; 32], epoch: u32, leaf: u32, counter: u64, plain: &[u8]) -> Vec<u8> {
    let mut header = Vec::with_capacity(HEADER + plain.len() + 16);
    header.push(VERSION);
    header.extend_from_slice(&epoch.to_be_bytes());
    header.extend_from_slice(&leaf.to_be_bytes());
    header.extend_from_slice(&counter.to_be_bytes());
    let cipher = Aes256Gcm::new_from_slice(&sender_key(base, leaf)).expect("32-byte key");
    let sealed = cipher.encrypt(Nonce::from_slice(&nonce(counter)), Payload { msg: plain, aad: &header }).expect("encryption doesn't fail");
    header.extend_from_slice(&sealed);
    header
}

/// The epoch, sender leaf and counter of a frame, without decrypting it.
pub fn peek(frame: &[u8]) -> Option<(u32, u32, u64)> {
    if frame.len() < HEADER + 16 || frame[0] != VERSION {
        return None;
    }
    let epoch = u32::from_be_bytes(frame[1..5].try_into().ok()?);
    let leaf = u32::from_be_bytes(frame[5..9].try_into().ok()?);
    let counter = u64::from_be_bytes(frame[9..17].try_into().ok()?);
    Some((epoch, leaf, counter))
}

/// Decrypt a frame with the base for its epoch. Returns (sender leaf, encoded audio).
pub fn open(base_for: impl Fn(u32) -> Option<[u8; 32]>, frame: &[u8]) -> Option<(u32, Vec<u8>)> {
    let (epoch, leaf, counter) = peek(frame)?;
    let base = base_for(epoch)?;
    let cipher = Aes256Gcm::new_from_slice(&sender_key(&base, leaf)).ok()?;
    let plain = cipher.decrypt(Nonce::from_slice(&nonce(counter)), Payload { msg: &frame[HEADER..], aad: &frame[..HEADER] }).ok()?;
    Some((leaf, plain))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_and_reject_tampering() {
        let base = [7u8; 32];
        let f = seal(&base, 3, 1, 42, b"opus bytes");
        assert_eq!(peek(&f), Some((3, 1, 42)));
        assert_eq!(open(|e| (e == 3).then_some(base), &f), Some((1, b"opus bytes".to_vec())));
        // Wrong epoch key, tampered ciphertext, a header claiming another sender: all refused.
        assert_eq!(open(|_| Some([8u8; 32]), &f), None);
        let mut t = f.clone();
        *t.last_mut().unwrap() ^= 1;
        assert_eq!(open(|_| Some(base), &t), None);
        let mut spoof = f.clone();
        spoof[8] = 2; // leaf 1 -> 2
        assert_eq!(open(|_| Some(base), &spoof), None);
        // Different senders get different keys.
        assert_ne!(sender_key(&base, 1), sender_key(&base, 2));
    }
}
