//! The two primitives Watch Together needs that are not worth writing by hand: AES-256-GCM and
//! P-256 ECDH, from RustCrypto (`aes-gcm`, `p256`; pure Rust, Apache-2.0 OR MIT). HKDF and
//! base64url are this crate's own (`crate::hkdf`, `crate::b64`).
//!
//! **No randomness here.** Neither crate is built with `getrandom`, and nothing in this module
//! draws a byte: an IV and an ECDH secret are arguments, which the caller's runner reads from the
//! OS (`/dev/urandom`) and hands in. That keeps every function a pure computation a Sans-IO machine
//! may ask for as an effect, and a test may pin.
//!
//! **Wire shapes are the web client's** (`packages/core/src/together.ts` and `apps/web/src/together.ts`
//! of the MultiPlex web client): a sealed box is `iv (12 bytes) ‖ ciphertext ‖ tag (16 bytes)`,
//! with no associated data; a public key is the 65-byte uncompressed SEC1 point
//! (`exportKey('raw')`); an ECDH secret is the 32-byte x coordinate (`deriveBits(…, 256)`).

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use p256::elliptic_curve::sec1::ToSec1Point;

/// Bytes of an AES-GCM IV (the web client's `new Uint8Array(12)`).
pub const GCM_IV_LEN: usize = 12;
/// Bytes of an AES-GCM tag.
pub const GCM_TAG_LEN: usize = 16;
/// Bytes of an uncompressed P-256 public key (`0x04 ‖ x ‖ y`).
pub const P256_PUBLIC_LEN: usize = 65;

/// AES-256-GCM of `plain` under `key` with `iv`: `iv ‖ ciphertext ‖ tag`. The IV must never repeat
/// under one key; the caller draws it.
pub fn seal(key: &[u8; 32], iv: &[u8; GCM_IV_LEN], plain: &[u8]) -> Vec<u8> {
    let cipher = Aes256Gcm::new(key.into());
    let nonce = Nonce::from(*iv);
    // Encryption only fails for a message longer than GCM allows (64 GiB), which no caller sends.
    let sealed = cipher.encrypt(&nonce, plain).unwrap_or_default();
    let mut out = Vec::with_capacity(GCM_IV_LEN + sealed.len());
    out.extend_from_slice(iv);
    out.extend_from_slice(&sealed);
    out
}

/// The plaintext of a box [`seal`] made, or `None` when it is too short, was sealed under another
/// key, or was changed in any byte.
pub fn open(key: &[u8; 32], sealed: &[u8]) -> Option<Vec<u8>> {
    if sealed.len() < GCM_IV_LEN + GCM_TAG_LEN {
        return None;
    }
    let (iv, body) = sealed.split_at(GCM_IV_LEN);
    let iv: [u8; GCM_IV_LEN] = iv.try_into().ok()?;
    Aes256Gcm::new(key.into()).decrypt(&Nonce::from(iv), body).ok()
}

/// A P-256 ECDH secret scalar. Built from 32 bytes the caller drew; it never leaves this type
/// except as the public point and the shared secret.
pub struct EcdhSecret(p256::SecretKey);

impl EcdhSecret {
    /// The secret whose big-endian scalar is `bytes`, or `None` for the zero scalar or one at or
    /// above the group order (probability about 2^-32 for uniform bytes): draw again.
    pub fn from_bytes(bytes: &[u8; 32]) -> Option<Self> {
        p256::SecretKey::from_slice(bytes).ok().map(EcdhSecret)
    }

    /// The public point, uncompressed (the web client's `exportKey('raw', publicKey)`).
    pub fn public_raw(&self) -> [u8; P256_PUBLIC_LEN] {
        let point = self.0.public_key().to_sec1_point(false);
        let mut out = [0u8; P256_PUBLIC_LEN];
        out.copy_from_slice(point.as_bytes());
        out
    }

    /// The shared secret with `peer` (an uncompressed or compressed SEC1 point): its x coordinate,
    /// as WebCrypto's `deriveBits({name: 'ECDH'}, …, 256)` returns it. `None` for bytes that are not
    /// a point on the curve.
    pub fn shared(&self, peer: &[u8]) -> Option<[u8; 32]> {
        let peer = p256::PublicKey::from_sec1_bytes(peer).ok()?;
        let shared = self.0.diffie_hellman(&peer);
        let mut out = [0u8; 32];
        out.copy_from_slice(shared.raw_secret_bytes().as_slice());
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    /// The GCM specification's (McGrew and Viega) test case 15: AES-256, a 96-bit IV, no AAD.
    #[test]
    fn aes_256_gcm_matches_the_gcm_spec_test_case_15() {
        let key: [u8; 32] = unhex("feffe9928665731c6d6a8f9467308308feffe9928665731c6d6a8f9467308308").try_into().unwrap();
        let iv: [u8; 12] = unhex("cafebabefacedbaddecaf888").try_into().unwrap();
        let plain = unhex(
            "d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a721c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b391aafd255",
        );
        let sealed = seal(&key, &iv, &plain);
        let expect = [
            "cafebabefacedbaddecaf888",
            "522dc1f099567d07f47f37a32a84427d643a8cdcbfe5c0c97598a2bd2555d1aa8cb08e48590dbb3da7b08b1056828838c5f61e6393ba7a0abcc9f662898015ad",
            "b094dac5d93471bdec1a502270e3cc6c",
        ]
        .concat();
        assert_eq!(sealed, unhex(&expect));
        assert_eq!(open(&key, &sealed).unwrap(), plain);
    }

    #[test]
    fn open_refuses_a_changed_box_another_key_or_a_short_one() {
        let key = [3u8; 32];
        let sealed = seal(&key, &[9u8; 12], b"salut");
        assert_eq!(sealed.len(), 12 + 5 + 16);
        for i in 0..sealed.len() {
            let mut bad = sealed.clone();
            bad[i] ^= 1;
            assert_eq!(open(&key, &bad), None, "byte {i}");
        }
        assert_eq!(open(&[4u8; 32], &sealed), None);
        assert_eq!(open(&key, &sealed[..27]), None);
        assert_eq!(open(&key, &[]), None);
        assert_eq!(open(&key, &seal(&key, &[0; 12], b"")).unwrap(), b"");
    }

    /// RFC 5903 section 8.1 (ECDH with P-256): both sides' publics and the shared x coordinate.
    #[test]
    fn p256_ecdh_matches_rfc_5903() {
        let i: [u8; 32] = unhex("c88f01f510d9ac3f70a292daa2316de544e9aab8afe84049c62a9c57862d1433").try_into().unwrap();
        let r: [u8; 32] = unhex("c6ef9c5d78ae012a011164acb397ce2088685d8f06bf9be0b283ab46476bee53").try_into().unwrap();
        let gi = unhex(
            "04dad0b65394221cf9b051e1feca5787d098dfe637fc90b9ef945d0c37725811805271a0461cdb8252d61f1c456fa3e59ab1f45b33accf5f58389e0577b8990bb3",
        );
        let gr = unhex(
            "04d12dfb5289c8d4f81208b70270398c342296970a0bccb74c736fc7554494bf6356fbf3ca366cc23e8157854c13c58d6aac23f046ada30f8353e74f33039872ab",
        );
        let shared = unhex("d6840f6b42f6edafd13116e0e12565202fef8e9ece7dce03812464d04b9442de");
        let si = EcdhSecret::from_bytes(&i).unwrap();
        let sr = EcdhSecret::from_bytes(&r).unwrap();
        assert_eq!(si.public_raw().to_vec(), gi);
        assert_eq!(sr.public_raw().to_vec(), gr);
        assert_eq!(si.shared(&gr).unwrap().to_vec(), shared);
        assert_eq!(sr.shared(&gi).unwrap().to_vec(), shared);
    }

    #[test]
    fn p256_refuses_the_zero_scalar_one_past_the_order_and_a_point_off_the_curve() {
        assert!(EcdhSecret::from_bytes(&[0; 32]).is_none());
        assert!(EcdhSecret::from_bytes(&[0xff; 32]).is_none());
        let s = EcdhSecret::from_bytes(&[1; 32]).unwrap();
        let mut off = s.public_raw();
        off[64] ^= 1;
        assert_eq!(s.shared(&off), None);
        assert_eq!(s.shared(&[]), None);
        assert_eq!(s.shared(&[4; 65]), None);
    }
}
