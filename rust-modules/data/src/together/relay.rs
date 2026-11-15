//! Finding the relay of a short-code session (the web's `relayCandidates`, `roomsUrl`,
//! `isRoomsAnswer`, `findRoom`). No request leaves from here: [`RoomSearch`] is the web's `findRoom`
//! as a fold, which says what to ask and takes each answer as an event, and resolves exactly as the
//! promise does (the best-ranked relay that knows a room, without waiting for worse-ranked ones).

use std::collections::BTreeMap;

use serde_json::Value;

use super::invite::relay_url;

/// The conventional path of a relay served at the Plex server's own address.
pub const RELAY_PATH: &str = "/multiplex-relay";
/// At most this many relay addresses are tried, and this many rooms (servers) asked of each.
pub const MAX_RELAY_CANDIDATES: usize = 16;
pub const MAX_ROOM_LOOKUP: usize = 16;

/// One published connection of a Plex server (plex.tv's resources).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlexConnection {
    pub uri: String,
    pub local: bool,
    pub relay: bool,
}

/// A Plex server this account sees: its identifier (`clientIdentifier`) and connections.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerInfo {
    pub id: String,
    pub connections: Vec<PlexConnection>,
}

fn is_ip(host: &str) -> bool {
    host.chars().all(|c| c.is_ascii_digit() || c == '.') || host.contains(':') || host.starts_with('[')
}

/// `(host, port)` of an `https` URI as the URL parser reads them (lower-cased host, port empty when
/// it is the default), or `None` for anything else.
fn https_host_port(uri: &str) -> Option<(String, String)> {
    let (scheme, rest) = uri.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.contains('@') || authority.is_empty() {
        return None;
    }
    if authority.starts_with('[') {
        let close = authority.find(']')?;
        let port = authority[close + 1..].strip_prefix(':').unwrap_or("");
        return Some((authority[..=close].to_ascii_lowercase(), port.to_string()));
    }
    let (host, port) = authority.rsplit_once(':').unwrap_or((authority, ""));
    if !port.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let n: u32 = if port.is_empty() { 443 } else { port.trim_start_matches('0').parse().unwrap_or(0) };
    if n > 65535 {
        return None;
    }
    Some((host.to_ascii_lowercase(), if n == 443 { String::new() } else { n.to_string() }))
}

/// Where a Plex server's relay may be: on the public names of its connections, under
/// `/multiplex-relay` then at the root, on the connection's port then on 443. Local and Plex-relay
/// connections, IP addresses, `*.plex.direct` and dotless names are skipped (no relay can present a
/// valid certificate there).
pub fn relay_candidates(servers: &[ServerInfo]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for srv in servers {
        for c in &srv.connections {
            if c.local || c.relay {
                continue;
            }
            let Some((host, port)) = https_host_port(&c.uri) else { continue };
            if is_ip(&host) || host.ends_with(".plex.direct") || !host.contains('.') {
                continue;
            }
            let port = if port.is_empty() { String::new() } else { format!(":{port}") };
            let mut forms = vec![format!("wss://{host}{port}{RELAY_PATH}"), format!("wss://{host}{port}")];
            if !port.is_empty() {
                forms.push(format!("wss://{host}{RELAY_PATH}"));
                forms.push(format!("wss://{host}"));
            }
            for f in forms {
                if !out.contains(&f) {
                    out.push(f);
                }
            }
        }
    }
    out.truncate(MAX_RELAY_CANDIDATES);
    out
}

/// True when a friend who sees `servers` finds the relay `relay` on their own.
pub fn relay_discoverable(relay: &str, servers: &[ServerInfo]) -> bool {
    let r = relay_url(relay);
    !r.is_empty() && relay_candidates(servers).contains(&r)
}

/// `encodeURIComponent`.
pub fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `http(s)` form of a `ws(s)` relay address (the web's `relay.replace(/^ws/, 'http')`).
pub fn http_base(relay: &str) -> String {
    match relay.strip_prefix("ws") {
        Some(rest) => format!("http{rest}"),
        None => relay.to_string(),
    }
}

/// `GET …/rooms?ids=a,b`: which of these rooms the relay holds.
pub fn rooms_url(relay: &str, rooms: &[String]) -> String {
    let ids: Vec<String> = rooms.iter().map(|r| encode_uri_component(r)).collect();
    format!("{}/rooms?ids={}", http_base(relay), ids.join(","))
}

/// A MultiPlex relay's answer to `/rooms`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoomsAnswer {
    pub found: Vec<String>,
    /// The relay lists friends' sessions, and of which Plex server (`server`).
    pub friends: bool,
    pub server: Option<String>,
}

impl RoomsAnswer {
    /// The web's `isRoomsAnswer`: `{multiplex: 'relay', found: [...]}`; anything else is not a relay.
    pub fn read(body: &str) -> Option<RoomsAnswer> {
        let v: Value = serde_json::from_str(body).ok()?;
        if v.get("multiplex")?.as_str()? != "relay" {
            return None;
        }
        let found = v.get("found")?.as_array()?.iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
        Some(RoomsAnswer {
            found,
            friends: v.get("friends").and_then(Value::as_bool) == Some(true),
            server: v.get("server").and_then(Value::as_str).map(str::to_string),
        })
    }
}

/// Addresses known to be relays (`true`) or known not to be (`false`, never asked again) during
/// this run: the web's `knownRelays`.
pub type KnownRelays = BTreeMap<String, bool>;

/// `[...new Set(xs)]`, first occurrence kept.
pub(crate) fn dedupe(xs: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for x in xs {
        if !out.contains(x) {
            out.push(x.clone());
        }
    }
    out
}

/// The web's `findRoom` as a fold. Build it, ask every relay of [`relays`](RoomSearch::relays) for
/// [`rooms`](RoomSearch::rooms) at once, hand each answer (or a failure) to
/// [`answer`](RoomSearch::answer), and read [`outcome`](RoomSearch::outcome).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoomSearch {
    list: Vec<String>,
    ids: Vec<String>,
    /// Per relay: `None` still asked, `Some(None)` nothing there, `Some(Some(room))` a hit.
    state: Vec<Option<Option<String>>>,
}

impl RoomSearch {
    pub fn new(relays: &[String], rooms: &[String], known: &KnownRelays) -> RoomSearch {
        let mut list: Vec<String> = dedupe(relays).into_iter().filter(|r| known.get(r) != Some(&false)).collect();
        list.truncate(MAX_RELAY_CANDIDATES);
        let mut ids = dedupe(rooms);
        ids.truncate(MAX_ROOM_LOOKUP);
        let state = vec![None; list.len()];
        RoomSearch { list, ids, state }
    }

    /// The relays to ask, in preference order (empty: nothing to try, the outcome is already known).
    pub fn relays(&self) -> &[String] {
        if self.ids.is_empty() { &[] } else { &self.list }
    }

    pub fn rooms(&self) -> &[String] {
        &self.ids
    }

    /// The answer of relay `index`: its body, or `None` when the request failed. Records in `known`
    /// whether a body was a relay's, as the web does (a failure records nothing).
    pub fn answer(&mut self, index: usize, body: Option<&str>, known: &mut KnownRelays) {
        let Some(slot) = self.state.get_mut(index) else { return };
        if slot.is_some() {
            return;
        }
        *slot = Some(match body {
            None => None,
            Some(body) => {
                let a = RoomsAnswer::read(body);
                known.insert(self.list[index].clone(), a.is_some());
                a.and_then(|a| self.ids.iter().find(|id| a.found.contains(id)).cloned())
            }
        });
    }

    /// `None` while a better-ranked relay has not answered; then the first `(relay, room)` hit, or
    /// `Some(None)` when none has the room.
    pub fn outcome(&self) -> Option<Option<(String, String)>> {
        if self.ids.is_empty() || self.list.is_empty() {
            return Some(None);
        }
        for (i, s) in self.state.iter().enumerate() {
            match s {
                None => return None,
                Some(Some(room)) => return Some(Some((self.list[i].clone(), room.clone()))),
                Some(None) => {}
            }
        }
        Some(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn(uri: &str, local: bool, relay: bool) -> PlexConnection {
        PlexConnection { uri: uri.into(), local, relay }
    }

    fn server() -> ServerInfo {
        ServerInfo {
            id: "s".into(),
            connections: vec![
                conn("https://192-168-1-10.abc.plex.direct:32400", true, false),
                conn("https://203-0-113-9.abc.plex.direct:32400", false, false),
                conn("https://203-0-113-9.abc.plex.direct:8443", false, true),
                conn("https://plex.exemple.fr:32400", false, false),
                conn("http://relais.exemple.fr", false, false),
                conn("https://203.0.113.9:443", false, false),
            ],
        }
    }

    /// The web's « recherche du relais par le serveur Plex ».
    #[test]
    fn candidates_are_public_https_names_path_then_root() {
        assert_eq!(
            relay_candidates(&[server()]),
            ["wss://plex.exemple.fr:32400/multiplex-relay", "wss://plex.exemple.fr:32400", "wss://plex.exemple.fr/multiplex-relay", "wss://plex.exemple.fr"]
        );
        let one = ServerInfo { id: "x".into(), connections: vec![conn("https://relais.exemple.fr", false, false)] };
        assert_eq!(relay_candidates(&[one]), ["wss://relais.exemple.fr/multiplex-relay", "wss://relais.exemple.fr"]);
    }

    #[test]
    fn candidates_are_bounded_and_unique() {
        let many: Vec<ServerInfo> = (0..30)
            .map(|i| ServerInfo {
                id: format!("{i}"),
                connections: vec![conn(&format!("https://s{i}.exemple.fr"), false, false), conn(&format!("https://s{i}.exemple.fr"), false, false)],
            })
            .collect();
        let list = relay_candidates(&many);
        assert_eq!(list.len(), 16);
        assert_eq!(dedupe(&list).len(), 16);
    }

    #[test]
    fn the_host_knows_whether_friends_find_its_relay() {
        assert!(relay_discoverable("https://plex.exemple.fr/multiplex-relay/", &[server()]));
        assert!(!relay_discoverable("relais.exemple.fr:1744", &[server()]));
        assert!(!relay_discoverable("", &[server()]));
    }

    fn s(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn best_ranked_relay_that_knows_a_room_without_waiting_for_worse_ones() {
        let mut known = KnownRelays::new();
        let mut f = RoomSearch::new(&s(&["wss://a", "wss://b", "wss://c"]), &s(&["r1", "r2"]), &known);
        assert_eq!(f.relays(), s(&["wss://a", "wss://b", "wss://c"]));
        assert_eq!(f.rooms(), s(&["r1", "r2"]));
        f.answer(1, Some(r#"{"multiplex":"relay","found":["r2"]}"#), &mut known);
        assert_eq!(f.outcome(), None); // a is still on its way
        f.answer(0, None, &mut known); // a: unreachable
        assert_eq!(f.outcome(), Some(Some(("wss://b".into(), "r2".into())))); // c never answers
    }

    #[test]
    fn waits_for_a_better_ranked_relay_still_on_its_way() {
        let mut known = KnownRelays::new();
        let mut f = RoomSearch::new(&s(&["wss://a", "wss://b"]), &s(&["r1"]), &known);
        f.answer(1, Some(r#"{"multiplex":"relay","found":["r1"]}"#), &mut known);
        assert_eq!(f.outcome(), None);
        f.answer(0, Some(r#"{"multiplex":"relay","found":["r1"]}"#), &mut known);
        assert_eq!(f.outcome(), Some(Some(("wss://a".into(), "r1".into()))));
    }

    #[test]
    fn what_is_not_a_relay_is_remembered_and_not_asked_again() {
        let mut known = KnownRelays::new();
        let mut f = RoomSearch::new(&s(&["wss://plex", "wss://relay"]), &s(&["r1"]), &known);
        f.answer(0, Some(r#"{"MediaContainer":{}}"#), &mut known);
        f.answer(1, Some(r#"{"multiplex":"relay","found":[]}"#), &mut known);
        assert_eq!(f.outcome(), Some(None));
        assert_eq!(known.get("wss://plex"), Some(&false));
        assert_eq!(known.get("wss://relay"), Some(&true));
        let again = RoomSearch::new(&s(&["wss://plex", "wss://relay"]), &s(&["r1"]), &known);
        assert_eq!(again.relays(), s(&["wss://relay"]));
    }

    #[test]
    fn nothing_to_try_finds_nothing() {
        let known = KnownRelays::new();
        assert_eq!(RoomSearch::new(&[], &s(&["r1"]), &known).outcome(), Some(None));
        let f = RoomSearch::new(&s(&["wss://a"]), &[], &known);
        assert_eq!(f.outcome(), Some(None));
        assert!(f.relays().is_empty());
    }
}
