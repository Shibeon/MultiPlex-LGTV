//! The upcoming-episodes store: owns `GET metadata.provider /library/metadata/<season hex>/children`
//! for the Detail page's season strip (`crate::upcoming`), keyed by the season's catalog id, and
//! the strip the page draws from it.
//!
//! Main-thread state plus an `Arc` mailbox. The store starts no worker and reads no registry or
//! clock: [`UpcomingStore::want`] takes the plex.tv identity from the [`WatchlistInput`] the Bridge
//! read and the interface's language tag as arguments, and puts one [`StoreJob::Upcoming`] on its
//! outbox ([`UpcomingStore::take_jobs`]) that the application's runner starts in the same call
//! (`Bridge::upcoming_feed`). Today's date is an argument of [`UpcomingStore::strip`].
//!
//! It keeps the announced episodes of the seasons of ONE show (the one the page is about): a
//! `want` naming another show forgets the others. A season is read once per show visit; a read
//! that failed is not asked again until the show is left (the strip then simply shows the server's
//! episodes, as it did before this store existed).
//!
//! **Every job is answered once, a refusal included.** A job the runner never started (the thread
//! ceiling) answers [`Outcome::NotStarted`] through the port's `refused`: the next pump owes the
//! season and asks again after [`RETRY_FRAMES`]. A read that panics lands as a failure.

use super::{StoreId, StoreJob};
use crate::metadata::EpisodeList;
use crate::upcoming::{upcoming_episodes, LiveUpcomingIo, ServerSeason, UpcomingEpisode, UpcomingIo};
use crate::watchlist::{encode_hex, WatchlistAuth, WatchlistInput};
use plx_machine::job::{Job, Port};
use plx_machine::machine::{Addr, MachineId, RequestId};
use plx_plex::plex::session::CurrentProfile;
use plx_plex::plex::watchlist::{ProviderEpisode, WatchlistError};
use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};

/// Frames a refused job waits before the next try. A refused job never blocks a frame.
const RETRY_FRAMES: u32 = 120;

/// What a job brings back to the mailbox, whichever way it ended.
enum Outcome {
    /// The provider answered with the season's announced episodes (an unknown season reads empty).
    Read(Vec<ProviderEpisode>),
    /// The read failed (offline, refused, unreadable, panicked): nothing is shown for the season.
    Failed,
    /// The runner never started the job: the season is owed again after [`RETRY_FRAMES`].
    NotStarted,
}

/// One job's answer, tagged with the request it answers.
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

/// The one read a [`StoreJob::Upcoming`] carries: who to ask as, which season, in which language.
/// The request id is the job's address; the seam rides on the port ([`UpcomingPort`]).
#[derive(Clone, Debug)]
pub struct UpcomingReq {
    auth: WatchlistAuth,
    hex: [u8; 12],
    language: String,
}

impl PartialEq for UpcomingReq {
    /// [`WatchlistAuth`] carries a credential and no `PartialEq` of its own: two requests are equal
    /// when they ask the same season in the same language as the same identity.
    fn eq(&self, other: &Self) -> bool {
        self.hex == other.hex
            && self.language == other.language
            && self.auth.client_id == other.auth.client_id
            && self.auth.credential == other.auth.credential
            && self.auth.generation == other.auth.generation
    }
}

impl UpcomingReq {
    /// The season the request reads.
    pub fn hex(&self) -> [u8; 12] {
        self.hex
    }
}

/// A read's answer, opaque to the runner.
pub struct UpcomingAnswer(Outcome);

/// Where an upcoming job's ONE answer goes: the store's mailbox as it was when the job was
/// emitted, and the I/O seam the read goes through. A store reset since holds a new mailbox, and
/// this port still reaches the retired one nobody pumps. Two ports are equal when they reach the
/// same mailbox.
#[derive(Clone)]
pub struct UpcomingPort {
    mailbox: Arc<Mailbox>,
    io: Arc<dyn UpcomingIo>,
}

impl PartialEq for UpcomingPort {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.mailbox, &other.mailbox)
    }
}

impl std::fmt::Debug for UpcomingPort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "UpcomingPort({:p})", Arc::as_ptr(&self.mailbox))
    }
}

impl Port<UpcomingAnswer> for UpcomingPort {
    fn done(&self, addr: Addr, out: UpcomingAnswer) {
        self.mailbox.post(Landing { req: addr.req.0, outcome: out.0 });
    }

    fn refused(&self, addr: Addr) {
        self.mailbox.post(Landing { req: addr.req.0, outcome: Outcome::NotStarted });
    }
}

/// An upcoming job's address: its store and the request id (every job takes a new one).
fn upcoming_addr(req: u32) -> Addr {
    Addr { to: MachineId::Store(StoreId::Upcoming.ord()), req: RequestId(req) }
}

/// The job's IO body: read the season through the port's seam and post the answer. The answer is
/// posted OUTSIDE the guard, so a panic lands as a failure and never as a missing answer.
pub fn finish_upcoming_job(port: &UpcomingPort, addr: Addr, req: UpcomingReq) {
    let UpcomingReq { auth, hex, language } = req;
    let io = &port.io;
    let language = (!language.is_empty()).then_some(language.as_str());
    let outcome = catch_unwind(AssertUnwindSafe(|| match io.read(&auth, &encode_hex(&hex), language) {
        Ok(episodes) => Outcome::Read(episodes),
        // The provider has no such season: nothing is announced for it.
        Err(WatchlistError::NotFound) => Outcome::Read(Vec::new()),
        Err(_) => Outcome::Failed,
    }))
    .unwrap_or_else(|_| {
        plx_base::eventlog::log("upcoming: the read panicked; the season shows no upcoming episodes");
        Outcome::Failed
    });
    port.done(addr, UpcomingAnswer(outcome));
}

/// Where one season of the show stands.
enum SeasonState {
    /// A job is out for it under request `req`.
    Asked { req: u32 },
    /// The runner refused the job: asked again once `wait` reaches [`RETRY_FRAMES`].
    Owed { wait: u32 },
    Landed(Vec<ProviderEpisode>),
    Failed,
}

/// The strip computed for one season's server episodes, and what it was computed from.
struct Memo {
    key: (u64, u64, u32, [u8; 12], Option<String>),
    strip: Arc<[UpcomingEpisode]>,
}

pub struct UpcomingStore {
    io: Arc<dyn UpcomingIo>,
    auth: Option<WatchlistAuth>,
    /// The profile generation under which seasons were settled with NOTHING asked (the input held no
    /// plex.tv credential). Another generation (a sign-in, a profile switch) forgets them, so the
    /// identity seated later asks again ([`Self::pump`]).
    unauthed_gen: Option<u32>,
    /// The language the held seasons were read in. A change forgets them.
    language: String,
    /// The show's seasons this store has asked about, by catalog id.
    seasons: Vec<([u8; 12], SeasonState)>,
    req: u32,
    mailbox: Arc<Mailbox>,
    jobs: Vec<StoreJob>,
    /// Moves whenever a landing or a reset changes what [`Self::strip`] may answer.
    gen: u32,
    memo: Option<Memo>,
}

impl Default for UpcomingStore {
    fn default() -> Self {
        Self::with_io(Arc::new(LiveUpcomingIo))
    }
}

impl UpcomingStore {
    /// A store over a given I/O seam (tests script it; production is [`LiveUpcomingIo`]).
    pub fn with_io(io: Arc<dyn UpcomingIo>) -> Self {
        Self {
            io,
            auth: None,
            unauthed_gen: None,
            language: String::new(),
            seasons: Vec::new(),
            req: 0,
            mailbox: Arc::new(Mailbox::default()),
            jobs: Vec::new(),
            gen: 0,
            memo: None,
        }
    }

    pub fn gen(&self) -> u32 {
        self.gen
    }

    /// Would [`Self::want`] for `hex` in `language` ask for anything? Lets the caller read the
    /// identity input only on a frame that needs it.
    pub fn wants(&self, hex: [u8; 12], language: &str) -> bool {
        language != self.language || self.state(hex).is_none()
    }

    fn state(&self, hex: [u8; 12]) -> Option<&SeasonState> {
        self.seasons.iter().find(|(h, _)| *h == hex).map(|(_, s)| s)
    }

    /// The page shows season `hex` of a show whose seasons are `show` (catalog ids): read its
    /// announced episodes unless they are held, asked, owed or failed. Seasons of another show are
    /// forgotten; an identity that is not the seated profile's, or another language, forgets them
    /// all. With no plex.tv credential in `input` nothing can be asked and nothing is. It does NOT
    /// start the read it emitted: the caller drains [`Self::take_jobs`] in the same call.
    pub fn want(&mut self, hex: [u8; 12], show: &[[u8; 12]], input: &WatchlistInput, language: &str) {
        if self.auth.as_ref().is_none_or(|a| !a.is_current(input.profile())) || language != self.language {
            self.forget();
            self.auth = input.auth().cloned();
            self.language = language.to_string();
        }
        let before = self.seasons.len();
        self.seasons.retain(|(h, _)| show.contains(h));
        if self.seasons.len() != before {
            self.gen = self.gen.wrapping_add(1);
        }
        if self.state(hex).is_some() || !show.contains(&hex) {
            return;
        }
        let Some(auth) = self.auth.clone() else {
            // Nothing can be asked: settle the season so the caller stops offering it every frame,
            // until the seated profile changes (`pump`).
            self.unauthed_gen = Some(input.profile().generation);
            self.seasons.push((hex, SeasonState::Failed));
            return;
        };
        self.ask(hex, auth);
    }

    fn ask(&mut self, hex: [u8; 12], auth: WatchlistAuth) {
        self.req = self.req.wrapping_add(1);
        let req = self.req;
        self.jobs.push(StoreJob::Upcoming(
            Job { addr: upcoming_addr(req), req: UpcomingReq { auth, hex, language: self.language.clone() } },
            self.upcoming_port(),
        ));
        match self.seasons.iter_mut().find(|(h, _)| *h == hex) {
            Some((_, state)) => *state = SeasonState::Asked { req },
            None => self.seasons.push((hex, SeasonState::Asked { req })),
        }
    }

    /// Forget every season and every request in flight (their landings go to a retired mailbox).
    fn forget(&mut self) {
        self.seasons.clear();
        self.mailbox = Arc::new(Mailbox::default());
        self.jobs.clear();
        self.memo = None;
        self.gen = self.gen.wrapping_add(1);
    }

    /// Sign-out or profile switch: forget the identity, the seasons and any request in flight. A
    /// job not yet taken to a runner is dropped with it, credential and all.
    pub fn reset(&mut self) {
        self.forget();
        self.auth = None;
        self.unauthed_gen = None;
        self.language.clear();
    }

    /// Take the jobs the step has emitted since the last take, in order. The caller OWES each one
    /// to a runner.
    #[must_use]
    pub fn take_jobs(&mut self) -> Vec<StoreJob> {
        std::mem::take(&mut self.jobs)
    }

    pub fn has_jobs(&self) -> bool {
        !self.jobs.is_empty()
    }

    /// The port that lands an answer into THIS store's current mailbox.
    pub fn upcoming_port(&self) -> UpcomingPort {
        UpcomingPort { mailbox: Arc::clone(&self.mailbox), io: Arc::clone(&self.io) }
    }

    /// Frame pass: ask again for a season the runner refused, then take the landings. Returns
    /// whether a season's announced episodes changed. `profile` is the one the Bridge read for this
    /// frame: an identity that is no longer seated forgets everything, and the next `want` asks
    /// again under the new one. Like [`Self::want`], a retry it emits is drained by the caller.
    pub fn pump(&mut self, profile: &CurrentProfile) -> bool {
        let unauthed_left = self.auth.is_none() && self.unauthed_gen.is_some_and(|g| g != profile.generation);
        if unauthed_left || self.auth.as_ref().is_some_and(|a| !a.is_current(profile)) {
            self.reset();
            return true;
        }
        let mut retry = Vec::new();
        for (hex, state) in &mut self.seasons {
            if let SeasonState::Owed { wait } = state {
                *wait += 1;
                if *wait >= RETRY_FRAMES {
                    retry.push(*hex);
                }
            }
        }
        if let Some(auth) = self.auth.clone() {
            for hex in retry {
                self.ask(hex, auth.clone());
            }
        }
        let mut changed = false;
        for landing in self.mailbox.take_all() {
            let Some((_, state)) = self.seasons.iter_mut()
                .find(|(_, s)| matches!(s, SeasonState::Asked { req } if *req == landing.req)) else { continue };
            *state = match landing.outcome {
                Outcome::Read(episodes) => {
                    changed = true;
                    SeasonState::Landed(episodes)
                }
                Outcome::Failed => SeasonState::Failed,
                Outcome::NotStarted => SeasonState::Owed { wait: 0 },
            };
        }
        if changed {
            self.gen = self.gen.wrapping_add(1);
        }
        changed
    }

    /// The announced episodes of season `hex`, once they landed.
    pub fn announced(&self, hex: [u8; 12]) -> Option<&[ProviderEpisode]> {
        match self.state(hex)? {
            SeasonState::Landed(episodes) => Some(episodes),
            _ => None,
        }
    }

    /// The upcoming episodes the strip of season `hex` appends after the server's `episodes`, as of
    /// `today` (`YYYY-MM-DD`: the viewer's local calendar date, as the app publishes it with
    /// `Bridge::publish_today`; `None` when unknown). Empty while the season's announcement has not
    /// landed or the server's list says too little ([`ServerSeason::of`]). Computed once per change
    /// of its inputs; the `Arc` is shared with the caller.
    pub fn strip(&mut self, hex: [u8; 12], episodes: &EpisodeList, today: Option<&str>) -> Arc<[UpcomingEpisode]> {
        let key = (episodes.id(), episodes.rev(), self.gen, hex, today.map(str::to_string));
        if let Some(memo) = self.memo.as_ref().filter(|m| m.key == key) {
            return Arc::clone(&memo.strip);
        }
        let strip: Arc<[UpcomingEpisode]> = match (self.announced(hex), ServerSeason::of(episodes)) {
            (Some(announced), Some(server)) => upcoming_episodes(announced, &server, today).into(),
            _ => Arc::from(Vec::new()),
        };
        self.memo = Some(Memo { key, strip: Arc::clone(&strip) });
        strip
    }
}

#[cfg(test)]
#[path = "upcoming_tests.rs"]
mod upcoming_tests;
