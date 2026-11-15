//! A session's room, host proof and key, and the sealing of its messages: the web client's
//! `deriveSession`, `deriveLegacy` and `encrypt`/`decrypt` (the grant hand-over is in `friends`).
//!
//! All of it is computation over `plx_base::{sha256, hkdf, crypto, b64}`. PBKDF2 at 200 000 rounds
//! is tens to hundreds of milliseconds, so `TogetherSession` never calls [`derive_session`] in its
//! step: it asks for it with a `Derive` effect the runner executes off the frame. An IV is always an
//! argument (the runner draws it).

use plx_base::b64::{decode_url, encode_url};
use plx_base::crypto;
use plx_base::hkdf::hkdf_sha256;

use super::invite::{clean, Invite};

/// PBKDF2-SHA-256 rounds of a short code (the web's `INVITE_ITERATIONS`).
pub const INVITE_ITERATIONS: u32 = 200_000;

/// What a session is reached and read with. `room` is the only part the relay sees; `owner` is the
/// host proof (members only); `key` never leaves the device.
#[derive(Clone, PartialEq, Eq)]
pub struct SessionKeys {
    pub room: String,
    pub owner: Option<String>,
    pub key: [u8; 32],
}

impl std::fmt::Debug for SessionKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionKeys").field("room", &self.room).field("owner", &self.owner).finish_non_exhaustive()
    }
}

/// HKDF-SHA-256 with an empty salt: room (16 bytes), host proof (16 bytes), AES key (32 bytes).
fn split(base: &[u8]) -> SessionKeys {
    let room: [u8; 16] = hkdf_sha256(&[], base, b"multiplex-together-room");
    let owner: [u8; 16] = hkdf_sha256(&[], base, b"multiplex-together-owner");
    let key: [u8; 32] = hkdf_sha256(&[], base, b"multiplex-together-key");
    SessionKeys { room: encode_url(&room), owner: Some(encode_url(&owner)), key }
}

/// Room, proof and key of a short-code session: PBKDF2 of the cleaned secret, salted with the
/// host's Plex server identifier, then [`split`].
pub fn derive_session(secret: &str, server_id: &str, iterations: u32) -> SessionKeys {
    let salt = format!("multiplex-together-v3|{server_id}");
    let base = plx_base::sha256::pbkdf2_hmac_sha256(clean(secret).as_bytes(), salt.as_bytes(), iterations);
    split(&base)
}

/// Room and key of an old invitation: v2 is [`split`] of its 96-bit secret, v1 carries both as is.
/// `None` for a short code (that needs [`derive_session`]) or bytes that do not decode.
pub fn derive_legacy(invite: &Invite) -> Option<SessionKeys> {
    match invite {
        Invite::V3 { .. } => None,
        Invite::V2 { secret, .. } => Some(split(&decode_url(secret)?)),
        Invite::V1 { room, key, .. } => Some(SessionKeys { room: room.clone(), owner: None, key: decode_url(key)?.try_into().ok()? }),
    }
}

/// A session message sealed for the relay: `iv ‖ AES-GCM(json)`.
pub fn seal_message(keys: &[u8; 32], iv: &[u8; 12], json: &str) -> Vec<u8> {
    crypto::seal(keys, iv, json.as_bytes())
}

/// The JSON of a frame sealed under `key`, or `None` (wrong key, damaged: the web ignores it).
pub fn open_message(key: &[u8; 32], frame: &[u8]) -> Option<Vec<u8>> {
    crypto::open(key, frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The web's « preuve d'hôte » and « clés de séance » tests.
    #[test]
    fn same_secret_and_server_same_session_another_server_another() {
        let a = derive_session("K7Q2-9XFD", "machine-a", 1000);
        let b = derive_session("k7q29xfd", "machine-a", 1000);
        let c = derive_session("K7Q29XFD", "machine-b", 1000);
        assert_eq!(a, b);
        assert_eq!(a.room.len(), 22);
        assert_ne!(c.room, a.room);
        assert_ne!(c.key, a.key);
        let owner = a.owner.clone().unwrap();
        assert!((16..=64).contains(&owner.len()));
        assert_ne!(owner, a.room);
        assert_ne!(c.owner, a.owner);
        let sealed = seal_message(&a.key, &[0; 12], "salut");
        assert_eq!(open_message(&b.key, &sealed).unwrap(), b"salut");
        assert_eq!(open_message(&c.key, &sealed), None);
    }

    #[test]
    fn legacy_rooms() {
        let v2 = Invite::V2 { relay: "wss://relais.exemple.fr".into(), secret: "q3Vb8xK2mT0wZr5L".into() };
        let d = derive_legacy(&v2).unwrap();
        assert_eq!(d.room.len(), 22);
        let v1 = Invite::V1 { relay: "wss://r".into(), room: "AAAAAAAAAAAAAAAAAAAAAA".into(), key: encode_url(&[7; 32]) };
        let d = derive_legacy(&v1).unwrap();
        assert_eq!((d.room.as_str(), d.key, d.owner), ("AAAAAAAAAAAAAAAAAAAAAA", [7; 32], None));
        assert_eq!(derive_legacy(&Invite::V3 { secret: "K7Q29XFD".into(), relay: None }), None);
    }
}
