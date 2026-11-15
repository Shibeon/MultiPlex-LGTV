#![deny(clippy::wildcard_enum_match_arm)]
//! `TogetherSession`: one Watch Together session, a `State + TogetherEvent -> TogetherFx` machine in
//! the shape of `HudStore` (registered in `ci/machines.ini`). It is the web client's
//! `apps/web/src/together.ts` without its IO: it opens no socket, makes no request, draws no random
//! byte, reads no clock and touches no player. Every one of those leaves as a [`TogetherFx`], and
//! every answer comes back as one addressed [`TogetherEvent`] (a `tag` for a request, a `conn` for a
//! socket). Time is the `dt_us` of [`TogetherEvent::Tick`].
//!
//! **Scope.** Invitation-code sessions (host a new one, join by code, the older v1/v2 codes), the
//! presence and Lamport rule of `sync`, and host take-over and step-down. A session is never
//! announced to the relay (it is private, `vis: false` in the host's ticks): the friends listing,
//! `announce` and the grant hand-over are being redesigned and are not wired (`friends` holds the
//! pure pieces).
//!
//! | event | in phase | effects, next phase |
//! |---|---|---|
//! | `Host` | any (leaves the current one) | `Derive` (PBKDF2 of a new secret, salted by the server); `Deriving` |
//! | `Join` | any (leaves the current one) | v1/v2: `OpenSocket`, `Live`. v3: `Derive` per visible server, then `HttpRooms` per candidate relay; `Live` on the best-ranked hit, else `Notice::JoinFailed` |
//! | `Derived`, `RoomsAnswered` | the plan that asked (by `tag`) | as above |
//! | `SocketOpened` / `SocketClosed` | `Live`, its `conn` | send a `want` tick / reconnect after 1, 2, 4 … 15 s |
//! | `SocketBinary` → `Opened` | `Live`, its `conn` | `Open`, then the message: peers, `sync`, host step-down, `Player` commands, `Notice::Action` |
//! | `Tick` | any | every 3 s a tick; peers silent for 12 s leave; the first participant takes the hand when the host is gone; reconnect timer |
//! | `Attach`, `Player`, `Detach`, `ShareTitle` | `Live` | `sync.poll`, an `intent` for a gesture, a tick |
//! | `Sealed` | the send that asked | `SendBinary` (and `CloseSocket` after the farewell `bye`) |
//! | `Leave` | `Live` | `bye`, then `CloseSocket`; `Idle` |
//!
//! A pair not listed (a stale `tag` or `conn`, a player event outside a session) is
//! [`Handled::No`] and changes nothing. Volume, subtitles, audio and quality have no field in any
//! message (`wire`), so nothing here can send them.

use std::collections::BTreeMap;

use plx_machine::machine::Handled;

use super::invite::{format_invite, invite_text, parse_invite, relay_url, secret_from_bytes, Invite};
use super::keys::{derive_legacy, SessionKeys, INVITE_ITERATIONS};
use super::relay::{dedupe, relay_candidates, rooms_url, KnownRelays, RoomSearch, ServerInfo, MAX_ROOM_LOOKUP};
use super::sync::{host_yields, PlayerCmd, PlayerObs, TogetherSync};
use super::wire::{read_message, Bye, Incoming, Intent, Message, SharedItem, SyncAction, Tick};

/// A presence every 3 s.
pub const TICK_EVERY_US: u64 = 3_000_000;
/// A member silent for 12 s has left.
pub const PEER_TIMEOUT_US: u64 = 12_000_000;
/// A `want` is answered at most once a second.
pub const WANT_REPLY_US: u64 = 1_000_000;
/// Reconnection delay: 1 s, doubled per failure, at most 15 s.
pub const RECONNECT_BASE_US: u64 = 1_000_000;
pub const RECONNECT_CEILING_US: u64 = 15_000_000;
/// The web's per-relay `/rooms` request deadline.
pub const ROOMS_TIMEOUT_MS: u32 = 4_000;

/// Who this device is in sessions: a random id for this run (8 base64url signs in the web), and
/// the account's display name and avatar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Me {
    pub id: String,
    pub name: String,
    pub thumb: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Connecting,
    Open,
}

/// Why joining by code failed (the web's `code`, `relay`, `notfound`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinError {
    /// The text is not an invitation.
    Code,
    /// No relay to ask: none configured, none given, none findable.
    Relay,
    /// No relay knows the session.
    NotFound,
}

/// What the UI is told.
#[derive(Clone, Debug, PartialEq)]
pub enum Notice {
    /// A session is open: the code to share (`full_code` carries the relay, host only).
    Opened { code: String, full_code: Option<String>, host: bool },
    Status(Status),
    /// Another participant played, paused or sought.
    Action { name: String, action: Option<SyncAction> },
    /// This device took the hand, or gave it back.
    HostChanged { host: bool },
    /// The session's title changed (the host moved on).
    Item(SharedItem),
    /// A host could not be started: no relay configured, or no randomness handed in.
    HostFailed,
    JoinFailed(JoinError),
    Left,
}

#[derive(Debug)]
pub enum TogetherEvent {
    /// The frame time.
    Tick { dt_us: u64 },
    /// Open a new session as host. `relay`: the relay setting as typed; `server`: the current Plex
    /// server's identifier (salts the secret); `random`: at least 16 bytes the runner drew.
    Host { me: Me, relay: String, server: Option<String>, item: Option<SharedItem>, random: Vec<u8> },
    /// Join by a pasted code. `relay`: the relay setting; `server` and `servers`: what this account
    /// sees (the secret is salted by the host's server, and the relay is found on servers' names).
    Join { me: Me, code: String, relay: String, server: Option<String>, servers: Vec<ServerInfo> },
    Derived { tag: u32, keys: SessionKeys },
    /// A `/rooms` answer body, or `None` for a failed request.
    RoomsAnswered { tag: u32, body: Option<String> },
    SocketOpened { conn: u32 },
    SocketClosed { conn: u32 },
    SocketBinary { conn: u32, bytes: Vec<u8> },
    Sealed { tag: u32, bytes: Vec<u8> },
    Opened { tag: u32, plain: Option<Vec<u8>> },
    /// The player plays `item` for the session from now on.
    Attach { item: SharedItem, obs: PlayerObs },
    /// An observation of the attached player (every 500 ms).
    Player(PlayerObs),
    Detach,
    /// Host: the session moves to this title.
    ShareTitle(SharedItem),
    Leave,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TogetherFx {
    /// PBKDF2 + HKDF off the frame (`keys::derive_session`), answered `Derived`.
    Derive { tag: u32, secret: String, server: String, iterations: u32 },
    /// `GET url` with a deadline, answered `RoomsAnswered`.
    HttpRooms { tag: u32, url: String, timeout_ms: u32 },
    OpenSocket { conn: u32, url: String },
    CloseSocket { conn: u32 },
    SendBinary { conn: u32, bytes: Vec<u8> },
    /// Draw an IV and `keys::seal_message`, answered `Sealed`.
    Seal { tag: u32, key: [u8; 32], plain: String },
    /// `keys::open_message`, answered `Opened`.
    Open { tag: u32, key: [u8; 32], bytes: Vec<u8> },
    Player(PlayerCmd),
    Notice(Notice),
}

/// A participant as last heard.
#[derive(Clone, Debug, PartialEq)]
pub struct Peer {
    pub id: String,
    pub name: String,
    pub thumb: Option<String>,
    pub host: bool,
    pub playing: bool,
    pub time: f64,
    /// In the player (not on a detail page).
    pub live: bool,
    pub item: Option<SharedItem>,
    seen_us: u64,
}

/// What the UI reads of a live session.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub code: String,
    pub full_code: Option<String>,
    pub host: bool,
    pub status: Status,
    pub item: Option<SharedItem>,
    pub peers: Vec<Peer>,
    /// The session's position now (for starting playback at the right place when joining).
    pub position: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
struct Live {
    me: Me,
    relay: String,
    keys: SessionKeys,
    code: String,
    full_code: Option<String>,
    host: bool,
    /// When this device took the hand (Lamport clock; 0 for the one that opened the session).
    host_claim: u64,
    status: Status,
    item: Option<SharedItem>,
    playing_item: Option<SharedItem>,
    peers: Vec<Peer>,
    sync: TogetherSync,
    conn: u32,
    retry: u32,
    reconnect_in_us: Option<u64>,
    since_tick_us: u64,
    last_want_reply_us: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
struct HostPlan {
    me: Me,
    relay: String,
    secret: String,
    item: Option<SharedItem>,
    tag: u32,
}

#[derive(Clone, Debug, PartialEq)]
struct JoinPlan {
    me: Me,
    code: String,
    ids: Vec<String>,
    relays: Vec<String>,
    /// `(tag, keys)` per server id, in `ids` order.
    derived: Vec<(u32, Option<SessionKeys>)>,
    search: Option<RoomSearch>,
    /// The `/rooms` request tag of each relay of `search`.
    asked: Vec<u32>,
}

// Not boxed: `ci/check-machine-shape.py` proves the state inert through std's plain wrappers only, and
// a `Box` would make every read of the phase opaque. One machine per app, so the size is moot.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
enum Phase {
    Idle,
    Deriving(HostPlan),
    Joining(JoinPlan),
    Live(Live),
}

/// What a `Seal` was for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SealFor {
    Send(u32),
    /// The `bye` of a session left: send it, then close the socket.
    Farewell(u32),
}

/// The parts of the state every phase shares, moved out of the machine for one transition.
struct Desk {
    seals: BTreeMap<u32, SealFor>,
    opens: BTreeMap<u32, u32>,
    known: KnownRelays,
    tags: u32,
    conns: u32,
    now_us: u64,
}

impl Desk {
    fn tag(&mut self) -> u32 {
        self.tags = self.tags.wrapping_add(1);
        self.tags
    }
    fn conn(&mut self) -> u32 {
        self.conns = self.conns.wrapping_add(1);
        self.conns
    }
}

/// The session machine. See the module doc.
pub struct TogetherSession {
    phase: Phase,
    /// Seals in flight, by tag.
    seals: BTreeMap<u32, SealFor>,
    /// Opens in flight: tag -> the socket the frame came on.
    opens: BTreeMap<u32, u32>,
    /// Addresses known to be relays, or not, during this run.
    known: KnownRelays,
    tags: u32,
    conns: u32,
    now_us: u64,
}

impl Default for TogetherSession {
    fn default() -> Self {
        Self::new()
    }
}

impl TogetherSession {
    pub fn new() -> Self {
        TogetherSession { phase: Phase::Idle, seals: BTreeMap::new(), opens: BTreeMap::new(), known: KnownRelays::new(), tags: 0, conns: 0, now_us: 0 }
    }

    /// The one writer: every state change is made here, on values moved out of the machine and
    /// moved back. The table is in the module doc.
    fn transition(&mut self, ev: TogetherEvent, fx: &mut Vec<TogetherFx>) -> Handled {
        let mut desk = Desk {
            seals: std::mem::take(&mut self.seals),
            opens: std::mem::take(&mut self.opens),
            known: std::mem::take(&mut self.known),
            tags: self.tags,
            conns: self.conns,
            now_us: self.now_us,
        };
        let phase = std::mem::replace(&mut self.phase, Phase::Idle);
        let (phase, handled) = reduce(phase, ev, &mut desk, fx);
        self.phase = phase;
        self.seals = desk.seals;
        self.opens = desk.opens;
        self.known = desk.known;
        self.tags = desk.tags;
        self.conns = desk.conns;
        self.now_us = desk.now_us;
        handled
    }

    pub fn step(&mut self, ev: TogetherEvent, fx: &mut Vec<TogetherFx>) -> Handled {
        self.transition(ev, fx)
    }

    /// The live session, if any.
    pub fn view(&self) -> Option<View> {
        match &self.phase {
            Phase::Live(live) => Some(view_of(live)),
            Phase::Idle | Phase::Deriving(_) | Phase::Joining(_) => None,
        }
    }

    /// A host start or a join is under way.
    pub fn busy(&self) -> bool {
        match &self.phase {
            Phase::Deriving(_) | Phase::Joining(_) => true,
            Phase::Idle | Phase::Live(_) => false,
        }
    }
}

fn view_of(live: &Live) -> View {
    View {
        code: live.code.clone(),
        full_code: live.full_code.clone(),
        host: live.host,
        status: live.status,
        item: live.item.clone(),
        peers: live.peers.clone(),
        position: live.sync.position().or_else(|| live.peers.iter().find(|p| p.host).map(|p| p.time)),
    }
}

fn reduce(phase: Phase, ev: TogetherEvent, desk: &mut Desk, fx: &mut Vec<TogetherFx>) -> (Phase, Handled) {
    match ev {
        TogetherEvent::Tick { dt_us } => {
            desk.now_us = desk.now_us.saturating_add(dt_us);
            match phase {
                Phase::Live(mut live) => {
                    on_tick(&mut live, dt_us, desk, fx);
                    (Phase::Live(live), Handled::Yes)
                }
                other @ (Phase::Idle | Phase::Deriving(_) | Phase::Joining(_)) => (other, Handled::Yes),
            }
        }
        TogetherEvent::Host { me, relay, server, item, random } => {
            leave(phase, desk, fx);
            let relay = relay_url(&relay);
            let secret = secret_from_bytes(&random);
            let (false, Some(secret)) = (relay.is_empty(), secret) else {
                fx.push(TogetherFx::Notice(Notice::HostFailed));
                return (Phase::Idle, Handled::Yes);
            };
            let tag = desk.tag();
            fx.push(TogetherFx::Derive { tag, secret: secret.clone(), server: server.unwrap_or_default(), iterations: INVITE_ITERATIONS });
            (Phase::Deriving(HostPlan { me, relay, secret, item, tag }), Handled::Yes)
        }
        TogetherEvent::Join { me, code, relay, server, servers } => {
            leave(phase, desk, fx);
            join(me, &code, &relay, server, &servers, desk, fx)
        }
        TogetherEvent::Derived { tag, keys } => match phase {
            Phase::Deriving(plan) if plan.tag == tag => {
                let code = format_invite(&plan.secret, None);
                let full = format_invite(&plan.secret, Some(&plan.relay));
                (open(plan.me, plan.relay, keys, code, Some(full), true, plan.item, desk, fx), Handled::Yes)
            }
            Phase::Joining(mut plan) => {
                let Some(slot) = plan.derived.iter_mut().find(|(t, k)| *t == tag && k.is_none()) else {
                    return (Phase::Joining(plan), Handled::No);
                };
                slot.1 = Some(keys);
                if plan.derived.iter().any(|(_, k)| k.is_none()) {
                    return (Phase::Joining(plan), Handled::Yes);
                }
                let rooms: Vec<String> = plan.derived.iter().filter_map(|(_, k)| k.as_ref().map(|k| k.room.clone())).collect();
                let search = RoomSearch::new(&plan.relays, &rooms, &desk.known);
                for relay in search.relays() {
                    let tag = desk.tag();
                    plan.asked.push(tag);
                    fx.push(TogetherFx::HttpRooms { tag, url: rooms_url(relay, search.rooms()), timeout_ms: ROOMS_TIMEOUT_MS });
                }
                plan.search = Some(search);
                (settle_join(plan, desk, fx), Handled::Yes)
            }
            other @ (Phase::Idle | Phase::Deriving(_) | Phase::Live(_)) => (other, Handled::No),
        },
        TogetherEvent::RoomsAnswered { tag, body } => match phase {
            Phase::Joining(mut plan) => {
                let Some(index) = plan.asked.iter().position(|t| *t == tag) else {
                    return (Phase::Joining(plan), Handled::No);
                };
                if let Some(search) = plan.search.as_mut() {
                    search.answer(index, body.as_deref(), &mut desk.known);
                }
                (settle_join(plan, desk, fx), Handled::Yes)
            }
            other @ (Phase::Idle | Phase::Deriving(_) | Phase::Live(_)) => (other, Handled::No),
        },
        TogetherEvent::SocketOpened { conn } => match phase {
            Phase::Live(mut live) if live.conn == conn => {
                live.retry = 0;
                live.status = Status::Open;
                fx.push(TogetherFx::Notice(Notice::Status(Status::Open)));
                // Arriving or returning: ask everyone's state now.
                let tick = tick_of(&live, true);
                send(&live, Message::Tick(tick), desk, fx);
                (Phase::Live(live), Handled::Yes)
            }
            other @ (Phase::Idle | Phase::Deriving(_) | Phase::Joining(_) | Phase::Live(_)) => (other, Handled::No),
        },
        TogetherEvent::SocketClosed { conn } => match phase {
            Phase::Live(mut live) if live.conn == conn && live.reconnect_in_us.is_none() => {
                live.status = Status::Connecting;
                fx.push(TogetherFx::Notice(Notice::Status(Status::Connecting)));
                // Network cut or relay restarted: reconnect, more and more patiently.
                let delay = RECONNECT_BASE_US.saturating_mul(1u64 << live.retry.min(20)).min(RECONNECT_CEILING_US);
                live.retry = live.retry.saturating_add(1);
                live.reconnect_in_us = Some(delay);
                (Phase::Live(live), Handled::Yes)
            }
            other @ (Phase::Idle | Phase::Deriving(_) | Phase::Joining(_) | Phase::Live(_)) => (other, Handled::No),
        },
        TogetherEvent::SocketBinary { conn, bytes } => match phase {
            Phase::Live(live) if live.conn == conn => {
                let tag = desk.tag();
                desk.opens.insert(tag, conn);
                fx.push(TogetherFx::Open { tag, key: live.keys.key, bytes });
                (Phase::Live(live), Handled::Yes)
            }
            other @ (Phase::Idle | Phase::Deriving(_) | Phase::Joining(_) | Phase::Live(_)) => (other, Handled::No),
        },
        TogetherEvent::Opened { tag, plain } => {
            let from = desk.opens.remove(&tag);
            match phase {
                Phase::Live(mut live) if from == Some(live.conn) => {
                    // A wrong key or a damaged frame is ignored, as the web ignores it.
                    if let Some(m) = plain.as_deref().and_then(read_message) {
                        on_message(&mut live, m, desk, fx);
                    }
                    (Phase::Live(live), Handled::Yes)
                }
                other @ (Phase::Idle | Phase::Deriving(_) | Phase::Joining(_) | Phase::Live(_)) => (other, if from.is_some() { Handled::Yes } else { Handled::No }),
            }
        }
        TogetherEvent::Sealed { tag, bytes } => {
            let Some(purpose) = desk.seals.remove(&tag) else { return (phase, Handled::No) };
            match purpose {
                SealFor::Farewell(conn) => {
                    fx.push(TogetherFx::SendBinary { conn, bytes });
                    fx.push(TogetherFx::CloseSocket { conn });
                }
                SealFor::Send(conn) => {
                    if let Phase::Live(live) = &phase {
                        if live.conn == conn && live.status == Status::Open {
                            fx.push(TogetherFx::SendBinary { conn, bytes });
                        }
                    }
                }
            }
            (phase, Handled::Yes)
        }
        TogetherEvent::Attach { item, obs } => match phase {
            Phase::Live(mut live) => {
                if live.host && live.item.as_ref().map(SharedItem::identity) != Some(item.identity()) {
                    live.item = Some(item.clone());
                    fx.push(TogetherFx::Notice(Notice::Item(item.clone())));
                }
                live.playing_item = Some(item);
                live.sync.attach(obs);
                let tick = tick_of(&live, false);
                send(&live, Message::Tick(tick), desk, fx);
                (Phase::Live(live), Handled::Yes)
            }
            other @ (Phase::Idle | Phase::Deriving(_) | Phase::Joining(_)) => (other, Handled::No),
        },
        TogetherEvent::Player(obs) => match phase {
            Phase::Live(mut live) if live.sync.attached() => {
                let mut cmds = Vec::new();
                let online = live.status == Status::Open;
                let out = live.sync.poll(obs, live.host, online, &mut cmds);
                fx.extend(cmds.into_iter().map(TogetherFx::Player));
                if let Some(out) = out {
                    let intent = Intent {
                        id: live.me.id.clone(),
                        name: live.me.name.clone(),
                        action: out.action,
                        time: out.time,
                        playing: out.playing,
                        c: out.c,
                        item: live.playing_item.clone(),
                    };
                    send(&live, Message::Intent(intent), desk, fx);
                }
                (Phase::Live(live), Handled::Yes)
            }
            other @ (Phase::Idle | Phase::Deriving(_) | Phase::Joining(_) | Phase::Live(_)) => (other, Handled::No),
        },
        TogetherEvent::Detach => match phase {
            Phase::Live(mut live) if live.sync.attached() => {
                live.sync.detach();
                live.playing_item = None;
                let tick = tick_of(&live, false);
                send(&live, Message::Tick(tick), desk, fx);
                (Phase::Live(live), Handled::Yes)
            }
            other @ (Phase::Idle | Phase::Deriving(_) | Phase::Joining(_) | Phase::Live(_)) => (other, Handled::No),
        },
        TogetherEvent::ShareTitle(item) => match phase {
            Phase::Live(mut live) if live.host => {
                live.item = Some(item.clone());
                fx.push(TogetherFx::Notice(Notice::Item(item)));
                let tick = tick_of(&live, false);
                send(&live, Message::Tick(tick), desk, fx);
                (Phase::Live(live), Handled::Yes)
            }
            other @ (Phase::Idle | Phase::Deriving(_) | Phase::Joining(_) | Phase::Live(_)) => (other, Handled::No),
        },
        TogetherEvent::Leave => match phase {
            Phase::Idle => (Phase::Idle, Handled::No),
            other @ (Phase::Deriving(_) | Phase::Joining(_) | Phase::Live(_)) => {
                leave(other, desk, fx);
                (Phase::Idle, Handled::Yes)
            }
        },
    }
}

/// Leaves whatever `phase` holds: a live session says `bye` (sealed, then the socket closes).
fn leave(phase: Phase, desk: &mut Desk, fx: &mut Vec<TogetherFx>) {
    let Phase::Live(live) = phase else { return };
    if live.status == Status::Open {
        let tag = desk.tag();
        desk.seals.insert(tag, SealFor::Farewell(live.conn));
        fx.push(TogetherFx::Seal { tag, key: live.keys.key, plain: Message::Bye(Bye { id: live.me.id.clone() }).to_json() });
    } else {
        fx.push(TogetherFx::CloseSocket { conn: live.conn });
    }
    desk.opens.retain(|_, conn| *conn != live.conn);
    fx.push(TogetherFx::Notice(Notice::Left));
}

fn join(me: Me, code: &str, relay_setting: &str, server: Option<String>, servers: &[ServerInfo], desk: &mut Desk, fx: &mut Vec<TogetherFx>) -> (Phase, Handled) {
    let Some(invite) = parse_invite(code) else {
        fx.push(TogetherFx::Notice(Notice::JoinFailed(JoinError::Code)));
        return (Phase::Idle, Handled::Yes);
    };
    let text = invite_text(&invite);
    let (secret, given) = match &invite {
        Invite::V3 { secret, relay } => (secret.clone(), relay.clone()),
        Invite::V2 { relay, .. } | Invite::V1 { relay, .. } => {
            let relay = relay.clone();
            return match derive_legacy(&invite) {
                Some(keys) => (open(me, relay, keys, text, None, false, None, desk, fx), Handled::Yes),
                None => {
                    fx.push(TogetherFx::Notice(Notice::JoinFailed(JoinError::Code)));
                    (Phase::Idle, Handled::Yes)
                }
            };
        }
    };
    let mut ids: Vec<String> = server.into_iter().chain(servers.iter().map(|s| s.id.clone())).filter(|id| !id.is_empty()).collect();
    ids = dedupe(&ids);
    ids.truncate(MAX_ROOM_LOOKUP);
    let relays: Vec<String> = given.into_iter().chain([relay_url(relay_setting)]).chain(relay_candidates(servers)).filter(|r| !r.is_empty()).collect();
    if relays.is_empty() {
        fx.push(TogetherFx::Notice(Notice::JoinFailed(JoinError::Relay)));
        return (Phase::Idle, Handled::Yes);
    }
    let mut derived = Vec::with_capacity(ids.len());
    for id in &ids {
        let tag = desk.tag();
        derived.push((tag, None));
        fx.push(TogetherFx::Derive { tag, secret: secret.clone(), server: id.clone(), iterations: INVITE_ITERATIONS });
    }
    let plan = JoinPlan { me, code: text, ids, relays, derived, search: None, asked: Vec::new() };
    if plan.derived.is_empty() {
        // No server to salt with: nothing can be found.
        fx.push(TogetherFx::Notice(Notice::JoinFailed(JoinError::NotFound)));
        return (Phase::Idle, Handled::Yes);
    }
    (Phase::Joining(plan), Handled::Yes)
}

/// A join whose search may have an outcome: open the session found, or report it not found.
fn settle_join(plan: JoinPlan, desk: &mut Desk, fx: &mut Vec<TogetherFx>) -> Phase {
    let Some(outcome) = plan.search.as_ref().and_then(RoomSearch::outcome) else { return Phase::Joining(plan) };
    let keys = outcome.and_then(|(relay, room)| {
        plan.derived.iter().filter_map(|(_, k)| k.clone()).find(|k| k.room == room).map(|k| (relay, k))
    });
    match keys {
        Some((relay, keys)) => open(plan.me, relay, keys, plan.code, None, false, None, desk, fx),
        None => {
            fx.push(TogetherFx::Notice(Notice::JoinFailed(JoinError::NotFound)));
            Phase::Idle
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn open(me: Me, relay: String, keys: SessionKeys, code: String, full_code: Option<String>, host: bool, item: Option<SharedItem>, desk: &mut Desk, fx: &mut Vec<TogetherFx>) -> Phase {
    let conn = desk.conn();
    fx.push(TogetherFx::OpenSocket { conn, url: format!("{relay}/r/{}", keys.room) });
    fx.push(TogetherFx::Notice(Notice::Opened { code: code.clone(), full_code: full_code.clone(), host }));
    fx.push(TogetherFx::Notice(Notice::Status(Status::Connecting)));
    let mut sync = TogetherSync::new(&me.id);
    sync.advance(desk.now_us);
    Phase::Live(Live {
        me,
        relay,
        keys,
        code,
        full_code,
        host,
        host_claim: 0,
        status: Status::Connecting,
        item,
        playing_item: None,
        peers: Vec::new(),
        sync,
        conn,
        retry: 0,
        reconnect_in_us: None,
        since_tick_us: 0,
        last_want_reply_us: None,
    })
}

/// Seals `msg` for the live socket, when it is open (a message while offline is not sent).
fn send(live: &Live, msg: Message, desk: &mut Desk, fx: &mut Vec<TogetherFx>) {
    if live.status != Status::Open {
        return;
    }
    let tag = desk.tag();
    desk.seals.insert(tag, SealFor::Send(live.conn));
    fx.push(TogetherFx::Seal { tag, key: live.keys.key, plain: msg.to_json() });
}

/// This device's presence. Its position is the player's when attached, else the session's.
fn tick_of(live: &Live, want: bool) -> Tick {
    let (time, playing) = match (live.sync.player_time(), live.sync.player_paused()) {
        (Some(t), Some(paused)) => (t, !paused),
        _ => (live.sync.position().unwrap_or(0.0), live.sync.reference().is_some_and(|r| r.playing)),
    };
    let stamp = live.sync.stamp();
    Tick {
        id: live.me.id.clone(),
        name: live.me.name.clone(),
        thumb: live.me.thumb.clone(),
        host: live.host,
        time,
        playing,
        item: live.playing_item.clone().or_else(|| live.item.clone()),
        live: live.sync.attached(),
        c: stamp.c,
        by: stamp.by,
        // A session this client hosts is private: never visible to friends.
        vis: live.host.then_some(false),
        hc: live.host.then_some(live.host_claim),
        want,
    }
}

fn on_tick(live: &mut Live, dt_us: u64, desk: &mut Desk, fx: &mut Vec<TogetherFx>) {
    live.sync.advance(dt_us);
    if let Some(left) = live.reconnect_in_us {
        if left <= dt_us {
            live.reconnect_in_us = None;
            live.conn = desk.conn();
            desk.opens.clear();
            fx.push(TogetherFx::OpenSocket { conn: live.conn, url: format!("{}/r/{}", live.relay, live.keys.room) });
        } else {
            live.reconnect_in_us = Some(left - dt_us);
        }
    }
    live.since_tick_us = live.since_tick_us.saturating_add(dt_us);
    if live.since_tick_us < TICK_EVERY_US {
        return;
    }
    live.since_tick_us = 0;
    let tick = tick_of(live, false);
    send(live, Message::Tick(tick), desk, fx);
    let now = desk.now_us;
    live.peers.retain(|p| now.saturating_sub(p.seen_us) <= PEER_TIMEOUT_US);
    // The host has gone: the first participant (by id) takes the hand.
    let me = live.me.id.as_str();
    if !live.host && live.status == Status::Open && !live.peers.is_empty() && live.peers.iter().all(|p| !p.host && p.id.as_str() > me) {
        live.host_claim = live.sync.clock() + 1;
        live.host = true;
        fx.push(TogetherFx::Notice(Notice::HostChanged { host: true }));
    }
}

/// What a message says of its author. A tick says everything; an intent leaves `thumb`, `host` and
/// `live` to what was last heard (`None`).
struct News<'a> {
    id: &'a str,
    name: &'a str,
    presence: Option<(&'a Option<String>, bool, bool)>,
    playing: bool,
    time: f64,
    item: &'a Option<SharedItem>,
}

fn upsert_peer(live: &mut Live, news: News<'_>, now: u64) {
    let prev = live.peers.iter().position(|p| p.id == news.id).map(|i| live.peers.remove(i));
    let (thumb, host, in_player) = match news.presence {
        Some((thumb, host, in_player)) => (thumb.clone(), host, in_player),
        None => (None, prev.as_ref().is_some_and(|p| p.host), prev.as_ref().is_none_or(|p| p.live)),
    };
    live.peers.push(Peer {
        id: news.id.to_string(),
        name: news.name.to_string(),
        thumb: thumb.or_else(|| prev.as_ref().and_then(|p| p.thumb.clone())),
        host,
        playing: news.playing,
        time: news.time,
        live: in_player,
        item: news.item.clone().or_else(|| prev.and_then(|p| p.item)),
        seen_us: now,
    });
    live.peers.sort_by(|a, b| b.host.cmp(&a.host).then_with(|| a.name.cmp(&b.name)));
}

fn on_message(live: &mut Live, m: Incoming, desk: &mut Desk, fx: &mut Vec<TogetherFx>) {
    let now = desk.now_us;
    match m {
        Incoming::Bye { id } => live.peers.retain(|p| p.id != id),
        Incoming::Tick(t) => {
            if t.id == live.me.id {
                return;
            }
            upsert_peer(live, News { id: &t.id, name: &t.name, presence: Some((&t.thumb, t.host, t.live)), playing: t.playing, time: t.time, item: &t.item }, now);
            // Two hosts (one came back after a cut): one stays, the other becomes a participant.
            if t.host && live.host && host_yields(live.host_claim, &live.me.id, t.hc, &t.id) {
                live.host = false;
                fx.push(TogetherFx::Notice(Notice::HostChanged { host: false }));
            }
            if t.host {
                if let Some(item) = &t.item {
                    if live.item.as_ref().map(SharedItem::identity) != Some(item.identity()) {
                        live.item = Some(item.clone());
                        fx.push(TogetherFx::Notice(Notice::Item(item.clone())));
                    }
                }
            }
            let mut cmds = Vec::new();
            live.sync.on_tick(&t, live.host, &mut cmds);
            fx.extend(cmds.into_iter().map(TogetherFx::Player));
            // Someone arrives or returns: they need our state now (at most once a second).
            if t.want && live.last_want_reply_us.is_none_or(|at| now.saturating_sub(at) > WANT_REPLY_US) {
                live.last_want_reply_us = Some(now);
                let tick = tick_of(live, false);
                send(live, Message::Tick(tick), desk, fx);
            }
        }
        Incoming::Intent(i) => {
            if i.id == live.me.id {
                return;
            }
            upsert_peer(live, News { id: &i.id, name: &i.name, presence: None, playing: i.playing, time: i.time, item: &i.item }, now);
            let mut cmds = Vec::new();
            if live.sync.on_intent(&i, &mut cmds) {
                fx.push(TogetherFx::Notice(Notice::Action { name: i.name.clone(), action: i.action }));
            }
            fx.extend(cmds.into_iter().map(TogetherFx::Player));
        }
    }
}


#[cfg(test)]
mod tests {
    use super::super::keys::{derive_session, open_message, seal_message};
    use super::super::relay::PlexConnection;
    use super::*;
    use serde_json::Value;

    /// Rounds the test runner derives with (the effect asks 200 000; both sides use the same).
    const ROUNDS: u32 = 1000;

    /// One device: the machine, and a runner that executes its derivations, seals and opens at
    /// once (as the app's runners do off the frame) and keeps the rest for the test to read.
    struct Device {
        s: TogetherSession,
        out: Vec<TogetherFx>,
        ivs: u8,
        derives: Vec<(String, String, u32)>,
    }

    impl Device {
        fn new() -> Device {
            Device { s: TogetherSession::new(), out: Vec::new(), ivs: 0, derives: Vec::new() }
        }

        fn step(&mut self, ev: TogetherEvent) -> Handled {
            let mut queue = vec![ev];
            let mut first = None;
            while let Some(ev) = queue.pop() {
                let mut fx = Vec::new();
                let h = self.s.step(ev, &mut fx);
                first.get_or_insert(h);
                for f in fx {
                    match f {
                        TogetherFx::Derive { tag, secret, server, iterations } => {
                            self.derives.push((secret.clone(), server.clone(), iterations));
                            queue.insert(0, TogetherEvent::Derived { tag, keys: derive_session(&secret, &server, ROUNDS) });
                        }
                        TogetherFx::Seal { tag, key, plain } => {
                            self.ivs = self.ivs.wrapping_add(1);
                            queue.insert(0, TogetherEvent::Sealed { tag, bytes: seal_message(&key, &[self.ivs; 12], &plain) });
                        }
                        TogetherFx::Open { tag, key, bytes } => queue.insert(0, TogetherEvent::Opened { tag, plain: open_message(&key, &bytes) }),
                        other @ (TogetherFx::HttpRooms { .. }
                        | TogetherFx::OpenSocket { .. }
                        | TogetherFx::CloseSocket { .. }
                        | TogetherFx::SendBinary { .. }
                        | TogetherFx::Player(_)
                        | TogetherFx::Notice(_)) => self.out.push(other),
                    }
                }
            }
            first.unwrap_or(Handled::No)
        }

        fn take(&mut self) -> Vec<TogetherFx> {
            std::mem::take(&mut self.out)
        }

        fn key(&self) -> [u8; 32] {
            match &self.s.phase {
                Phase::Live(l) => l.keys.key,
                Phase::Idle | Phase::Deriving(_) | Phase::Joining(_) => panic!("not live"),
            }
        }

        fn conn(&self) -> u32 {
            match &self.s.phase {
                Phase::Live(l) => l.conn,
                Phase::Idle | Phase::Deriving(_) | Phase::Joining(_) => panic!("not live"),
            }
        }

        fn view(&self) -> View {
            self.s.view().expect("live")
        }

        /// The JSON of every frame sent since the last take.
        fn sent(&mut self) -> Vec<Value> {
            let key = self.key();
            let out = self.take();
            out.iter()
                .filter_map(|f| if let TogetherFx::SendBinary { bytes, .. } = f { Some(serde_json::from_slice(&open_message(&key, bytes).unwrap()).unwrap()) } else { None })
                .collect()
        }

        fn tick(&mut self, us: u64) {
            self.step(TogetherEvent::Tick { dt_us: us });
        }
    }

    fn me(id: &str, name: &str) -> Me {
        Me { id: id.into(), name: name.into(), thumb: None }
    }

    fn movie() -> SharedItem {
        SharedItem { guid: "plex://movie/1".into(), server: Some("srv-1".into()), key: Some("42".into()), kind: "movie".into(), title: "Le Film".into(), sub: None }
    }

    fn notices(out: &[TogetherFx]) -> Vec<Notice> {
        out.iter().filter_map(|f| if let TogetherFx::Notice(n) = f { Some(n.clone()) } else { None }).collect()
    }

    /// A host on `wss://relais.exemple.fr`, its socket open. Returns the device and its code.
    fn hosting(id: &str) -> (Device, String) {
        let mut h = Device::new();
        h.step(TogetherEvent::Host { me: me(id, "Hôte"), relay: "relais.exemple.fr".into(), server: Some("srv-1".into()), item: Some(movie()), random: (0u8..32).collect() });
        let out = h.take();
        let Some(Notice::Opened { code, full_code, host: true }) = notices(&out).first().cloned() else { panic!("{out:?}") };
        assert_eq!(full_code.as_deref(), Some(format!("{code}@relais.exemple.fr").as_str()));
        let conn = h.conn();
        h.step(TogetherEvent::SocketOpened { conn });
        (h, code)
    }

    /// A participant joined through the code the host printed with its relay.
    fn joining(id: &str, code: &str) -> Device {
        let mut g = Device::new();
        g.step(TogetherEvent::Join { me: me(id, "Ami"), code: format!("{code}@relais.exemple.fr"), relay: String::new(), server: Some("srv-1".into()), servers: Vec::new() });
        let out = g.take();
        let rooms: Vec<(u32, String)> = out.iter().filter_map(|f| if let TogetherFx::HttpRooms { tag, url, .. } = f { Some((*tag, url.clone())) } else { None }).collect();
        assert_eq!(rooms.len(), 1, "{out:?}");
        let room = derive_session(code.trim_start_matches("MPX-"), "srv-1", ROUNDS).room;
        assert_eq!(rooms[0].1, format!("https://relais.exemple.fr/rooms?ids={room}"));
        g.step(TogetherEvent::RoomsAnswered { tag: rooms[0].0, body: Some(format!(r#"{{"multiplex":"relay","found":["{room}"]}}"#)) });
        let out = g.take();
        assert!(out.contains(&TogetherFx::OpenSocket { conn: g.conn(), url: format!("wss://relais.exemple.fr/r/{room}") }), "{out:?}");
        let conn = g.conn();
        g.step(TogetherEvent::SocketOpened { conn });
        g
    }

    /// The relay: every frame one device sends reaches the others.
    fn relay(from: &mut Device, to: &mut [&mut Device]) {
        let out = from.take();
        for f in &out {
            if let TogetherFx::SendBinary { bytes, .. } = f {
                for d in to.iter_mut() {
                    let conn = d.conn();
                    d.step(TogetherEvent::SocketBinary { conn, bytes: bytes.clone() });
                }
            }
        }
        from.out.extend(out.into_iter().filter(|f| !matches!(f, TogetherFx::SendBinary { .. })));
    }

    #[test]
    fn a_host_opens_a_private_session_salted_by_its_server() {
        let (mut h, code) = hosting("m");
        assert_eq!(h.derives.len(), 1);
        let (secret, server, iterations) = h.derives[0].clone();
        assert_eq!((server.as_str(), iterations), ("srv-1", INVITE_ITERATIONS));
        assert_eq!(code, format_invite(&secret, None));
        assert_eq!(h.key(), derive_session(&secret, "srv-1", ROUNDS).key);
        let sent = h.sent();
        assert_eq!(sent.len(), 1);
        // Arriving asks for everyone's state; a host's tick is private and carries its claim.
        assert_eq!((sent[0]["t"].as_str(), sent[0]["want"].as_bool(), sent[0]["vis"].as_bool(), sent[0]["hc"].as_u64()), (Some("tick"), Some(true), Some(false), Some(0)));
        assert_eq!(sent[0]["item"]["guid"], "plex://movie/1");
        let v = h.view();
        assert!(v.host && v.status == Status::Open && v.peers.is_empty());
    }

    #[test]
    fn a_host_needs_a_relay_and_randomness() {
        let mut d = Device::new();
        d.step(TogetherEvent::Host { me: me("m", "H"), relay: String::new(), server: None, item: None, random: vec![1; 32] });
        assert_eq!(notices(&d.take()), [Notice::HostFailed]);
        d.step(TogetherEvent::Host { me: me("m", "H"), relay: "wss://r.fr".into(), server: None, item: None, random: vec![255; 64] });
        assert_eq!(notices(&d.take()), [Notice::HostFailed]);
        assert!(d.s.view().is_none());
    }

    #[test]
    fn joining_by_code_finds_the_room_and_meets_the_host() {
        let (mut h, code) = hosting("m");
        h.take();
        let mut g = joining("z", &code);
        assert_eq!(g.key(), h.key());
        assert!(!g.view().host);
        // The participant's "want" reaches the host, who answers at once with its state.
        relay(&mut g, &mut [&mut h]);
        relay(&mut h, &mut [&mut g]);
        let hv = h.view();
        assert_eq!(hv.peers.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["z"]);
        let gv = g.view();
        assert_eq!(gv.peers.iter().map(|p| (p.id.as_str(), p.host)).collect::<Vec<_>>(), [("m", true)]);
        assert_eq!(gv.item.map(|i| i.guid), Some("plex://movie/1".into()));
    }

    #[test]
    fn join_failures_are_named() {
        let mut g = Device::new();
        g.step(TogetherEvent::Join { me: me("z", "A"), code: "bonjour".into(), relay: String::new(), server: None, servers: Vec::new() });
        assert_eq!(notices(&g.take()), [Notice::JoinFailed(JoinError::Code)]);
        g.step(TogetherEvent::Join { me: me("z", "A"), code: "MPX-K7Q2-9XFD".into(), relay: String::new(), server: Some("srv-1".into()), servers: Vec::new() });
        assert_eq!(notices(&g.take()), [Notice::JoinFailed(JoinError::Relay)]);
        // Two relays from the server's address; neither has the room.
        let servers = vec![ServerInfo { id: "srv-1".into(), connections: vec![PlexConnection { uri: "https://r.exemple.fr".into(), local: false, relay: false }] }];
        g.step(TogetherEvent::Join { me: me("z", "A"), code: "MPX-K7Q2-9XFD".into(), relay: String::new(), server: None, servers });
        let asks: Vec<u32> = g.take().iter().filter_map(|f| if let TogetherFx::HttpRooms { tag, .. } = f { Some(*tag) } else { None }).collect();
        assert_eq!(asks.len(), 2);
        assert!(g.s.busy());
        assert_eq!(g.step(TogetherEvent::RoomsAnswered { tag: 9999, body: None }), Handled::No);
        g.step(TogetherEvent::RoomsAnswered { tag: asks[1], body: Some(r#"{"multiplex":"relay","found":[]}"#.into()) });
        assert!(notices(&g.take()).is_empty(), "the better-ranked relay has not answered");
        g.step(TogetherEvent::RoomsAnswered { tag: asks[0], body: None });
        assert_eq!(notices(&g.take()), [Notice::JoinFailed(JoinError::NotFound)]);
        assert!(!g.s.busy());
    }

    #[test]
    fn an_old_code_opens_its_room_directly() {
        let mut g = Device::new();
        g.step(TogetherEvent::Join { me: me("z", "A"), code: "MPX-q3Vb8xK2mT0wZr5L@relais.exemple.fr".into(), relay: String::new(), server: None, servers: Vec::new() });
        let out = g.take();
        assert!(g.derives.is_empty());
        assert!(matches!(&out[0], TogetherFx::OpenSocket { url, .. } if url.starts_with("wss://relais.exemple.fr/r/")), "{out:?}");
    }

    #[test]
    fn a_tick_every_three_seconds_and_silent_peers_leave() {
        let (mut h, code) = hosting("m");
        let mut g = joining("z", &code);
        relay(&mut g, &mut [&mut h]);
        h.take();
        h.tick(2_900_000);
        assert!(h.sent().is_empty());
        h.tick(100_000);
        let sent = h.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["want"], Value::Null, "want only when true");
        for _ in 0..3 {
            h.tick(TICK_EVERY_US);
        }
        // 12 s since "z" was heard: still there (the web keeps it up to 12 s).
        assert_eq!(h.view().peers.len(), 1);
        h.tick(TICK_EVERY_US);
        assert!(h.view().peers.is_empty());
    }

    #[test]
    fn a_want_is_answered_at_most_once_a_second() {
        let (mut h, code) = hosting("m");
        let mut g = joining("z", &code);
        h.take();
        let want = g.take();
        let resend = |h: &mut Device| {
            for f in &want {
                if let TogetherFx::SendBinary { bytes, .. } = f {
                    let conn = h.conn();
                    h.step(TogetherEvent::SocketBinary { conn, bytes: bytes.clone() });
                }
            }
        };
        resend(&mut h);
        assert_eq!(h.sent().len(), 1);
        h.tick(500_000);
        resend(&mut h);
        assert!(h.sent().is_empty());
        h.tick(600_000);
        resend(&mut h);
        assert_eq!(h.sent().len(), 1);
    }

    #[test]
    fn the_first_participant_takes_the_hand_and_a_returning_host_steps_down() {
        let (mut h, code) = hosting("m");
        let mut a = joining("a", &code);
        let mut z = joining("z", &code);
        relay(&mut a, &mut [&mut h, &mut z]);
        relay(&mut z, &mut [&mut h, &mut a]);
        relay(&mut h, &mut [&mut a, &mut z]);
        a.take();
        z.take();
        // The host goes silent. After 12 s it has left; "a" (the smaller id) takes the hand.
        for _ in 0..5 {
            a.tick(TICK_EVERY_US);
            z.tick(TICK_EVERY_US);
            relay(&mut a, &mut [&mut z]);
            relay(&mut z, &mut [&mut a]);
        }
        let an = notices(&a.take());
        assert!(an.contains(&Notice::HostChanged { host: true }), "{an:?}");
        assert!(a.view().host);
        assert!(!z.view().host);
        assert!(!notices(&z.take()).contains(&Notice::HostChanged { host: true }));
        // Its ticks carry when it took the hand: after everything it has seen.
        a.tick(TICK_EVERY_US);
        let sent = a.sent();
        assert!(sent[0]["hc"].as_u64().unwrap() >= 1);
        assert_eq!(sent[0]["vis"], false);
        // The old host comes back still believing it hosts (hc 0): it yields to the later claim.
        let back: Vec<u8> = {
            let t = Message::Tick(Tick { id: "a".into(), name: "Ami".into(), thumb: None, host: true, time: 0.0, playing: false, item: None, live: false, c: 0, by: String::new(), vis: Some(false), hc: Some(1), want: false });
            seal_message(&h.key(), &[3; 12], &t.to_json())
        };
        let conn = h.conn();
        h.take();
        h.step(TogetherEvent::SocketBinary { conn, bytes: back.clone() });
        assert!(notices(&h.take()).contains(&Notice::HostChanged { host: false }));
        assert!(!h.view().host);
        // The reverse: a claim of 0 from a smaller id does not move "a".
        let older = Message::Tick(Tick { id: "0".into(), name: "H".into(), thumb: None, host: true, time: 0.0, playing: false, item: None, live: false, c: 0, by: String::new(), vis: None, hc: Some(0), want: false });
        let conn = a.conn();
        let key = a.key();
        a.step(TogetherEvent::SocketBinary { conn, bytes: seal_message(&key, &[4; 12], &older.to_json()) });
        assert!(a.view().host);
    }

    #[test]
    fn a_cut_socket_reconnects_more_and_more_patiently() {
        let (mut h, _) = hosting("m");
        h.take();
        let first = h.conn();
        h.step(TogetherEvent::SocketClosed { conn: first });
        assert_eq!(notices(&h.take()), [Notice::Status(Status::Connecting)]);
        h.tick(999_000);
        assert!(!h.take().iter().any(|f| matches!(f, TogetherFx::OpenSocket { .. })));
        h.tick(1_000);
        let second = h.conn();
        assert_ne!(second, first);
        assert!(h.take().iter().any(|f| matches!(f, TogetherFx::OpenSocket { conn, url } if *conn == second && url.contains("/r/"))));
        // A late event of the old socket changes nothing.
        assert_eq!(h.step(TogetherEvent::SocketOpened { conn: first }), Handled::No);
        h.step(TogetherEvent::SocketClosed { conn: second });
        h.tick(1_999_000);
        assert!(!h.take().iter().any(|f| matches!(f, TogetherFx::OpenSocket { .. })));
        h.tick(1_000);
        let third = h.conn();
        h.step(TogetherEvent::SocketOpened { conn: third });
        let sent = h.sent();
        assert_eq!(sent.len(), 1, "back: asks for everyone's state");
        assert_eq!(sent[0]["want"], true);
        assert_eq!(h.view().status, Status::Open);
    }

    #[test]
    fn a_gesture_reaches_the_others_and_moves_their_player() {
        let (mut h, code) = hosting("m");
        let mut g = joining("z", &code);
        relay(&mut g, &mut [&mut h]);
        relay(&mut h, &mut [&mut g]);
        let at = |time: f64, paused: bool| PlayerObs { time, paused, ready: true, seeking: false };
        h.step(TogetherEvent::Attach { item: movie(), obs: at(0.0, true) });
        g.step(TogetherEvent::Attach { item: movie(), obs: at(0.0, true) });
        h.step(TogetherEvent::Player(at(0.0, true)));
        g.step(TogetherEvent::Player(at(0.0, true)));
        relay(&mut h, &mut [&mut g]);
        relay(&mut g, &mut [&mut h]);
        h.take();
        g.take();
        h.tick(500_000);
        g.tick(500_000);
        // The participant seeks to 300 s: an intent leaves, the host's player follows.
        g.step(TogetherEvent::Player(at(300.0, true)));
        let sent = g.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!((sent[0]["t"].as_str(), sent[0]["action"].as_str(), sent[0]["time"].as_f64(), sent[0]["c"].as_u64()), (Some("intent"), Some("seek"), Some(300.0), Some(1)));
        let bytes = seal_message(&g.key(), &[7; 12], &serde_json::to_string(&sent[0]).unwrap());
        let conn = h.conn();
        h.step(TogetherEvent::SocketBinary { conn, bytes });
        let out = h.take();
        assert!(out.contains(&TogetherFx::Player(PlayerCmd::Seek(300.0))), "{out:?}");
        assert!(notices(&out).contains(&Notice::Action { name: "Ami".into(), action: Some(SyncAction::Seek) }));
    }

    #[test]
    fn leaving_says_bye_then_closes() {
        let (mut h, _) = hosting("m");
        let key = h.key();
        let conn = h.conn();
        h.take();
        assert_eq!(h.step(TogetherEvent::Leave), Handled::Yes);
        let out = h.take();
        let TogetherFx::SendBinary { conn: c, bytes } = &out[1] else { panic!("{out:?}") };
        assert_eq!(*c, conn);
        assert_eq!(open_message(&key, bytes).unwrap(), br#"{"t":"bye","id":"m"}"#);
        assert_eq!(out[2], TogetherFx::CloseSocket { conn });
        assert_eq!(out[0], TogetherFx::Notice(Notice::Left));
        assert!(h.s.view().is_none());
        assert_eq!(h.step(TogetherEvent::Leave), Handled::No);
    }

    #[test]
    fn stray_answers_and_player_events_outside_a_session_change_nothing() {
        let mut d = Device::new();
        let obs = PlayerObs { time: 1.0, paused: false, ready: true, seeking: false };
        assert_eq!(d.step(TogetherEvent::Player(obs)), Handled::No);
        assert_eq!(d.step(TogetherEvent::Sealed { tag: 4, bytes: vec![1] }), Handled::No);
        assert_eq!(d.step(TogetherEvent::Opened { tag: 4, plain: None }), Handled::No);
        assert_eq!(d.step(TogetherEvent::SocketBinary { conn: 1, bytes: vec![1] }), Handled::No);
        assert_eq!(d.step(TogetherEvent::Detach), Handled::No);
        assert_eq!(d.step(TogetherEvent::Tick { dt_us: 1 }), Handled::Yes);
        assert!(d.take().is_empty());
    }
}
