//! **The Watch Together store**: the IO side of [`TogetherSession`], with no IO of its own.
//!
//! The machine is pure; this store turns each of its effects into a job and each job's answer back
//! into one addressed event. It owns the session, a queue of the UI's commands, and an `Arc`
//! mailbox the jobs answer into. It starts no worker, reads no clock, registry or file: the Bridge
//! hands [`TogetherStore::pump`] everything as values ([`TogetherInput`]: the seated profile, the
//! plex.tv identity, the servers this account sees with their own tokens, the settings, a snapshot
//! of the player, whether this build may write, and the app's logical clock), and the application's
//! runner (`app/adapters/together.rs`) starts the jobs [`TogetherStore::take_jobs`] returns.
//!
//! **Standalone, not a `StoreId`.** The `Stores` aggregate's stores are the server-derived data the
//! screens read through `StoreCmd`; this one holds a live socket, long-lived per process, which no
//! screen owns. It is shaped like `HudStore`: its own job enum ([`TogetherJob`]), its own runner, and
//! its own machine id (`MachineId::Together`) for the jobs' addresses.
//!
//! - **Jobs**: randomness (`/dev/urandom` in the runner), PBKDF2 off the frame, seal and open,
//!   the grant seal, `GET /rooms`, `POST /friends` (with the server's own token, or a delegation token
//!   the owner's server hands out), the plex.tv friends read, and the key exchange of a friend's
//!   session. A socket is a job too ([`TogetherJob::Socket`]) whose events land addressed by `conn`;
//!   sends and closes are [`TogetherJob::Send`] / [`TogetherJob::Close`], executed by the runner on
//!   the socket's outbox in the order pushed.
//! - **Tokens never enter the machine.** `AskFriends` names a relay and a server; the store captures
//!   that server's credential from its input ([`Credential`]) into the job. The plex.tv account token
//!   goes only to plex.tv (the friends read) and to the owner's own server (the delegation request),
//!   never to a relay.
//! - **No announce without permission** ([`TogetherInput::may_announce`]): a Guest boot, a managed
//!   profile, a device with no plex.tv identity, or a debug build talking to a real server (the
//!   write gate, `plx_plex::plex::together::together_write_gate`) never announces, never asks a
//!   delegation token and never hands a key over. Code sessions stay available to all of them.
//! - **The player** is read from a snapshot and driven through [`TogetherPlayerRequest`]s that the
//!   Bridge takes ([`TogetherStore::take_player_requests`]).
//!
//! **Every job is answered once**, a refusal included: a refused or panicked job lands as the
//! negative event its request owes (`RoomsAnswered { body: None }`, `SocketClosed`, …).

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};

use plx_base::b64::encode_url;
use plx_base::crypto::EcdhSecret;
use plx_machine::job::{Job, Port};
use plx_machine::machine::{Addr, MachineId, RequestId};

use crate::together::friends::{
    friend_allow_list, friends_body, key_request_text, open_grant, parse_plex_friends, seal_grant, server_credential, Control, FriendsAnswer,
    GrantPayload,
};
use crate::together::keys::{derive_session, open_message, seal_message, SessionKeys};
use crate::together::relay::{http_base, ServerInfo};
use crate::together::session::{FriendSession, JoinError, Me, Notice, TogetherEvent, TogetherFx, TogetherSession, View};
use crate::together::sync::{PlayerCmd, PlayerObs};
use crate::together::wire::SharedItem;

/// A player observation goes to the session this often while attached.
pub const PLAYER_EVERY_US: u64 = 500_000;
/// A friend's key exchange waits this long for the host's grant (the web's 10 s).
pub const KEY_WAIT_MS: u32 = 10_000;
/// `POST /friends` and the plex.tv friends read wait this long.
pub const FRIENDS_TIMEOUT_MS: u32 = 6_000;
/// Draws of an ephemeral grant key before giving up (a draw that is not a valid P-256 scalar is
/// drawn again; the odds of one are about 2^-32).
const GRANT_DRAWS: usize = 4;

/// The plex.tv identity the together jobs act as. The account token is a secret: it is not printed.
#[derive(Clone, PartialEq, Eq)]
pub struct TogetherAuth {
    pub client_id: String,
    /// The plex.tv account id (excluded from its own friend allow-list).
    pub account_id: Option<String>,
    account_token: String,
}

impl TogetherAuth {
    pub fn new(client_id: &str, account_id: Option<&str>, account_token: &str) -> TogetherAuth {
        TogetherAuth { client_id: client_id.into(), account_id: account_id.map(str::to_string), account_token: account_token.into() }
    }

    /// The plex.tv account token, for the IO alone.
    pub fn account_token(&self) -> &str {
        &self.account_token
    }
}

impl fmt::Debug for TogetherAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TogetherAuth").field("client_id", &self.client_id).field("account_id", &self.account_id).finish_non_exhaustive()
    }
}

/// A server this account sees, as the together jobs need it: its identity and public connections
/// (relay discovery), whether the account owns it, a reachable address, and its own access token
/// (plex.tv `resources`' `accessToken`; for an owned server it is the account token).
#[derive(Clone, PartialEq, Eq)]
pub struct TogetherServer {
    pub info: ServerInfo,
    pub owned: bool,
    /// A base URL the server answers on (for the owner's delegation request).
    pub base: Option<String>,
    access_token: Option<String>,
}

impl TogetherServer {
    pub fn new(info: ServerInfo, owned: bool, base: Option<String>, access_token: Option<String>) -> TogetherServer {
        TogetherServer { info, owned, base, access_token }
    }
}

impl fmt::Debug for TogetherServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TogetherServer").field("info", &self.info).field("owned", &self.owned).field("base", &self.base).finish_non_exhaustive()
    }
}

/// How a relay is shown access to a server: the server's own token, or a delegation token the
/// owner's server hands out (`/security/token?type=delegation&scope=all`, asked with the account
/// token from the server itself, never from the relay).
#[derive(Clone, PartialEq, Eq)]
pub enum Credential {
    Share(String),
    Delegation { base: String, token: String },
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Credential::Share(_) => f.write_str("Share(<hidden>)"),
            Credential::Delegation { base, .. } => f.debug_struct("Delegation").field("base", base).finish_non_exhaustive(),
        }
    }
}

/// The seated profile, as participants see it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TogetherProfile {
    pub name: String,
    pub thumb: Option<String>,
}

/// The settings the store reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TogetherSettings {
    /// The relay as typed in Settings (may be empty: found on the servers' names).
    pub relay: String,
    /// A new hosted session is visible to the host's friends (default on).
    pub visible_default: bool,
    /// Tell the user when another participant plays, pauses or seeks (default on).
    pub notify: bool,
}

impl Default for TogetherSettings {
    fn default() -> TogetherSettings {
        TogetherSettings { relay: String::new(), visible_default: true, notify: true }
    }
}

/// What the player plays, read by the Bridge each frame.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerSnapshot {
    pub item: SharedItem,
    pub obs: PlayerObs,
}

/// Everything a pump reads, as values.
#[derive(Clone, Copy, Debug)]
pub struct TogetherInput<'a> {
    pub profile: Option<&'a TogetherProfile>,
    pub auth: Option<&'a TogetherAuth>,
    /// A `--guest` boot.
    pub guest: bool,
    /// A managed Plex Home profile.
    pub managed: bool,
    /// The write gate allows writes here (always in a release build; in a debug build only against
    /// a loopback server, the armed mock, or the live switch).
    pub writes_permitted: bool,
    pub settings: &'a TogetherSettings,
    /// The browsed server's identifier.
    pub server: Option<&'a str>,
    pub servers: &'a [TogetherServer],
    pub player: Option<&'a PlayerSnapshot>,
    /// The app's logical clock (ms, wrapping).
    pub now_ms: u32,
}

impl TogetherInput<'_> {
    /// May this device announce a session, ask a delegation token and hand a key over?
    pub fn may_announce(&self) -> bool {
        self.auth.is_some() && !self.guest && !self.managed && self.writes_permitted
    }

    fn infos(&self) -> Vec<ServerInfo> {
        self.servers.iter().map(|s| s.info.clone()).collect()
    }

    /// The credential a relay is shown for `server`, if any (see [`Credential`]).
    pub fn credential(&self, server: &str) -> Option<Credential> {
        let s = self.servers.iter().find(|s| s.info.id == server)?;
        let account = self.auth.map(TogetherAuth::account_token);
        if let Some(token) = server_credential(s.access_token.as_deref(), account) {
            return Some(Credential::Share(token));
        }
        match (s.owned && self.may_announce(), &s.base, account) {
            (true, Some(base), Some(token)) if !token.is_empty() => Some(Credential::Delegation { base: base.clone(), token: token.to_string() }),
            _ => None,
        }
    }
}

/// What the UI asks, applied in order on the next pump.
#[derive(Clone, Debug, PartialEq)]
pub enum TogetherCmd {
    /// Host a new session on the title playing or shown.
    Host { item: Option<SharedItem> },
    Join { code: String },
    Leave,
    SetVisible(bool),
    ShareTitle(SharedItem),
    RefreshFriends,
    JoinFriend { relay: String, room: String },
}

/// What the store asks of the player (lane 4 wires them).
#[derive(Clone, Debug, PartialEq)]
pub enum TogetherPlayerRequest {
    Seek(f64),
    Play,
    Pause,
    /// The session plays `item` now: open it (at the session's position when known).
    Follow { item: SharedItem, position: Option<f64> },
}

/// One unit of together work. The request id is the job's address.
pub enum TogetherReq {
    Random { n: usize },
    Derive { secret: String, server: String, iterations: u32 },
    Seal { key: [u8; 32], plain: String },
    Open { key: [u8; 32], bytes: Vec<u8> },
    SealGrant { request_pub: String, payload: GrantPayload },
    Rooms { url: String, timeout_ms: u32 },
    AskFriends { relay: String, credential: Credential },
    FriendIds { auth: TogetherAuth },
    RequestKey { relay: String, room: String, ticket: String },
}

impl fmt::Debug for TogetherReq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TogetherReq::Random { n } => f.debug_struct("Random").field("n", n).finish(),
            TogetherReq::Derive { server, iterations, .. } => f.debug_struct("Derive").field("server", server).field("iterations", iterations).finish_non_exhaustive(),
            TogetherReq::Seal { plain, .. } => f.debug_struct("Seal").field("len", &plain.len()).finish_non_exhaustive(),
            TogetherReq::Open { bytes, .. } => f.debug_struct("Open").field("len", &bytes.len()).finish_non_exhaustive(),
            TogetherReq::SealGrant { .. } => f.write_str("SealGrant(<hidden>)"),
            TogetherReq::Rooms { url, timeout_ms } => f.debug_struct("Rooms").field("url", url).field("timeout_ms", timeout_ms).finish(),
            TogetherReq::AskFriends { relay, credential } => f.debug_struct("AskFriends").field("relay", relay).field("credential", credential).finish(),
            TogetherReq::FriendIds { auth } => f.debug_struct("FriendIds").field("auth", auth).finish(),
            TogetherReq::RequestKey { relay, room, .. } => f.debug_struct("RequestKey").field("relay", relay).field("room", room).finish_non_exhaustive(),
        }
    }
}

/// A frame for a socket.
#[derive(Clone, PartialEq, Eq)]
pub enum Frame {
    Text(String),
    Binary(Vec<u8>),
}

impl fmt::Debug for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Frame::Text(t) => write!(f, "Text({} bytes)", t.len()),
            Frame::Binary(b) => write!(f, "Binary({} bytes)", b.len()),
        }
    }
}

/// What the runner is asked to do.
#[derive(Debug)]
pub enum TogetherJob {
    Work(Job<TogetherReq>, TogetherPort),
    /// Dial `url` and keep the connection until it ends or is closed; its events land by `conn`.
    Socket { conn: u32, url: String, port: TogetherPort },
    Send { conn: u32, frame: Frame },
    Close { conn: u32 },
}

/// What a socket reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SocketNews {
    Opened,
    Text(String),
    Binary(Vec<u8>),
    /// Ended, however (a refusal, a failure, a close): reported once, last.
    Closed,
}

/// The IO a together job goes through. Production is the application's `LiveTogetherIo`; tests
/// script it.
pub trait TogetherIo: Send + Sync {
    /// `n` bytes of `/dev/urandom`, or `None`.
    fn random(&self, n: usize) -> Option<Vec<u8>>;
    /// `GET url` (a relay's `/rooms`): the body of a 200, else `None`.
    fn get(&self, url: &str, timeout_ms: u32) -> Option<String>;
    /// `POST url` with a `text/plain` body (a relay's `/friends`): the body of a 200, else `None`.
    fn post_text(&self, url: &str, body: &str, timeout_ms: u32) -> Option<String>;
    /// The owner's server's delegation token (`/security/token?type=delegation&scope=all`).
    fn delegation(&self, base: &str, token: &str) -> Option<String>;
    /// plex.tv `/api/users/` (XML) as `auth`.
    fn friends_xml(&self, auth: &TogetherAuth) -> Option<String>;
    /// A short-lived socket to `url`: `first` is sent once it opens, every event is handed to
    /// `read` until it answers, and after [`KEY_WAIT_MS`] with no answer it ends as `NoHost`.
    fn exchange(&self, url: &str, first: &str, read: &mut dyn FnMut(SocketNews) -> Option<Result<GrantPayload, JoinError>>) -> Result<GrantPayload, JoinError>;
}

/// A friend's side of the key hand-over (the web's `joinFriend` socket): a fresh P-256 key, the
/// `key-request` it sends, and the reading of the relay's answers.
pub struct KeyExchange {
    secret: [u8; 32],
    public: String,
}

impl KeyExchange {
    /// `None` when `random` is not a valid scalar (draw again).
    pub fn new(random: &[u8; 32]) -> Option<KeyExchange> {
        let public = encode_url(&EcdhSecret::from_bytes(random)?.public_raw());
        Some(KeyExchange { secret: *random, public })
    }

    pub fn request_text(&self, ticket: &str) -> String {
        key_request_text(ticket, &self.public)
    }

    /// The answer an event brings, if it ends the exchange.
    pub fn read(&self, news: SocketNews) -> Option<Result<GrantPayload, JoinError>> {
        match news {
            SocketNews::Opened | SocketNews::Binary(_) => None,
            SocketNews::Closed => Some(Err(JoinError::NotFound)),
            SocketNews::Text(t) => match Control::read(&t)? {
                Control::KeyGrant { public, sealed } => Some(open_grant(&self.secret, &self.public, &public, &sealed).ok_or(JoinError::Failed)),
                Control::Error(e) if e == "nohost" => Some(Err(JoinError::NoHost)),
                Control::Error(_) => Some(Err(JoinError::Failed)),
                Control::Announced | Control::KeyRequest { .. } => None,
            },
        }
    }
}

impl Drop for KeyExchange {
    fn drop(&mut self) {
        plx_base::crypto::Zeroize::zeroize(&mut self.secret);
    }
}

/// `wss://relay/r/room` from a relay as listed (`https://…` or `wss://…`).
pub fn room_socket_url(relay: &str, room: &str) -> String {
    let base = http_base(relay);
    let ws = match base.strip_prefix("http") {
        Some(rest) => format!("ws{rest}"),
        None => base,
    };
    format!("{ws}/r/{room}")
}

enum Outcome {
    Random(Option<Vec<u8>>),
    Derived(SessionKeys),
    Sealed(Option<Vec<u8>>),
    Opened(Option<Vec<u8>>),
    Grant(Option<(String, String)>),
    Body(Option<String>),
    Ids(Option<Vec<String>>),
    Key(Result<GrantPayload, JoinError>),
    Panicked,
    NotStarted,
}

enum Landing {
    Done { req: u32, outcome: Outcome },
    Socket { conn: u32, news: SocketNews },
}

#[derive(Default)]
struct Mailbox {
    mail: Mutex<VecDeque<Landing>>,
}

impl Mailbox {
    fn post(&self, landing: Landing) {
        self.mail.lock().unwrap_or_else(|e| e.into_inner()).push_back(landing);
        plx_machine::idle::invalidate();
    }

    fn take_all(&self) -> Vec<Landing> {
        self.mail.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect()
    }
}

/// A job's answer, opaque to the runner.
pub struct TogetherAnswer(Outcome);

/// Where a together job's answer, or a socket's events, go: the store's mailbox.
#[derive(Clone)]
pub struct TogetherPort {
    mailbox: Arc<Mailbox>,
}

impl TogetherPort {
    /// A socket's event (the runner's `ws::run` callback).
    pub fn socket(&self, conn: u32, news: SocketNews) {
        self.mailbox.post(Landing::Socket { conn, news });
    }
}

impl PartialEq for TogetherPort {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.mailbox, &other.mailbox)
    }
}

impl fmt::Debug for TogetherPort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TogetherPort({:p})", Arc::as_ptr(&self.mailbox))
    }
}

impl Port<TogetherAnswer> for TogetherPort {
    fn done(&self, addr: Addr, out: TogetherAnswer) {
        self.mailbox.post(Landing::Done { req: addr.req.0, outcome: out.0 });
    }

    fn refused(&self, addr: Addr) {
        self.mailbox.post(Landing::Done { req: addr.req.0, outcome: Outcome::NotStarted });
    }
}

fn seal_grant_with(io: &dyn TogetherIo, request_pub: &str, payload: &GrantPayload) -> Option<(String, String)> {
    for _ in 0..GRANT_DRAWS {
        let bytes = io.random(44)?;
        let (eph, iv) = bytes.split_at(32);
        let eph: [u8; 32] = eph.try_into().ok()?;
        let iv: [u8; 12] = iv.try_into().ok()?;
        if EcdhSecret::from_bytes(&eph).is_none() {
            continue;
        }
        return seal_grant(request_pub, payload, &eph, &iv);
    }
    None
}

fn ask_friends(io: &dyn TogetherIo, relay: &str, credential: &Credential) -> Option<String> {
    let token = match credential {
        Credential::Share(t) => t.clone(),
        Credential::Delegation { base, token } => io.delegation(base, token)?,
    };
    io.post_text(&format!("{}/friends", http_base(relay)), &friends_body(&token), FRIENDS_TIMEOUT_MS)
}

fn request_key(io: &dyn TogetherIo, relay: &str, room: &str, ticket: &str) -> Result<GrantPayload, JoinError> {
    let mut exchange = None;
    for _ in 0..GRANT_DRAWS {
        let bytes: [u8; 32] = io.random(32).and_then(|b| b.try_into().ok()).ok_or(JoinError::Failed)?;
        exchange = KeyExchange::new(&bytes);
        if exchange.is_some() {
            break;
        }
    }
    let exchange = exchange.ok_or(JoinError::Failed)?;
    io.exchange(&room_socket_url(relay, room), &exchange.request_text(ticket), &mut |news| exchange.read(news))
}

/// The job's IO body: run the request through `io` and post the answer, OUTSIDE the guard, so a
/// panic lands as a failure and never as a missing answer.
pub fn finish_together_job(port: &TogetherPort, addr: Addr, req: TogetherReq, io: &dyn TogetherIo) {
    let outcome = catch_unwind(AssertUnwindSafe(|| match req {
        TogetherReq::Random { n } => Outcome::Random(io.random(n).filter(|b| b.len() == n)),
        TogetherReq::Derive { secret, server, iterations } => Outcome::Derived(derive_session(&secret, &server, iterations)),
        TogetherReq::Seal { key, plain } => {
            Outcome::Sealed(io.random(12).and_then(|iv| <[u8; 12]>::try_from(iv.as_slice()).ok()).map(|iv| seal_message(&key, &iv, &plain)))
        }
        TogetherReq::Open { key, bytes } => Outcome::Opened(open_message(&key, &bytes)),
        TogetherReq::SealGrant { request_pub, payload } => Outcome::Grant(seal_grant_with(io, &request_pub, &payload)),
        TogetherReq::Rooms { url, timeout_ms } => Outcome::Body(io.get(&url, timeout_ms)),
        TogetherReq::AskFriends { relay, credential } => Outcome::Body(ask_friends(io, &relay, &credential)),
        TogetherReq::FriendIds { auth } => {
            Outcome::Ids(io.friends_xml(&auth).map(|xml| friend_allow_list(&parse_plex_friends(&xml), auth.account_id.as_deref())))
        }
        TogetherReq::RequestKey { relay, room, ticket } => Outcome::Key(request_key(io, &relay, &room, &ticket)),
    }))
    .unwrap_or_else(|_| {
        plx_base::eventlog::log("together: a job panicked; it lands as a failure");
        Outcome::Panicked
    });
    port.done(addr, TogetherAnswer(outcome));
}

/// What a request in flight owes the session when it lands.
enum Pending {
    Me,
    Host { item: Option<SharedItem>, relay: String, server: Option<String>, visible: bool, servers: Vec<ServerInfo> },
    Derive(u32),
    Seal(u32),
    Open(u32),
    Grant(u32),
    Rooms(u32),
    Friends(u32),
    Ids(u32),
    Key(u32),
}

/// The identity a session belongs to: a change leaves it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity {
    account: Option<String>,
    profile: Option<String>,
}

/// The together store. See the module doc.
pub struct TogetherStore {
    session: TogetherSession,
    mailbox: Arc<Mailbox>,
    jobs: Vec<TogetherJob>,
    next_req: u32,
    inflight: HashMap<u32, Pending>,
    cmds: VecDeque<TogetherCmd>,
    me_id: Option<String>,
    me_asked: bool,
    permit: Option<bool>,
    identity: Option<Identity>,
    last_ms: Option<u32>,
    attached: Option<SharedItem>,
    since_obs_us: u64,
    notices: Vec<Notice>,
    player: Vec<TogetherPlayerRequest>,
}

impl Default for TogetherStore {
    fn default() -> Self {
        Self::new()
    }
}

impl TogetherStore {
    pub fn new() -> TogetherStore {
        TogetherStore {
            session: TogetherSession::new(),
            mailbox: Arc::default(),
            jobs: Vec::new(),
            next_req: 1,
            inflight: HashMap::new(),
            cmds: VecDeque::new(),
            me_id: None,
            me_asked: false,
            permit: None,
            identity: None,
            last_ms: None,
            attached: None,
            since_obs_us: 0,
            notices: Vec::new(),
            player: Vec::new(),
        }
    }

    pub fn port(&self) -> TogetherPort {
        TogetherPort { mailbox: self.mailbox.clone() }
    }

    /// Queue a UI command (applied on the next pump, once this run's participant id is drawn).
    pub fn command(&mut self, cmd: TogetherCmd) {
        self.cmds.push_back(cmd);
    }

    pub fn take_jobs(&mut self) -> Vec<TogetherJob> {
        std::mem::take(&mut self.jobs)
    }

    pub fn take_notices(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.notices)
    }

    pub fn take_player_requests(&mut self) -> Vec<TogetherPlayerRequest> {
        std::mem::take(&mut self.player)
    }

    pub fn view(&self) -> Option<View> {
        self.session.view()
    }

    pub fn friend_sessions(&self) -> &[FriendSession] {
        self.session.friend_sessions()
    }

    /// Anything to do without a frame's input changing (a landing waiting, a command queued).
    pub fn busy(&self) -> bool {
        self.session.busy() || !self.cmds.is_empty() || !self.inflight.is_empty()
    }

    /// One frame: the clock, the identity, the permission, the landings, the commands, the player.
    pub fn pump(&mut self, input: &TogetherInput<'_>) {
        let dt_ms = self.last_ms.map_or(0, |last| input.now_ms.wrapping_sub(last));
        self.last_ms = Some(input.now_ms);
        let mut events = VecDeque::new();

        let identity = Identity { account: input.auth.map(|a| a.account_id.clone().unwrap_or_else(|| a.client_id.clone())), profile: input.profile.map(|p| p.name.clone()) };
        if self.identity.as_ref().is_some_and(|i| *i != identity) {
            self.cmds.clear();
            self.step_all(input, VecDeque::from([TogetherEvent::Leave]));
            self.permit = None;
            self.attached = None;
        }
        self.identity = Some(identity);

        if self.me_id.is_none() && !self.me_asked {
            self.me_asked = true;
            self.work(TogetherReq::Random { n: 6 }, Pending::Me);
        }
        let may = input.may_announce();
        if self.permit != Some(may) {
            self.permit = Some(may);
            events.push_back(TogetherEvent::Permit { announce: may });
        }
        for landing in self.mailbox.take_all() {
            if let Some(ev) = self.land(landing) {
                events.push_back(ev);
            }
        }
        self.step_all(input, events);

        if self.me_id.is_some() {
            while let Some(cmd) = self.cmds.pop_front() {
                if let Some(ev) = self.apply(cmd, input) {
                    self.step_all(input, VecDeque::from([ev]));
                }
            }
        }
        let player = self.observe(input.player, u64::from(dt_ms) * 1000);
        self.step_all(input, player);
        if dt_ms > 0 {
            self.step_all(input, VecDeque::from([TogetherEvent::Tick { dt_us: u64::from(dt_ms) * 1000 }]));
        }
    }

    fn me(&self, input: &TogetherInput<'_>) -> Option<Me> {
        let profile = input.profile;
        Some(Me { id: self.me_id.clone()?, name: profile.map_or_else(String::new, |p| p.name.clone()), thumb: profile.and_then(|p| p.thumb.clone()) })
    }

    fn apply(&mut self, cmd: TogetherCmd, input: &TogetherInput<'_>) -> Option<TogetherEvent> {
        let me = self.me(input)?;
        match cmd {
            TogetherCmd::Host { item } => {
                let pending = Pending::Host {
                    item,
                    relay: input.settings.relay.clone(),
                    server: input.server.map(str::to_string),
                    visible: input.settings.visible_default,
                    servers: input.infos(),
                };
                self.work(TogetherReq::Random { n: 32 }, pending);
                None
            }
            TogetherCmd::Join { code } => {
                Some(TogetherEvent::Join { me, code, relay: input.settings.relay.clone(), server: input.server.map(str::to_string), servers: input.infos() })
            }
            TogetherCmd::Leave => Some(TogetherEvent::Leave),
            TogetherCmd::SetVisible(on) => Some(TogetherEvent::SetVisible(on)),
            TogetherCmd::ShareTitle(item) => Some(TogetherEvent::ShareTitle(item)),
            // A managed profile reads no friends' sessions (it has no Friends page, as on the web).
            TogetherCmd::RefreshFriends => (!input.managed && !input.guest && input.auth.is_some()).then(|| TogetherEvent::RefreshFriends { servers: input.infos() }),
            TogetherCmd::JoinFriend { relay, room } => (!input.managed && !input.guest).then_some(TogetherEvent::JoinFriend { me, relay, room }),
        }
    }

    /// Attach, observe every [`PLAYER_EVERY_US`], detach.
    fn observe(&mut self, player: Option<&PlayerSnapshot>, dt_us: u64) -> VecDeque<TogetherEvent> {
        let mut out = VecDeque::new();
        if self.session.view().is_none() {
            self.attached = None;
            return out;
        }
        match (player, &self.attached) {
            (Some(p), Some(item)) if *item == p.item => {
                self.since_obs_us += dt_us;
                if self.since_obs_us >= PLAYER_EVERY_US {
                    self.since_obs_us = 0;
                    out.push_back(TogetherEvent::Player(p.obs));
                }
            }
            (Some(p), _) => {
                self.attached = Some(p.item.clone());
                self.since_obs_us = 0;
                out.push_back(TogetherEvent::Attach { item: p.item.clone(), obs: p.obs });
            }
            (None, Some(_)) => {
                self.attached = None;
                out.push_back(TogetherEvent::Detach);
            }
            (None, None) => {}
        }
        out
    }

    fn land(&mut self, landing: Landing) -> Option<TogetherEvent> {
        match landing {
            Landing::Socket { conn, news } => Some(match news {
                SocketNews::Opened => TogetherEvent::SocketOpened { conn },
                SocketNews::Text(text) => TogetherEvent::SocketText { conn, text },
                SocketNews::Binary(bytes) => TogetherEvent::SocketBinary { conn, bytes },
                SocketNews::Closed => TogetherEvent::SocketClosed { conn },
            }),
            Landing::Done { req, outcome } => {
                let pending = self.inflight.remove(&req)?;
                self.answer(pending, outcome)
            }
        }
    }

    /// The event a landing owes; a failure (`Panicked`, `NotStarted`) lands as the request's
    /// negative answer.
    fn answer(&mut self, pending: Pending, outcome: Outcome) -> Option<TogetherEvent> {
        match (pending, outcome) {
            (Pending::Me, Outcome::Random(Some(b))) => {
                self.me_id = Some(encode_url(&b));
                None
            }
            (Pending::Me, _) => {
                // Asked again on the next pump.
                self.me_asked = false;
                None
            }
            (Pending::Host { item, relay, server, visible, servers }, outcome) => {
                let random = match outcome {
                    Outcome::Random(Some(b)) => b,
                    _ => Vec::new(),
                };
                let me = Me { id: self.me_id.clone()?, name: String::new(), thumb: None };
                Some(TogetherEvent::Host { me, relay, server, item, random, visible, servers })
            }
            (Pending::Derive(tag), Outcome::Derived(keys)) => Some(TogetherEvent::Derived { tag, keys }),
            (Pending::Derive(_), _) => {
                // A derivation that did not run leaves the session that waits on it.
                self.notices.push(Notice::HostFailed);
                Some(TogetherEvent::Leave)
            }
            (Pending::Seal(tag), Outcome::Sealed(Some(bytes))) => Some(TogetherEvent::Sealed { tag, bytes }),
            (Pending::Seal(_), _) => None,
            (Pending::Open(tag), Outcome::Opened(plain)) => Some(TogetherEvent::Opened { tag, plain }),
            (Pending::Open(tag), _) => Some(TogetherEvent::Opened { tag, plain: None }),
            (Pending::Grant(tag), Outcome::Grant(sealed)) => Some(TogetherEvent::GrantSealed { tag, sealed }),
            (Pending::Grant(tag), _) => Some(TogetherEvent::GrantSealed { tag, sealed: None }),
            (Pending::Rooms(tag), Outcome::Body(body)) => Some(TogetherEvent::RoomsAnswered { tag, body }),
            (Pending::Rooms(tag), _) => Some(TogetherEvent::RoomsAnswered { tag, body: None }),
            (Pending::Friends(tag), Outcome::Body(body)) => Some(TogetherEvent::FriendsAnswered { tag, answer: body.as_deref().and_then(FriendsAnswer::read) }),
            (Pending::Friends(tag), _) => Some(TogetherEvent::FriendsAnswered { tag, answer: None }),
            (Pending::Ids(tag), Outcome::Ids(ids)) => Some(TogetherEvent::FriendIdsRead { tag, ids }),
            (Pending::Ids(tag), _) => Some(TogetherEvent::FriendIdsRead { tag, ids: None }),
            (Pending::Key(tag), Outcome::Key(answer)) => Some(TogetherEvent::FriendKey { tag, answer }),
            (Pending::Key(tag), _) => Some(TogetherEvent::FriendKey { tag, answer: Err(JoinError::Failed) }),
        }
    }

    fn work(&mut self, req: TogetherReq, pending: Pending) {
        let id = self.next_req;
        self.next_req = self.next_req.wrapping_add(1).max(1);
        self.inflight.insert(id, pending);
        let addr = Addr { to: MachineId::Together, req: RequestId(id) };
        self.jobs.push(TogetherJob::Work(Job { addr, req }, self.port()));
    }

    fn step_all(&mut self, input: &TogetherInput<'_>, mut events: VecDeque<TogetherEvent>) {
        while let Some(mut ev) = events.pop_front() {
            if let TogetherEvent::Host { me, .. } | TogetherEvent::Join { me, .. } | TogetherEvent::JoinFriend { me, .. } = &mut ev {
                if let Some(m) = self.me(input) {
                    *me = m;
                }
            }
            let mut fx = Vec::new();
            self.session.step(ev, &mut fx);
            for f in fx {
                if let Some(ev) = self.execute(f, input) {
                    events.push_back(ev);
                }
            }
        }
    }

    /// One effect: a job, a frame, a request to the player, a notice. Returns an event to step at
    /// once when the effect is answered without IO (no credential, no account).
    fn execute(&mut self, fx: TogetherFx, input: &TogetherInput<'_>) -> Option<TogetherEvent> {
        match fx {
            TogetherFx::Derive { tag, secret, server, iterations } => self.work(TogetherReq::Derive { secret, server, iterations }, Pending::Derive(tag)),
            TogetherFx::HttpRooms { tag, url, timeout_ms } => self.work(TogetherReq::Rooms { url, timeout_ms }, Pending::Rooms(tag)),
            TogetherFx::OpenSocket { conn, url } => {
                let port = self.port();
                self.jobs.push(TogetherJob::Socket { conn, url, port });
            }
            TogetherFx::CloseSocket { conn } => self.jobs.push(TogetherJob::Close { conn }),
            TogetherFx::SendBinary { conn, bytes } => self.jobs.push(TogetherJob::Send { conn, frame: Frame::Binary(bytes) }),
            TogetherFx::SendText { conn, text } => self.jobs.push(TogetherJob::Send { conn, frame: Frame::Text(text) }),
            TogetherFx::Seal { tag, key, plain } => self.work(TogetherReq::Seal { key, plain }, Pending::Seal(tag)),
            TogetherFx::Open { tag, key, bytes } => self.work(TogetherReq::Open { key, bytes }, Pending::Open(tag)),
            TogetherFx::Player(cmd) => self.player.push(match cmd {
                PlayerCmd::Seek(t) => TogetherPlayerRequest::Seek(t),
                PlayerCmd::Play => TogetherPlayerRequest::Play,
                PlayerCmd::Pause => TogetherPlayerRequest::Pause,
            }),
            TogetherFx::Notice(n) => self.notice(n, input),
            TogetherFx::AskFriends { tag, relay, server } => match input.credential(&server) {
                Some(credential) => self.work(TogetherReq::AskFriends { relay, credential }, Pending::Friends(tag)),
                None => return Some(TogetherEvent::FriendsAnswered { tag, answer: None }),
            },
            TogetherFx::ReadFriendIds { tag } => match input.auth {
                Some(auth) if input.may_announce() => self.work(TogetherReq::FriendIds { auth: auth.clone() }, Pending::Ids(tag)),
                Some(_) | None => return Some(TogetherEvent::FriendIdsRead { tag, ids: None }),
            },
            TogetherFx::SealGrant { tag, request_pub, payload } => self.work(TogetherReq::SealGrant { request_pub, payload }, Pending::Grant(tag)),
            TogetherFx::RequestKey { tag, relay, room, ticket } => self.work(TogetherReq::RequestKey { relay, room, ticket }, Pending::Key(tag)),
        }
        None
    }

    fn notice(&mut self, n: Notice, input: &TogetherInput<'_>) {
        match &n {
            Notice::Item(item) => {
                let position = self.session.view().and_then(|v| v.position);
                self.player.push(TogetherPlayerRequest::Follow { item: item.clone(), position });
            }
            Notice::Action { .. } if !input.settings.notify => return,
            Notice::Opened { .. }
            | Notice::Status(_)
            | Notice::Action { .. }
            | Notice::HostChanged { .. }
            | Notice::HostFailed
            | Notice::JoinFailed(_)
            | Notice::Left
            | Notice::Visible(_)
            | Notice::Listed(_)
            | Notice::FriendSessions(_) => {}
        }
        self.notices.push(n);
    }
}

#[cfg(test)]
#[path = "together_tests.rs"]
mod tests;
