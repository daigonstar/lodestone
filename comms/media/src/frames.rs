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
//! - Video frames (VP8) are the same, except that the first bytes of the VP8 frame stay readable (10 on
//!   a keyframe, 3 otherwise: frame type, version and size, nothing of the picture), as in WebRTC's own
//!   end-to-end encryption, so the transport can still tell keyframes apart. The header (version 2)
//!   goes at the end, and covers the readable bytes too.

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

pub const VIDEO_VERSION: u8 = 2;

/// How many leading bytes of a VP8 frame stay readable.
fn vp8_clear(frame: &[u8]) -> usize {
    let key = frame.first().is_some_and(|b| b & 1 == 0);
    frame.len().min(if key { 10 } else { 3 })
}

fn header(version: u8, epoch: u32, leaf: u32, counter: u64) -> [u8; HEADER] {
    let mut h = [0u8; HEADER];
    h[0] = version;
    h[1..5].copy_from_slice(&epoch.to_be_bytes());
    h[5..9].copy_from_slice(&leaf.to_be_bytes());
    h[9..17].copy_from_slice(&counter.to_be_bytes());
    h
}

/// Encrypt one encoded VP8 frame: readable VP8 header bytes, ciphertext, then the 17-byte header.
pub fn seal_video(base: &[u8; 32], epoch: u32, leaf: u32, counter: u64, frame: &[u8]) -> Vec<u8> {
    let n = vp8_clear(frame);
    let h = header(VIDEO_VERSION, epoch, leaf, counter);
    let mut aad = frame[..n].to_vec();
    aad.extend_from_slice(&h);
    let cipher = Aes256Gcm::new_from_slice(&sender_key(base, leaf)).expect("32-byte key");
    let sealed = cipher.encrypt(Nonce::from_slice(&nonce(counter)), Payload { msg: &frame[n..], aad: &aad }).expect("encryption doesn't fail");
    let mut out = Vec::with_capacity(n + sealed.len() + HEADER);
    out.extend_from_slice(&frame[..n]);
    out.extend_from_slice(&sealed);
    out.extend_from_slice(&h);
    out
}

/// Decrypt a video frame. Returns (sender leaf, VP8 frame).
pub fn open_video(base_for: impl Fn(u32) -> Option<[u8; 32]>, data: &[u8]) -> Option<(u32, Vec<u8>)> {
    if data.len() < HEADER + 16 + 1 {
        return None;
    }
    let (body, h) = data.split_at(data.len() - HEADER);
    if h[0] != VIDEO_VERSION {
        return None;
    }
    let epoch = u32::from_be_bytes(h[1..5].try_into().ok()?);
    let leaf = u32::from_be_bytes(h[5..9].try_into().ok()?);
    let counter = u64::from_be_bytes(h[9..17].try_into().ok()?);
    let n = vp8_clear(body);
    if body.len() < n + 16 {
        return None;
    }
    let mut aad = body[..n].to_vec();
    aad.extend_from_slice(h);
    let base = base_for(epoch)?;
    let cipher = Aes256Gcm::new_from_slice(&sender_key(&base, leaf)).ok()?;
    let rest = cipher.decrypt(Nonce::from_slice(&nonce(counter)), Payload { msg: &body[n..], aad: &aad }).ok()?;
    let mut frame = body[..n].to_vec();
    frame.extend_from_slice(&rest);
    Some((leaf, frame))
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

    #[test]
    fn video_frames_keep_the_keyframe_bit_readable() {
        let base = [9u8; 32];
        let key: Vec<u8> = (0..200u8).map(|i| if i == 0 { 0x10 } else { i }).collect(); // P bit 0: keyframe
        let delta: Vec<u8> = (0..200u8).map(|i| if i == 0 { 0x11 } else { i }).collect();
        for (f, clear) in [(&key, 10), (&delta, 3)] {
            let s = seal_video(&base, 5, 2, 7, f);
            assert_eq!(&s[..clear], &f[..clear], "VP8 header bytes stay readable");
            assert_ne!(&s[clear..clear + 20], &f[clear..clear + 20], "the picture is encrypted");
            assert_eq!(open_video(|e| (e == 5).then_some(base), &s), Some((2, f.clone())));
            // Tampering with a readable byte, or the wrong key, is refused.
            let mut t = s.clone();
            t[1] ^= 1;
            assert_eq!(open_video(|_| Some(base), &t), None);
            assert_eq!(open_video(|_| Some([1u8; 32]), &s), None);
            // An audio frame isn't accepted as video, or the reverse.
            assert_eq!(open(|_| Some(base), &s), None);
        }
        assert_eq!(open_video(|_| Some(base), &seal(&base, 5, 2, 8, b"opus")), None);
    }
}
