//! PROVISIONAL: the pieces of the web client's "sessions visible to friends" that are pure and
//! protocol-neutral, kept apart because that protocol (the relay's `POST /friends`, `announce`,
//! the key-request/key-grant ticket flow) is being redesigned so visibility is strictly between the
//! host's Plex friends. Nothing in `TogetherSession` uses this module: a session this client opens is
//! never announced. What is here is the web's `serverCredential`, `RelayBackoff` (driven by `dt`)
//! and `findRelay` as a fold, with its rule that a relay's Plex server identifier is checked BEFORE
//! any token is sent, and the grant crypto (`sealGrant`/`openGrant`: ECDH P-256, HKDF, AES-GCM),
//! byte-for-byte the web's (`interop.json`, `grants`).

use std::collections::BTreeMap;

use plx_base::b64::{decode_url, encode_url};
use plx_base::crypto::{self, EcdhSecret};
use plx_base::hkdf::hkdf_sha256;
use serde::Serialize;
use serde_json::Value;

use super::invite::{clean, INVITE_LENGTH};
use super::relay::{dedupe, KnownRelays, RoomsAnswer, MAX_RELAY_CANDIDATES};
use super::wire::js_number;

/// The token that proves access to a server's relay: the server's own, never the account's. `None`
/// when plex.tv hands the account token for it (the caller may ask the server for a delegated one).
pub fn server_credential(access_token: Option<&str>, account_token: Option<&str>) -> Option<String> {
    access_token.filter(|t| !t.is_empty() && Some(*t) != account_token).map(str::to_string)
}

/// Relays that failed or were useless, set aside for `base` ms, doubled on each failure up to
/// `max`. Time is the `dt` handed to [`advance`](RelayBackoff::advance); nothing reads a clock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayBackoff {
    now_ms: u64,
    base_ms: u64,
    max_ms: u64,
    misses: BTreeMap<String, (u64, u64)>,
}

impl Default for RelayBackoff {
    fn default() -> Self {
        RelayBackoff::new(60_000, 3_600_000)
    }
}

impl RelayBackoff {
    pub fn new(base_ms: u64, max_ms: u64) -> RelayBackoff {
        RelayBackoff { now_ms: 0, base_ms, max_ms, misses: BTreeMap::new() }
    }

    pub fn advance(&mut self, dt_ms: u64) {
        self.now_ms = self.now_ms.saturating_add(dt_ms);
    }

    pub fn blocked(&self, relay: &str) -> bool {
        self.misses.get(relay).is_some_and(|&(until, _)| until > self.now_ms)
    }

    pub fn fail(&mut self, relay: &str) {
        let delay = match self.misses.get(relay) {
            Some(&(_, d)) => (d * 2).min(self.max_ms),
            None => self.base_ms,
        };
        self.misses.insert(relay.to_string(), (self.now_ms + delay, delay));
    }

    pub fn ok(&mut self, relay: &str) {
        self.misses.remove(relay);
    }
}

/// A relay found by [`RelaySearch`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundRelay {
    pub relay: String,
    pub friends: bool,
    pub server: Option<String>,
}

/// The web's `findRelay` as a fold: ask each of [`relays`](RelaySearch::relays) `GET …/rooms` (no
/// room), feed each answer, read [`outcome`](RelaySearch::outcome). With `server`, only a relay that
/// lists AND whose Plex is that server counts, which is known before any token leaves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelaySearch {
    list: Vec<String>,
    server: Option<String>,
    state: Vec<Option<Option<FoundRelay>>>,
}

impl RelaySearch {
    pub fn new(relays: &[String], server: Option<&str>, known: &KnownRelays, backoff: &RelayBackoff) -> RelaySearch {
        let mut list: Vec<String> =
            dedupe(relays).into_iter().filter(|r| known.get(r) != Some(&false) && !backoff.blocked(r)).collect();
        list.truncate(MAX_RELAY_CANDIDATES);
        let state = vec![None; list.len()];
        RelaySearch { list, server: server.map(str::to_string), state }
    }

    pub fn relays(&self) -> &[String] {
        &self.list
    }

    /// Relay `index`'s body, or `None` for a failed request.
    pub fn answer(&mut self, index: usize, body: Option<&str>, known: &mut KnownRelays, backoff: &mut RelayBackoff) {
        let Some(relay) = self.list.get(index).cloned() else { return };
        if self.state[index].is_some() {
            return;
        }
        let hit = match body {
            None => {
                backoff.fail(&relay);
                None
            }
            Some(body) => {
                let a = RoomsAnswer::read(body);
                known.insert(relay.clone(), a.is_some());
                let hit = a.map(|a| FoundRelay { relay: relay.clone(), friends: a.friends, server: a.server });
                let useful = hit.as_ref().is_some_and(|h| self.server.is_none() || (h.friends && h.server == self.server));
                if useful {
                    backoff.ok(&relay);
                } else {
                    backoff.fail(&relay);
                }
                hit.filter(|_| useful)
            }
        };
        self.state[index] = Some(hit);
    }

    pub fn outcome(&self) -> Option<Option<FoundRelay>> {
        for s in &self.state {
            match s {
                None => return None,
                Some(Some(hit)) => return Some(Some(hit.clone())),
                Some(None) => {}
            }
        }
        Some(None)
    }
}

// ---- Grant: the crypto the web defines; the ticket protocol around it is being redesigned. ----


/// Where the session stands, handed to the friend with the secret.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GrantState {
    #[serde(serialize_with = "js_number")]
    pub time: f64,
    pub playing: bool,
    pub c: u64,
    pub by: String,
}

/// What the host hands an admitted friend, sealed for them alone.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GrantPayload {
    pub secret: String,
    pub server: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<GrantState>,
}

const GRANT_SALT: &[u8] = b"multiplex-together-grant-v1";

fn grant_key(mine: &EcdhSecret, peer_pub: &str, info: &str) -> Option<[u8; 32]> {
    let shared = mine.shared(&decode_url(peer_pub)?)?;
    Some(hkdf_sha256(GRANT_SALT, &shared, info.as_bytes()))
}

/// The web's `sealGrant` with its two random draws handed in: the host's ephemeral secret
/// (`ephemeral`, `None` back when it is not a valid scalar: draw again) and the IV. Returns the
/// host's public key and the box, both base64url.
pub fn seal_grant(request_pub: &str, payload: &GrantPayload, ephemeral: &[u8; 32], iv: &[u8; 12]) -> Option<(String, String)> {
    let mine = EcdhSecret::from_bytes(ephemeral)?;
    let my_pub = encode_url(&mine.public_raw());
    let key = grant_key(&mine, request_pub, &format!("{request_pub}|{my_pub}"))?;
    let json = serde_json::to_string(payload).ok()?;
    Some((my_pub, encode_url(&crypto::seal(&key, iv, json.as_bytes()))))
}

/// The web's `openGrant`: `None` when the box is not for this request, damaged, or does not carry
/// an eight-sign secret and a server.
pub fn open_grant(request_secret: &[u8; 32], request_pub: &str, grant_pub: &str, sealed: &str) -> Option<GrantPayload> {
    let mine = EcdhSecret::from_bytes(request_secret)?;
    let key = grant_key(&mine, grant_pub, &format!("{request_pub}|{grant_pub}"))?;
    let plain = crypto::open(&key, &decode_url(sealed)?)?;
    let v: Value = serde_json::from_slice(&plain).ok()?;
    let secret = v.get("secret")?.as_str()?.to_string();
    let server = v.get("server")?.as_str()?.to_string();
    if clean(&secret).len() != INVITE_LENGTH || server.is_empty() {
        return None;
    }
    let state = v.get("state").and_then(Value::as_object).map(|s| GrantState {
        time: super::wire::num(s.get("time")),
        playing: super::wire::truthy(s.get("playing")),
        c: s.get("c").and_then(Value::as_f64).filter(|c| c.is_finite() && *c >= 0.0).map_or(0, |c| c as u64),
        by: s.get("by").and_then(Value::as_str).unwrap_or("").to_string(),
    });
    Some(GrantPayload { secret, server, state })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|x| x.to_string()).collect()
    }

    /// Ported from the web's `togetherfriends.test.ts`.
    #[test]
    fn the_servers_token_never_the_accounts() {
        assert_eq!(server_credential(Some("srv-token"), Some("account-token")).as_deref(), Some("srv-token"));
        assert_eq!(server_credential(Some("account-token"), Some("account-token")), None);
        assert_eq!(server_credential(Some(""), Some("account-token")), None);
    }

    fn run(search: &mut RelaySearch, answers: &[Option<&str>], known: &mut KnownRelays, backoff: &mut RelayBackoff) -> Option<FoundRelay> {
        for (i, a) in answers.iter().enumerate() {
            search.answer(i, *a, known, backoff);
        }
        search.outcome().expect("all answered")
    }

    #[test]
    fn best_ranked_relay_and_whether_it_lists() {
        let mut known = KnownRelays::new();
        let mut backoff = RelayBackoff::default();
        let mut f = RelaySearch::new(&s(&["wss://a.exemple.fr/multiplex-relay", "wss://a.exemple.fr", "wss://b.exemple.fr"]), None, &known, &backoff);
        let got = run(&mut f, &[None, Some(r#"{"multiplex":"relay","found":[],"friends":true}"#), Some(r#"{"multiplex":"relay","found":[]}"#)], &mut known, &mut backoff);
        assert_eq!(got, Some(FoundRelay { relay: "wss://a.exemple.fr".into(), friends: true, server: None }));
        assert_eq!(known.get("wss://a.exemple.fr/multiplex-relay"), None);
        let mut f = RelaySearch::new(&s(&["wss://nope.exemple.fr"]), None, &known, &backoff);
        assert_eq!(run(&mut f, &[Some(r#"{"hello":1}"#)], &mut known, &mut backoff), None);
    }

    #[test]
    fn a_relay_of_another_server_or_that_does_not_list_is_set_aside() {
        let mut known = KnownRelays::new();
        let mut backoff = RelayBackoff::default();
        let answers = [
            Some(r#"{"multiplex":"relay","found":[],"friends":true,"server":"autre-serveur"}"#),
            Some(r#"{"multiplex":"relay","found":[],"friends":false}"#),
            Some(r#"{"multiplex":"relay","found":[],"friends":true,"server":"srv-1"}"#),
        ];
        let mut f = RelaySearch::new(&s(&["wss://a.exemple.fr", "wss://b.exemple.fr", "wss://c.exemple.fr"]), Some("srv-1"), &known, &backoff);
        let got = run(&mut f, &answers, &mut known, &mut backoff);
        assert_eq!(got, Some(FoundRelay { relay: "wss://c.exemple.fr".into(), friends: true, server: Some("srv-1".into()) }));
    }

    #[test]
    fn a_failed_relay_waits_out_a_growing_delay() {
        let mut known = KnownRelays::new();
        let mut backoff = RelayBackoff::new(60_000, 3_600_000);
        let x = s(&["wss://x.exemple.fr"]);
        let mut f = RelaySearch::new(&x, None, &known, &backoff);
        assert_eq!(run(&mut f, &[None], &mut known, &mut backoff), None);
        assert!(RelaySearch::new(&x, None, &known, &backoff).relays().is_empty());
        backoff.advance(60_001);
        let mut f = RelaySearch::new(&x, None, &known, &backoff);
        assert_eq!(f.relays().len(), 1);
        assert_eq!(run(&mut f, &[None], &mut known, &mut backoff), None);
        backoff.advance(60_001); // the delay doubled: not yet
        assert!(RelaySearch::new(&x, None, &known, &backoff).relays().is_empty());
        backoff.advance(60_001);
        let mut f = RelaySearch::new(&x, Some("s"), &known, &backoff);
        let got = run(&mut f, &[Some(r#"{"multiplex":"relay","found":[],"friends":true,"server":"s"}"#)], &mut known, &mut backoff);
        assert_eq!(got.map(|f| f.relay), Some("wss://x.exemple.fr".into()));
        assert!(!backoff.blocked("wss://x.exemple.fr"));
    }

    /// The web's « remise de la clé » test: only the requester opens it, and a damaged box fails.
    #[test]
    fn only_the_requester_opens_a_grant() {
        let req = EcdhSecret::from_bytes(&[5; 32]).unwrap();
        let req_pub = encode_url(&req.public_raw());
        let payload = GrantPayload {
            secret: "K7Q29XFD".into(),
            server: "server-1".into(),
            state: Some(GrantState { time: 61.0, playing: true, c: 3, by: "h".into() }),
        };
        let (grant_pub, sealed) = seal_grant(&req_pub, &payload, &[9; 32], &[1; 12]).unwrap();
        assert_eq!(open_grant(&[5; 32], &req_pub, &grant_pub, &sealed).unwrap(), payload);
        let other = EcdhSecret::from_bytes(&[6; 32]).unwrap();
        assert_eq!(open_grant(&[6; 32], &encode_url(&other.public_raw()), &grant_pub, &sealed), None);
        let mut bad = sealed.clone();
        bad.replace_range(bad.len() - 4.., if sealed.ends_with("AAAA") { "BBBB" } else { "AAAA" });
        assert_eq!(open_grant(&[5; 32], &req_pub, &grant_pub, &bad), None);
        assert_eq!(seal_grant(&req_pub, &payload, &[0; 32], &[1; 12]), None);
    }
}
