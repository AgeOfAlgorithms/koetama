//! The room and its key (PROTOCOL.md "Who makes the room"), and the end-to-end encryption of a voice packet:
//!   payload = nonce (12 random bytes) | ChaCha20-Poly1305(key, nonce, plaintext, aad = from as u16 big-endian)
//! (the 16-byte tag at the end). The aad binds the packet to its sender: the relay stamps `from`, and a packet it
//! (or anyone without the key) changed or re-addressed does not decrypt.
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};

pub const KEY_LEN: usize = 32;
pub const NONCE_LEN: usize = 12;
pub const TAG_LEN: usize = 16;
/// hex digits of a room's name (16 bytes)
pub const ROOM_HEX: usize = 32;

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// n random bytes from the OS.
pub fn random_bytes(n: usize) -> Result<Vec<u8>, String> {
    let mut b = vec![0u8; n];
    getrandom::fill(&mut b).map_err(|e| format!("no randomness from the system: {e}"))?;
    Ok(b)
}

/// A fresh room for a game session, as the 'r' message's text: "<room: 32 hex>:<key: 64 hex>" (OS randomness).
pub fn new_room() -> Result<String, String> {
    Ok(format!("{}:{}", hex(&random_bytes(ROOM_HEX / 2)?), hex(&random_bytes(KEY_LEN)?)))
}

/// A key from its 64 lower-case hex digits (the feed's form), or None.
pub fn key_from_hex(s: &str) -> Option<[u8; KEY_LEN]> {
    if s.len() != 2 * KEY_LEN || !s.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)) {
        return None;
    }
    let mut k = [0u8; KEY_LEN];
    for (i, b) in k.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(k)
}

/// The payload for plaintext from player `from` (a fresh random nonce).
pub fn seal(key: &[u8; KEY_LEN], from: u16, plain: &[u8]) -> Result<Vec<u8>, String> {
    let nonce = random_bytes(NONCE_LEN)?;
    seal_with(key, from, plain, &nonce)
}

/// seal with a given nonce (tests).
pub fn seal_with(key: &[u8; KEY_LEN], from: u16, plain: &[u8], nonce: &[u8]) -> Result<Vec<u8>, String> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let aad = from.to_be_bytes();
    let sealed = cipher
        .encrypt(Nonce::from_slice(nonce), Payload { msg: plain, aad: &aad })
        .map_err(|_| "could not encrypt".to_string())?;
    let mut out = Vec::with_capacity(NONCE_LEN + sealed.len());
    out.extend_from_slice(nonce);
    out.extend_from_slice(&sealed);
    Ok(out)
}

/// The plaintext of a payload from player `from`, or None (another key, another sender, changed: dropped).
pub fn open(key: &[u8; KEY_LEN], from: u16, payload: &[u8]) -> Option<Vec<u8>> {
    if payload.len() < NONCE_LEN + TAG_LEN {
        return None;
    }
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let aad = from.to_be_bytes();
    cipher.decrypt(Nonce::from_slice(&payload[..NONCE_LEN]), Payload { msg: &payload[NONCE_LEN..], aad: &aad }).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rooms_are_random_hex() {
        let (a, b) = (new_room().unwrap(), new_room().unwrap());
        assert_ne!(a, b);
        let (room, key) = a.split_once(':').unwrap();
        assert!(room.len() == 32 && key.len() == 64);
        assert!(a.bytes().all(|c| c == b':' || c.is_ascii_digit() || (b'a'..=b'f').contains(&c)));
        assert!(key_from_hex(key).is_some());
        assert!(key_from_hex(&key.to_uppercase()).is_none() && key_from_hex(&key[1..]).is_none());
        assert_eq!(key_from_hex(&"0f".repeat(32)).unwrap(), [15u8; 32]);
    }

    #[test]
    fn sealed_opens_only_with_the_key_and_sender() {
        let key = [7u8; 32];
        let p = seal(&key, 12, b"hello voice").unwrap();
        assert_eq!(p.len(), NONCE_LEN + 11 + TAG_LEN);
        assert_eq!(open(&key, 12, &p).unwrap(), b"hello voice");
        assert!(open(&[8u8; 32], 12, &p).is_none(), "another key");
        assert!(open(&key, 13, &p).is_none(), "another sender (aad)");
        for i in 0..p.len() {
            let mut t = p.clone();
            t[i] ^= 1;
            assert!(open(&key, 12, &t).is_none(), "byte {i} changed");
        }
        assert!(open(&key, 12, &p[..20]).is_none() && open(&key, 12, &[]).is_none());
        assert_ne!(seal(&key, 12, b"x").unwrap()[..12], seal(&key, 12, b"x").unwrap()[..12], "a fresh nonce each time");
    }

    /// The RFC 8439 AEAD test vector (section 2.8.2) - the same cipher anyone else's implementation would use.
    #[test]
    fn rfc8439_vector() {
        let key: Vec<u8> = (0x80u8..=0x9f).collect();
        let nonce = [0x07, 0, 0, 0, 0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47];
        let aad = [0x50, 0x51, 0x52, 0x53, 0xc0, 0xc1, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7];
        let msg = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&key));
        let out = cipher.encrypt(Nonce::from_slice(&nonce), Payload { msg, aad: &aad }).unwrap();
        assert_eq!(hex(&out[..16]), "d31a8d34648e60db7b86afbc53ef7ec2");
        assert_eq!(hex(&out[out.len() - 16..]), "1ae10b594f09e26a7e902ecbd0600691");
    }
}
