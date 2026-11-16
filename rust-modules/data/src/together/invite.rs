//! Invitation codes and relay addresses, ported from the web client's `packages/core/src/together.ts`
//! (`INVITE_ALPHABET`, `makeSecret`, `formatInvite`, `parseInvite`, `inviteText`, `relayUrl`).
//!
//! The web parses with three regular expressions; this crate has no regex engine, so each is
//! written out as the matcher it is (leftmost match, greedy, the same character classes and
//! JavaScript's `\s`), and the interop fixtures replay the web's own answers on the same texts.

use serde::Serialize;
use serde_json::Value;

/// 31 signs: digits and capitals without the ones confused by eye or ear (no 0/O, no 1/I/L).
pub const INVITE_ALPHABET: &str = "23456789ABCDEFGHJKMNPQRSTUVWXYZ";
/// Signs in a secret: 8 x log2(31), about 39.6 bits.
pub const INVITE_LENGTH: usize = 8;

/// A pasted invitation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Invite {
    /// `MPX-K7Q2-9XFD`: a short secret; the relay is found through the host's Plex server, or given
    /// after `@` when the host could not make it findable.
    V3 { secret: String, relay: Option<String> },
    /// `MPX-<16 base64url signs>@<relay>` (before short codes).
    V2 { relay: String, secret: String },
    /// `MPX-` and base64url JSON of relay, room and key (the very first ones).
    V1 { relay: String, room: String, key: String },
}

/// The secret of a new invitation from the random bytes `random` hands out (the web's
/// `makeSecret`): each call is asked for 16 bytes, and a byte at or above 248 (8 x 31) is dropped so
/// no sign is favoured. The caller's runner draws the bytes; nothing here reads the OS.
pub fn make_secret(mut random: impl FnMut(&mut [u8])) -> String {
    let alphabet = INVITE_ALPHABET.as_bytes();
    let limit = 256 - (256 % alphabet.len());
    let mut out = String::with_capacity(INVITE_LENGTH);
    while out.len() < INVITE_LENGTH {
        let mut chunk = [0u8; INVITE_LENGTH * 2];
        random(&mut chunk);
        for b in chunk {
            if (b as usize) < limit && out.len() < INVITE_LENGTH {
                out.push(alphabet[b as usize % alphabet.len()] as char);
            }
        }
    }
    out
}

/// [`make_secret`] over a pool of bytes the runner already drew: `None` when the pool runs out
/// before eight signs are accepted (hand in 64 bytes and that is about one chance in 10^40).
pub fn secret_from_bytes(pool: &[u8]) -> Option<String> {
    let mut chunks = pool.chunks(INVITE_LENGTH * 2);
    let mut short = false;
    let secret = make_secret(|buf| match chunks.next() {
        Some(c) if c.len() == buf.len() => buf.copy_from_slice(c),
        _ => {
            short = true;
            buf.fill(0);
        }
    });
    (!short).then_some(secret)
}

/// `MPX-K7Q2-9XFD`, or `MPX-K7Q2-9XFD@relay.example` when the relay has to be given.
pub fn format_invite(secret: &str, relay: Option<&str>) -> String {
    let s: Vec<char> = secret.to_uppercase().chars().collect();
    let head: String = s.iter().take(4).collect();
    let tail: String = s.iter().skip(4).collect();
    match relay.filter(|r| !r.is_empty()) {
        Some(r) => format!("MPX-{head}-{tail}@{}", r.strip_prefix("wss://").unwrap_or(r)),
        None => format!("MPX-{head}-{tail}"),
    }
}

/// The web's `clean`: letters and digits only, upper case.
pub fn clean(s: &str) -> String {
    s.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_uppercase()).collect()
}

/// The text to share again for an invitation already read (an old one is shared as it was).
pub fn invite_text(c: &Invite) -> String {
    match c {
        Invite::V3 { secret, relay } => format_invite(secret, relay.as_deref()),
        Invite::V2 { relay, secret } => format!("MPX-{secret}@{}", relay.strip_prefix("wss://").unwrap_or(relay)),
        Invite::V1 { relay, room, key } => {
            #[derive(Serialize)]
            struct V1<'a> {
                r: &'a str,
                i: &'a str,
                k: &'a str,
            }
            let json = serde_json::to_string(&V1 { r: relay, i: room, k: key }).unwrap_or_default();
            format!("MPX-{}", plx_base::b64::encode_url(json.as_bytes()))
        }
    }
}

/// JavaScript's `\s`.
fn js_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}'
        | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

/// The web's `SEP`: `[\s\-–—_.]`.
fn sep(c: char) -> bool {
    js_space(c) || matches!(c, '-' | '–' | '—' | '_' | '.')
}

/// The web's `CHAR` under the `i` flag: `[2-9A-HJKMNP-Z]`, either case.
fn sign(c: char) -> bool {
    INVITE_ALPHABET.contains(c.to_ascii_uppercase()) && c.is_ascii()
}

fn b64url_sign(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// `CHAR(?:SEP*CHAR){7}` from `i`: the end of the eighth sign, or `None`.
fn signs_at(s: &[char], mut i: usize) -> Option<usize> {
    for n in 0..INVITE_LENGTH {
        if n > 0 {
            while i < s.len() && sep(s[i]) {
                i += 1;
            }
        }
        if i < s.len() && sign(s[i]) {
            i += 1;
        } else {
            return None;
        }
    }
    Some(i)
}

/// `[^\s]+` from `i`, greedy: its end, or `None` when empty.
fn non_space_run(s: &[char], i: usize) -> Option<usize> {
    let mut j = i;
    while j < s.len() && !js_space(s[j]) {
        j += 1;
    }
    (j > i).then_some(j)
}

/// The web's `SHORT`: `MPX SEP* (8 signs) (?![A-Za-z0-9]) (?:\s*@\s*([^\s]+))?`, case-insensitive,
/// leftmost match. Returns the signs' text and the relay text.
fn short_match(s: &[char]) -> Option<(String, Option<String>)> {
    for start in 0..s.len() {
        if s.len() < start + 3 || !s[start..start + 3].iter().zip("MPX".chars()).all(|(a, b)| a.to_ascii_uppercase() == b) {
            continue;
        }
        let mut i = start + 3;
        while i < s.len() && sep(s[i]) {
            i += 1;
        }
        let Some(end) = signs_at(s, i) else { continue };
        if end < s.len() && s[end].is_ascii_alphanumeric() {
            continue;
        }
        let code: String = s[i..end].iter().collect();
        let mut j = end;
        while j < s.len() && js_space(s[j]) {
            j += 1;
        }
        let mut relay = None;
        if j < s.len() && s[j] == '@' {
            let mut k = j + 1;
            while k < s.len() && js_space(s[k]) {
                k += 1;
            }
            if let Some(stop) = non_space_run(s, k) {
                relay = Some(s[k..stop].iter().collect());
            }
        }
        return Some((code, relay));
    }
    None
}

/// The web's `BARE`: the whole (trimmed) text is the eight signs.
fn bare_match(s: &[char]) -> Option<String> {
    (signs_at(s, 0) == Some(s.len())).then(|| s.iter().collect())
}

/// The web's v2 expression: `MPX-([A-Za-z0-9_-]{16})@([^\s]+)`, case-sensitive.
fn v2_match(s: &[char]) -> Option<(String, String)> {
    for start in 0..s.len() {
        if s.len() < start + 4 + 16 + 2 || s[start..start + 4] != ['M', 'P', 'X', '-'] {
            continue;
        }
        let body = &s[start + 4..start + 20];
        if !body.iter().all(|&c| b64url_sign(c)) || s[start + 20] != '@' {
            continue;
        }
        let Some(stop) = non_space_run(s, start + 21) else { continue };
        return Some((body.iter().collect(), s[start + 21..stop].iter().collect()));
    }
    None
}

/// The web's v1 expression: `MPX-([A-Za-z0-9_-]{20,600})`, greedy, leftmost.
fn v1_match(s: &[char]) -> Option<String> {
    for start in 0..s.len() {
        if s.len() < start + 4 || s[start..start + 4] != ['M', 'P', 'X', '-'] {
            continue;
        }
        let run = s[start + 4..].iter().take(600).take_while(|&&c| b64url_sign(c)).count();
        if run >= 20 {
            return Some(s[start + 4..start + 4 + run].iter().collect());
        }
    }
    None
}

/// Reads an invitation out of pasted text (any case, free spaces and dashes, text around it).
pub fn parse_invite(text: &str) -> Option<Invite> {
    let trimmed: Vec<char> = text.trim_matches(js_space).chars().collect();
    if let Some((secret, relay)) = v2_match(&trimmed) {
        let relay = relay_url(&relay);
        return (!relay.is_empty()).then_some(Invite::V2 { relay, secret });
    }
    let short = short_match(&trimmed).or_else(|| bare_match(&trimmed).map(|code| (code, None)));
    if let Some((code, relay)) = short {
        let secret = clean(&code);
        let Some(relay) = relay else { return Some(Invite::V3 { secret, relay: None }) };
        let relay = relay_url(relay.trim_end_matches([')', ']', '.', ',', ';', '!', '?', '»', '"', '\'']));
        return (!relay.is_empty()).then_some(Invite::V3 { secret, relay: Some(relay) });
    }
    let raw = v1_match(&trimmed)?;
    let json: Value = serde_json::from_slice(&String::from_utf8_lossy(&plx_base::b64::decode_url(&raw)?).into_owned().into_bytes()).ok()?;
    let field = |k: &str| json.get(k).and_then(Value::as_str).filter(|s| !s.is_empty());
    let (relay, room, key) = (field("r")?, field("i")?, field("k")?);
    let ok = (relay.starts_with("ws://") || relay.starts_with("wss://"))
        && (16..=64).contains(&room.len())
        && room.chars().all(b64url_sign)
        && plx_base::b64::decode_url(key).is_some_and(|k| k.len() == 32);
    ok.then(|| Invite::V1 { relay: relay.into(), room: room.into(), key: key.into() })
}

/// A relay address as typed (`https://…`, `wss://…` or a bare name) -> `wss://…` without a final
/// `/`, or `""` when it is not one. The web's `relayUrl`, which leans on the WHATWG URL parser: this
/// is that parser's behaviour on the inputs a relay address takes (scheme, lower-cased host, port
/// without the scheme's default, path; query and fragment dropped). A user name, a non-ASCII host
/// or an IPv6 literal with a zone are refused rather than normalized.
pub fn relay_url(input: &str) -> String {
    let s = input.trim_matches(js_space).trim_end_matches('/');
    if s.is_empty() {
        return String::new();
    }
    let has_scheme = s.find("://").is_some_and(|i| i > 0 && s[..i].chars().all(|c| c.is_ascii_alphabetic()));
    let owned;
    let s = if has_scheme {
        s
    } else {
        owned = format!("https://{s}");
        &owned
    };
    parse_ws_url(s).unwrap_or_default()
}

/// Scheme://host[:port][/path] of a special scheme, switched to ws/wss.
fn parse_ws_url(s: &str) -> Option<String> {
    let (scheme, rest) = s.split_once("://")?;
    let scheme = match scheme.to_ascii_lowercase().as_str() {
        "https" | "wss" => "wss",
        "http" | "ws" => "ws",
        _ => return None,
    };
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    if authority.contains('@') || authority.is_empty() {
        return None;
    }
    let (host, port) = if let Some(after) = authority.strip_prefix('[') {
        let close = after.find(']')?;
        let host = &authority[..close + 2];
        let tail = &after[close + 1..];
        (host.to_string(), if tail.is_empty() { None } else { Some(tail.strip_prefix(':')?) })
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), Some(p)),
            None => (authority.to_string(), None),
        }
    };
    let host = host.to_ascii_lowercase();
    if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || "-._~[]:".contains(c)) {
        return None;
    }
    let default = if scheme == "wss" { 443 } else { 80 };
    let port = match port {
        None | Some("") => None,
        Some(p) => {
            if !p.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let n: u32 = p.trim_start_matches('0').parse().unwrap_or(0);
            if n > 65535 {
                return None;
            }
            (n != default).then_some(n)
        }
    };
    let mut segments: Vec<&str> = Vec::new();
    for seg in path.split('/').skip(1) {
        match seg {
            "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    let mut out = format!("{scheme}://{host}");
    if let Some(p) = port {
        out.push_str(&format!(":{p}"));
    }
    for seg in segments {
        out.push('/');
        for ch in seg.chars() {
            if ch.is_ascii_graphic() && !"\"<>`{}".contains(ch) {
                out.push(ch);
            } else {
                let mut buf = [0u8; 4];
                for b in ch.encode_utf8(&mut buf).bytes() {
                    out.push_str(&format!("%{b:02X}"));
                }
            }
        }
    }
    Some(out.trim_end_matches('/').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The web's `together.test.ts`, « invitations courtes ».
    #[test]
    fn alphabet_has_31_unambiguous_signs() {
        assert_eq!(INVITE_ALPHABET.len(), 31);
        let mut seen: Vec<char> = INVITE_ALPHABET.chars().collect();
        seen.dedup();
        assert_eq!(seen.len(), 31);
        for c in "0O1IL".chars() {
            assert!(!INVITE_ALPHABET.contains(c));
        }
    }

    #[test]
    fn secret_is_uniform_over_the_alphabet() {
        // A deterministic byte stream (xorshift) stands in for the runner's draw.
        let mut x = 0x2545_f491_4f6c_dd1du64;
        let mut counts = std::collections::BTreeMap::new();
        for _ in 0..4000 {
            let s = make_secret(|buf| {
                for b in buf.iter_mut() {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    *b = (x >> 24) as u8;
                }
            });
            assert_eq!(s.len(), 8);
            assert!(s.chars().all(|c| INVITE_ALPHABET.contains(c)), "{s}");
            for c in s.chars() {
                *counts.entry(c).or_insert(0i32) += 1;
            }
        }
        assert_eq!(counts.len(), 31);
        for n in counts.values() {
            assert!((n - 1032).abs() < 200, "{n}");
        }
    }

    #[test]
    fn secret_drops_the_bytes_that_would_bias_it() {
        let mut calls = 0;
        let s = make_secret(|buf| {
            buf.fill(if calls == 0 { 255 } else { 0 });
            calls += 1;
        });
        assert_eq!(s, "22222222");
    }

    #[test]
    fn secret_from_a_drawn_pool() {
        assert_eq!(secret_from_bytes(&[0; 16]).as_deref(), Some("22222222"));
        assert_eq!(secret_from_bytes(&[255; 64]), None);
        assert_eq!(secret_from_bytes(&[0; 8]), None);
        let mut pool = vec![255u8; 16];
        pool.extend([1u8; 16]);
        assert_eq!(secret_from_bytes(&pool).as_deref(), Some("33333333"));
    }

    #[test]
    fn format_and_share() {
        assert_eq!(format_invite("K7Q29XFD", None), "MPX-K7Q2-9XFD");
        assert_eq!(format_invite("k7q29xfd", Some("wss://relais.exemple.fr")), "MPX-K7Q2-9XFD@relais.exemple.fr");
        assert_eq!(invite_text(&Invite::V3 { secret: "K7Q29XFD".into(), relay: None }), "MPX-K7Q2-9XFD");
    }

    #[test]
    fn relay_url_is_normalized() {
        assert_eq!(relay_url("https://relais.exemple.fr/"), "wss://relais.exemple.fr");
        assert_eq!(relay_url("relais.exemple.fr:1744"), "wss://relais.exemple.fr:1744");
        assert_eq!(relay_url("http://localhost:8787"), "ws://localhost:8787");
        assert_eq!(relay_url("ftp://x.fr"), "");
        assert_eq!(relay_url("https://user@x.fr"), "");
        assert_eq!(relay_url("https://x.fr:99999"), "");
        assert_eq!(relay_url("https://x.fr/a/../b/./c/"), "wss://x.fr/b/c");
    }
}
