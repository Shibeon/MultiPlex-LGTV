//! The pure pieces of "sessions visible to friends", as the web defines them since its friends-only
//! protocol (MultiPlex 82328b6, af52bea, 08537b6; `packages/core/src/together.ts`, "Séances visibles
//! par les amis", and `apps/relay/README.md`).
//!
//! A relay that knows its Plex server (`PLEX_URL`) and its owner's plex.tv account (`PLEX_TOKEN`)
//! lists a session only to the Plex friends of its host: the host's announce carries the plex.tv
//! account ids of its friends ([`friend_allow_list`], read by the host itself from plex.tv
//! `/api/users/` with its own account token: [`parse_plex_friends`]), re-sent when it changes or is
//! re-read ([`friends_for_announce`]). Everyone proves who they are with the token proper to that
//! server ([`server_credential`], or a delegation token the server hands its owner), and the
//! relay shows the host's name as plex.tv names it, never the one announced. The plex.tv account
//! token never goes to a relay. A relay counts only when its `/rooms` says `friends` AND
//! `friendsOnly`, and its Plex server is the one expected ([`RelaySearch`]), which is known BEFORE
//! any token leaves; the `/friends` answer is checked again ([`FriendsAnswer::lists_for`]).
//!
//! Also here: [`RelayBackoff`] (driven by `dt`), the grant crypto (`sealGrant`/`openGrant`: ECDH
//! P-256, HKDF, AES-GCM, byte for byte the web's, `interop.json`, `grants`) and the relay's control
//! messages ([`Control`], [`announce_text`]).

use std::collections::BTreeMap;

use plx_base::b64::{decode_url, encode_url};
use plx_base::crypto::{self, EcdhSecret, Zeroize};
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

/// At most this many friends in an announce (the relay refuses a longer list).
pub const MAX_FRIEND_ALLOW: usize = 1000;
/// The host's friend list is read from plex.tv at most this often (the web's `FRIENDS_TTL_MS`).
pub const FRIENDS_TTL_US: u64 = 120_000_000;

/// A Plex friend, as plex.tv lists it (`GET https://plex.tv/api/users/`, `MyPlexUser`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlexFriend {
    /// The plex.tv account number (attribute `id`): the one the relay finds among the shares.
    pub id: String,
    pub name: String,
    pub username: Option<String>,
    pub thumb: Option<String>,
}

fn unxml(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut rest = v;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let decoded = tail.find(';').filter(|&end| end <= 12).and_then(|end| {
            let e = &tail[1..end];
            let ch = if let Some(hex) = e.strip_prefix("#x").or_else(|| e.strip_prefix("#X")) {
                u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
            } else if let Some(dec) = e.strip_prefix('#') {
                dec.parse::<u32>().ok().and_then(char::from_u32)
            } else {
                match e.to_ascii_lowercase().as_str() {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    _ => None,
                }
            };
            ch.map(|c| (c, end))
        });
        match decoded {
            Some((c, end)) => {
                out.push(c);
                rest = &tail[end + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The attributes of every `<tag …>` element of a plex.tv XML answer (the web's `xmlElements`):
/// only double-quoted attributes count.
pub fn xml_elements(xml: &str, tag: &str) -> Vec<BTreeMap<String, String>> {
    let mut out = Vec::new();
    let open = format!("<{tag}");
    let mut rest = xml;
    while let Some(at) = rest.find(&open) {
        let after = &rest[at + open.len()..];
        rest = after;
        // `<tag` followed by whitespace, `/` or `>`: not `<Tagged`.
        match after.chars().next() {
            Some(c) if c.is_whitespace() || c == '/' || c == '>' => {}
            _ => continue,
        }
        let Some(end) = after.find('>') else { break };
        let body = &after[..end];
        let mut attrs = BTreeMap::new();
        let mut cur = body;
        while let Some(eq) = cur.find("=\"") {
            let name = cur[..eq].rsplit(|c: char| c.is_whitespace()).next().unwrap_or("");
            let after_q = &cur[eq + 2..];
            let Some(close) = after_q.find('"') else { break };
            let valid = name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '.' | '-'));
            if valid {
                attrs.insert(name.to_string(), unxml(&after_q[..close]));
            }
            cur = &after_q[close + 1..];
        }
        out.push(attrs);
        rest = &after[end..];
    }
    out
}

/// A plex.tv account number (a positive integer of at most 12 digits) as text, else `None` (the
/// web's `accountId`: `"007"` reads `"7"`).
pub fn account_id(v: &str) -> Option<String> {
    if v.is_empty() || v.len() > 12 || !v.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u64 = v.parse().ok()?;
    (n > 0).then(|| n.to_string())
}

/// The friends of an account from plex.tv's `GET /api/users/` XML (elements `User`: `id`, `title`,
/// `username`, `thumb`), the web's `parsePlexFriends`.
pub fn parse_plex_friends(xml: &str) -> Vec<PlexFriend> {
    xml_elements(xml, "User")
        .into_iter()
        .filter_map(|u| {
            let id = account_id(u.get("id").map_or("", String::as_str))?;
            let nonempty = |k: &str| u.get(k).filter(|v| !v.is_empty()).cloned();
            let name = nonempty("title").or_else(|| nonempty("username")).map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| id.clone());
            Some(PlexFriend { id, name, username: nonempty("username"), thumb: nonempty("thumb") })
        })
        .collect()
}

/// The allow-list joined to the announce of a visible session (the web's `friendAllowList`): the
/// host's friends' account numbers, without duplicates, without the host, at most
/// [`MAX_FRIEND_ALLOW`]. Nothing else (no name, no address) leaves.
pub fn friend_allow_list(friends: &[PlexFriend], self_id: Option<&str>) -> Vec<String> {
    let me = self_id.and_then(account_id);
    let mut out: Vec<String> = Vec::new();
    for f in friends {
        if let Some(id) = account_id(&f.id) {
            if Some(&id) != me.as_ref() && !out.contains(&id) {
                out.push(id);
            }
        }
        if out.len() >= MAX_FRIEND_ALLOW {
            break;
        }
    }
    out
}

/// The friend list handed to the relay during this announce, and the plex.tv read it came from
/// (the web's `FriendsSent`; `at` is the reader's clock in µs).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FriendsSent {
    pub ids: String,
    pub at: u64,
}

/// What to join to a refresh of an announce (the web's `friendsForAnnounce`). `None`: plex.tv did
/// not answer, so the caller releases the announce (never a list that may be stale). Otherwise
/// the list to send (`Some` when it changed OR was re-read since it was last sent: the relay keeps
/// only accounts it already knows, so a friend just shared with enters at the next send), and the
/// new `sent`.
pub fn friends_for_announce(ids: Option<&[String]>, sent: &FriendsSent, read_at: u64) -> Option<(Option<Vec<String>>, FriendsSent)> {
    let ids = ids?;
    let key = ids.join(",");
    if key == sent.ids && read_at <= sent.at {
        return Some((None, sent.clone()));
    }
    Some((Some(ids.to_vec()), FriendsSent { ids: key, at: read_at }))
}

/// A visible session as a relay lists it to an allowed friend. `name` is the host's plex.tv name as
/// the relay verified it (what a host announces there is ignored).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FriendCard {
    pub name: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guid: Option<String>,
    /// The title's rating key on the relay's server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(serialize_with = "js_number")]
    pub time: f64,
    pub playing: bool,
}

impl FriendCard {
    fn read(v: &Value) -> Option<FriendCard> {
        let text = |k: &str| v.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
        Some(FriendCard {
            name: text("name")?,
            title: text("title")?,
            sub: text("sub"),
            kind: text("type"),
            guid: text("guid"),
            key: text("key"),
            time: v.get("time").and_then(Value::as_f64).filter(|t| t.is_finite() && *t >= 0.0).unwrap_or(0.0),
            playing: v.get("playing").and_then(Value::as_bool) == Some(true),
        })
    }

    /// The web's `cardSig`: a change of these re-announces.
    pub fn sig(&self) -> String {
        let o = |x: &Option<String>| x.clone().unwrap_or_else(|| "".into());
        format!("{}|{}|{}|{}|{}", self.title, o(&self.sub), o(&self.key), o(&self.guid), self.playing)
    }
}

/// One session of a `/friends` answer.
#[derive(Clone, Debug, PartialEq)]
pub struct ListedSession {
    pub room: String,
    pub card: FriendCard,
    pub count: u32,
    /// Milliseconds since the host last announced.
    pub age_ms: u64,
}

/// A relay's answer to `POST /friends` (the web's `FriendsAnswer`).
#[derive(Clone, PartialEq)]
pub struct FriendsAnswer {
    /// The Plex server (`machineIdentifier`) the relay checked access against.
    pub server: String,
    pub friends_only: bool,
    /// Short-lived entry ticket: announce a session, enter a listed one. A secret: not printed.
    pub ticket: String,
    pub ttl_s: f64,
    pub sessions: Vec<ListedSession>,
}

impl std::fmt::Debug for FriendsAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FriendsAnswer")
            .field("server", &self.server)
            .field("friends_only", &self.friends_only)
            .field("ticket", &"<redacted>")
            .field("ttl_s", &self.ttl_s)
            .field("sessions", &self.sessions)
            .finish()
    }
}

impl FriendsAnswer {
    /// The web's `isFriendsAnswer`, then its sessions (a malformed one is dropped).
    pub fn read(body: &str) -> Option<FriendsAnswer> {
        let v: Value = serde_json::from_str(body).ok()?;
        if v.get("multiplex")?.as_str()? != "relay" {
            return None;
        }
        let server = v.get("server")?.as_str()?.to_string();
        let ticket = v.get("ticket")?.as_str()?.to_string();
        let sessions = v
            .get("sessions")?
            .as_array()?
            .iter()
            .filter_map(|s| {
                Some(ListedSession {
                    room: s.get("room")?.as_str()?.to_string(),
                    card: FriendCard::read(s.get("card")?)?,
                    count: s.get("count").and_then(Value::as_u64).unwrap_or(0).min(u64::from(u32::MAX)) as u32,
                    age_ms: s.get("age").and_then(Value::as_f64).filter(|a| a.is_finite() && *a >= 0.0).map_or(0, |a| a as u64),
                })
            })
            .collect();
        Some(FriendsAnswer {
            server,
            friends_only: v.get("friendsOnly").and_then(Value::as_bool) == Some(true),
            ticket,
            ttl_s: v.get("ttl").and_then(Value::as_f64).filter(|t| t.is_finite() && *t > 0.0).unwrap_or(0.0),
            sessions,
        })
    }

    /// Whether this answer may be used for `server`: the relay's Plex IS that server and it lists
    /// to the host's friends only (the web's `answer.server !== id || !answer.friendsOnly`).
    pub fn lists_for(&self, server: &str) -> bool {
        self.friends_only && self.server == server
    }
}

/// The `POST /friends` body: the server token in the body, never in the address or a header.
pub fn friends_body(token: &str) -> String {
    serde_json::json!({ "token": token }).to_string()
}

#[derive(Serialize)]
struct AnnounceOut<'a> {
    t: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    ticket: Option<&'a str>,
    proof: &'a str,
    card: &'a FriendCard,
    #[serde(skip_serializing_if = "Option::is_none")]
    friends: Option<&'a [String]>,
}

/// The `announce` control message: with the ticket the first time on a connection (and then the
/// friend list is required), without it for a refresh.
pub fn announce_text(ticket: Option<&str>, proof: &str, card: &FriendCard, friends: Option<&[String]>) -> String {
    serde_json::to_string(&AnnounceOut { t: "announce", ticket, proof, card, friends }).unwrap_or_default()
}

/// `{"t":"release"}`, `{"t":"unlist"}`.
pub fn control_text(t: &str) -> String {
    serde_json::json!({ "t": t }).to_string()
}

/// `{"t":"key-grant","peer":…,"pub":…,"box":…}`: the host's sealed grant for one requester.
pub fn key_grant_text(peer: &str, grant_pub: &str, sealed: &str) -> String {
    #[derive(Serialize)]
    struct Out<'a> {
        t: &'static str,
        peer: &'a str,
        #[serde(rename = "pub")]
        public: &'a str,
        #[serde(rename = "box")]
        sealed: &'a str,
    }
    serde_json::to_string(&Out { t: "key-grant", peer, public: grant_pub, sealed }).unwrap_or_default()
}

/// `{"t":"key-request","ticket":…,"pub":…}`: a friend asks the host for the key, the ticket in this
/// message (never in the address, which proxies log).
pub fn key_request_text(ticket: &str, request_pub: &str) -> String {
    #[derive(Serialize)]
    struct Out<'a> {
        t: &'static str,
        ticket: &'a str,
        #[serde(rename = "pub")]
        public: &'a str,
    }
    serde_json::to_string(&Out { t: "key-request", ticket, public: request_pub }).unwrap_or_default()
}

/// The error codes after which the relay does not list the session (the web's `onControl`).
pub const UNLISTED_ERRORS: [&str; 7] = ["ticket", "unavailable", "owner", "private", "proof", "card", "friends"];

/// A text message of the relay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Control {
    Announced,
    Error(String),
    /// A friend asks the host for the key: its socket id on the relay and its public key.
    KeyRequest { peer: String, public: String },
    /// The host's answer, for a requester.
    KeyGrant { public: String, sealed: String },
}

impl Control {
    pub fn read(text: &str) -> Option<Control> {
        let v: Value = serde_json::from_str(text).ok()?;
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        match v.get("t")?.as_str()? {
            "announced" => Some(Control::Announced),
            "error" => Some(Control::Error(s("error").unwrap_or_default())),
            "key-request" => Some(Control::KeyRequest { peer: s("peer")?, public: s("pub")? }),
            "key-grant" => Some(Control::KeyGrant { public: s("pub").filter(|p| !p.is_empty())?, sealed: s("box").filter(|b| !b.is_empty())? }),
            _ => None,
        }
    }
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
    let mut shared = mine.shared(&decode_url(peer_pub)?)?;
    let key = hkdf_sha256(GRANT_SALT, &shared, info.as_bytes());
    shared.zeroize();
    Some(key)
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
        let got = run(&mut f, &[None, Some(r#"{"multiplex":"relay","found":[],"friends":true,"friendsOnly":true}"#), Some(r#"{"multiplex":"relay","found":[]}"#)], &mut known, &mut backoff);
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
            Some(r#"{"multiplex":"relay","found":[],"friends":true,"friendsOnly":true,"server":"autre-serveur"}"#),
            Some(r#"{"multiplex":"relay","found":[],"friends":false}"#),
            Some(r#"{"multiplex":"relay","found":[],"friends":true,"friendsOnly":true,"server":"srv-1"}"#),
        ];
        let mut f = RelaySearch::new(&s(&["wss://a.exemple.fr", "wss://b.exemple.fr", "wss://c.exemple.fr"]), Some("srv-1"), &known, &backoff);
        let got = run(&mut f, &answers, &mut known, &mut backoff);
        assert_eq!(got, Some(FoundRelay { relay: "wss://c.exemple.fr".into(), friends: true, server: Some("srv-1".into()) }));
    }

    /// A relay from before the friends-only protocol listed sessions to every user of the server:
    /// `friends` alone is not enough, it must also say `friendsOnly` (the web's findRelay, 08537b6).
    #[test]
    fn a_relay_that_lists_to_every_user_does_not_count_as_listing_friends() {
        let mut known = KnownRelays::new();
        let mut backoff = RelayBackoff::default();
        let old = Some(r#"{"multiplex":"relay","found":[],"friends":true,"server":"srv-1"}"#);
        let odd = Some(r#"{"multiplex":"relay","found":[],"friends":true,"friendsOnly":"yes","server":"srv-1"}"#);
        let mut f = RelaySearch::new(&s(&["wss://a.exemple.fr", "wss://b.exemple.fr"]), Some("srv-1"), &known, &backoff);
        assert_eq!(run(&mut f, &[old, odd], &mut known, &mut backoff), None);
        let mut f = RelaySearch::new(&s(&["wss://c.exemple.fr"]), None, &known, &backoff);
        let got = run(&mut f, &[old], &mut known, &mut backoff).unwrap();
        assert!(!got.friends);
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
        let got = run(&mut f, &[Some(r#"{"multiplex":"relay","found":[],"friends":true,"friendsOnly":true,"server":"s"}"#)], &mut known, &mut backoff);
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

    const USERS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<MediaContainer friendlyName="myPlex" identifier="com.plexapp.plugins.myplex" size="4">
  <User id="2001" title="Camille" username="camille" email="camille@exemple.fr" thumb="https://plex.tv/users/a/avatar" home="0">
    <Server id="7" serverId="9" machineIdentifier="srv-1" name="nas" lastSeenAt="1" numLibraries="2" allLibraries="0" owned="1" pending="0"/>
  </User>
  <User id="2004" title="L&#233;a &amp; co" username="" thumb="" home="1"/>
  <User id="abc" title="Illisible"/>
  <User id="2001" title="Camille (doublon)"/>
</MediaContainer>"#;

    fn friend(id: &str) -> PlexFriend {
        PlexFriend { id: id.into(), name: id.into(), username: None, thumb: None }
    }

    /// Ported from the web's `togetherfriends.test.ts`, « liste d'amis jointe à l'annonce ».
    #[test]
    fn plex_tv_friends_read_as_the_web_reads_them() {
        let got = parse_plex_friends(USERS);
        assert_eq!(
            got,
            vec![
                PlexFriend { id: "2001".into(), name: "Camille".into(), username: Some("camille".into()), thumb: Some("https://plex.tv/users/a/avatar".into()) },
                PlexFriend { id: "2004".into(), name: "Léa & co".into(), username: None, thumb: None },
                PlexFriend { id: "2001".into(), name: "Camille (doublon)".into(), username: None, thumb: None },
            ]
        );
        assert!(parse_plex_friends(r#"<MediaContainer size="0"/>"#).is_empty());
        assert!(parse_plex_friends("pas du XML").is_empty());
        // `<Users>` is not `<User>`.
        assert!(parse_plex_friends(r#"<Users id="5"/>"#).is_empty());
    }

    #[test]
    fn only_account_numbers_without_duplicates_or_self_and_bounded() {
        let f = parse_plex_friends(USERS);
        assert_eq!(friend_allow_list(&f, None), s(&["2001", "2004"]));
        assert_eq!(friend_allow_list(&f, Some("2004")), s(&["2001"]));
        assert_eq!(friend_allow_list(&f, Some("2001")), s(&["2004"]));
        let odd: Vec<PlexFriend> = ["0", "-3", "12.5", "0042"].iter().map(|i| friend(i)).collect();
        assert_eq!(friend_allow_list(&odd, None), s(&["42"]));
        assert!(friend_allow_list(&[], None).is_empty());
        let many: Vec<PlexFriend> = (1..=MAX_FRIEND_ALLOW + 50).map(|i| friend(&i.to_string())).collect();
        assert_eq!(friend_allow_list(&many, None).len(), MAX_FRIEND_ALLOW);
        // A friend removed on plex.tv is gone from the next list.
        let after = parse_plex_friends(&USERS.replace(r#"<User id="2004" title="L&#233;a &amp; co" username="" thumb="" home="1"/>"#, ""));
        assert_eq!(friend_allow_list(&after, None), s(&["2001"]));
    }

    #[test]
    fn the_list_is_resent_when_it_changes_or_was_re_read_and_never_kept_stale() {
        let sent = FriendsSent { ids: "2001".into(), at: 10 };
        assert_eq!(friends_for_announce(None, &FriendsSent { ids: "2001,2004".into(), at: 5 }, 5), None);
        assert_eq!(friends_for_announce(Some(&s(&["2001"])), &sent, 10), Some((None, sent.clone())));
        assert_eq!(friends_for_announce(Some(&s(&["2001"])), &sent, 20), Some((Some(s(&["2001"])), FriendsSent { ids: "2001".into(), at: 20 })));
        assert_eq!(friends_for_announce(Some(&[]), &sent, 10), Some((Some(vec![]), FriendsSent { ids: String::new(), at: 10 })));
    }

    #[test]
    fn a_friends_answer_reads_as_the_web_checks_it() {
        let a = FriendsAnswer::read(r#"{"multiplex":"relay","server":"abc","ticket":"t","ttl":120,"sessions":[]}"#).unwrap();
        assert_eq!((a.server.as_str(), a.friends_only, a.ttl_s), ("abc", false, 120.0));
        assert!(!a.lists_for("abc"), "an answer without friendsOnly is not used");
        assert!(FriendsAnswer::read(r#"{"multiplex":"relay","found":[]}"#).is_none());
        let body = r#"{"multiplex":"relay","server":"srv-1","friendsOnly":true,"ticket":"T1","ttl":300,"sessions":[
            {"room":"FFFFFFFFFFFFFFFFFFFFFF","card":{"name":"Dominique (dom)","title":"Film","time":12.5,"playing":true,"key":"42"},"count":2,"age":1500},
            {"room":"X","card":{"title":"sans nom"},"count":1,"age":0}]}"#;
        let a = FriendsAnswer::read(body).unwrap();
        assert!(a.lists_for("srv-1") && !a.lists_for("srv-2"));
        assert_eq!(a.sessions.len(), 1, "a card without the verified name is dropped");
        let l = &a.sessions[0];
        assert_eq!((l.count, l.age_ms, l.card.time, l.card.playing, l.card.key.as_deref()), (2, 1500, 12.5, true, Some("42")));
        assert!(!format!("{a:?}").contains("T1"), "the ticket is not printed");
    }

    #[test]
    fn control_messages_are_written_and_read_as_the_relay_speaks() {
        let card = FriendCard { name: "Moi".into(), title: "Film".into(), sub: None, kind: Some("movie".into()), guid: None, key: Some("7".into()), time: 61.0, playing: false };
        assert_eq!(
            announce_text(Some("TK"), "proofproofproofproof", &card, Some(&s(&["2001"]))),
            r#"{"t":"announce","ticket":"TK","proof":"proofproofproofproof","card":{"name":"Moi","title":"Film","type":"movie","key":"7","time":61,"playing":false},"friends":["2001"]}"#
        );
        assert_eq!(announce_text(None, "p", &card, None), r#"{"t":"announce","proof":"p","card":{"name":"Moi","title":"Film","type":"movie","key":"7","time":61,"playing":false}}"#);
        assert_eq!(control_text("release"), r#"{"t":"release"}"#);
        assert_eq!(key_grant_text("sid", "PUB", "BOX"), r#"{"t":"key-grant","peer":"sid","pub":"PUB","box":"BOX"}"#);
        assert_eq!(key_request_text("TK", "PUB"), r#"{"t":"key-request","ticket":"TK","pub":"PUB"}"#);
        assert_eq!(friends_body("tok"), r#"{"token":"tok"}"#);
        assert_eq!(Control::read(r#"{"t":"announced"}"#), Some(Control::Announced));
        assert_eq!(Control::read(r#"{"t":"error","error":"owner"}"#), Some(Control::Error("owner".into())));
        assert_eq!(Control::read(r#"{"t":"key-request","peer":"p","pub":"k"}"#), Some(Control::KeyRequest { peer: "p".into(), public: "k".into() }));
        assert_eq!(Control::read(r#"{"t":"key-grant","pub":"k"}"#), None);
        assert_eq!(Control::read("nope"), None);
        assert_eq!(card.sig(), "Film||7||false");
    }
}
