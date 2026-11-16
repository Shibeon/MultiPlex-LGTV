//! **Watch Together's Plex requests** (`plx_data::stores::together` asks them through the app's
//! together runner): the account's Plex friends, read from plex.tv `/api/users/` with the account's
//! own token to build the host's allow-list, and the delegation token an owner's server hands out
//! (`/security/token?type=delegation&scope=all`) so a relay can check access to that server without
//! ever seeing the account token.
//!
//! **The dev-build write gate** ([`together_write_gate`]): announcing a session to a real server's
//! relay, and asking a real server for a delegation token, are writes on behalf of the household's
//! account. A `devtriggers` build makes neither unless the server is on loopback (the mock), the
//! plex.tv / Discover stand-in is armed, or the owner's live switch is on — the same switch as the
//! Watchlist's ([`super::watchlist`]). Release builds always may.

use super::account::{plex_tv, stand_in_armed, AccountClient};

/// The friends list is a few kilobytes per hundred friends; a body past this is not read.
pub const MAX_FRIENDS_XML_BYTES: usize = 4 << 20;
/// A delegation answer is one token; a body past this is not one.
pub const MAX_DELEGATION_BYTES: usize = 64 << 10;

/// Both requests wait at most this long (the web's 6 s friends budget, rounded to whole seconds).
const TIMEOUTS: plx_net::net::Timeouts = plx_net::net::Timeouts { connect_s: 4, total_s: 6, ..plx_net::net::API };

/// The pure decision of [`together_write_gate`].
fn permit(devtriggers_build: bool, server_is_loopback: bool, stand_in: bool, live: bool) -> bool {
    !devtriggers_build || server_is_loopback || stand_in || live
}

/// May this build announce to the relay of a server, and ask that server a delegation token?
/// `server_is_loopback`: the server's address is `127.0.0.1` / `localhost` (the mock).
pub fn together_write_gate(server_is_loopback: bool) -> bool {
    permit(cfg!(feature = "devtriggers"), server_is_loopback, stand_in_armed(), super::watchlist::live_writes_armed())
}

/// Is `base` (`scheme://host[:port]`) a loopback address?
pub fn is_loopback_base(base: &str) -> bool {
    let Some((_, rest)) = base.split_once("://") else { return false };
    let authority = rest.split('/').next().unwrap_or("");
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => authority.rsplit_once(':').map_or(authority, |(h, _)| h),
    };
    host == "127.0.0.1" || host.eq_ignore_ascii_case("localhost") || host == "::1"
}

fn xml_headers(mut headers: Vec<String>) -> Vec<String> {
    for h in &mut headers {
        if h.starts_with("Accept:") {
            *h = "Accept: application/xml".into();
        }
    }
    headers
}

impl AccountClient {
    /// `GET plex.tv/api/users/`: the account's Plex friends as XML (`friends::parse_plex_friends`
    /// reads it), or `None` when plex.tv did not answer 200.
    pub fn plex_friends_xml(&self) -> Option<String> {
        let url = format!("{}/api/users/", plex_tv());
        let resp = plx_net::net::request_evidence(&url, &xml_headers(self.headers()), "GET", None, TIMEOUTS, false, Some(MAX_FRIENDS_XML_BYTES), None).ok()?;
        if resp.status != 200 {
            return None;
        }
        String::from_utf8(resp.body).ok()
    }
}

/// The owner's server at `base` hands out a delegation token, asked with the account token
/// `token` (the server's owner). `None` when the gate refuses ([`together_write_gate`]), the server
/// does not answer 200, or the answer carries no token.
pub fn delegation_token(base: &str, client_id: &str, token: &str) -> Option<String> {
    if !together_write_gate(is_loopback_base(base)) {
        plx_base::eventlog::log("together: delegation token refused (debug build, real server, live switch off)");
        return None;
    }
    let url = format!("{}/security/token?type=delegation&scope=all", base.trim_end_matches('/'));
    let headers = AccountClient::new(client_id, Some(token)).headers();
    let resp = plx_net::net::request_evidence(&url, &headers, "GET", None, TIMEOUTS, false, Some(MAX_DELEGATION_BYTES), None).ok()?;
    if resp.status != 200 {
        return None;
    }
    read_delegation(&resp.body)
}

/// `{"MediaContainer":{"token":"…"}}`'s token.
fn read_delegation(body: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    v.get("MediaContainer")?.get("token")?.as_str().filter(|t| !t.is_empty()).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_debug_build_writes_only_to_loopback_a_stand_in_or_with_the_live_switch() {
        assert!(permit(false, false, false, false), "a release build always may");
        assert!(!permit(true, false, false, false));
        assert!(permit(true, true, false, false));
        assert!(permit(true, false, true, false));
        assert!(permit(true, false, false, true));
    }

    #[test]
    fn loopback_bases() {
        for yes in ["http://127.0.0.1:32400", "http://localhost:1/x", "https://[::1]:32400", "http://LOCALHOST"] {
            assert!(is_loopback_base(yes), "{yes}");
        }
        for no in ["https://127-0-0-1.abc.plex.direct:32400", "http://127.0.0.1.evil.example:1", "127.0.0.1", "http://10.0.0.2:32400"] {
            assert!(!is_loopback_base(no), "{no}");
        }
    }

    #[test]
    fn the_delegation_answer_is_its_token() {
        assert_eq!(read_delegation(br#"{"MediaContainer":{"size":0,"token":"transient-abc"}}"#), Some("transient-abc".into()));
        assert_eq!(read_delegation(br#"{"MediaContainer":{"token":""}}"#), None);
        assert_eq!(read_delegation(b"<MediaContainer token=\"x\"/>"), None);
    }

    #[test]
    fn the_friends_read_asks_for_xml() {
        let h = xml_headers(vec!["Accept: application/json".into(), "X-Plex-Token: t".into()]);
        assert_eq!(h[0], "Accept: application/xml");
    }
}
