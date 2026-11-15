//! HKDF-SHA-256 (RFC 5869), hand-written over `crate::sha256::HmacKey` like the PBKDF2 next to it,
//! so the crate keeps one SHA-256 and one HMAC.
//!
//! The consumer is Watch Together (`plx_data::together`): a session's room number, host proof and
//! AES key are three HKDF expansions of one PBKDF2 output (empty salt, distinct `info`), and a grant
//! box's key is HKDF of an ECDH secret. Those are exactly WebCrypto's `deriveBits({name: 'HKDF'})`
//! on the web client, and the interop fixtures in `plx_data::together` hold both sides to the same
//! bytes. Pure functions over byte slices, graded against the RFC's SHA-256 test cases.

use crate::sha256::HmacKey;

/// The largest output HKDF-Expand may produce with SHA-256: 255 blocks of 32 bytes.
pub const HKDF_SHA256_MAX_OUT: usize = 255 * 32;

/// HKDF-Extract: the pseudorandom key. An empty `salt` is a block of zeros (RFC 5869 §2.2), which
/// HMAC's key padding already is, so it needs no special case.
pub fn extract(salt: &[u8], ikm: &[u8]) -> [u8; 32] {
    HmacKey::new(salt).mac(ikm)
}

/// HKDF-Expand into `out`. `None` (and `out` untouched) when `out` is longer than
/// [`HKDF_SHA256_MAX_OUT`], the one length the RFC refuses.
pub fn expand(prk: &[u8; 32], info: &[u8], out: &mut [u8]) -> Option<()> {
    if out.len() > HKDF_SHA256_MAX_OUT {
        return None;
    }
    let key = HmacKey::new(prk);
    let mut previous: [u8; 32] = [0; 32];
    for (index, chunk) in out.chunks_mut(32).enumerate() {
        let counter = [index as u8 + 1];
        let block = if index == 0 {
            key.mac_parts(&[info, &counter])
        } else {
            key.mac_parts(&[&previous, info, &counter])
        };
        chunk.copy_from_slice(&block[..chunk.len()]);
        previous = block;
    }
    Some(())
}

/// Extract then expand to `N` bytes (`N` at most [`HKDF_SHA256_MAX_OUT`], checked at compile time).
pub fn hkdf_sha256<const N: usize>(salt: &[u8], ikm: &[u8], info: &[u8]) -> [u8; N] {
    const { assert!(N <= HKDF_SHA256_MAX_OUT) };
    let prk = extract(salt, ikm);
    let mut out = [0u8; N];
    // Cannot fail: N is bounded above.
    let _ = expand(&prk, info, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    /// RFC 5869 Appendix A, test cases 1 to 3 (the SHA-256 ones): basic, long inputs and outputs
    /// (several blocks, `T(i-1)` chained), and zero-length salt and info (the Watch Together shape).
    #[test]
    fn hkdf_sha256_matches_rfc_5869() {
        // Test case 1.
        let ikm = [0x0bu8; 22];
        let salt = unhex("000102030405060708090a0b0c");
        let info = unhex("f0f1f2f3f4f5f6f7f8f9");
        let prk = extract(&salt, &ikm);
        assert_eq!(hex(&prk), "077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5");
        let okm: [u8; 42] = hkdf_sha256(&salt, &ikm, &info);
        assert_eq!(
            hex(&okm),
            "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"
        );

        // Test case 2.
        let ikm: Vec<u8> = (0x00..=0x4f).collect();
        let salt: Vec<u8> = (0x60..=0xaf).collect();
        let info: Vec<u8> = (0xb0..=0xff).collect();
        let prk = extract(&salt, &ikm);
        assert_eq!(hex(&prk), "06a6b88c5853361a06104c9ceb35b45cef760014904671014a193f40c15fc244");
        let okm: [u8; 82] = hkdf_sha256(&salt, &ikm, &info);
        assert_eq!(
            hex(&okm),
            "b11e398dc80327a1c8e7f78c596a49344f012eda2d4efad8a050cc4c19afa97c\
             59045a99cac7827271cb41c65e590e09da3275600c2f09b8367793a9aca3db71\
             cc30c58179ec3e87c14c01d5c1f3434f1d87"
        );

        // Test case 3: no salt, no info.
        let ikm = [0x0bu8; 22];
        let prk = extract(&[], &ikm);
        assert_eq!(hex(&prk), "19ef24a32c717b167f33a91d6f648bdf96596776afdb6377ac434c1c293ccb04");
        let okm: [u8; 42] = hkdf_sha256(&[], &ikm, &[]);
        assert_eq!(
            hex(&okm),
            "8da4e775a563c18f715f802a063c5a31b8a11f5c5ee1879ec3454e5f3c738d2d9d201395faa4b61a96c8"
        );
    }

    /// A prefix of a longer expansion is the shorter expansion (the counter is per block), and the
    /// RFC's one refused length is refused.
    #[test]
    fn expand_is_prefix_stable_and_bounded() {
        let prk = extract(b"salt", b"ikm");
        let mut long = [0u8; 100];
        expand(&prk, b"info", &mut long).unwrap();
        let mut short = [0u8; 16];
        expand(&prk, b"info", &mut short).unwrap();
        assert_eq!(&long[..16], &short);
        let mut too_long = vec![7u8; HKDF_SHA256_MAX_OUT + 1];
        assert_eq!(expand(&prk, b"info", &mut too_long), None);
        assert!(too_long.iter().all(|&b| b == 7));
        let mut max = vec![0u8; HKDF_SHA256_MAX_OUT];
        assert_eq!(expand(&prk, b"info", &mut max), Some(()));
    }
}
