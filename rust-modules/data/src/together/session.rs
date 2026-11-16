#![deny(clippy::wildcard_enum_match_arm)]
//! `TogetherSession`: one Watch Together session, a `State + TogetherEvent -> TogetherFx` machine in
//! the shape of `HudStore` (registered in `ci/machines.ini`). It is the web client's
//! `apps/web/src/together.ts` without its IO: it opens no socket, makes no request, draws no random
//! byte, reads no clock and touches no player. Every one of those leaves as a [`TogetherFx`], and
//! every answer comes back as one addressed [`TogetherEvent`] (a `tag` for a request, a `conn` for a
//! socket). Time is the `dt_us` of [`TogetherEvent::Tick`].
//!
//! **Scope.** Invitation-code sessions (host a new one, join by code, the older v1/v2 codes), the
//! presence and Lamport rule of `sync`, host take-over and step-down, and sessions visible to the
//! host's Plex friends (the web's friends-only protocol, `friends`): the host's announce to its
//! relay (`/rooms` checked for `friends`, `friendsOnly` and the host's server BEFORE any token
//! leaves, then `POST /friends` with the server's own token, then the announce with the host's
//! friend allow-list, re-read from plex.tv at most every 2 minutes and released when plex.tv does not
//! answer), the key hand-over to a friend the relay admitted, the listing of friends' sessions
//! (`RefreshFriends`) and entering one (`JoinFriend`). No token is held here: `AskFriends` names the
//! relay and the server, and the runner resolves the server's credential itself.
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
//! | `Leave` | `Live` | `release` (an announced host), `bye`, then `CloseSocket`; `Idle` |
//! | `SetVisible` | `Live` | host: announce or `unlist`; a tick |
//! | `SocketText` | `Live`, its `conn` | `announced` / an error: listed or not; `key-request`: `SealGrant` |
//! | `FriendsAnswered`, `FriendIdsRead`, `GrantSealed` | the request that asked (by `tag`) | the announce, the listing, `SendText` |
//! | `RefreshFriends` | any | `HttpRooms` per candidate relay of each server, `AskFriends` per relay found; `Notice::FriendSessions` |
//! | `JoinFriend` | any (leaves the current one) | `RequestKey`, then `Derive`; `Live` when the room is the one listed |
//!
//! A pair not listed (a stale `tag` or `conn`, a player event outside a session) is
//! [`Handled::No`] and changes nothing. Volume, subtitles, audio and quality have no field in any
//! message (`wire`), so nothing here can send them.

use std::collections::BTreeMap;
use std::fmt;

use plx_machine::machine::Handled;

use super::friends::{
    announce_text, control_text, friends_for_announce, key_grant_text, Control, FriendCard, FriendsAnswer, FriendsSent, GrantPayload, GrantState,
    RelayBackoff, RelaySearch, FRIENDS_TTL_US, UNLISTED_ERRORS,
};
use super::invite::{format_invite, invite_text, parse_invite, relay_url, secret_from_bytes, Invite};
use super::keys::{derive_legacy, SessionKeys, INVITE_ITERATIONS};
use super::relay::{dedupe, relay_candidates, relay_discoverable, rooms_url, KnownRelays, RoomSearch, RoomsAnswer, ServerInfo, MAX_ROOM_LOOKUP};
use super::sync::{host_yields, tick_clock, PlayerCmd, PlayerObs, Stamp, TogetherSync};
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
/// An announced card is re-sent on a change at most every 5 s, and its position every 5 presence
/// ticks (15 s).
pub const CARD_EVERY_US: u64 = 5_000_000;
pub const CARD_REFRESH_TICKS: u32 = 5;
/// A relay that does not list the session is asked again after 1 min, doubled up to 30 min.
pub const LIST_RETRY_BASE_US: u64 = 60_000_000;
pub const LIST_RETRY_CEILING_US: u64 = 1_800_000_000;
/// A host hands the key to at most this many requesters per window, once each.
pub const GRANTS_PER_WINDOW: usize = 5;
pub const GRANT_WINDOW_US: u64 = 10_000_000;
/// A listed session's ticket is taken for expired this long before the relay's `ttl` says.
pub const TICKET_MARGIN_US: u64 = 15_000_000;

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
    /// No relay knows the session (or a friend's session is no longer listed).
    NotFound,
    /// A friend's session: its host is not connected to hand the key over.
    NoHost,
    /// A friend's session: the key hand-over failed or did not open the listed room.
    Failed,
}

/// Where the announce of a visible session stands (the web's `listed`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Listed {
    Pending,
    Yes,
    No,
}

/// A friend's visible session, as a relay listed it (the web's `FriendSession`). `ticket` lets this
/// account in once; it is a secret and is not printed.
#[derive(Clone, PartialEq)]
pub struct FriendSession {
    pub relay: String,
    /// The relay's Plex server (the one access was checked against).
    pub server: String,
    pub room: String,
    pub card: FriendCard,
    pub count: u32,
    /// When the host last announced, on this machine's clock (µs).
    pub at_us: u64,
    ticket: String,
    ticket_until_us: u64,
}

impl fmt::Debug for FriendSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FriendSession")
            .field("relay", &self.relay)
            .field("server", &self.server)
            .field("room", &self.room)
            .field("card", &self.card)
            .field("count", &self.count)
            .field("at_us", &self.at_us)
            .finish_non_exhaustive()
    }
}

impl FriendSession {
    /// The title of a friend's session, to find it at home (the web's `friendItem`).
    pub fn item(&self) -> SharedItem {
        SharedItem {
            guid: self.card.guid.clone().unwrap_or_default(),
            server: Some(self.server.clone()),
            key: self.card.key.clone(),
            kind: self.card.kind.clone().unwrap_or_else(|| "movie".into()),
            title: self.card.title.clone(),
            sub: self.card.sub.clone(),
        }
    }

    /// Its position at `now_us` (the web's `friendTime`).
    pub fn time_at(&self, now_us: u64) -> f64 {
        let ran = if self.card.playing { now_us.saturating_sub(self.at_us) as f64 / 1e6 } else { 0.0 };
        self.card.time + ran
    }
}

/// What the UI is told.
#[derive(Clone, PartialEq)]
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
    /// The session is visible to the host's friends, or not (the host's choice, read from its ticks).
    Visible(bool),
    /// Where the announce stands (`None`: not announcing).
    Listed(Option<Listed>),
    /// The friends' sessions found by the last `RefreshFriends`.
    FriendSessions(Vec<FriendSession>),
}

pub enum TogetherEvent {
    /// The frame time.
    Tick { dt_us: u64 },
    /// Open a new session as host. `relay`: the relay setting as typed; `server`: the current Plex
    /// server's identifier (salts the secret); `random`: at least 16 bytes the runner drew.
    /// `visible`: listed to the host's Plex friends; `servers`: what this account sees (the host's
    /// server's public names say whether friends find the relay).
    Host { me: Me, relay: String, server: Option<String>, item: Option<SharedItem>, random: Vec<u8>, visible: bool, servers: Vec<ServerInfo> },
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
    /// Host: list the session to its Plex friends, or not.
    SetVisible(bool),
    /// A text (control) message of the relay.
    SocketText { conn: u32, text: String },
    /// The relay's `/friends` answer, or `None` (no credential, a failed request, not a relay).
    FriendsAnswered { tag: u32, answer: Option<FriendsAnswer> },
    /// This account's friend allow-list (`friend_allow_list` of plex.tv `/api/users/`), or `None`
    /// when plex.tv did not answer.
    FriendIdsRead { tag: u32, ids: Option<Vec<String>> },
    /// A grant sealed for a requester: `(host public key, box)`, or `None` (an invalid key).
    GrantSealed { tag: u32, sealed: Option<(String, String)> },
    /// Ask the relays of `servers` for the sessions visible to this account.
    RefreshFriends { servers: Vec<ServerInfo> },
    /// Enter the friend's session `room` listed by `relay`.
    JoinFriend { me: Me, relay: String, room: String },
    /// The host's key hand-over for a `RequestKey`.
    FriendKey { tag: u32, answer: Result<GrantPayload, JoinError> },
}

#[derive(Clone, PartialEq)]
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
    /// A control message on the session's socket.
    SendText { conn: u32, text: String },
    /// `POST {relay}/friends` with the token proper to `server` (its own access token, or a
    /// delegation token it hands its owner; never the plex.tv account token), answered
    /// `FriendsAnswered`. The relay was found on that server's public names and said it is that
    /// server's (`/rooms`), which was checked before this effect.
    AskFriends { tag: u32, relay: String, server: String },
    /// Read this account's Plex friends (plex.tv `/api/users/`, with its own account token, here),
    /// answered `FriendIdsRead`.
    ReadFriendIds { tag: u32 },
    /// Draw an ephemeral P-256 key and an IV and `friends::seal_grant`, answered `GrantSealed`.
    SealGrant { tag: u32, request_pub: String, payload: GrantPayload },
    /// Enter a listed session: a socket to `{relay}/r/{room}`, `key-request` with the ticket and a
    /// fresh public key, wait for `key-grant` (10 s) and open it; answered `FriendKey`.
    RequestKey { tag: u32, relay: String, room: String, ticket: String },
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
#[derive(Clone, PartialEq)]
pub struct View {
    pub code: String,
    pub full_code: Option<String>,
    pub host: bool,
    pub status: Status,
    pub item: Option<SharedItem>,
    pub peers: Vec<Peer>,
    /// The session's position now (for starting playback at the right place when joining).
    pub position: Option<f64>,
    /// Visible to the host's friends.
    pub visible: bool,
    /// Host of a visible session: where its announce stands.
    pub listed: Option<Listed>,
}

#[derive(Clone, PartialEq)]
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
    /// Visible to the host's friends (the host's choice; a participant reads it from the host).
    visible: bool,
    /// The invitation's secret and the server that salts it: what a friend is handed.
    grant: Option<(String, String)>,
    /// The servers this account saw when the session opened.
    servers: Vec<ServerInfo>,
    announce: Announce,
    listed: Option<Listed>,
    /// The last card sent: its signature and when.
    card_sent: (String, u64),
    /// Not listed: asked again at `.0`, after a delay of `.1`.
    list_retry: (u64, u64),
    friends_sent: FriendsSent,
    granted: Vec<String>,
    grant_times: Vec<u64>,
    presence_ticks: u32,
}

/// The announce of a visible session since the socket last opened (the web's `announcing`).
#[derive(Clone, Debug, PartialEq)]
enum Announce {
    Off,
    /// Proving access: the relay's `/rooms`, then `/friends`, then the friend list.
    Rooms(u32),
    Friends(u32),
    Ids(u32, FriendsAnswer),
    /// Announced; `Some(tag)` while a refresh waits for the friend list.
    On(Option<u32>),
}

#[derive(Clone, PartialEq)]
struct HostPlan {
    me: Me,
    relay: String,
    secret: String,
    item: Option<SharedItem>,
    tag: u32,
    server: String,
    visible: bool,
    servers: Vec<ServerInfo>,
}

/// Entering a friend's session.
#[derive(Clone, Debug, PartialEq)]
struct FriendPlan {
    me: Me,
    relay: String,
    room: String,
    stage: FriendStage,
    /// A listing was already made again for this join (an expired ticket).
    refreshed: bool,
}

#[derive(Clone, PartialEq)]
enum FriendStage {
    /// The ticket had expired: a listing is under way, and the session is looked up again after it.
    Refresh,
    Key { tag: u32, session: FriendSession },
    Derive { tag: u32, session: FriendSession, payload: GrantPayload },
}

impl fmt::Debug for FriendStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FriendStage::Refresh => f.write_str("Refresh"),
            FriendStage::Key { tag, session } => f.debug_struct("Key").field("tag", tag).field("session", session).finish(),
            FriendStage::Derive { tag, session, .. } => f.debug_struct("Derive").field("tag", tag).field("session", session).finish_non_exhaustive(),
        }
    }
}

/// One server of a friends' listing.
#[derive(Clone, Debug, PartialEq)]
struct ListServer {
    id: String,
    search: RelaySearch,
    /// The `/rooms` request tag of each relay of `search`.
    asked: Vec<u32>,
    stage: ListStage,
}

#[derive(Clone, Debug, PartialEq)]
enum ListStage {
    Search,
    Asking(u32, String),
    Done(Vec<FriendSession>),
}

#[derive(Clone, PartialEq)]
struct JoinPlan {
    me: Me,
    code: String,
    secret: String,
    servers: Vec<ServerInfo>,
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
    FriendJoin(FriendPlan),
    Live(Live),
}

/// What a `Seal` was for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SealFor {
    Send(u32),
    /// The `bye` of a session left: send it, then close the socket.
    Farewell(u32),
}

// ---- Debug without secrets. An invitation code IS the session's secret, the random bytes of a
// host become it, and a key opens every message: none of them is printed, like `SessionKeys`.

const HIDDEN: &str = "<redacted>";

impl fmt::Debug for Notice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Notice::Opened { full_code, host, .. } => {
                f.debug_struct("Opened").field("code", &HIDDEN).field("full_code", &full_code.as_ref().map(|_| HIDDEN)).field("host", host).finish()
            }
            Notice::Status(s) => f.debug_tuple("Status").field(s).finish(),
            Notice::Action { name, action } => f.debug_struct("Action").field("name", name).field("action", action).finish(),
            Notice::HostChanged { host } => f.debug_struct("HostChanged").field("host", host).finish(),
            Notice::Item(i) => f.debug_tuple("Item").field(i).finish(),
            Notice::HostFailed => f.write_str("HostFailed"),
            Notice::JoinFailed(e) => f.debug_tuple("JoinFailed").field(e).finish(),
            Notice::Left => f.write_str("Left"),
            Notice::Visible(v) => f.debug_tuple("Visible").field(v).finish(),
            Notice::Listed(l) => f.debug_tuple("Listed").field(l).finish(),
            Notice::FriendSessions(list) => f.debug_tuple("FriendSessions").field(list).finish(),
        }
    }
}

impl fmt::Debug for TogetherEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TogetherEvent::Tick { dt_us } => f.debug_struct("Tick").field("dt_us", dt_us).finish(),
            TogetherEvent::Host { me, relay, server, item, random, visible, servers } => f
                .debug_struct("Host")
                .field("me", me)
                .field("relay", relay)
                .field("server", server)
                .field("item", item)
                .field("random", &format_args!("<{} bytes>", random.len()))
                .field("visible", visible)
                .field("servers", servers)
                .finish(),
            TogetherEvent::Join { me, relay, server, servers, .. } => {
                f.debug_struct("Join").field("me", me).field("code", &HIDDEN).field("relay", relay).field("server", server).field("servers", servers).finish()
            }
            TogetherEvent::Derived { tag, keys } => f.debug_struct("Derived").field("tag", tag).field("keys", keys).finish(),
            TogetherEvent::RoomsAnswered { tag, body } => f.debug_struct("RoomsAnswered").field("tag", tag).field("body", body).finish(),
            TogetherEvent::SocketOpened { conn } => f.debug_struct("SocketOpened").field("conn", conn).finish(),
            TogetherEvent::SocketClosed { conn } => f.debug_struct("SocketClosed").field("conn", conn).finish(),
            TogetherEvent::SocketBinary { conn, bytes } => f.debug_struct("SocketBinary").field("conn", conn).field("bytes", &bytes.len()).finish(),
            TogetherEvent::Sealed { tag, bytes } => f.debug_struct("Sealed").field("tag", tag).field("bytes", &bytes.len()).finish(),
            TogetherEvent::Opened { tag, plain } => f.debug_struct("Opened").field("tag", tag).field("plain", &plain.as_ref().map(Vec::len)).finish(),
            TogetherEvent::Attach { item, obs } => f.debug_struct("Attach").field("item", item).field("obs", obs).finish(),
            TogetherEvent::Player(o) => f.debug_tuple("Player").field(o).finish(),
            TogetherEvent::Detach => f.write_str("Detach"),
            TogetherEvent::ShareTitle(i) => f.debug_tuple("ShareTitle").field(i).finish(),
            TogetherEvent::Leave => f.write_str("Leave"),
            TogetherEvent::SetVisible(v) => f.debug_tuple("SetVisible").field(v).finish(),
            TogetherEvent::SocketText { conn, text } => f.debug_struct("SocketText").field("conn", conn).field("bytes", &text.len()).finish(),
            TogetherEvent::FriendsAnswered { tag, answer } => f.debug_struct("FriendsAnswered").field("tag", tag).field("answer", answer).finish(),
            TogetherEvent::FriendIdsRead { tag, ids } => f.debug_struct("FriendIdsRead").field("tag", tag).field("ids", &ids.as_ref().map(Vec::len)).finish(),
            TogetherEvent::GrantSealed { tag, sealed } => f.debug_struct("GrantSealed").field("tag", tag).field("sealed", &sealed.is_some()).finish(),
            TogetherEvent::RefreshFriends { servers } => f.debug_struct("RefreshFriends").field("servers", servers).finish(),
            TogetherEvent::JoinFriend { me, relay, room } => f.debug_struct("JoinFriend").field("me", me).field("relay", relay).field("room", room).finish(),
            TogetherEvent::FriendKey { tag, answer } => {
                f.debug_struct("FriendKey").field("tag", tag).field("answer", &answer.as_ref().map(|_| HIDDEN)).finish()
            }
        }
    }
}

impl fmt::Debug for TogetherFx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TogetherFx::Derive { tag, server, iterations, .. } => {
                f.debug_struct("Derive").field("tag", tag).field("secret", &HIDDEN).field("server", server).field("iterations", iterations).finish()
            }
            TogetherFx::HttpRooms { tag, url, timeout_ms } => f.debug_struct("HttpRooms").field("tag", tag).field("url", url).field("timeout_ms", timeout_ms).finish(),
            TogetherFx::OpenSocket { conn, url } => f.debug_struct("OpenSocket").field("conn", conn).field("url", url).finish(),
            TogetherFx::CloseSocket { conn } => f.debug_struct("CloseSocket").field("conn", conn).finish(),
            TogetherFx::SendBinary { conn, bytes } => f.debug_struct("SendBinary").field("conn", conn).field("bytes", &bytes.len()).finish(),
            TogetherFx::Seal { tag, plain, .. } => f.debug_struct("Seal").field("tag", tag).field("key", &HIDDEN).field("plain", plain).finish(),
            TogetherFx::Open { tag, bytes, .. } => f.debug_struct("Open").field("tag", tag).field("key", &HIDDEN).field("bytes", &bytes.len()).finish(),
            TogetherFx::Player(c) => f.debug_tuple("Player").field(c).finish(),
            TogetherFx::Notice(n) => f.debug_tuple("Notice").field(n).finish(),
            // A control message can carry a ticket or a grant: its length only.
            TogetherFx::SendText { conn, text } => f.debug_struct("SendText").field("conn", conn).field("bytes", &text.len()).finish(),
            TogetherFx::AskFriends { tag, relay, server } => f.debug_struct("AskFriends").field("tag", tag).field("relay", relay).field("server", server).finish(),
            TogetherFx::ReadFriendIds { tag } => f.debug_struct("ReadFriendIds").field("tag", tag).finish(),
            TogetherFx::SealGrant { tag, .. } => f.debug_struct("SealGrant").field("tag", tag).finish_non_exhaustive(),
            TogetherFx::RequestKey { tag, relay, room, .. } => f.debug_struct("RequestKey").field("tag", tag).field("relay", relay).field("room", room).finish_non_exhaustive(),
        }
    }
}

impl fmt::Debug for View {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("View")
            .field("code", &HIDDEN)
            .field("host", &self.host)
            .field("status", &self.status)
            .field("item", &self.item)
            .field("peers", &self.peers)
            .field("position", &self.position)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for Live {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Live")
            .field("me", &self.me)
            .field("relay", &self.relay)
            .field("keys", &self.keys)
            .field("code", &HIDDEN)
            .field("host", &self.host)
            .field("host_claim", &self.host_claim)
            .field("status", &self.status)
            .field("peers", &self.peers)
            .field("conn", &self.conn)
            .field("visible", &self.visible)
            .field("announce", &self.announce)
            .field("listed", &self.listed)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for HostPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostPlan").field("me", &self.me).field("relay", &self.relay).field("secret", &HIDDEN).field("tag", &self.tag).field("visible", &self.visible).finish_non_exhaustive()
    }
}

impl fmt::Debug for JoinPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JoinPlan")
            .field("me", &self.me)
            .field("code", &HIDDEN)
            .field("ids", &self.ids)
            .field("relays", &self.relays)
            .field("derived", &self.derived)
            .field("asked", &self.asked)
            .finish_non_exhaustive()
    }
}

/// The parts of the state every phase shares, moved out of the machine for one transition.
struct Desk {
    seals: BTreeMap<u32, SealFor>,
    opens: BTreeMap<u32, u32>,
    known: KnownRelays,
    tags: u32,
    conns: u32,
    now_us: u64,
    backoff: RelayBackoff,
    friend_ids: Option<(Vec<String>, u64)>,
    grants: BTreeMap<u32, (u32, String)>,
    listing: Vec<ListServer>,
    list_servers: Vec<ServerInfo>,
    sessions: Vec<FriendSession>,
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
    /// Relays that failed or were useless for the friends' listing (the web's `relayBackoff`).
    backoff: RelayBackoff,
    /// This account's friend allow-list and when it was read (cached [`FRIENDS_TTL_US`]).
    friend_ids: Option<(Vec<String>, u64)>,
    /// Grants being sealed: tag -> (socket, requester).
    grants: BTreeMap<u32, (u32, String)>,
    /// The friends' listing under way (empty when none is).
    listing: Vec<ListServer>,
    /// The servers the last listing asked.
    list_servers: Vec<ServerInfo>,
    /// The friends' sessions the last listing found.
    sessions: Vec<FriendSession>,
}

impl Default for TogetherSession {
    fn default() -> Self {
        Self::new()
    }
}

impl TogetherSession {
    pub fn new() -> Self {
        TogetherSession {
            phase: Phase::Idle,
            seals: BTreeMap::new(),
            opens: BTreeMap::new(),
            known: KnownRelays::new(),
            tags: 0,
            conns: 0,
            now_us: 0,
            backoff: RelayBackoff::default(),
            friend_ids: None,
            grants: BTreeMap::new(),
            listing: Vec::new(),
            list_servers: Vec::new(),
            sessions: Vec::new(),
        }
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
            backoff: std::mem::take(&mut self.backoff),
            friend_ids: self.friend_ids.take(),
            grants: std::mem::take(&mut self.grants),
            listing: std::mem::take(&mut self.listing),
            list_servers: std::mem::take(&mut self.list_servers),
            sessions: std::mem::take(&mut self.sessions),
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
        self.backoff = desk.backoff;
        self.friend_ids = desk.friend_ids;
        self.grants = desk.grants;
        self.listing = desk.listing;
        self.list_servers = desk.list_servers;
        self.sessions = desk.sessions;
        handled
    }

    pub fn step(&mut self, ev: TogetherEvent, fx: &mut Vec<TogetherFx>) -> Handled {
        self.transition(ev, fx)
    }

    /// The live session, if any.
    pub fn view(&self) -> Option<View> {
        match &self.phase {
            Phase::Live(live) => Some(view_of(live)),
            Phase::Idle | Phase::Deriving(_) | Phase::Joining(_) | Phase::FriendJoin(_) => None,
        }
    }

    /// A host start or a join is under way.
    pub fn busy(&self) -> bool {
        match &self.phase {
            Phase::Deriving(_) | Phase::Joining(_) | Phase::FriendJoin(_) => true,
            Phase::Idle | Phase::Live(_) => false,
        }
    }

    /// The friends' sessions the last listing found.
    pub fn friend_sessions(&self) -> &[FriendSession] {
        &self.sessions
    }

    /// A friends' listing is under way.
    pub fn listing(&self) -> bool {
        !self.listing.is_empty()
    }

    /// The machine's clock (the sum of the ticks' `dt_us`).
    pub fn now_us(&self) -> u64 {
        self.now_us
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
        visible: live.visible,
        listed: live.listed,
    }
}

fn reduce(phase: Phase, ev: TogetherEvent, desk: &mut Desk, fx: &mut Vec<TogetherFx>) -> (Phase, Handled) {
    match ev {
        TogetherEvent::Tick { dt_us } => {
            desk.now_us = desk.now_us.saturating_add(dt_us);
            desk.backoff.advance(dt_us / 1000);
            match phase {
                Phase::Live(mut live) => {
                    on_tick(&mut live, dt_us, desk, fx);
                    (Phase::Live(live), Handled::Yes)
                }
                other @ (Phase::Idle | Phase::Deriving(_) | Phase::Joining(_) | Phase::FriendJoin(_)) => (other, Handled::Yes),
            }
        }
        TogetherEvent::Host { me, relay, server, item, random, visible, servers } => {
            leave(phase, desk, fx);
            let relay = relay_url(&relay);
            let secret = secret_from_bytes(&random);
            let (false, Some(secret)) = (relay.is_empty(), secret) else {
                fx.push(TogetherFx::Notice(Notice::HostFailed));
                return (Phase::Idle, Handled::Yes);
            };
            let tag = desk.tag();
            let server = server.unwrap_or_default();
            fx.push(TogetherFx::Derive { tag, secret: secret.clone(), server: server.clone(), iterations: INVITE_ITERATIONS });
            // The web: visible only with a server to salt (and so to list on).
            let visible = visible && !server.is_empty();
            (Phase::Deriving(HostPlan { me, relay, secret, item, tag, server, visible, servers }), Handled::Yes)
        }
        TogetherEvent::Join { me, code, relay, server, servers } => {
            leave(phase, desk, fx);
            join(me, &code, &relay, server, &servers, desk, fx)
        }
        TogetherEvent::Derived { tag, keys } => match phase {
            Phase::Deriving(plan) if plan.tag == tag => {
                let code = format_invite(&plan.secret, None);
                let full = format_invite(&plan.secret, Some(&plan.relay));
                let grant = (!plan.server.is_empty()).then(|| (plan.secret.clone(), plan.server.clone()));
                let opening = Opening {
                    me: plan.me,
                    relay: plan.relay,
                    keys,
                    code,
                    full_code: Some(full),
                    host: true,
                    item: plan.item,
                    grant,
                    visible: plan.visible,
                    servers: plan.servers,
                };
                (open(opening, desk, fx), Handled::Yes)
            }
            Phase::FriendJoin(FriendPlan { me, relay, room, stage: FriendStage::Derive { tag: t, session, payload }, .. }) if t == tag => {
                if keys.room != room {
                    fx.push(TogetherFx::Notice(Notice::JoinFailed(JoinError::Failed)));
                    return (Phase::Idle, Handled::Yes);
                }
                let opening = Opening {
                    me,
                    relay,
                    keys,
                    code: format_invite(&payload.secret, None),
                    full_code: None,
                    host: false,
                    item: Some(session.item()),
                    grant: Some((payload.secret.clone(), payload.server.clone())),
                    visible: true,
                    servers: desk.list_servers.clone(),
                };
                let mut phase = open(opening, desk, fx);
                if let (Phase::Live(live), Some(st)) = (&mut phase, payload.state) {
                    live.sync.adopt(st.time, st.playing, Stamp { c: st.c, by: st.by });
                }
                (phase, Handled::Yes)
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
            other @ (Phase::Idle | Phase::Deriving(_) | Phase::FriendJoin(_) | Phase::Live(_)) => (other, Handled::No),
        },
        TogetherEvent::RoomsAnswered { tag, body } if listing_asked(desk, tag) => {
            list_rooms_answered(tag, body.as_deref(), desk);
            (settle_listing(phase, desk, fx), Handled::Yes)
        }
        TogetherEvent::RoomsAnswered { tag, body } => match phase {
            Phase::Live(mut live) if live.announce == Announce::Rooms(tag) => {
                on_announce_rooms(&mut live, body.as_deref(), desk, fx);
                (Phase::Live(live), Handled::Yes)
            }
            Phase::Joining(mut plan) => {
                let Some(index) = plan.asked.iter().position(|t| *t == tag) else {
                    return (Phase::Joining(plan), Handled::No);
                };
                if let Some(search) = plan.search.as_mut() {
                    search.answer(index, body.as_deref(), &mut desk.known);
                }
                (settle_join(plan, desk, fx), Handled::Yes)
            }
            other @ (Phase::Idle | Phase::Deriving(_) | Phase::FriendJoin(_) | Phase::Live(_)) => (other, Handled::No),
        },
        TogetherEvent::SocketOpened { conn } => match phase {
            Phase::Live(mut live) if live.conn == conn => {
                live.retry = 0;
                live.status = Status::Open;
                // A new connection: nothing announced or handed over on it yet.
                live.announce = Announce::Off;
                live.granted.clear();
                fx.push(TogetherFx::Notice(Notice::Status(Status::Open)));
                // Arriving or returning: ask everyone's state now.
                let tick = tick_of(&live, true);
                send(&live, Message::Tick(tick), desk, fx);
                announce(&mut live, false, desk, fx);
                (Phase::Live(live), Handled::Yes)
            }
            other @ (Phase::Idle | Phase::Deriving(_) | Phase::Joining(_) | Phase::FriendJoin(_) | Phase::Live(_)) => (other, Handled::No),
        },
        TogetherEvent::SocketClosed { conn } => match phase {
            Phase::Live(mut live) if live.conn == conn && live.reconnect_in_us.is_none() => {
                live.status = Status::Connecting;
                live.announce = Announce::Off;
                fx.push(TogetherFx::Notice(Notice::Status(Status::Connecting)));
                // Network cut or relay restarted: reconnect, more and more patiently.
                let delay = RECONNECT_BASE_US.saturating_mul(1u64 << live.retry.min(20)).min(RECONNECT_CEILING_US);
                live.retry = live.retry.saturating_add(1);
                live.reconnect_in_us = Some(delay);
                (Phase::Live(live), Handled::Yes)
            }
            other @ (Phase::Idle | Phase::Deriving(_) | Phase::Joining(_) | Phase::FriendJoin(_) | Phase::Live(_)) => (other, Handled::No),
        },
        TogetherEvent::SocketBinary { conn, bytes } => match phase {
            Phase::Live(live) if live.conn == conn => {
                let tag = desk.tag();
                desk.opens.insert(tag, conn);
                fx.push(TogetherFx::Open { tag, key: live.keys.key, bytes });
                (Phase::Live(live), Handled::Yes)
            }
            other @ (Phase::Idle | Phase::Deriving(_) | Phase::Joining(_) | Phase::FriendJoin(_) | Phase::Live(_)) => (other, Handled::No),
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
                other @ (Phase::Idle | Phase::Deriving(_) | Phase::Joining(_) | Phase::FriendJoin(_) | Phase::Live(_)) => (other, if from.is_some() { Handled::Yes } else { Handled::No }),
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
            other @ (Phase::Idle | Phase::Deriving(_) | Phase::Joining(_) | Phase::FriendJoin(_)) => (other, Handled::No),
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
                    announce(&mut live, false, desk, fx);
                }
                (Phase::Live(live), Handled::Yes)
            }
            other @ (Phase::Idle | Phase::Deriving(_) | Phase::Joining(_) | Phase::FriendJoin(_) | Phase::Live(_)) => (other, Handled::No),
        },
        TogetherEvent::Detach => match phase {
            Phase::Live(mut live) if live.sync.attached() => {
                live.sync.detach();
                live.playing_item = None;
                let tick = tick_of(&live, false);
                send(&live, Message::Tick(tick), desk, fx);
                (Phase::Live(live), Handled::Yes)
            }
            other @ (Phase::Idle | Phase::Deriving(_) | Phase::Joining(_) | Phase::FriendJoin(_) | Phase::Live(_)) => (other, Handled::No),
        },
        TogetherEvent::ShareTitle(item) => match phase {
            Phase::Live(mut live) if live.host => {
                live.item = Some(item.clone());
                fx.push(TogetherFx::Notice(Notice::Item(item)));
                let tick = tick_of(&live, false);
                send(&live, Message::Tick(tick), desk, fx);
                announce(&mut live, false, desk, fx);
                (Phase::Live(live), Handled::Yes)
            }
            other @ (Phase::Idle | Phase::Deriving(_) | Phase::Joining(_) | Phase::FriendJoin(_) | Phase::Live(_)) => (other, Handled::No),
        },
        TogetherEvent::Leave => match phase {
            Phase::Idle => (Phase::Idle, Handled::No),
            other @ (Phase::Deriving(_) | Phase::Joining(_) | Phase::FriendJoin(_) | Phase::Live(_)) => {
                leave(other, desk, fx);
                (Phase::Idle, Handled::Yes)
            }
        },
        TogetherEvent::SetVisible(on) => match phase {
            Phase::Live(mut live) => {
                set_visible(&mut live, on, desk, fx);
                (Phase::Live(live), Handled::Yes)
            }
            other @ (Phase::Idle | Phase::Deriving(_) | Phase::Joining(_) | Phase::FriendJoin(_)) => (other, Handled::No),
        },
        TogetherEvent::SocketText { conn, text } => match phase {
            Phase::Live(mut live) if live.conn == conn => {
                if let Some(c) = Control::read(&text) {
                    on_control(&mut live, c, desk, fx);
                }
                (Phase::Live(live), Handled::Yes)
            }
            other @ (Phase::Idle | Phase::Deriving(_) | Phase::Joining(_) | Phase::FriendJoin(_) | Phase::Live(_)) => (other, Handled::No),
        },
        TogetherEvent::FriendsAnswered { tag, answer } if desk.listing.iter().any(|l| matches!(&l.stage, ListStage::Asking(t, _) if *t == tag)) => {
            list_friends_answered(tag, answer, desk);
            (settle_listing(phase, desk, fx), Handled::Yes)
        }
        TogetherEvent::FriendsAnswered { tag, answer } => match phase {
            Phase::Live(mut live) if live.announce == Announce::Friends(tag) => {
                on_announce_friends(&mut live, answer, desk, fx);
                (Phase::Live(live), Handled::Yes)
            }
            other @ (Phase::Idle | Phase::Deriving(_) | Phase::Joining(_) | Phase::FriendJoin(_) | Phase::Live(_)) => (other, Handled::No),
        },
        TogetherEvent::FriendIdsRead { tag, ids } => {
            if let Some(ids) = &ids {
                desk.friend_ids = Some((ids.clone(), desk.now_us));
            }
            match phase {
                Phase::Live(mut live) if matches!(&live.announce, Announce::Ids(t, _) | Announce::On(Some(t)) if *t == tag) => {
                    on_friend_ids(&mut live, ids, desk, fx);
                    (Phase::Live(live), Handled::Yes)
                }
                other @ (Phase::Idle | Phase::Deriving(_) | Phase::Joining(_) | Phase::FriendJoin(_) | Phase::Live(_)) => (other, Handled::No),
            }
        }
        TogetherEvent::GrantSealed { tag, sealed } => {
            let Some((conn, peer)) = desk.grants.remove(&tag) else { return (phase, Handled::No) };
            if let (Phase::Live(live), Some((public, sealed))) = (&phase, sealed) {
                if live.conn == conn && live.status == Status::Open {
                    fx.push(TogetherFx::SendText { conn, text: key_grant_text(&peer, &public, &sealed) });
                }
            }
            (phase, Handled::Yes)
        }
        TogetherEvent::RefreshFriends { servers } => {
            start_listing(servers, desk, fx);
            (settle_listing(phase, desk, fx), Handled::Yes)
        }
        TogetherEvent::JoinFriend { me, relay, room } => {
            leave(phase, desk, fx);
            let plan = FriendPlan { me, relay, room, stage: FriendStage::Refresh, refreshed: false };
            (enter_friend(plan, desk, fx), Handled::Yes)
        }
        TogetherEvent::FriendKey { tag, answer } => match phase {
            Phase::FriendJoin(FriendPlan { me, relay, room, stage: FriendStage::Key { tag: t, session }, refreshed }) if t == tag => match answer {
                Ok(payload) => {
                    let tag = desk.tag();
                    fx.push(TogetherFx::Derive { tag, secret: payload.secret.clone(), server: payload.server.clone(), iterations: INVITE_ITERATIONS });
                    (Phase::FriendJoin(FriendPlan { me, relay, room, stage: FriendStage::Derive { tag, session, payload }, refreshed }), Handled::Yes)
                }
                Err(e) => {
                    fx.push(TogetherFx::Notice(Notice::JoinFailed(e)));
                    (Phase::Idle, Handled::Yes)
                }
            },
            other @ (Phase::Idle | Phase::Deriving(_) | Phase::Joining(_) | Phase::FriendJoin(_) | Phase::Live(_)) => (other, Handled::No),
        },
    }
}

/// The web's `card()`: title and position of the session (and this account's name, which the relay
/// replaces with the one plex.tv gives it). Nothing about the device, the address or the stream.
fn card_of(live: &Live) -> Option<FriendCard> {
    let it = live.item.as_ref().or(live.playing_item.as_ref())?;
    let (time, playing) = here(live);
    Some(FriendCard {
        name: live.me.name.clone(),
        title: it.title.clone(),
        sub: it.sub.clone(),
        kind: Some(it.kind.clone()),
        guid: Some(it.guid.clone()).filter(|g| !g.is_empty()),
        key: it.key.clone(),
        time,
        playing,
    })
}

/// Where this device is: its player's position when attached, else the session's.
fn here(live: &Live) -> (f64, bool) {
    match (live.sync.player_time(), live.sync.player_paused()) {
        (Some(t), Some(paused)) => (t, !paused),
        _ => (live.sync.position().unwrap_or(0.0), live.sync.reference().is_some_and(|r| r.playing)),
    }
}

/// The friend allow-list, if read less than [`FRIENDS_TTL_US`] ago: `(ids, read at)`.
fn fresh_friends(desk: &Desk) -> Option<(Vec<String>, u64)> {
    desk.friend_ids.clone().filter(|(_, at)| desk.now_us.saturating_sub(*at) < FRIENDS_TTL_US)
}

fn set_listed(live: &mut Live, listed: Option<Listed>, fx: &mut Vec<TogetherFx>) {
    if live.listed != listed {
        live.listed = listed;
        fx.push(TogetherFx::Notice(Notice::Listed(listed)));
    }
}

/// The relay does not list the session: a stable "no", asked again later and later.
fn not_listed(live: &mut Live, now_us: u64, fx: &mut Vec<TogetherFx>) {
    live.announce = Announce::Off;
    let delay = if live.list_retry.1 > 0 { live.list_retry.1.saturating_mul(2).min(LIST_RETRY_CEILING_US) } else { LIST_RETRY_BASE_US };
    live.list_retry = (now_us.saturating_add(delay), delay);
    set_listed(live, Some(Listed::No), fx);
}

/// The web's `announce`: host of a visible session, announce it to the relay. The first time on a
/// connection it proves access (the relay's `/rooms` says it lists to friends only and is the
/// host's server's, then `/friends` with the server's token gives a ticket) and sends the friend
/// list; afterwards the card is re-sent on a change (at most every 5 s) and its position on
/// `refresh`, with the friend list re-read (cached 2 min) and sent when it changed or was re-read.
fn announce(live: &mut Live, refresh: bool, desk: &mut Desk, fx: &mut Vec<TogetherFx>) {
    let Some((_, server)) = live.grant.clone() else { return };
    if !live.host || !live.visible || live.keys.owner.is_none() || live.status != Status::Open {
        return;
    }
    let Some(card) = card_of(live) else { return };
    let now = desk.now_us;
    match &live.announce {
        Announce::Rooms(_) | Announce::Friends(_) | Announce::Ids(..) | Announce::On(Some(_)) => {}
        Announce::On(None) => {
            let sig = card.sig();
            if !refresh && (sig == live.card_sent.0 || now.saturating_sub(live.card_sent.1) < CARD_EVERY_US) {
                return;
            }
            live.card_sent = (sig, now);
            match fresh_friends(desk) {
                Some((ids, at)) => refresh_announce(live, Some(ids), at, now, fx),
                None => {
                    let tag = desk.tag();
                    live.announce = Announce::On(Some(tag));
                    fx.push(TogetherFx::ReadFriendIds { tag });
                }
            }
        }
        Announce::Off => {
            if now < live.list_retry.0 {
                return;
            }
            if live.listed.is_none() {
                set_listed(live, Some(Listed::Pending), fx);
            }
            // The server's token goes only to a relay found on that server's own public names.
            let mine: Vec<ServerInfo> = live.servers.iter().filter(|r| r.id == server).cloned().collect();
            if mine.is_empty() || !relay_discoverable(&live.relay, &mine) {
                return not_listed(live, now, fx);
            }
            let tag = desk.tag();
            live.announce = Announce::Rooms(tag);
            fx.push(TogetherFx::HttpRooms { tag, url: rooms_url(&live.relay, &[]), timeout_ms: ROOMS_TIMEOUT_MS });
        }
    }
}

/// A refresh's friend list is in (`None`: plex.tv did not answer, the announce is released: never
/// a list that may be stale).
fn refresh_announce(live: &mut Live, ids: Option<Vec<String>>, read_at: u64, now_us: u64, fx: &mut Vec<TogetherFx>) {
    live.announce = Announce::On(None);
    match friends_for_announce(ids.as_deref(), &live.friends_sent, read_at) {
        None => {
            fx.push(TogetherFx::SendText { conn: live.conn, text: control_text("release") });
            not_listed(live, now_us, fx);
        }
        Some((friends, sent)) => {
            live.friends_sent = sent;
            let proof = live.keys.owner.clone().unwrap_or_default();
            if let Some(card) = card_of(live) {
                fx.push(TogetherFx::SendText { conn: live.conn, text: announce_text(None, &proof, &card, friends.as_deref()) });
            }
        }
    }
}

fn on_announce_rooms(live: &mut Live, body: Option<&str>, desk: &mut Desk, fx: &mut Vec<TogetherFx>) {
    let server = live.grant.as_ref().map(|(_, s)| s.clone()).unwrap_or_default();
    let hit = body.and_then(RoomsAnswer::read).filter(|a| a.friends && a.server.as_deref() == Some(server.as_str()));
    if hit.is_none() {
        return not_listed(live, desk.now_us, fx);
    }
    let tag = desk.tag();
    live.announce = Announce::Friends(tag);
    fx.push(TogetherFx::AskFriends { tag, relay: live.relay.clone(), server });
}

fn on_announce_friends(live: &mut Live, answer: Option<FriendsAnswer>, desk: &mut Desk, fx: &mut Vec<TogetherFx>) {
    let server = live.grant.as_ref().map(|(_, s)| s.clone()).unwrap_or_default();
    let Some(answer) = answer.filter(|a| a.lists_for(&server)) else { return not_listed(live, desk.now_us, fx) };
    match fresh_friends(desk) {
        Some((ids, at)) => first_announce(live, answer, Some(ids), at, desk.now_us, fx),
        None => {
            let tag = desk.tag();
            live.announce = Announce::Ids(tag, answer);
            fx.push(TogetherFx::ReadFriendIds { tag });
        }
    }
}

fn on_friend_ids(live: &mut Live, ids: Option<Vec<String>>, desk: &mut Desk, fx: &mut Vec<TogetherFx>) {
    let now = desk.now_us;
    match std::mem::replace(&mut live.announce, Announce::Off) {
        Announce::Ids(_, answer) => first_announce(live, answer, ids, now, now, fx),
        Announce::On(Some(_)) => refresh_announce(live, ids, now, now, fx),
        other @ (Announce::Off | Announce::Rooms(_) | Announce::Friends(_) | Announce::On(None)) => live.announce = other,
    }
}

/// The first announce on this connection: ticket, proof, card and the friend list (without which
/// nothing is announced: never a session visible to everyone).
fn first_announce(live: &mut Live, answer: FriendsAnswer, ids: Option<Vec<String>>, read_at: u64, now_us: u64, fx: &mut Vec<TogetherFx>) {
    let Some(ids) = ids else { return not_listed(live, now_us, fx) };
    let (Some(card), true, true) = (card_of(live), live.visible, live.host) else {
        live.announce = Announce::Off;
        return;
    };
    let proof = live.keys.owner.clone().unwrap_or_default();
    live.announce = Announce::On(None);
    live.card_sent = (card.sig(), now_us);
    live.friends_sent = FriendsSent { ids: ids.join(","), at: read_at };
    fx.push(TogetherFx::SendText { conn: live.conn, text: announce_text(Some(&answer.ticket), &proof, &card, Some(&ids)) });
}

/// A host that gives the hand back releases its announce.
fn step_down(live: &mut Live, fx: &mut Vec<TogetherFx>) {
    if matches!(live.announce, Announce::On(_)) {
        fx.push(TogetherFx::SendText { conn: live.conn, text: control_text("release") });
    }
    live.announce = Announce::Off;
    live.host = false;
    set_listed(live, None, fx);
    fx.push(TogetherFx::Notice(Notice::HostChanged { host: false }));
}

/// The web's `setVisible`.
fn set_visible(live: &mut Live, on: bool, desk: &mut Desk, fx: &mut Vec<TogetherFx>) {
    if live.visible != on {
        live.visible = on;
        fx.push(TogetherFx::Notice(Notice::Visible(on)));
    }
    if !on {
        set_listed(live, None, fx);
    }
    if !live.host {
        return;
    }
    if on {
        live.list_retry.0 = 0;
        announce(live, false, desk, fx);
    } else {
        if live.status == Status::Open {
            fx.push(TogetherFx::SendText { conn: live.conn, text: control_text("unlist") });
        }
        live.announce = Announce::Off;
    }
    let tick = tick_of(live, false);
    send(live, Message::Tick(tick), desk, fx);
}

/// A text message of the relay on the session's socket (the web's `onControl`).
fn on_control(live: &mut Live, c: Control, desk: &mut Desk, fx: &mut Vec<TogetherFx>) {
    match c {
        Control::Announced => {
            live.list_retry = (0, 0);
            set_listed(live, Some(Listed::Yes), fx);
        }
        Control::Error(e) => {
            if UNLISTED_ERRORS.contains(&e.as_str()) {
                not_listed(live, desk.now_us, fx);
            }
        }
        Control::KeyRequest { peer, public } => hand_over(live, peer, public, desk, fx),
        Control::KeyGrant { .. } => {}
    }
}

/// A friend the relay admitted asks for the key: the invitation's secret and where the session
/// stands, sealed for its ephemeral key alone. Once per requester, a few per 10 s.
fn hand_over(live: &mut Live, peer: String, public: String, desk: &mut Desk, fx: &mut Vec<TogetherFx>) {
    let Some((secret, server)) = live.grant.clone() else { return };
    if !live.host || !live.visible || !matches!(live.announce, Announce::On(_)) {
        return;
    }
    let now = desk.now_us;
    live.grant_times.retain(|at| now.saturating_sub(*at) < GRANT_WINDOW_US);
    if live.granted.contains(&peer) || live.grant_times.len() >= GRANTS_PER_WINDOW {
        return;
    }
    live.granted.push(peer.clone());
    live.grant_times.push(now);
    let (time, playing) = here(live);
    let stamp = live.sync.stamp();
    let payload = GrantPayload { secret, server, state: Some(GrantState { time, playing, c: stamp.c, by: stamp.by }) };
    let tag = desk.tag();
    desk.grants.insert(tag, (live.conn, peer));
    fx.push(TogetherFx::SealGrant { tag, request_pub: public, payload });
}

// ---- The friends' listing (the web's `refreshFriendSessions`) and entering one (`joinFriend`).

fn listing_asked(desk: &Desk, tag: u32) -> bool {
    desk.listing.iter().any(|l| matches!(l.stage, ListStage::Search) && l.asked.contains(&tag))
}

/// For each server this account sees (at most [`MAX_ROOM_LOOKUP`]): its relay, searched on its
/// public names and checked to list to friends only and to be that server's, BEFORE any token
/// leaves; then the list, asked with THAT server's token. A listing under way is replaced.
fn start_listing(servers: Vec<ServerInfo>, desk: &mut Desk, fx: &mut Vec<TogetherFx>) {
    desk.listing.clear();
    let list: Vec<ServerInfo> = servers.into_iter().take(MAX_ROOM_LOOKUP).collect();
    for res in &list {
        let search = RelaySearch::new(&relay_candidates(std::slice::from_ref(res)), Some(&res.id), &desk.known, &desk.backoff);
        let mut asked = Vec::new();
        for relay in search.relays() {
            let tag = desk.tag();
            asked.push(tag);
            fx.push(TogetherFx::HttpRooms { tag, url: rooms_url(relay, &[]), timeout_ms: ROOMS_TIMEOUT_MS });
        }
        desk.listing.push(ListServer { id: res.id.clone(), search, asked, stage: ListStage::Search });
    }
    desk.list_servers = list;
}

fn list_rooms_answered(tag: u32, body: Option<&str>, desk: &mut Desk) {
    let Desk { listing, known, backoff, .. } = desk;
    for l in listing.iter_mut() {
        if let Some(i) = l.asked.iter().position(|t| *t == tag) {
            l.search.answer(i, body, known, backoff);
        }
    }
}

fn list_friends_answered(tag: u32, answer: Option<FriendsAnswer>, desk: &mut Desk) {
    let now = desk.now_us;
    for l in desk.listing.iter_mut() {
        let ListStage::Asking(t, relay) = &l.stage else { continue };
        if *t != tag {
            continue;
        }
        let relay = relay.clone();
        let found = answer.as_ref().filter(|a| a.lists_for(&l.id)).map(|a| {
            let until = now.saturating_add((a.ttl_s * 1e6) as u64).saturating_sub(TICKET_MARGIN_US);
            a.sessions
                .iter()
                .map(|s| FriendSession {
                    relay: relay.clone(),
                    server: l.id.clone(),
                    room: s.room.clone(),
                    card: s.card.clone(),
                    count: s.count,
                    at_us: now.saturating_sub(s.age_ms.saturating_mul(1000)),
                    ticket: a.ticket.clone(),
                    ticket_until_us: until,
                })
                .collect()
        });
        l.stage = ListStage::Done(found.unwrap_or_default());
    }
}

/// Moves each server of the listing on; when every one is done, publishes the sessions (the one
/// followed here left out) and resumes a friend join that waited for a fresh ticket.
fn settle_listing(phase: Phase, desk: &mut Desk, fx: &mut Vec<TogetherFx>) -> Phase {
    if desk.listing.is_empty() {
        return phase;
    }
    let mut asks = Vec::new();
    for l in desk.listing.iter_mut() {
        if !matches!(l.stage, ListStage::Search) {
            continue;
        }
        match l.search.outcome() {
            None => {}
            Some(None) => l.stage = ListStage::Done(Vec::new()),
            Some(Some(found)) => asks.push((l.id.clone(), found.relay)),
        }
    }
    for (id, relay) in asks {
        let tag = desk.tag();
        if let Some(l) = desk.listing.iter_mut().find(|l| l.id == id && matches!(l.stage, ListStage::Search)) {
            l.stage = ListStage::Asking(tag, relay.clone());
            fx.push(TogetherFx::AskFriends { tag, relay, server: id });
        }
    }
    if !desk.listing.iter().all(|l| matches!(l.stage, ListStage::Done(_))) {
        return phase;
    }
    let current = match &phase {
        Phase::Live(live) => Some(live.keys.room.clone()),
        Phase::Idle | Phase::Deriving(_) | Phase::Joining(_) | Phase::FriendJoin(_) => None,
    };
    let mut found = Vec::new();
    for l in std::mem::take(&mut desk.listing) {
        if let ListStage::Done(list) = l.stage {
            found.extend(list.into_iter().filter(|s| Some(&s.room) != current.as_ref()));
        }
    }
    desk.sessions = found.clone();
    fx.push(TogetherFx::Notice(Notice::FriendSessions(found)));
    match phase {
        Phase::FriendJoin(plan) if plan.stage == FriendStage::Refresh => enter_friend(plan, desk, fx),
        other @ (Phase::Idle | Phase::Deriving(_) | Phase::Joining(_) | Phase::FriendJoin(_) | Phase::Live(_)) => other,
    }
}

/// Enter the listed session of `plan` with its ticket while the ticket is valid; an expired one
/// lists again first, once (a session that the fresh listing lacks is not found).
fn enter_friend(mut plan: FriendPlan, desk: &mut Desk, fx: &mut Vec<TogetherFx>) -> Phase {
    let listed = desk.sessions.iter().find(|s| s.room == plan.room && s.relay == plan.relay).cloned();
    match listed {
        Some(session) if desk.now_us <= session.ticket_until_us => {
            let tag = desk.tag();
            fx.push(TogetherFx::RequestKey { tag, relay: session.relay.clone(), room: session.room.clone(), ticket: session.ticket.clone() });
            plan.stage = FriendStage::Key { tag, session };
            Phase::FriendJoin(plan)
        }
        Some(_) if !plan.refreshed && !desk.list_servers.is_empty() => {
            plan.refreshed = true;
            plan.stage = FriendStage::Refresh;
            let servers = desk.list_servers.clone();
            start_listing(servers, desk, fx);
            settle_listing(Phase::FriendJoin(plan), desk, fx)
        }
        Some(_) | None => {
            fx.push(TogetherFx::Notice(Notice::JoinFailed(JoinError::NotFound)));
            Phase::Idle
        }
    }
}

/// Leaves whatever `phase` holds: a live session says `bye` (sealed, then the socket closes).
fn leave(phase: Phase, desk: &mut Desk, fx: &mut Vec<TogetherFx>) {
    let Phase::Live(live) = phase else { return };
    // The host leaves: its announce is released (not made private); only its account may
    // announce the session again.
    if live.host && live.status == Status::Open && matches!(live.announce, Announce::On(_)) {
        fx.push(TogetherFx::SendText { conn: live.conn, text: control_text("release") });
    }
    desk.grants.retain(|_, (conn, _)| *conn != live.conn);
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
                Some(keys) => {
                    let opening = Opening { me, relay, keys, code: text, full_code: None, host: false, item: None, grant: None, visible: false, servers: servers.to_vec() };
                    (open(opening, desk, fx), Handled::Yes)
                }
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
    let plan = JoinPlan { me, code: text, secret, servers: servers.to_vec(), ids, relays, derived, search: None, asked: Vec::new() };
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
        plan.derived.iter().enumerate().find_map(|(i, (_, k))| k.clone().filter(|k| k.room == room).map(|k| (relay.clone(), k, plan.ids[i].clone())))
    });
    match keys {
        Some((relay, keys, server)) => {
            // A participant by code is handed what a friend would be (it may take the hand), and
            // the session is private until the host says otherwise.
            let opening = Opening {
                me: plan.me,
                relay,
                keys,
                code: plan.code,
                full_code: None,
                host: false,
                item: None,
                grant: Some((plan.secret, server)),
                visible: false,
                servers: plan.servers,
            };
            open(opening, desk, fx)
        }
        None => {
            fx.push(TogetherFx::Notice(Notice::JoinFailed(JoinError::NotFound)));
            Phase::Idle
        }
    }
}

/// What a session opens with.
struct Opening {
    me: Me,
    relay: String,
    keys: SessionKeys,
    code: String,
    full_code: Option<String>,
    host: bool,
    item: Option<SharedItem>,
    grant: Option<(String, String)>,
    visible: bool,
    servers: Vec<ServerInfo>,
}

fn open(o: Opening, desk: &mut Desk, fx: &mut Vec<TogetherFx>) -> Phase {
    let Opening { me, relay, keys, code, full_code, host, item, grant, visible, servers } = o;
    // The followed session is no friend's session to list.
    desk.sessions.retain(|s| s.room != keys.room);
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
        visible,
        grant,
        servers,
        announce: Announce::Off,
        listed: None,
        card_sent: (String::new(), 0),
        list_retry: (0, 0),
        friends_sent: FriendsSent::default(),
        granted: Vec::new(),
        grant_times: Vec::new(),
        presence_ticks: 0,
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
    let (time, playing) = here(live);
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
        // Whether the host lists the session to its friends; who would take the hand keeps it.
        vis: live.host.then_some(live.visible),
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
            live.announce = Announce::Off;
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
    live.presence_ticks = live.presence_ticks.wrapping_add(1);
    let now = desk.now_us;
    live.peers.retain(|p| now.saturating_sub(p.seen_us) <= PEER_TIMEOUT_US);
    // The host has gone: the first participant (by id) takes the hand.
    let me = live.me.id.as_str();
    if !live.host && live.status == Status::Open && !live.peers.is_empty() && live.peers.iter().all(|p| !p.host && p.id.as_str() > me) {
        live.host_claim = tick_clock(live.sync.clock());
        live.host = true;
        fx.push(TogetherFx::Notice(Notice::HostChanged { host: true }));
        // It tries to announce it, but the relay lists it only for the original host's account.
        announce(live, false, desk, fx);
    }
    // Host of a visible session: the announced position is refreshed every 15 s.
    if live.host && live.presence_ticks % CARD_REFRESH_TICKS == 0 {
        announce(live, true, desk, fx);
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
                step_down(live, fx);
            }
            if t.host && !live.host {
                if let Some(vis) = t.vis.filter(|v| *v != live.visible) {
                    live.visible = vis;
                    fx.push(TogetherFx::Notice(Notice::Visible(vis)));
                }
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
            let applied = live.sync.on_intent(&i, &mut cmds);
            if applied {
                fx.push(TogetherFx::Notice(Notice::Action { name: i.name.clone(), action: i.action }));
            }
            fx.extend(cmds.into_iter().map(TogetherFx::Player));
            // The card changes only for play/pause (the position follows at the next refresh).
            if applied {
                announce(live, false, desk, fx);
            }
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
                        | TogetherFx::Notice(_)
                        | TogetherFx::SendText { .. }
                        | TogetherFx::AskFriends { .. }
                        | TogetherFx::ReadFriendIds { .. }
                        | TogetherFx::SealGrant { .. }
                        | TogetherFx::RequestKey { .. }) => self.out.push(other),
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
                Phase::Idle | Phase::Deriving(_) | Phase::Joining(_) | Phase::FriendJoin(_) => panic!("not live"),
            }
        }

        fn conn(&self) -> u32 {
            match &self.s.phase {
                Phase::Live(l) => l.conn,
                Phase::Idle | Phase::Deriving(_) | Phase::Joining(_) | Phase::FriendJoin(_) => panic!("not live"),
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
        h.step(TogetherEvent::Host { me: me(id, "Hôte"), relay: "relais.exemple.fr".into(), server: Some("srv-1".into()), item: Some(movie()), random: (0u8..32).collect(), visible: false, servers: Vec::new() });
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
        let room = derive_session(code.trim_start_matches("MPX-"), "srv-1", ROUNDS).room.clone();
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
        d.step(TogetherEvent::Host { me: me("m", "H"), relay: String::new(), server: None, item: None, random: vec![1; 32], visible: false, servers: Vec::new() });
        assert_eq!(notices(&d.take()), [Notice::HostFailed]);
        d.step(TogetherEvent::Host { me: me("m", "H"), relay: "wss://r.fr".into(), server: None, item: None, random: vec![255; 64], visible: false, servers: Vec::new() });
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

    /// Nothing that can open a session prints in a log: not the secret (nor the code that carries
    /// it), not the random bytes it came from, not the AES key.
    #[test]
    fn debug_output_never_carries_a_secret_or_a_key() {
        let random: Vec<u8> = (0u8..32).collect();
        let host = TogetherEvent::Host { me: me("m", "H"), relay: "wss://r.fr".into(), server: None, item: None, random: random.clone(), visible: false, servers: Vec::new() };
        let mut d = TogetherSession::new();
        let mut fx = Vec::new();
        d.step(host, &mut fx);
        let TogetherFx::Derive { secret, .. } = fx[0].clone() else { panic!("{fx:?}") };
        let key = [0xab; 32];
        let code = format_invite(&secret, None);
        let dumps = [
            format!("{:?}", TogetherEvent::Host { me: me("m", "H"), relay: "wss://r.fr".into(), server: None, item: None, random, visible: false, servers: Vec::new() }),
            format!("{:?}", TogetherEvent::Join { me: me("m", "H"), code: code.clone(), relay: String::new(), server: None, servers: Vec::new() }),
            format!("{fx:?}"),
            format!("{:?}", d.phase),
            format!("{:?}", TogetherFx::Seal { tag: 1, key, plain: "{}".into() }),
            format!("{:?}", TogetherFx::Open { tag: 1, key, bytes: vec![1, 2] }),
            format!("{:?}", Notice::Opened { code: code.clone(), full_code: Some(format!("{code}@r.fr")), host: true }),
        ];
        let compact = secret.replace('-', "");
        for dump in &dumps {
            assert!(!dump.contains(&compact) && !dump.contains(&secret[..4]), "{dump}");
            assert!(!dump.contains("171") && !dump.contains("0, 1, 2, 3"), "{dump}");
        }
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


    // ---- Sessions visible to friends (the web's friends-only protocol).

    const RELAY: &str = "wss://plex.exemple.fr:32400/multiplex-relay";

    /// The host's server, published on a public name: its relay is found there.
    fn srv1() -> ServerInfo {
        ServerInfo { id: "srv-1".into(), connections: vec![PlexConnection { uri: "https://plex.exemple.fr:32400".into(), local: false, relay: false }] }
    }

    fn rooms_ok(server: &str) -> String {
        format!(r#"{{"multiplex":"relay","found":[],"friends":true,"friendsOnly":true,"server":"{server}"}}"#)
    }

    fn friends_ok(server: &str, sessions: &str) -> FriendsAnswer {
        FriendsAnswer::read(&format!(r#"{{"multiplex":"relay","server":"{server}","friendsOnly":true,"ticket":"TICKET-1","ttl":300,"sessions":[{sessions}]}}"#)).unwrap()
    }

    fn texts(out: &[TogetherFx]) -> Vec<Value> {
        out.iter().filter_map(|f| if let TogetherFx::SendText { text, .. } = f { Some(serde_json::from_str(text).unwrap()) } else { None }).collect()
    }

    fn find<T>(out: &[TogetherFx], pick: impl Fn(&TogetherFx) -> Option<T>) -> Vec<T> {
        out.iter().filter_map(pick).collect()
    }

    /// A visible host whose socket just opened: returns the device and the tag of its `/rooms` ask.
    fn visible_host() -> (Device, u32) {
        let mut h = Device::new();
        h.step(TogetherEvent::Host { me: me("m", "Hôte"), relay: RELAY.into(), server: Some("srv-1".into()), item: Some(movie()), random: (0u8..32).collect(), visible: true, servers: vec![srv1()] });
        h.take();
        let conn = h.conn();
        h.step(TogetherEvent::SocketOpened { conn });
        let out = h.take();
        assert!(notices(&out).contains(&Notice::Listed(Some(Listed::Pending))), "{out:?}");
        let rooms = find(&out, |f| if let TogetherFx::HttpRooms { tag, url, .. } = f { Some((*tag, url.clone())) } else { None });
        assert_eq!(rooms.len(), 1, "{out:?}");
        assert_eq!(rooms[0].1, "https://plex.exemple.fr:32400/multiplex-relay/rooms?ids=");
        (h, rooms[0].0)
    }

    /// The whole announce, then the relay's `announced`.
    fn announced() -> Device {
        let (mut h, rooms) = visible_host();
        h.step(TogetherEvent::RoomsAnswered { tag: rooms, body: Some(rooms_ok("srv-1")) });
        let out = h.take();
        let asks = find(&out, |f| if let TogetherFx::AskFriends { tag, relay, server } = f { Some((*tag, relay.clone(), server.clone())) } else { None });
        assert_eq!(asks.len(), 1, "{out:?}");
        assert_eq!((asks[0].1.as_str(), asks[0].2.as_str()), (RELAY, "srv-1"));
        h.step(TogetherEvent::FriendsAnswered { tag: asks[0].0, answer: Some(friends_ok("srv-1", "")) });
        let out = h.take();
        let reads = find(&out, |f| if let TogetherFx::ReadFriendIds { tag } = f { Some(*tag) } else { None });
        assert_eq!(reads.len(), 1, "{out:?}");
        h.step(TogetherEvent::FriendIdsRead { tag: reads[0], ids: Some(vec!["2001".into(), "2004".into()]) });
        let out = h.take();
        let sent = texts(&out);
        assert_eq!(sent.len(), 1, "{out:?}");
        let proof = match &h.s.phase {
            Phase::Live(l) => l.keys.owner.clone().unwrap(),
            Phase::Idle | Phase::Deriving(_) | Phase::Joining(_) | Phase::FriendJoin(_) => panic!(),
        };
        assert_eq!(sent[0]["t"], "announce");
        assert_eq!(sent[0]["ticket"], "TICKET-1");
        assert_eq!(sent[0]["proof"], proof.as_str());
        assert_eq!(sent[0]["friends"], serde_json::json!(["2001", "2004"]));
        assert_eq!(sent[0]["card"], serde_json::json!({"name": "Hôte", "title": "Le Film", "type": "movie", "guid": "plex://movie/1", "key": "42", "time": 0, "playing": false}));
        let conn = h.conn();
        h.step(TogetherEvent::SocketText { conn, text: r#"{"t":"announced"}"#.into() });
        assert_eq!(notices(&h.take()), [Notice::Listed(Some(Listed::Yes))]);
        assert_eq!(h.view().listed, Some(Listed::Yes));
        h
    }

    #[test]
    fn a_visible_host_proves_access_then_announces_with_its_friend_list() {
        let mut h = announced();
        // Its ticks say the session is visible.
        h.tick(TICK_EVERY_US);
        let ticks = h.sent();
        assert_eq!(ticks[0]["vis"], true);
    }

    #[test]
    fn no_token_leaves_for_a_relay_that_does_not_list_to_friends_only_or_is_another_servers() {
        for body in [
            Some(r#"{"multiplex":"relay","found":[],"friends":true,"server":"srv-1"}"#.to_string()),
            Some(rooms_ok("autre-serveur")),
            Some(r#"{"multiplex":"relay","found":[],"friends":false}"#.to_string()),
            None,
        ] {
            let (mut h, rooms) = visible_host();
            h.step(TogetherEvent::RoomsAnswered { tag: rooms, body });
            let out = h.take();
            assert!(find(&out, |f| matches!(f, TogetherFx::AskFriends { .. }).then_some(())).is_empty(), "{out:?}");
            assert_eq!(notices(&out), [Notice::Listed(Some(Listed::No))]);
        }
    }

    #[test]
    fn a_relay_not_on_the_servers_public_names_gets_nothing() {
        let mut h = Device::new();
        h.step(TogetherEvent::Host { me: me("m", "H"), relay: "wss://ailleurs.exemple.fr".into(), server: Some("srv-1".into()), item: Some(movie()), random: (0u8..32).collect(), visible: true, servers: vec![srv1()] });
        h.take();
        let conn = h.conn();
        h.step(TogetherEvent::SocketOpened { conn });
        let out = h.take();
        assert!(find(&out, |f| matches!(f, TogetherFx::HttpRooms { .. } | TogetherFx::AskFriends { .. }).then_some(())).is_empty(), "{out:?}");
        assert!(notices(&out).contains(&Notice::Listed(Some(Listed::No))));
    }

    #[test]
    fn without_a_friend_list_nothing_is_announced() {
        let (mut h, rooms) = visible_host();
        h.step(TogetherEvent::RoomsAnswered { tag: rooms, body: Some(rooms_ok("srv-1")) });
        let ask = find(&h.take(), |f| if let TogetherFx::AskFriends { tag, .. } = f { Some(*tag) } else { None })[0];
        h.step(TogetherEvent::FriendsAnswered { tag: ask, answer: Some(friends_ok("srv-1", "")) });
        let read = find(&h.take(), |f| if let TogetherFx::ReadFriendIds { tag } = f { Some(*tag) } else { None })[0];
        h.step(TogetherEvent::FriendIdsRead { tag: read, ids: None });
        let out = h.take();
        assert!(texts(&out).is_empty(), "{out:?}");
        assert_eq!(notices(&out), [Notice::Listed(Some(Listed::No))]);
        // An answer that does not say friendsOnly is not used either.
        let (mut h, rooms) = visible_host();
        h.step(TogetherEvent::RoomsAnswered { tag: rooms, body: Some(rooms_ok("srv-1")) });
        let ask = find(&h.take(), |f| if let TogetherFx::AskFriends { tag, .. } = f { Some(*tag) } else { None })[0];
        let old = FriendsAnswer::read(r#"{"multiplex":"relay","server":"srv-1","ticket":"T","ttl":300,"sessions":[]}"#);
        h.step(TogetherEvent::FriendsAnswered { tag: ask, answer: old });
        let out = h.take();
        assert!(find(&out, |f| matches!(f, TogetherFx::ReadFriendIds { .. }).then_some(())).is_empty());
        assert_eq!(notices(&out), [Notice::Listed(Some(Listed::No))]);
    }

    #[test]
    fn refreshes_resend_the_list_only_when_re_read_and_release_when_plex_tv_is_mute() {
        let mut h = announced();
        // Every 5 presence ticks (15 s) the position is refreshed; the list read 15 s ago is cached
        // and was already sent: not joined again.
        for _ in 0..CARD_REFRESH_TICKS {
            h.tick(TICK_EVERY_US);
        }
        let refresh = texts(&h.take());
        assert_eq!(refresh.len(), 1, "{refresh:?}");
        assert_eq!(refresh[0]["t"], "announce");
        assert!(refresh[0].get("ticket").is_none() && refresh[0].get("friends").is_none(), "{refresh:?}");
        // Past the 2-minute cache, plex.tv is asked again; a re-read list is sent even unchanged.
        let mut reads = Vec::new();
        for _ in 0..(FRIENDS_TTL_US / TICK_EVERY_US) {
            h.tick(TICK_EVERY_US);
            let out = h.take();
            reads.extend(find(&out, |f| if let TogetherFx::ReadFriendIds { tag } = f { Some(*tag) } else { None }));
            if !reads.is_empty() {
                break;
            }
        }
        assert_eq!(reads.len(), 1);
        h.step(TogetherEvent::FriendIdsRead { tag: reads[0], ids: Some(vec!["2001".into()]) });
        let sent = texts(&h.take());
        assert_eq!(sent[0]["friends"], serde_json::json!(["2001"]), "a friend removed on plex.tv leaves the list");
        // plex.tv mute at the next read: the announce is released, never kept with a stale list.
        let mut read = None;
        for _ in 0..(2 * FRIENDS_TTL_US / TICK_EVERY_US) {
            h.tick(TICK_EVERY_US);
            if let Some(t) = find(&h.take(), |f| if let TogetherFx::ReadFriendIds { tag } = f { Some(*tag) } else { None }).first() {
                read = Some(*t);
                break;
            }
        }
        h.step(TogetherEvent::FriendIdsRead { tag: read.unwrap(), ids: None });
        let out = h.take();
        assert_eq!(texts(&out), [serde_json::json!({"t": "release"})]);
        assert_eq!(notices(&out), [Notice::Listed(Some(Listed::No))]);
    }

    #[test]
    fn an_admitted_friend_gets_the_key_once_and_a_burst_is_bounded() {
        let mut h = announced();
        let conn = h.conn();
        h.tick(1_000);
        h.take();
        h.step(TogetherEvent::SocketText { conn, text: r#"{"t":"key-request","peer":"sid-1","pub":"PUB-1"}"#.into() });
        let out = h.take();
        let seals = find(&out, |f| if let TogetherFx::SealGrant { tag, request_pub, payload } = f { Some((*tag, request_pub.clone(), payload.clone())) } else { None });
        assert_eq!(seals.len(), 1, "{out:?}");
        let (tag, public, payload) = seals[0].clone();
        assert_eq!(public, "PUB-1");
        assert_eq!(payload.server, "srv-1");
        assert_eq!(format_invite(&payload.secret, None), h.view().code);
        assert!(payload.state.is_some());
        assert!(!format!("{out:?}").contains(&payload.secret), "the secret is not printed");
        h.step(TogetherEvent::GrantSealed { tag, sealed: Some(("HOSTPUB".into(), "BOX".into())) });
        assert_eq!(texts(&h.take()), [serde_json::json!({"t": "key-grant", "peer": "sid-1", "pub": "HOSTPUB", "box": "BOX"})]);
        // The same requester again: nothing. Then at most 5 per 10 s.
        h.step(TogetherEvent::SocketText { conn, text: r#"{"t":"key-request","peer":"sid-1","pub":"PUB-1"}"#.into() });
        assert!(h.take().is_empty());
        for i in 2..10 {
            h.step(TogetherEvent::SocketText { conn, text: format!(r#"{{"t":"key-request","peer":"sid-{i}","pub":"P"}}"#) });
        }
        assert_eq!(find(&h.take(), |f| matches!(f, TogetherFx::SealGrant { .. }).then_some(())).len(), GRANTS_PER_WINDOW - 1);
    }

    #[test]
    fn a_private_session_hands_nothing_and_unlisting_says_so() {
        let mut h = announced();
        let conn = h.conn();
        h.step(TogetherEvent::SetVisible(false));
        let out = h.take();
        assert_eq!(texts(&out), [serde_json::json!({"t": "unlist"})]);
        assert!(notices(&out).contains(&Notice::Visible(false)));
        h.step(TogetherEvent::SocketText { conn, text: r#"{"t":"key-request","peer":"sid-1","pub":"PUB-1"}"#.into() });
        assert!(h.take().is_empty());
    }

    #[test]
    fn an_announced_host_releases_when_it_leaves_and_a_relay_error_unlists() {
        let mut h = announced();
        h.step(TogetherEvent::Leave);
        let out = h.take();
        assert_eq!(texts(&out), [serde_json::json!({"t": "release"})]);
        assert!(out.iter().position(|f| matches!(f, TogetherFx::SendText { .. })) < out.iter().position(|f| matches!(f, TogetherFx::SendBinary { .. })), "release, then bye");
        let mut h = announced();
        let conn = h.conn();
        h.step(TogetherEvent::SocketText { conn, text: r#"{"t":"error","error":"owner"}"#.into() });
        assert_eq!(notices(&h.take()), [Notice::Listed(Some(Listed::No))]);
    }

    /// A friend: lists the sessions of the servers it sees, then enters one with the ticket and the
    /// key the host handed over.
    #[test]
    fn a_friend_lists_and_enters_a_visible_session() {
        let host_keys = derive_session("K7Q29XFD", "srv-1", ROUNDS);
        let session = format!(r#"{{"room":"{}","card":{{"name":"Dominique (dom)","title":"Le Film","type":"movie","key":"42","time":100,"playing":true}},"count":2,"age":2000}}"#, host_keys.room);
        let mut f = Device::new();
        f.tick(10_000_000);
        f.step(TogetherEvent::RefreshFriends { servers: vec![srv1()] });
        let out = f.take();
        let rooms = find(&out, |x| if let TogetherFx::HttpRooms { tag, url, .. } = x { Some((*tag, url.clone())) } else { None });
        assert_eq!(rooms.len(), 4, "the four candidate forms of the public name: {rooms:?}");
        // The best-ranked form answers as srv-1's relay; the others fail.
        f.step(TogetherEvent::RoomsAnswered { tag: rooms[0].0, body: Some(rooms_ok("srv-1")) });
        for (tag, _) in &rooms[1..] {
            f.step(TogetherEvent::RoomsAnswered { tag: *tag, body: None });
        }
        let out = f.take();
        let asks = find(&out, |x| if let TogetherFx::AskFriends { tag, relay, server } = x { Some((*tag, relay.clone(), server.clone())) } else { None });
        assert_eq!(asks.len(), 1, "{out:?}");
        assert_eq!((asks[0].1.as_str(), asks[0].2.as_str()), (RELAY, "srv-1"));
        f.step(TogetherEvent::FriendsAnswered { tag: asks[0].0, answer: Some(friends_ok("srv-1", &session)) });
        let out = f.take();
        let Some(Notice::FriendSessions(list)) = notices(&out).into_iter().next() else { panic!("{out:?}") };
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].relay.as_str(), list[0].room.as_str(), list[0].server.as_str()), (RELAY, host_keys.room.as_str(), "srv-1"));
        assert_eq!(list[0].time_at(f.s.now_us()), 102.0);
        assert!(!format!("{list:?}").contains("TICKET-1"));
        // Enter it.
        f.step(TogetherEvent::JoinFriend { me: me("f", "Ami"), relay: RELAY.into(), room: host_keys.room.clone() });
        let out = f.take();
        let req = find(&out, |x| if let TogetherFx::RequestKey { tag, relay, room, ticket } = x { Some((*tag, relay.clone(), room.clone(), ticket.clone())) } else { None });
        assert_eq!(req.len(), 1, "{out:?}");
        assert_eq!(req[0].3, "TICKET-1");
        assert!(!format!("{out:?}").contains("TICKET-1"), "the ticket is not printed");
        let payload = GrantPayload { secret: "K7Q29XFD".into(), server: "srv-1".into(), state: Some(GrantState { time: 101.0, playing: true, c: 4, by: "h".into() }) };
        f.step(TogetherEvent::FriendKey { tag: req[0].0, answer: Ok(payload) });
        let out = f.take();
        assert!(out.contains(&TogetherFx::OpenSocket { conn: f.conn(), url: format!("{RELAY}/r/{}", host_keys.room) }), "{out:?}");
        let v = f.view();
        assert!(!v.host && v.visible);
        assert_eq!(v.item.as_ref().map(|i| i.title.as_str()), Some("Le Film"));
        assert_eq!(v.position, Some(101.0));
        assert_eq!(f.key(), host_keys.key);
    }

    #[test]
    fn a_grant_for_another_room_or_a_refused_request_fails_the_join() {
        let room = derive_session("K7Q29XFD", "srv-1", ROUNDS).room.clone();
        let session = format!(r#"{{"room":"{room}","card":{{"name":"D","title":"T","time":0,"playing":false}},"count":1,"age":0}}"#);
        let listed = |f: &mut Device| {
            f.step(TogetherEvent::RefreshFriends { servers: vec![srv1()] });
            let rooms = find(&f.take(), |x| if let TogetherFx::HttpRooms { tag, .. } = x { Some(*tag) } else { None });
            f.step(TogetherEvent::RoomsAnswered { tag: rooms[0], body: Some(rooms_ok("srv-1")) });
            for t in &rooms[1..] {
                f.step(TogetherEvent::RoomsAnswered { tag: *t, body: None });
            }
            let ask = find(&f.take(), |x| if let TogetherFx::AskFriends { tag, .. } = x { Some(*tag) } else { None })[0];
            f.step(TogetherEvent::FriendsAnswered { tag: ask, answer: Some(friends_ok("srv-1", &session)) });
            f.take();
            f.step(TogetherEvent::JoinFriend { me: me("f", "Ami"), relay: RELAY.into(), room: room.clone() });
            find(&f.take(), |x| if let TogetherFx::RequestKey { tag, .. } = x { Some(*tag) } else { None })[0]
        };
        let mut f = Device::new();
        let tag = listed(&mut f);
        f.step(TogetherEvent::FriendKey { tag, answer: Ok(GrantPayload { secret: "ZZZZZZZZ".into(), server: "srv-1".into(), state: None }) });
        assert_eq!(notices(&f.take()), [Notice::JoinFailed(JoinError::Failed)]);
        assert!(f.s.view().is_none());
        let mut f = Device::new();
        let tag = listed(&mut f);
        f.step(TogetherEvent::FriendKey { tag, answer: Err(JoinError::NoHost) });
        assert_eq!(notices(&f.take()), [Notice::JoinFailed(JoinError::NoHost)]);
        // A room never listed is not found.
        let mut f = Device::new();
        f.step(TogetherEvent::JoinFriend { me: me("f", "Ami"), relay: RELAY.into(), room: room.clone() });
        assert_eq!(notices(&f.take()), [Notice::JoinFailed(JoinError::NotFound)]);
    }

    #[test]
    fn an_expired_ticket_lists_again_before_entering() {
        let room = derive_session("K7Q29XFD", "srv-1", ROUNDS).room.clone();
        let session = format!(r#"{{"room":"{room}","card":{{"name":"D","title":"T","time":0,"playing":false}},"count":1,"age":0}}"#);
        let mut f = Device::new();
        let list = |f: &mut Device| {
            let rooms = find(&f.take(), |x| if let TogetherFx::HttpRooms { tag, .. } = x { Some(*tag) } else { None });
            f.step(TogetherEvent::RoomsAnswered { tag: rooms[0], body: Some(rooms_ok("srv-1")) });
            for t in &rooms[1..] {
                f.step(TogetherEvent::RoomsAnswered { tag: *t, body: None });
            }
            let ask = find(&f.take(), |x| if let TogetherFx::AskFriends { tag, .. } = x { Some(*tag) } else { None })[0];
            f.step(TogetherEvent::FriendsAnswered { tag: ask, answer: Some(friends_ok("srv-1", &session)) });
        };
        f.step(TogetherEvent::RefreshFriends { servers: vec![srv1()] });
        list(&mut f);
        f.take();
        f.tick(300_000_000);
        f.step(TogetherEvent::JoinFriend { me: me("f", "Ami"), relay: RELAY.into(), room: room.clone() });
        assert!(f.s.busy());
        // Only srv-1's relay that answered is asked again (the others are set aside a while).
        list(&mut f);
        let out = f.take();
        assert_eq!(find(&out, |x| matches!(x, TogetherFx::RequestKey { .. }).then_some(())).len(), 1, "{out:?}");
    }
}
