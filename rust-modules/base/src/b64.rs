//! Standard-alphabet, padded base64 (RFC 4648 section 4), hand-written so there is no base64
//! dependency. A leaf module: it references no other crate module, so `keymanager` (sealed
//! key blobs) and `spki` (pin strings) can share it without `spki` joining the keymanager cycle.
//!
//! Also the URL-safe alphabet without padding (RFC 4648 section 5), [`encode_url`] /
//! [`decode_url`]: Watch Together's room numbers, host proofs, public keys and grant boxes, which
//! the web client writes with `btoa` and the `+/` -> `-_` swap and reads back with `atob`.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const URL_ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// URL-safe base64 without padding (the web client's `b64`).
pub fn encode_url(bytes: &[u8]) -> String {
    let mut out = String::with_capacity((bytes.len() * 4).div_ceil(3));
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..=chunk.len() {
            out.push(URL_ALPHABET[(n >> (18 - i * 6)) as usize & 63] as char);
        }
    }
    out
}

/// URL-safe base64, padded or not (the web client's `unb64`, which is `atob` after the `-_` ->
/// `+/` swap: so the standard alphabet's two signs are read too, like `atob` reads them). `None`
/// for a sign outside both alphabets, padding anywhere but the end, or a length `atob` refuses
/// (one sign left over). The unused low bits of the last sign are ignored, as `atob` ignores them.
pub fn decode_url(text: &str) -> Option<Vec<u8>> {
    let body = text.trim_end_matches('=');
    let padding = text.len() - body.len();
    if padding > 2 || (padding > 0 && text.len() % 4 != 0) || body.len() % 4 == 1 {
        return None;
    }
    let mut acc = 0u32;
    let mut bits = 0u32;
    let mut out = Vec::with_capacity(body.len() * 3 / 4);
    for ch in body.bytes() {
        let value = match ch {
            b'A'..=b'Z' => ch - b'A',
            b'a'..=b'z' => ch - b'a' + 26,
            b'0'..=b'9' => ch - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        } as u32;
        acc = (acc << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - i * 6)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn decode(text: &str) -> Option<Vec<u8>> {
    if text.len() % 4 != 0 {
        return None;
    }
    let mut acc = 0u32;
    let mut bits = 0u32;
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    for ch in text.bytes() {
        if ch == b'=' {
            break;
        }
        let value = ALPHABET.iter().position(|&x| x == ch)? as u32;
        acc = (acc << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_matches_the_rfc_4648_vectors() {
        for (raw, enc) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")] {
            assert_eq!(encode(raw.as_bytes()), enc);
        }
    }

    /// RFC 4648 section 10's vectors without their padding, and the two signs that differ.
    #[test]
    fn url_alphabet_round_trips_without_padding() {
        for (raw, enc) in [("", ""), ("f", "Zg"), ("fo", "Zm8"), ("foo", "Zm9v"), ("foob", "Zm9vYg"),
            ("fooba", "Zm9vYmE"), ("foobar", "Zm9vYmFy")] {
            assert_eq!(encode_url(raw.as_bytes()), enc);
            assert_eq!(decode_url(enc).unwrap(), raw.as_bytes());
        }
        assert_eq!(encode_url(&[0xfb, 0xff, 0xbf]), "-_-_");
        assert_eq!(decode_url("-_-_").unwrap(), [0xfb, 0xff, 0xbf]);
        // `atob` after the swap reads the standard signs and padding as well.
        assert_eq!(decode_url("+/+/").unwrap(), [0xfb, 0xff, 0xbf]);
        assert_eq!(decode_url("Zg==").unwrap(), b"f");
        for raw in [&[][..], &[0], &[1, 2], &[0xff; 16], &[7; 32], &[0x80; 65]] {
            assert_eq!(decode_url(&encode_url(raw)).unwrap(), raw);
            assert_eq!(encode_url(raw), encode(raw).trim_end_matches('=').replace('+', "-").replace('/', "_"));
        }
    }

    #[test]
    fn url_decode_refuses_what_atob_refuses() {
        for bad in ["Z", "Zm9vY", "Zm=9", "Zg=", "Zg===", "Zm9v!", "é"] {
            assert_eq!(decode_url(bad), None, "{bad}");
        }
    }
}
