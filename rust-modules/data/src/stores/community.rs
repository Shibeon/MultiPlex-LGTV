//! **The Friends page's store**: the friends' activity feed (community.plex.tv `activityFeed`), the
//! account's "share what I watch" setting (`userPrivacy` / `updatePrivacy`), what plays right now on
//! the servers the account owns (`GET /status/sessions`), and which of the feed's titles the user's
//! servers hold (the Watchlist's guid lookup, `crate::watchlist::match_rows`).
//!
//! Main-thread state plus an `Arc` mailbox. The store starts no worker, reads no registry and no
//! clock: [`CommunityStore::pump`] is handed the plex.tv identity the Bridge read, whether the profile
//! is a managed Plex Home member, whether the page is on screen, the captured server keys and the
//! frame's `dt`, and puts [`StoreJob::Community`] jobs on its outbox that the application's runner
//! starts in the same call (`Bridge::community_feed`).
//!
//! - **Nothing is read for a managed profile** (the owner's rule, shared with My list and the web
//!   app): such a profile has no Friends tab, and a pump that is told `managed` empties the store and
//!   emits nothing.
//! - **Nothing is read while the page is not on screen.** The feed and the privacy setting are read
//!   when the page shows, and the feed again when it shows after [`FEED_STALE_SECS`]. The live
//!   sessions are read when the page shows and every [`LIVE_EVERY_SECS`] while it stays up; a
//!   server that answers 401/403 (the account does not own it) ends the polling for the identity,
//!   and the section is hidden whenever nothing plays.
//! - **The privacy write is a command, never a step's decision**: [`CommunityStore::set_share`] is
//!   called by the Bridge only for the page's explicit press of the toggle. No pump emits it.
//!
//! **Every job is answered once, a refusal included.** A job the runner never started answers
//! `NotStarted` through the port's `refused`: the read is owed again after [`RETRY_SECS`]; a refused
//! write is reported failed (the user presses again). A job that panics lands as a failure.

use super::watchlist_job::{AuthValue, Snapshot};
use super::{StoreId, StoreJob};
use crate::watchlist::model::{Asked, Kind, Reach, ServerMatch};
use crate::watchlist::{match_rows, Answer, WatchlistAuth, WatchlistInput, WatchlistIo};
use plx_machine::job::{Job, Port};
use plx_machine::machine::{Addr, MachineId, RequestId};
use plx_plex::plex::account::AccountClient;
use plx_plex::plex::community::FriendActivity;
use plx_plex::plex::live::{LiveError, LiveSession};
use plx_plex::plex::watchlist::WatchlistError;
use plx_plex::plex::{Client, ClientKeys, KeyedClients, ServerId};
use std::collections::{HashMap, VecDeque};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};

/// How many entries of the feed are read (the web app reads 80).
pub const FEED_FIRST: u32 = 80;
/// The feed is read again when the page shows this long after it last landed.
pub const FEED_STALE_SECS: f32 = 120.0;
/// The live sessions' cadence while the page is up.
pub const LIVE_EVERY_SECS: f32 = 10.0;
/// A refused (never started) read waits this long before it is asked again.
pub const RETRY_SECS: f32 = 2.0;

/// The I/O a community job goes through. Production is [`LiveCommunityIo`]; tests script it.
pub trait CommunityIo: Send + Sync {
    fn feed(&self, auth: &WatchlistAuth, first: u32) -> Result<Vec<FriendActivity>, WatchlistError>;
    fn shares(&self, auth: &WatchlistAuth) -> Result<bool, WatchlistError>;
    /// **The write.** Reached only through a [`CommunityReq::SetPrivacy`] job.
    fn set_shares(&self, auth: &WatchlistAuth, share: bool) -> Result<(), WatchlistError>;
    /// What plays on the captured servers this account owns ([`live_on_owned`]).
    fn live(&self, input: &Snapshot) -> Result<Vec<LiveRow>, LiveError>;
    fn matches(&self, input: &Snapshot, rows: &[(String, Kind)]) -> Vec<Answer>;
}

/// The production seam: plex.tv's community service as the captured identity (re-checked against
/// the seated one by `lease` just before the request), and the captured servers' clients through
/// `lookup` (only while their key still matches the capture).
pub struct LiveCommunityIo {
    pub lookup: fn(ServerId) -> Option<&'static Client>,
    pub lease: fn(&WatchlistAuth) -> bool,
}

impl LiveCommunityIo {
    fn account(&self, auth: &WatchlistAuth) -> Result<AccountClient, WatchlistError> {
        if !(self.lease)(auth) {
            return Err(WatchlistError::Unauthorized);
        }
        Ok(AccountClient::new(&auth.client_id, Some(&auth.credential)))
    }
}

impl CommunityIo for LiveCommunityIo {
    fn feed(&self, auth: &WatchlistAuth, first: u32) -> Result<Vec<FriendActivity>, WatchlistError> {
        let client = self.account(auth)?;
        let mut transport = client.community_transport();
        client.activity_feed(first, &mut transport)
    }
    fn shares(&self, auth: &WatchlistAuth) -> Result<bool, WatchlistError> {
        let client = self.account(auth)?;
        let mut transport = client.community_transport();
        client.shares_watching(&mut transport)
    }
    fn set_shares(&self, auth: &WatchlistAuth, share: bool) -> Result<(), WatchlistError> {
        let client = self.account(auth)?;
        let mut transport = client.community_transport();
        client.set_shares_watching(share, &mut transport)
    }
    fn live(&self, input: &Snapshot) -> Result<Vec<LiveRow>, LiveError> {
        let clients = KeyedClients::new(&input.keys, self.lookup);
        live_on_owned(input, |sid| clients.client(sid).ok_or(LiveError::Failed)?.live_sessions())
    }
    fn matches(&self, input: &Snapshot, rows: &[(String, Kind)]) -> Vec<Answer> {
        let io = super::watchlist_job::ports::LiveWatchlistIo { input: input.clone(), lookup: self.lookup, lease: self.lease };
        match_rows(&io as &dyn WatchlistIo, rows)
    }
}

/// One community job's request. The request id is the job's address.
#[derive(Debug, PartialEq)]
pub enum CommunityReq {
    Feed { auth: AuthValue },
    Privacy { auth: AuthValue },
    /// **The write**: emitted only by [`CommunityStore::set_share`].
    SetPrivacy { auth: AuthValue, share: bool },
    /// What plays on the captured servers this account owns (copy rank 0 or 1, reachable).
    Live { input: Snapshot },
    /// Which captured servers hold the feed's titles.
    Match { input: Snapshot, rows: Vec<(String, Kind)> },
}

/// What a job brings back.
enum Outcome {
    Feed(Result<Vec<FriendActivity>, WatchlistError>),
    Privacy(Result<bool, WatchlistError>),
    SetPrivacy(bool, Result<(), WatchlistError>),
    Live(Result<Vec<(ServerId, LiveSession)>, LiveError>),
    Matched(Snapshot, Vec<Answer>),
    /// The job ran and panicked: whatever it was for failed.
    Panicked,
    NotStarted,
}

struct Landing {
    req: u32,
    outcome: Outcome,
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
pub struct CommunityAnswer(Outcome);

/// Where a community job's ONE answer goes: the store's mailbox as it was when the job was emitted.
/// Two ports are equal when they reach the same mailbox.
#[derive(Clone)]
pub struct CommunityPort {
    mailbox: Arc<Mailbox>,
}

impl PartialEq for CommunityPort {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.mailbox, &other.mailbox)
    }
}

impl std::fmt::Debug for CommunityPort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CommunityPort({:p})", Arc::as_ptr(&self.mailbox))
    }
}

impl Port<CommunityAnswer> for CommunityPort {
    fn done(&self, addr: Addr, out: CommunityAnswer) {
        self.mailbox.post(Landing { req: addr.req.0, outcome: out.0 });
    }

    fn refused(&self, addr: Addr) {
        self.mailbox.post(Landing { req: addr.req.0, outcome: Outcome::NotStarted });
    }
}

fn community_addr(req: u32) -> Addr {
    Addr { to: MachineId::Store(StoreId::Community.ord()), req: RequestId(req) }
}

/// **What plays on the servers the account owns**: `read` asked of every captured server that is
/// reachable and the account's own (copy rank 0, the browsed one, or 1, another it owns; a shared
/// one would answer 403). Any server that answers makes the answer (the others add nothing);
/// `Forbidden` only when there was a server to ask and every one refused; no server to ask yet is
/// an empty answer, so the section is polled again once one is captured.
pub fn live_on_owned(input: &Snapshot, mut read: impl FnMut(ServerId) -> Result<Vec<LiveSession>, LiveError>) -> Result<Vec<LiveRow>, LiveError> {
    let servers: Vec<ServerId> = input.roster().into_iter().filter(|s| input.reachable(*s) && input.copy_rank(*s) <= 1).collect();
    let (mut sessions, mut answered, mut forbidden) = (Vec::new(), servers.is_empty(), 0usize);
    for sid in &servers {
        match read(*sid) {
            Ok(list) => {
                answered = true;
                sessions.extend(list.into_iter().map(|s| (*sid, s)));
            }
            Err(LiveError::Forbidden) => forbidden += 1,
            Err(LiveError::Failed) => {}
        }
    }
    match (answered, forbidden == servers.len()) {
        (true, _) => Ok(sessions),
        (false, true) => Err(LiveError::Forbidden),
        (false, false) => Err(LiveError::Failed),
    }
}

/// The job's IO body: run the request through `io` and post the answer, OUTSIDE the guard, so a
/// panic lands as a failure and never as a missing answer.
pub fn finish_community_job(port: &CommunityPort, addr: Addr, req: CommunityReq, io: &dyn CommunityIo) {
    let outcome = catch_unwind(AssertUnwindSafe(|| match req {
        CommunityReq::Feed { auth } => Outcome::Feed(io.feed(&auth.0, FEED_FIRST)),
        CommunityReq::Privacy { auth } => Outcome::Privacy(io.shares(&auth.0)),
        CommunityReq::SetPrivacy { auth, share } => Outcome::SetPrivacy(share, io.set_shares(&auth.0, share)),
        CommunityReq::Live { input } => Outcome::Live(io.live(&input)),
        CommunityReq::Match { input, rows } => {
            let answers = io.matches(&input, &rows);
            Outcome::Matched(input, answers)
        }
    }))
    .unwrap_or_else(|_| {
        plx_base::eventlog::log("community: a job panicked; it lands as a failure");
        Outcome::Panicked
    });
    port.done(addr, CommunityAnswer(outcome));
}

/// Where the feed stands.
#[derive(Clone, Debug, PartialEq)]
pub enum FeedState {
    /// No plex.tv account to read it as (no credential, or a managed profile).
    NoAccount,
    Loading,
    Failed,
    Ready(Arc<[FriendActivity]>),
}

/// Where the "share what I watch" setting stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShareState {
    /// Not read yet (or no account).
    Unknown,
    /// The read failed.
    Failed,
    /// The account's setting.
    Known(bool),
}

/// One live session and the server it plays on.
pub type LiveRow = (ServerId, LiveSession);

/// A slot's request in flight, or a wait before asking again.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Ask {
    Idle,
    Out(u32),
    /// Refused by the runner: asked again once the wait runs out.
    Owed(f32),
}

impl Ask {
    fn out(self) -> Option<u32> {
        match self {
            Ask::Out(r) => Some(r),
            _ => None,
        }
    }
}

/// Live polling.
#[derive(Clone, Copy, Debug, PartialEq)]
enum LivePoll {
    /// Read when the wait runs out (0: on the next visible pump).
    Every(f32),
    /// A request is out.
    Out(u32),
    /// The account owns no server it may read the sessions of: never asked again for this identity.
    Forbidden,
}

/// The identity the store holds, compared each pump.
#[derive(Clone, Debug, PartialEq)]
struct Identity {
    generation: u32,
    auth: Option<AuthValue>,
}

pub struct CommunityStore {
    identity: Option<Identity>,
    feed: FeedState,
    feed_ask: Ask,
    /// Seconds since the feed last landed.
    feed_age: f32,
    shares: ShareState,
    share_ask: Ask,
    /// The value a write in flight asks for.
    writing: Option<(u32, bool)>,
    /// The last write failed (or was refused): the page says so until the next press.
    write_failed: bool,
    live: Arc<[LiveRow]>,
    live_poll: LivePoll,
    /// Guid to the best held copy (lowest copy rank) the last match found.
    held: HashMap<String, (u8, ServerMatch)>,
    match_ask: Ask,
    /// Moves on every feed landing.
    feed_rev: u32,
    /// The feed landing and roster the last match was asked for; another one asks again.
    matched_for: Option<(u32, u32)>,
    was_visible: bool,
    req: u32,
    mailbox: Arc<Mailbox>,
    jobs: Vec<StoreJob>,
    gen: u32,
}

impl Default for CommunityStore {
    fn default() -> Self {
        Self {
            identity: None,
            feed: FeedState::NoAccount,
            feed_ask: Ask::Idle,
            feed_age: 0.0,
            shares: ShareState::Unknown,
            share_ask: Ask::Idle,
            writing: None,
            write_failed: false,
            live: Arc::from(Vec::new()),
            live_poll: LivePoll::Every(0.0),
            held: HashMap::new(),
            match_ask: Ask::Idle,
            feed_rev: 0,
            matched_for: None,
            was_visible: false,
            req: 0,
            mailbox: Arc::new(Mailbox::default()),
            jobs: Vec::new(),
            gen: 0,
        }
    }
}

/// What a pump is handed: read by the Bridge, never by the store.
pub struct CommunityInput<'a> {
    /// The seated profile and its plex.tv identity (`Bridge::watchlist_input`).
    pub watch: &'a WatchlistInput,
    /// The profile is a managed Plex Home member: nothing is read.
    pub managed: bool,
    /// The Friends page is on screen.
    pub visible: bool,
    /// The captured server keys (the live read and the guid match go through them).
    pub keys: &'a ClientKeys,
}

impl CommunityStore {
    pub fn gen(&self) -> u32 {
        self.gen
    }

    fn touch(&mut self) {
        self.gen = self.gen.wrapping_add(1);
    }

    pub fn feed(&self) -> &FeedState {
        &self.feed
    }

    pub fn shares(&self) -> ShareState {
        self.shares
    }

    /// The value a write in flight asks for.
    pub fn writing(&self) -> Option<bool> {
        self.writing.map(|(_, share)| share)
    }

    pub fn write_failed(&self) -> bool {
        self.write_failed
    }

    /// What plays right now; empty (the section is hidden) when nothing does, the account owns no
    /// server, or nothing was read.
    pub fn live(&self) -> &Arc<[LiveRow]> {
        &self.live
    }

    /// The copy of `guid` a press opens: the best ranked server that holds it.
    pub fn held(&self, guid: &str) -> Option<ServerMatch> {
        self.held.get(guid).map(|(_, m)| *m)
    }

    /// Take the jobs emitted since the last take. The caller OWES each one to a runner.
    #[must_use]
    pub fn take_jobs(&mut self) -> Vec<StoreJob> {
        std::mem::take(&mut self.jobs)
    }

    pub fn has_jobs(&self) -> bool {
        !self.jobs.is_empty()
    }

    /// Forget everything: a new identity, a managed profile, a sign-out. Landings of requests in
    /// flight go to the retired mailbox, and a job not yet taken is dropped with its credential.
    pub fn reset(&mut self) {
        *self = Self { req: self.req, gen: self.gen.wrapping_add(1), ..Self::default() };
    }

    fn emit(&mut self, req: CommunityReq) -> u32 {
        self.req = self.req.wrapping_add(1);
        let id = self.req;
        self.jobs.push(StoreJob::Community(
            Job { addr: community_addr(id), req },
            CommunityPort { mailbox: Arc::clone(&self.mailbox) },
        ));
        id
    }

    fn auth(&self) -> Option<AuthValue> {
        self.identity.as_ref().and_then(|i| i.auth.clone())
    }

    /// **The explicit press of the page's toggle**: ask plex.tv to share (`true`) or stop sharing
    /// what the account watches. Ignored while a write is out, without an account, or for a managed
    /// profile. The ONLY place the write is emitted; the caller drains [`Self::take_jobs`].
    pub fn set_share(&mut self, share: bool, input: &CommunityInput<'_>) -> bool {
        self.adopt(input);
        let Some(auth) = self.auth() else { return false };
        if self.writing.is_some() {
            return false;
        }
        let req = self.emit(CommunityReq::SetPrivacy { auth, share });
        self.writing = Some((req, share));
        self.write_failed = false;
        self.touch();
        true
    }

    /// Try the feed again after it failed (the page's Try again).
    pub fn retry(&mut self) {
        if self.feed == FeedState::Failed {
            self.feed = FeedState::Loading;
            self.feed_ask = Ask::Idle;
            self.touch();
        }
    }

    /// Take the identity `input` names: another profile or credential forgets everything held.
    fn adopt(&mut self, input: &CommunityInput<'_>) {
        let identity = Identity {
            generation: input.watch.profile().generation,
            auth: if input.managed { None } else { input.watch.auth().cloned().map(AuthValue) },
        };
        if self.identity.as_ref() != Some(&identity) {
            if self.identity.is_some() {
                self.reset();
            }
            self.feed = if identity.auth.is_some() { FeedState::Loading } else { FeedState::NoAccount };
            self.identity = Some(identity);
            self.touch();
        }
    }

    /// The frame pass: take the landings, then, while the page is on screen, ask for what is owed.
    /// Returns whether anything the page reads changed. A managed profile empties the store and
    /// asks for nothing.
    pub fn pump(&mut self, input: &CommunityInput<'_>, dt: f32) -> bool {
        let before = self.gen;
        if input.managed {
            if self.identity.is_some() || self.has_jobs() {
                self.reset();
            }
            self.identity = None;
            self.was_visible = input.visible;
            return self.gen != before;
        }
        self.adopt(input);
        self.land();
        let dt = dt.max(0.0);
        self.feed_age += dt;
        let shown = input.visible && !self.was_visible;
        self.was_visible = input.visible;
        if input.visible {
            self.ask_owed(input, dt, shown);
        }
        self.gen != before
    }

    fn ask_owed(&mut self, input: &CommunityInput<'_>, dt: f32, shown: bool) {
        for ask in [&mut self.feed_ask, &mut self.share_ask, &mut self.match_ask] {
            if let Ask::Owed(wait) = ask {
                *wait -= dt;
                if *wait <= 0.0 {
                    *ask = Ask::Idle;
                }
            }
        }
        if let Some(auth) = self.auth() {
            let stale = shown && matches!(self.feed, FeedState::Ready(_)) && self.feed_age >= FEED_STALE_SECS;
            if self.feed_ask == Ask::Idle && (self.feed == FeedState::Loading || stale) {
                self.feed_ask = Ask::Out(self.emit(CommunityReq::Feed { auth: auth.clone() }));
            }
            if self.share_ask == Ask::Idle && self.shares == ShareState::Unknown {
                self.share_ask = Ask::Out(self.emit(CommunityReq::Privacy { auth }));
            }
        }
        if let LivePoll::Every(wait) = self.live_poll {
            let wait = if shown { 0.0 } else { wait - dt };
            self.live_poll = if wait <= 0.0 {
                LivePoll::Out(self.emit(CommunityReq::Live { input: Snapshot::new(input.keys, None) }))
            } else {
                LivePoll::Every(wait)
            };
        }
        if let FeedState::Ready(feed) = &self.feed {
            let wanted = (self.feed_rev, input.keys.roster_gen());
            if self.match_ask == Ask::Idle && self.matched_for != Some(wanted) {
                let mut rows: Vec<(String, Kind)> = Vec::new();
                for a in feed.iter() {
                    let Some(guid) = a.guid() else { continue };
                    let kind = if guid.starts_with("plex://movie/") { Kind::Movie } else { Kind::Show };
                    if !rows.iter().any(|(g, _)| *g == guid) {
                        rows.push((guid, kind));
                    }
                }
                self.matched_for = Some(wanted);
                if !rows.is_empty() {
                    self.match_ask = Ask::Out(self.emit(CommunityReq::Match { input: Snapshot::new(input.keys, None), rows }));
                }
            }
        }
    }

    fn land(&mut self) {
        for Landing { req, outcome } in self.mailbox.take_all() {
            let feed = self.feed_ask.out() == Some(req);
            let share = self.share_ask.out() == Some(req);
            let matched = self.match_ask.out() == Some(req);
            let live = self.live_poll == LivePoll::Out(req);
            let write = self.writing.is_some_and(|(r, _)| r == req);
            match outcome {
                Outcome::NotStarted => {
                    if feed { self.feed_ask = Ask::Owed(RETRY_SECS) }
                    if share { self.share_ask = Ask::Owed(RETRY_SECS) }
                    if matched { self.match_ask = Ask::Owed(RETRY_SECS); self.matched_for = None }
                    if live { self.live_poll = LivePoll::Every(RETRY_SECS) }
                    if write { self.writing = None; self.write_failed = true; self.touch() }
                }
                Outcome::Panicked => {
                    if feed { self.feed_ask = Ask::Idle; self.fail_feed() }
                    if share { self.share_ask = Ask::Idle; self.shares = ShareState::Failed; self.touch() }
                    if matched { self.match_ask = Ask::Idle }
                    if live { self.live_poll = LivePoll::Every(LIVE_EVERY_SECS) }
                    if write { self.writing = None; self.write_failed = true; self.touch() }
                }
                Outcome::Feed(result) if feed => {
                    self.feed_ask = Ask::Idle;
                    match result {
                        Ok(list) => {
                            self.feed = FeedState::Ready(list.into());
                            self.feed_age = 0.0;
                            self.feed_rev = self.feed_rev.wrapping_add(1);
                            self.touch();
                        }
                        Err(_) => self.fail_feed(),
                    }
                }
                Outcome::Privacy(result) if share => {
                    self.share_ask = Ask::Idle;
                    // A write that landed first is the newer answer.
                    if self.writing.is_none() {
                        self.shares = result.map_or(ShareState::Failed, ShareState::Known);
                    }
                    self.touch();
                }
                Outcome::SetPrivacy(share, result) if write => {
                    self.writing = None;
                    match result {
                        Ok(()) => self.shares = ShareState::Known(share),
                        Err(_) => self.write_failed = true,
                    }
                    self.touch();
                }
                Outcome::Live(result) if live => {
                    let next = match result {
                        Ok(rows) => {
                            self.live = rows.into();
                            LivePoll::Every(LIVE_EVERY_SECS)
                        }
                        Err(LiveError::Forbidden) => {
                            self.live = Arc::from(Vec::new());
                            LivePoll::Forbidden
                        }
                        // A failed read keeps what was shown and tries again at the cadence.
                        Err(LiveError::Failed) => LivePoll::Every(LIVE_EVERY_SECS),
                    };
                    self.live_poll = next;
                    self.touch();
                }
                Outcome::Matched(input, answers) if matched => {
                    self.match_ask = Ask::Idle;
                    let mut held: HashMap<String, (u8, ServerMatch)> = HashMap::new();
                    for (guid, sid, asked) in answers {
                        let Asked::Held(m, _) = asked else { continue };
                        let rank = input.copy_rank(sid);
                        if held.get(&guid).is_none_or(|(r, _)| rank < *r) {
                            held.insert(guid, (rank, m));
                        }
                    }
                    self.held = held;
                    self.touch();
                }
                // An answer to a request this store no longer waits for.
                _ => {}
            }
        }
    }

    /// A feed read failed: a feed already shown stays; a first read reads as failed.
    fn fail_feed(&mut self) {
        if !matches!(self.feed, FeedState::Ready(_)) {
            self.feed = FeedState::Failed;
        }
        self.touch();
    }
}

#[cfg(test)]
#[path = "community_tests.rs"]
mod community_tests;
