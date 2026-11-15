use super::*;
use crate::stores::watchlist_job::SnapshotData;
use plx_plex::plex::ListingFacts;
use plx_plex::plex::community::{ActivityKind, Friend};
use plx_plex::plex::live::LiveState;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A scripted seam: answers what the test set, and counts every write it was asked for.
struct Script {
    feed: Mutex<Result<Vec<FriendActivity>, WatchlistError>>,
    shares: Result<bool, WatchlistError>,
    write: Result<(), WatchlistError>,
    live: Mutex<Result<Vec<LiveRow>, LiveError>>,
    answers: Vec<Answer>,
    writes: AtomicUsize,
    lives: AtomicUsize,
}

impl Default for Script {
    fn default() -> Self {
        Self {
            feed: Mutex::new(Ok(vec![activity("a", "5d776b59ad5437001f79c6f8", ActivityKind::Movie)])),
            shares: Ok(true),
            write: Ok(()),
            live: Mutex::new(Ok(Vec::new())),
            answers: Vec::new(),
            writes: AtomicUsize::new(0),
            lives: AtomicUsize::new(0),
        }
    }
}

impl CommunityIo for Script {
    fn feed(&self, _: &WatchlistAuth, _: u32) -> Result<Vec<FriendActivity>, WatchlistError> {
        self.feed.lock().unwrap().clone()
    }
    fn shares(&self, _: &WatchlistAuth) -> Result<bool, WatchlistError> {
        self.shares.clone()
    }
    fn set_shares(&self, _: &WatchlistAuth, _: bool) -> Result<(), WatchlistError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.write.clone()
    }
    fn live(&self, _: &Snapshot) -> Result<Vec<LiveRow>, LiveError> {
        self.lives.fetch_add(1, Ordering::SeqCst);
        self.live.lock().unwrap().clone()
    }
    fn matches(&self, _: &Snapshot, _: &[(String, Kind)]) -> Vec<Answer> {
        self.answers.clone()
    }
}

fn activity(id: &str, hex: &str, kind: ActivityKind) -> FriendActivity {
    FriendActivity {
        id: id.into(), date: String::new(), at: Some(0),
        user: Friend { id: "1".into(), name: "Camille".into(), username: None, avatar: None },
        kind, title: "Titre".into(), show: None, season: None, episode: None, year: None, poster: None, art: None,
        catalog_hex: Some(hex.into()),
    }
}

fn session(user: &str) -> LiveSession {
    LiveSession {
        user: user.into(), user_thumb: String::new(), rating_key: "12".into(), kind: "movie".into(), title: "Film".into(),
        show: String::new(), season: 0, episode: 0, thumb: String::new(), view_offset_ms: 1, duration_ms: 2,
        state: LiveState::Playing,
    }
}

struct Rig {
    store: CommunityStore,
    watch: WatchlistInput,
    keys: ClientKeys,
    managed: bool,
    visible: bool,
    /// Every job's request kind, in the order they were emitted.
    log: Vec<&'static str>,
    /// The logical clock handed to each pump (ms, wrapping), started near the wrap so a deadline
    /// that crosses it is exercised by every test.
    now: u32,
}

impl Rig {
    fn new() -> Self {
        Self { store: CommunityStore::default(), watch: WatchlistInput::for_test(1), keys: ClientKeys::default(),
            managed: false, visible: true, log: Vec::new(), now: u32::MAX - 5_000 }
    }

    /// The toggle's press, with this frame's input.
    fn press(&mut self, share: bool) -> bool {
        let input = CommunityInput { watch: &self.watch, managed: self.managed, visible: self.visible, keys: &self.keys };
        self.store.set_share(share, &input)
    }

    /// A pump whose jobs are left on the outbox.
    fn step(&mut self, dt: f32) {
        self.now = self.now.wrapping_add((dt * 1000.0).round() as u32);
        let input = CommunityInput { watch: &self.watch, managed: self.managed, visible: self.visible, keys: &self.keys };
        self.store.pump(&input, self.now);
    }

    /// Run every emitted job through `io` (the runner's part), logging what each asked.
    fn run(&mut self, io: &dyn CommunityIo) {
        for job in self.store.take_jobs() {
            let StoreJob::Community(job, port) = job else { panic!("not a community job") };
            self.log.push(match &job.req {
                CommunityReq::Feed { .. } => "feed",
                CommunityReq::Privacy { .. } => "privacy",
                CommunityReq::SetPrivacy { .. } => "WRITE",
                CommunityReq::Live { .. } => "live",
                CommunityReq::Match { .. } => "match",
            });
            finish_community_job(&port, job.addr, job.req, io);
        }
    }

    /// Refuse every emitted job (the runner's thread ceiling).
    fn refuse(&mut self) {
        for job in self.store.take_jobs() {
            let StoreJob::Community(job, port) = job else { panic!("not a community job") };
            port.refused(job.addr);
        }
    }

    fn pump(&mut self, dt: f32, io: &dyn CommunityIo) -> bool {
        self.now = self.now.wrapping_add((dt * 1000.0).round() as u32);
        let input = CommunityInput { watch: &self.watch, managed: self.managed, visible: self.visible, keys: &self.keys };
        let changed = self.store.pump(&input, self.now);
        self.run(io);
        changed
    }

    fn writes(&self) -> usize {
        self.log.iter().filter(|k| **k == "WRITE").count()
    }
}

/// The owner's rule: a managed Plex Home profile has no Friends page and nothing is read for it,
/// not even while something shows it, and the toggle's command is refused.
#[test]
fn a_managed_profile_reads_nothing_and_writes_nothing() {
    let io = Script::default();
    let mut rig = Rig::new();
    rig.managed = true;
    for _ in 0..50 {
        rig.pump(1.0, &io);
    }
    assert!(!rig.press(false));
    rig.run(&io);
    assert!(rig.log.is_empty(), "{:?}", rig.log);
    assert_eq!(rig.store.feed(), &FeedState::NoAccount);
    assert_eq!(io.lives.load(Ordering::SeqCst), 0);
}

/// A profile that becomes managed (a switch) forgets what the previous one read.
#[test]
fn becoming_managed_forgets_the_feed() {
    let io = Script::default();
    let mut rig = Rig::new();
    rig.pump(0.0, &io);
    rig.pump(0.0, &io);
    assert!(matches!(rig.store.feed(), FeedState::Ready(_)));
    rig.managed = true;
    rig.pump(0.0, &io);
    assert_eq!(rig.store.feed(), &FeedState::NoAccount);
    assert_eq!(rig.store.shares(), ShareState::Unknown);
}

/// Nothing is read while the page is not on screen; showing it reads the feed, the privacy
/// setting and the live sessions once.
#[test]
fn reads_start_when_the_page_shows() {
    let io = Script::default();
    let mut rig = Rig::new();
    rig.visible = false;
    for _ in 0..30 {
        rig.pump(1.0, &io);
    }
    assert!(rig.log.is_empty(), "{:?}", rig.log);
    rig.visible = true;
    rig.pump(0.016, &io);
    assert_eq!(rig.log, ["feed", "privacy", "live"]);
    rig.pump(0.016, &io);
    assert_eq!(rig.store.shares(), ShareState::Known(true));
    let FeedState::Ready(feed) = rig.store.feed() else { panic!("the feed landed") };
    assert_eq!(feed.len(), 1);
}

/// **The privacy write happens only on the explicit command**: hours of pumping, shown and hidden,
/// landed and failed, never emit it; one `set_share` emits exactly one, and its landing is the
/// setting the page shows.
#[test]
fn the_privacy_write_is_emitted_only_by_the_explicit_command() {
    let io = Script { shares: Err(WatchlistError::Transport), ..Script::default() };
    let mut rig = Rig::new();
    for i in 0..2_000 {
        rig.visible = i % 300 < 200;
        rig.pump(0.5, &io);
    }
    assert_eq!(rig.writes(), 0);
    assert_eq!(io.writes.load(Ordering::SeqCst), 0);
    assert_eq!(rig.store.shares(), ShareState::Failed);

    assert!(rig.press(false));
    assert!(!rig.press(true), "one write at a time");
    assert_eq!(rig.store.writing(), Some(false));
    rig.run(&io);
    rig.pump(0.0, &io);
    assert_eq!((rig.writes(), io.writes.load(Ordering::SeqCst)), (1, 1));
    assert_eq!(rig.store.shares(), ShareState::Known(false));
    assert_eq!(rig.store.writing(), None);
    assert!(!rig.store.write_failed());
}

/// A refused or failed write is reported and changes nothing; it is not retried on its own.
#[test]
fn a_failed_write_is_reported_and_not_retried() {
    let io = Script { write: Err(WatchlistError::Refused), ..Script::default() };
    let mut rig = Rig::new();
    rig.pump(0.0, &io);
    rig.pump(0.0, &io);
    rig.press(false);
    rig.run(&io);
    for _ in 0..100 {
        rig.pump(1.0, &io);
    }
    assert_eq!(rig.writes(), 1);
    assert!(rig.store.write_failed());
    assert_eq!(rig.store.shares(), ShareState::Known(true));

    rig.press(false);
    rig.refuse();
    rig.pump(0.0, &io);
    assert!(rig.store.write_failed(), "a write the runner never started is a failure too");
    assert_eq!(rig.writes(), 1, "and is not asked again");
}

/// The live section is hidden when nothing plays, and for good (for this identity) when the
/// account owns no server it may read (401/403); while something plays it is polled at the
/// cadence while the page is up, and not at all while it is not.
#[test]
fn the_live_section_hides_on_forbidden_or_empty_and_polls_while_shown() {
    let io = Script::default();
    let mut rig = Rig::new();
    rig.pump(0.0, &io);
    rig.pump(0.0, &io);
    assert!(rig.store.live().is_empty(), "nothing plays: hidden");

    *io.live.lock().unwrap() = Ok(vec![(ServerId::from_raw(1), session("Camille"))]);
    let before = io.lives.load(Ordering::SeqCst);
    for _ in 0..9 {
        rig.pump(1.0, &io);
    }
    assert_eq!(io.lives.load(Ordering::SeqCst), before, "not before the cadence");
    rig.pump(1.0, &io);
    rig.pump(0.0, &io);
    assert_eq!(io.lives.load(Ordering::SeqCst), before + 1);
    assert_eq!(rig.store.live().len(), 1);

    rig.visible = false;
    for _ in 0..60 {
        rig.pump(1.0, &io);
    }
    assert_eq!(io.lives.load(Ordering::SeqCst), before + 1, "no polling while the page is not shown");

    *io.live.lock().unwrap() = Err(LiveError::Forbidden);
    rig.visible = true;
    rig.pump(0.0, &io);
    rig.pump(0.0, &io);
    assert!(rig.store.live().is_empty(), "401/403: hidden");
    let after = io.lives.load(Ordering::SeqCst);
    for _ in 0..60 {
        rig.pump(1.0, &io);
    }
    assert_eq!(io.lives.load(Ordering::SeqCst), after, "and never asked again for this identity");
}

/// A transient failure keeps what was shown and asks again at the cadence.
#[test]
fn a_failed_live_read_keeps_the_section() {
    let io = Script { live: Mutex::new(Ok(vec![(ServerId::from_raw(1), session("Léo"))])), ..Script::default() };
    let mut rig = Rig::new();
    rig.pump(0.0, &io);
    rig.pump(0.0, &io);
    assert_eq!(rig.store.live().len(), 1);
    *io.live.lock().unwrap() = Err(LiveError::Failed);
    for _ in 0..11 {
        rig.pump(1.0, &io);
    }
    assert_eq!(rig.store.live().len(), 1);
}

/// The owned-server rule: forbidden only when every owned server refused; no server to ask yet is
/// an empty answer (polled again), and one that answers makes the answer.
#[test]
fn live_on_owned_reads_only_owned_reachable_servers() {
    let (a, b, shared) = (ServerId::from_raw(1), ServerId::from_raw(2), ServerId::from_raw(3));
    let input = Snapshot::from(SnapshotData { reach: vec![(a, true, 0), (b, true, 1), (shared, true, 2)], ..SnapshotData::default() });
    let mut asked = Vec::new();
    let got = live_on_owned(&input, |sid| {
        asked.push(sid);
        if sid == a { Err(LiveError::Forbidden) } else { Ok(vec![session("Inès")]) }
    });
    assert_eq!(asked, [a, b], "a shared server is not asked");
    assert_eq!(got.unwrap().iter().map(|(s, _)| *s).collect::<Vec<_>>(), [b]);
    assert_eq!(live_on_owned(&input, |_| Err(LiveError::Forbidden)), Err(LiveError::Forbidden));
    assert_eq!(live_on_owned(&Snapshot::default(), |_| panic!("no server to ask")), Ok(Vec::new()));
}

/// The feed's titles are matched against the servers once per landing; the best ranked copy wins.
#[test]
fn the_feed_is_matched_and_the_best_copy_opens() {
    let guid = "plex://movie/5d776b59ad5437001f79c6f8".to_string();
    let (current, shared) = (ServerId::from_raw(1), ServerId::from_raw(2));
    let io = Script {
        answers: vec![
            (guid.clone(), shared, Asked::Held(ServerMatch::new(shared, 9), ListingFacts::default())),
            (guid.clone(), current, Asked::Held(ServerMatch::new(current, 4), ListingFacts::default())),
        ],
        ..Script::default()
    };
    let mut rig = Rig::new();
    rig.pump(0.0, &io);
    rig.pump(0.0, &io);
    rig.pump(0.0, &io);
    assert_eq!(rig.log.iter().filter(|k| **k == "match").count(), 1);
    // The default snapshot ranks every server 2: the first answer stands until a better one comes.
    assert!(rig.store.held(&guid).is_some());
    assert_eq!(rig.store.held("plex://show/000000000000000000000000"), None);
    for _ in 0..5 {
        rig.pump(1.0, &io);
    }
    assert_eq!(rig.log.iter().filter(|k| **k == "match").count(), 1, "matched once per landing");
}

/// A read the runner refused is owed and asked again after the wait.
#[test]
fn a_refused_read_is_asked_again_after_the_wait() {
    let io = Script::default();
    let mut rig = Rig::new();
    rig.step(0.0);
    rig.refuse();
    rig.pump(0.0, &io);
    assert!(rig.log.is_empty());
    rig.pump(RETRY_MS as f32 / 1000.0, &io);
    assert_eq!(rig.log, ["feed", "privacy", "live"]);
}

/// The feed is read again when the page shows after it went stale, not on every visit.
#[test]
fn a_stale_feed_is_read_again_when_the_page_shows() {
    let io = Script::default();
    let mut rig = Rig::new();
    rig.pump(0.0, &io);
    rig.pump(0.0, &io);
    let feeds = |rig: &Rig| rig.log.iter().filter(|k| **k == "feed").count();
    rig.visible = false;
    rig.pump(10.0, &io);
    rig.visible = true;
    rig.pump(0.0, &io);
    assert_eq!(feeds(&rig), 1, "fresh: not read again");
    rig.visible = false;
    rig.pump(FEED_STALE_MS as f32 / 1000.0, &io);
    rig.visible = true;
    rig.pump(0.0, &io);
    assert_eq!(feeds(&rig), 2);
}

/// Without a plex.tv credential the feed says so and nothing is asked of plex.tv.
#[test]
fn no_credential_reads_no_feed() {
    let io = Script::default();
    let mut rig = Rig::new();
    rig.watch = WatchlistInput::new(WatchlistInput::profile_for_test(1), None, "client");
    rig.pump(0.0, &io);
    assert_eq!(rig.store.feed(), &FeedState::NoAccount);
    assert!(!rig.log.iter().any(|k| matches!(*k, "feed" | "privacy" | "WRITE")), "{:?}", rig.log);
}
