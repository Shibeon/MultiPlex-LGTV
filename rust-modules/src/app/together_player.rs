//! **Watch Together's player side**, once a frame while a session is live: what the session asked
//! of the player is performed, every Follow goes through the one gate, and what the player plays
//! is handed back as the session's snapshot. The decisions are `plx_data::together::player`'s
//! (pure); this is where they meet the player, through the doors every other caller uses.
//!
//! - **Seek, Play, Pause** go to `plx_media::player::together::perform` (the backend-agnostic seek
//!   and the one transport door), only while the session drives this playback (its last snapshot
//!   was handed). They go around the application's gesture count (`route::note_gesture`), so the
//!   session never reads its own change back as the person's. They waive no resume reset: only a
//!   playback a Follow opened owes none (`route::waive_resume_reset`), so a seek on the host's own
//!   Play from Start leaves that reset owed.
//! - **Follow** opens the session's title through the launch-link wait (`MenuPlayAwait::for_together`):
//!   the detail request, the play door, `request_loaded_hero` and `start_playback`, as a normal play
//!   does, at the session's position now (`View::position`, else the Follow's own). Nothing plays: it opens. The same title plays: nothing to do.
//!   The session's previous title plays: it is replaced in place. The person's own title plays, or
//!   their own play waits for its page (a card's or a link's wait, never overwritten): the gate
//!   holds the Follow as a question (`FollowState::Asking`) for the UI, answered with
//!   [`TogetherFeed::answer_follow`]. The wait carries what the gate let it replace, and the gate
//!   decides again when the page lands ([`Bridge::together_still_opens`]): a playback the person
//!   started meanwhile is asked about, never stopped. The session ending takes the opening with it,
//!   and a landing with no session opens nothing. A title on no server this device reaches is not
//!   opened (`FollowState::Missing`); the gate fails closed.
//! - **Held** while the app is hidden or the foreground lifecycle brings its parked playback back
//!   (the launch link's own predicates, `window_activity.allow_present` and `awaiting_load`): the
//!   session's requests stay queued, a Follow's opening in flight goes back to the queue to be
//!   gated afresh (and opened at the session's place then), and no snapshot is handed, so the session neither drives a parked playback nor
//!   reads its clock.
//! - **The snapshot** is the playing title (identified once per playback, `SharedItem` as the web's
//!   `sharedOf` builds it) and the player's observation (`plx_media::player::together::facts`). A
//!   participant's player is handed only while it plays the session's title (`attachable`); no player
//!   page, no engine, or a failed one hands nothing, which the session hears as the player leaving.
use plx_data::stores::together::{PlayerSnapshot, TogetherCmd, TogetherPlayerRequest};
use plx_data::together::player::{self as gate, FollowAnswer, FollowAsk, FollowState, FollowVerdict};
use plx_data::together::session::View;
use plx_data::together::sync::PlayerCmd;
use plx_data::together::wire::SharedItem;

use super::super::{AppHost, Bridge};
use super::TogetherFeed;

/// Everything the step reaches, the application's own (the player machine, its adapter, the pages
/// and the launch wait).
pub(crate) struct PlayerSide<'a> {
    pub(crate) player: &'a mut plx_media::player::machine::Player,
    pub(crate) pa: &'a mut plx_media::player::adapter::PlayerAdapter,
    pub(crate) pages: &'a mut plx_ui::dispatch::Dispatcher<AppHost>,
    pub(crate) menu_play_await: &'a mut Option<crate::app::input::MenuPlayAwait>,
    pub(crate) now: u32,
    /// The app is hidden, or the foreground lifecycle is bringing the parked playback back: the
    /// player is not this step's to drive or to read.
    pub(crate) held: bool,
}

impl TogetherFeed {
    /// Where the gate stands with the session's last Follow, for the UI.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn follow_state(&self) -> &FollowState {
        &self.follow
    }

    /// The person's answer to the gate's question ([`FollowState::Asking`]), performed next frame.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn answer_follow(&mut self, answer: FollowAnswer) {
        self.answer = Some(answer);
        plx_machine::idle::invalidate();
    }

    /// The session drives the player: the last snapshot an unheld frame handed was one (a held
    /// frame hands none, and does not forget this).
    fn driving(&self) -> bool {
        self.drives
    }

    /// What the player plays. Its `(server, ratingKey)` is known from the request on, which is all
    /// the gate needs (and a title it cannot name is a different title); the whole item, the one
    /// the session is handed, once the leaf's own data has landed. That item is identified once per
    /// playback: the session compares items whole, so a title that grew its guid a frame later
    /// would read as a new one.
    fn playing_item(&mut self, ps: &plx_media::route::PlaybackSession, meta: plx_data::metadata::MetadataView<'_>) -> Option<Playing> {
        let (sid, rk) = match plx_media::route::requested_item(ps) {
            Some((sid, rk)) => (sid, rk.to_owned()),
            None => (plx_media::route::cur_sid(ps), plx_media::route::cur_rk(ps)),
        };
        if rk.is_empty() {
            return None;
        }
        if let Some(((s, r), item)) = &self.item {
            if plx_plex::plex::same_item((*s, r), (sid, &rk)) {
                return Some(Playing { item: item.clone(), known: true });
            }
        }
        match shared_item(sid, &rk, meta) {
            Some(item) => {
                self.item = Some(((sid, rk), item.clone()));
                Some(Playing { item, known: true })
            }
            None => {
                let server = machine_of(sid);
                let item = SharedItem { guid: String::new(), server, key: Some(rk), kind: String::new(), title: String::new(), sub: None };
                Some(Playing { item, known: false })
            }
        }
    }
}

/// What the player plays, as far as it is known ([`TogetherFeed::playing_item`]).
#[derive(Clone)]
struct Playing {
    item: SharedItem,
    /// The leaf's own data has landed: the item is whole and may be handed to the session.
    known: bool,
}

fn machine_of(sid: plx_plex::plex::ServerId) -> Option<String> {
    plx_plex::plex::client_for(sid).map(|c| c.machine_id().to_owned()).filter(|m| !m.is_empty())
}

/// The playing leaf as the session names it (the web's `sharedOf`): its portable guid when the
/// loaded page is that leaf, its server's identifier and rating key, its type, and a title an
/// episode takes from its show (`S1 · E2 — Name` under it). `None` until the leaf's own data has
/// landed (`playing()` names it), so no half-known title is ever attached.
fn shared_item(sid: plx_plex::plex::ServerId, rk: &str, view: plx_data::metadata::MetadataView<'_>) -> Option<SharedItem> {
    let playing = view.playing().filter(|p| plx_plex::plex::same_item((p.sid, &p.rk), (sid, rk)))?;
    let guid = view.current().filter(|d| plx_plex::plex::same_item((d.sid, &d.rk), (sid, rk))).map(|d| d.guid.clone()).unwrap_or_default();
    let np = view.now_playing();
    let episode = playing.kind == "episode" || np.is_some_and(|n| n.is_real_episode);
    let (title, sub) = match np {
        Some(n) if episode => (n.title.clone(), Some(format!("S{} · E{} — {}", n.season, n.index, n.ep_title))),
        Some(n) => (n.title.clone(), (n.year > 0).then(|| n.year.to_string())),
        None => (String::new(), None),
    };
    let kind = if playing.kind.is_empty() { if episode { "episode" } else { "movie" }.to_owned() } else { playing.kind.clone() };
    Some(SharedItem { guid, server: machine_of(sid), key: Some(rk.to_owned()), kind, title, sub })
}

impl Bridge {
    /// Once a frame, after the together pump and before the launch wait's tick: the live session's
    /// player side ([`Bridge::together_player_step`]).
    pub(crate) fn together_player_frame(&mut self, side: PlayerSide<'_>) {
        let view = self.together.view();
        self.together_player_step(view.as_ref(), side);
    }

    /// Perform what the session asked, gate its Follows, and hand it the player's snapshot, `view`
    /// being the live session's (none: nothing to drive, nothing to ask, and a request left over
    /// is moot).
    pub(crate) fn together_player_step(&mut self, view: Option<&View>, side: PlayerSide<'_>) {
        let PlayerSide { player, pa, pages, menu_play_await, now, held } = side;
        let Some(view) = view else {
            // The session is over: so is a Follow's opening in flight (its title is no one's now).
            if menu_play_await.as_ref().is_some_and(crate::app::input::MenuPlayAwait::is_together) {
                *menu_play_await = None;
            }
            let together = &mut self.together;
            together.live = false;
            let _ = together.take_player_requests();
            together.answer = None;
            together.follow = FollowState::Idle;
            together.item = None;
            together.drives = false;
            together.set_player(None);
            return;
        };
        self.together.live = true;
        if held {
            self.together_hold(menu_play_await);
            return;
        }
        let playing = self.together_playing(&player.session, pages);
        let playing_item = playing.as_ref().map(|p| &p.item);
        for req in self.together.take_player_requests() {
            let cmd = match req {
                TogetherPlayerRequest::Seek(t) => PlayerCmd::Seek(t),
                TogetherPlayerRequest::Play => PlayerCmd::Play,
                TogetherPlayerRequest::Pause => PlayerCmd::Pause,
                TogetherPlayerRequest::Follow { item, position } => {
                    // The session's place now: a Follow held through a hide is not opened at the
                    // place it came with (a stale start costs a second seek, a reload on a transcode).
                    let position = view.position.or(position);
                    match gate::follow_verdict(&item, playing_item, self.together.driving()) {
                        FollowVerdict::Already => self.together.follow = FollowState::Idle,
                        FollowVerdict::Start | FollowVerdict::Switch => {
                            self.together_open(item, position, playing_item.cloned(), false, menu_play_await, now);
                        }
                        FollowVerdict::Ask => {
                            let playing = playing_item.cloned().unwrap_or_else(|| item.clone());
                            self.together.follow = FollowState::Asking(FollowAsk { item, position, playing });
                            plx_machine::idle::invalidate();
                        }
                    }
                    continue;
                }
            };
            if !self.together.driving() || playing.is_none() {
                continue; // the session's last word was about a player it no longer drives
            }
            // A refusal is asked again by the sync rule; a seek waives nothing (the module doc).
            let _ = plx_media::player::together::perform(&mut player.session, pa, cmd);
        }
        if let Some(answer) = self.together.answer.take() {
            match (answer, std::mem::take(&mut self.together.follow)) {
                (FollowAnswer::Join, FollowState::Asking(ask)) => {
                    // The person's consent: what plays now (or their own pending play) is left.
                    let position = view.position.or(ask.position);
                    self.together_open(ask.item, position, playing_item.cloned(), true, menu_play_await, now);
                }
                (FollowAnswer::Leave, FollowState::Asking(_)) => self.together.command(TogetherCmd::Leave),
                (_, other) => self.together.follow = other,
            }
        }
        // An opening ends once the wait has asked the player for the title (from there the snapshot
        // carries it, and a failed start is the player leaving), or on the wait giving up without
        // asking (its server never answered, or nothing in it was playable): a miss.
        if let FollowState::Opening { item } = &self.together.follow {
            let waiting = menu_play_await.as_ref().is_some_and(|w| w.is_together());
            let requested = plx_media::route::requested_item(&player.session)
                .is_some_and(|(sid, rk)| item.key.as_deref() == Some(rk) && item.server.is_some() && machine_of(sid) == item.server);
            if requested {
                self.together.follow = FollowState::Idle;
            } else if !waiting {
                let item = item.clone();
                plx_base::eventlog::log(&format!("together: {} could not be opened", item.identity()));
                self.together.follow = FollowState::Missing { item };
                plx_machine::idle::invalidate();
            }
        }
        let snapshot = playing
            .filter(|p| p.known && gate::attachable(view.host, view.item.as_ref(), &p.item))
            .and_then(|p| {
                let facts = plx_media::player::together::facts(&player.session, pa);
                gate::observation(&facts).map(|obs| PlayerSnapshot { item: p.item, obs })
            });
        self.together.drives = snapshot.is_some();
        self.together.set_player(snapshot);
    }

    /// What the player plays for the gate: nothing without the player page, or under a preview.
    fn together_playing(&mut self, ps: &plx_media::route::PlaybackSession, pages: &plx_ui::dispatch::Dispatcher<AppHost>) -> Option<Playing> {
        if super::super::player(pages).is_none() || plx_media::route::is_preview(ps) {
            return None;
        }
        self.together.playing_item(ps, self.stores.metadata_view())
    }

    /// A held frame: nothing is performed and nothing is read. The requests stay queued (the
    /// answer to a question too), a Follow's opening in flight goes back to the front of the queue
    /// to be gated afresh once the player is the app's again, and the session is handed no
    /// snapshot.
    fn together_hold(&mut self, menu_play_await: &mut Option<crate::app::input::MenuPlayAwait>) {
        if menu_play_await.as_ref().is_some_and(crate::app::input::MenuPlayAwait::is_together) {
            if let Some(open) = menu_play_await.take().and_then(crate::app::input::MenuPlayAwait::into_together) {
                self.together.requests.insert(0, TogetherPlayerRequest::Follow { item: open.item, position: open.position });
            }
        }
        if matches!(self.together.follow, FollowState::Opening { .. }) {
            self.together.follow = FollowState::Idle;
        }
        self.together.set_player(None);
    }

    /// Open the session's title at its position, through the launch-link wait, replacing
    /// `replaces` (what played when the gate decided; `None`: nothing). A title on no server this
    /// device reaches is not opened. The person's own play waiting for its page is never
    /// overwritten without their `consent` (a Join): the Follow is held as the gate's question.
    fn together_open(&mut self, item: SharedItem, position: Option<f64>, replaces: Option<SharedItem>, consent: bool,
        menu_play_await: &mut Option<crate::app::input::MenuPlayAwait>, now: u32) {
        let sid = item.server.as_deref().and_then(plx_plex::plex::id_of_machine);
        let rk = item.key.clone().filter(|k| !k.is_empty());
        let (Some(sid), Some(rk)) = (sid, rk) else {
            plx_base::eventlog::log(&format!("together: {} is on no server this device reaches", item.identity()));
            self.together.follow = FollowState::Missing { item };
            plx_machine::idle::invalidate();
            return;
        };
        if let Some(pending) = menu_play_await.as_ref().filter(|w| !consent && !w.is_together()) {
            let (psid, prk) = pending.title();
            let playing = SharedItem { guid: String::new(), server: machine_of(psid), key: Some(prk.to_owned()), kind: String::new(), title: String::new(), sub: None };
            plx_base::eventlog::log(&format!("together: {} asks first: the person's own play waits for rk={prk}", item.identity()));
            self.together.follow = FollowState::Asking(FollowAsk { item, position, playing });
            plx_machine::idle::invalidate();
            return;
        }
        let at_ns = position.filter(|p| p.is_finite()).map_or(0, |p| (p.max(0.0) * 1e9) as i64);
        self.metadata_run(plx_data::stores::metadata::MetadataCmd::RequestDetail { sid, rk: rk.clone() });
        let open = crate::app::input::TogetherOpen { at_ns, item: item.clone(), position, replaces };
        *menu_play_await = Some(crate::app::input::MenuPlayAwait::for_together(sid, rk, open, now));
        self.together.follow = FollowState::Opening { item };
        plx_machine::idle::invalidate();
    }

    /// The gate again, as a Follow's page lands (`input::play_loaded_hero`): nothing opens once the
    /// session is over, and the wait may play only over what the gate let it replace (or over
    /// nothing). The same title playing by now ends the
    /// opening (`Already`); any other playback (the person started their own while the page loaded)
    /// holds the Follow as the gate's question, and nothing is stopped.
    pub(crate) fn together_still_opens(&mut self, ps: &plx_media::route::PlaybackSession,
        pages: &plx_ui::dispatch::Dispatcher<AppHost>, open: &crate::app::input::TogetherOpen) -> bool {
        if !self.together.live {
            plx_base::eventlog::log(&format!("together: {} not opened: the session is over", open.item.identity()));
            self.together.follow = FollowState::Idle;
            return false;
        }
        let playing = self.together_playing(ps, pages);
        let now = playing.as_ref().map(|p| &p.item);
        let replaced = match (now, open.replaces.as_ref()) {
            (None, _) => true,
            (Some(now), Some(then)) => gate::same_title(now, then),
            (Some(_), None) => false,
        };
        match gate::follow_verdict(&open.item, now, self.together.driving()) {
            FollowVerdict::Start => true,
            FollowVerdict::Switch | FollowVerdict::Ask if replaced => true,
            FollowVerdict::Already => {
                self.together.follow = FollowState::Idle;
                plx_machine::idle::invalidate();
                false
            }
            FollowVerdict::Switch | FollowVerdict::Ask => {
                let playing = now.cloned().unwrap_or_else(|| open.item.clone());
                plx_base::eventlog::log(&format!("together: {} asks first: another title plays now", open.item.identity()));
                self.together.follow = FollowState::Asking(FollowAsk { item: open.item.clone(), position: open.position, playing });
                plx_machine::idle::invalidate();
                false
            }
        }
    }
}

/// The player side against the production launch path: the gate, the launch-link wait
/// (`input::menu_play_tick`, its play door and `request_loaded_hero`), `start_playback` and the
/// route's request, with a loopback server registered under the session's server identifier and
/// the title's page seeded as landed. The session itself is a `View` handed to the step (a live
/// session needs the relay: `adapters::together_e2e_tests`).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::bridge::test_support::Pms;
    use plx_data::metadata::Detail;
    use plx_data::stores::metadata::MetadataCmd;
    use plx_data::together::session::Status;
    use plx_screens::registry::AppArg;

    const MACHINE: &str = "machine-du-salon";
    const FILM: &str = "film-42";

    fn refuse(_path: &str) -> (&'static str, String) {
        ("404 Not Found", String::new())
    }

    fn film() -> SharedItem {
        SharedItem {
            guid: "plex://movie/5d776b59ad5437001f79c6f8".into(),
            server: Some(MACHINE.into()),
            key: Some(FILM.into()),
            kind: "movie".into(),
            title: "Le Film".into(),
            sub: Some("2019".into()),
        }
    }

    fn view(host: bool, item: Option<SharedItem>, position: Option<f64>) -> View {
        View {
            code: "ABCD-EFGH".into(),
            full_code: None,
            host,
            status: Status::Open,
            item,
            peers: Vec::new(),
            position,
            visible: false,
            listed: None,
        }
    }

    /// The film's page as its detail request lands it: the person had stopped it at 45:00.
    fn film_page(sid: plx_plex::plex::ServerId) -> Detail {
        Detail {
            sid,
            rk: FILM.into(),
            guid: film().guid,
            kind: "movie".into(),
            title: "Le Film".into(),
            part: "/library/parts/42/file.mkv".into(),
            resume_ms: 2_700_000,
            dur_ms: 6_000_000,
            ..Default::default()
        }
    }

    struct Rig {
        player: plx_media::player::machine::Player,
        pa: plx_media::player::adapter::PlayerAdapter,
        pages: plx_ui::dispatch::Dispatcher<AppHost>,
        bridge: Bridge,
        wait: Option<crate::app::input::MenuPlayAwait>,
        sid: plx_plex::plex::ServerId,
        now: u32,
        _pms: Pms,
        _guard: plx_base::testlock::Serial,
    }

    impl Rig {
        /// The film's page has landed (a Follow's detail request is answered by it), its server is
        /// registered under the session's identifier, and the person had stopped it at 45:00.
        fn new() -> Rig {
            let guard = plx_base::testlock::serial();
            assert!(plx_net::net::global_init() && plx_net::net::available() && plx_net::net::threaded_tls_ready());
            let pms = Pms::serve(refuse);
            let mt = unsafe { plx_base::task::MainThread::assume() };
            let mut bridge = Bridge::for_test(|| 0);
            plx_plex::plex::reset_servers_for_test();
            let sid = plx_plex::plex::register_for_test(MACHINE, "127.0.0.1", pms.port, "t", "c-together");
            plx_plex::plex::client_for(sid).unwrap().set_link(plx_plex::plex::probe::Location::Local);
            plx_data::metadata::set_current_for_test(bridge.metadata_mut().state_mut(), Some(film_page(sid)));
            Rig {
                player: plx_media::player::machine::Player::new(),
                pa: plx_media::player::adapter::PlayerAdapter::new(mt),
                pages: plx_ui::dispatch::Dispatcher::<AppHost>::new(),
                bridge,
                wait: None,
                sid,
                now: 0,
                _pms: pms,
                _guard: guard,
            }
        }

        /// One frame of the run loop's order: the together step, then the launch wait.
        fn frame(&mut self, view: &View) {
            self.frame_as(view, false);
        }

        /// One frame with the app hidden or its parked playback being restored (`held`).
        fn frame_as(&mut self, view: &View, held: bool) {
            let _frame = plx_base::task::FrameScope::enter();
            self.now += 16;
            self.bridge.together_player_step(Some(view), PlayerSide {
                player: &mut self.player,
                pa: &mut self.pa,
                pages: &mut self.pages,
                menu_play_await: &mut self.wait,
                now: self.now,
                held,
            });
            if self.wait.is_some() {
                unsafe {
                    crate::app::input::menu_play_tick(&mut self.player.session, &mut self.pa, &mut self.pages, &mut self.bridge, &mut self.wait, self.now);
                }
            }
        }

        /// A frame with no live session (the person left, or the host ended it).
        fn frame_without_session(&mut self) {
            let _frame = plx_base::task::FrameScope::enter();
            self.now += 16;
            self.bridge.together_player_step(None, PlayerSide {
                player: &mut self.player,
                pa: &mut self.pa,
                pages: &mut self.pages,
                menu_play_await: &mut self.wait,
                now: self.now,
                held: false,
            });
            self.tick_wait();
        }

        /// The launch wait's tick alone.
        fn tick_wait(&mut self) {
            if self.wait.is_some() {
                unsafe {
                    crate::app::input::menu_play_tick(&mut self.player.session, &mut self.pa, &mut self.pages, &mut self.bridge, &mut self.wait, self.now);
                }
            }
        }

        /// Put the player page on top, as a playback's start does.
        fn mount_player(&mut self) {
            super::super::show_page(&mut self.pages, AppArg::Player);
            for _ in 0..60 {
                self.now += 16;
                let tick = plx_ui::fixture::tick(self.now);
                super::super::frame(&mut self.pages, &mut self.bridge, tick, Vec::new());
            }
            assert!(super::super::player(&self.pages).is_some(), "the player page is up");
        }

        fn follow(&mut self, item: SharedItem, position: Option<f64>) {
            self.bridge.together.requests.push(TogetherPlayerRequest::Follow { item, position });
        }

        /// The person plays `rk` on the player page.
        fn play_own(&mut self, rk: &str) {
            let mut meta = plx_data::stores::metadata::MetadataStore::default();
            assert!(plx_media::route::request_play(&mut self.player.session, &mut meta, plx_platform::devcaps::caps(), self.sid,
                rk, "/library/parts/7/file.mkv", "h264", "aac", "Autre", "", None, ""));
            self.mount_player();
        }

        /// The host plays the film (its own Play from Start) and its leaf has landed: the step
        /// hands it to the session.
        fn host_plays_the_film(&mut self, session: &View) {
            let mut meta = plx_data::stores::metadata::MetadataStore::default();
            assert!(plx_media::route::request_play(&mut self.player.session, &mut meta, plx_platform::devcaps::caps(), self.sid,
                FILM, "/library/parts/42/file.mkv", "h264", "aac", "Le Film", "", None, ""));
            self.mount_player();
            self.bridge.metadata_run(MetadataCmd::SetNowPlaying(Some(plx_data::metadata::NowPlaying {
                is_episode: false, is_real_episode: false, is_movie: true, title: "Le Film".into(), ep_title: String::new(),
                season: 0, index: 0, summary: String::new(), year: 2019, dur_ms: 6_000_000, rating: String::new(),
                thumb: String::new(), detail_rk: FILM.into(),
            })));
            plx_data::metadata::set_playing_for_test(self.bridge.metadata_mut().state_mut(), self.sid, FILM, "movie");
            self.frame(session);
            assert!(self.bridge.together.player.is_some(), "the host's playback is handed");
        }

        fn requested(&self) -> Option<(plx_plex::plex::ServerId, String)> {
            plx_media::route::requested_item(&self.player.session).map(|(s, r)| (s, r.to_owned()))
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            // As `menu_play_season_tests`' rig: withdraw a resolve this test began and join its
            // worker before the registry goes.
            plx_media::route::cancel_play(&mut self.player.session);
            let joined = plx_media::route::wait_resolve_workers_for_test(std::time::Duration::from_secs(5));
            self.bridge.metadata_mut().run(MetadataCmd::Clear);
            plx_plex::plex::reset_servers_for_test();
            assert!(joined || std::thread::panicking(), "a resolve worker outlived its test");
        }
    }

    /// The host plays the film; the participant's player is idle. The Follow opens it through the
    /// launch wait at the session's place (10:00), not at the 45:00 the person had left it at, and
    /// that playback owes no resume reset when it stops.
    #[test]
    fn a_participant_follows_the_hosts_title_at_the_sessions_place() {
        let mut rig = Rig::new();
        let session = view(false, Some(film()), Some(600.0));
        rig.follow(film(), Some(600.0));
        rig.frame(&session);
        assert_eq!(rig.bridge.together.follow_state(), &FollowState::Opening { item: film() });
        assert!(rig.wait.is_none(), "the page had landed: the wait played on its first tick");
        assert_eq!(rig.requested(), Some((rig.sid, FILM.to_owned())), "the film was asked for, on the session's server");
        assert_eq!(plx_media::route::unpresented_resume_ns(&rig.player.session), 600_000_000_000, "at the session's place");
        assert!(plx_media::route::resume_reset_waived(&rig.player.session), "a join owes no resume reset");
        rig.frame(&session);
        assert_eq!(rig.bridge.together.follow_state(), &FollowState::Idle, "the opening is over");
        assert!(rig.bridge.together.player.is_none(), "no player page yet: nothing is handed to the session");
    }

    /// The person watches their own title: the Follow is held as a question and nothing is opened.
    /// Leave keeps their playback and leaves the session; Join opens the session's title in place.
    #[test]
    fn a_follow_while_the_persons_own_title_plays_is_held_as_a_question() {
        let mut rig = Rig::new();
        let mut meta = plx_data::stores::metadata::MetadataStore::default();
        assert!(plx_media::route::request_play(&mut rig.player.session, &mut meta, plx_platform::devcaps::caps(), rig.sid,
            "autre-7", "/library/parts/7/file.mkv", "h264", "aac", "Autre", "", None, ""));
        rig.mount_player();
        let session = view(false, Some(film()), Some(600.0));
        rig.follow(film(), Some(600.0));
        rig.frame(&session);
        let FollowState::Asking(ask) = rig.bridge.together.follow_state().clone() else {
            panic!("held: {:?}", rig.bridge.together.follow_state());
        };
        assert_eq!(ask.item, film());
        assert_eq!(ask.playing.key.as_deref(), Some("autre-7"));
        assert!(rig.wait.is_none(), "nothing opened");
        assert_eq!(rig.requested(), Some((rig.sid, "autre-7".to_owned())), "the person's playback stands");
        assert!(rig.bridge.together.player.is_none(), "a participant's own title is never driven");

        // Leave: the person's playback stands and the session is left.
        rig.bridge.together.answer_follow(FollowAnswer::Leave);
        rig.frame(&session);
        assert_eq!(rig.bridge.together.follow_state(), &FollowState::Idle);
        assert!(rig.bridge.together.store.due(0), "the Leave is queued for the next pump");
        assert_eq!(rig.requested(), Some((rig.sid, "autre-7".to_owned())));

        // Asked again, Join: the session's title replaces it in place, at the session's place now.
        rig.follow(film(), Some(600.0));
        rig.frame(&session);
        assert!(matches!(rig.bridge.together.follow_state(), FollowState::Asking(_)));
        rig.bridge.together.answer_follow(FollowAnswer::Join);
        let later = view(false, Some(film()), Some(630.0));
        rig.frame(&later);
        assert_eq!(rig.requested(), Some((rig.sid, FILM.to_owned())), "joined in place");
        assert_eq!(plx_media::route::unpresented_resume_ns(&rig.player.session), 630_000_000_000);
        assert!(plx_media::route::resume_reset_waived(&rig.player.session));
    }

    /// A title on a server this device does not see is never opened: the gate fails closed.
    #[test]
    fn a_title_on_no_reachable_server_is_missing_and_nothing_opens() {
        let mut rig = Rig::new();
        let elsewhere = SharedItem { server: Some("serveur-inconnu".into()), ..film() };
        let session = view(false, Some(elsewhere.clone()), Some(600.0));
        rig.follow(elsewhere.clone(), Some(600.0));
        rig.frame(&session);
        assert_eq!(rig.bridge.together.follow_state(), &FollowState::Missing { item: elsewhere });
        assert!(rig.wait.is_none() && rig.requested().is_none(), "nothing was opened");
    }

    /// The host's own playback is the session's: once its leaf is known it is handed (attached);
    /// the host's own Follow for it is `Already`; leaving the player hands nothing (the session
    /// hears the player leave, `Detach`), and with no session nothing is handed or asked.
    #[test]
    fn the_hosts_playback_is_handed_and_leaving_the_player_hands_nothing() {
        let mut rig = Rig::new();
        let mut meta = plx_data::stores::metadata::MetadataStore::default();
        assert!(plx_media::route::request_play(&mut rig.player.session, &mut meta, plx_platform::devcaps::caps(), rig.sid,
            FILM, "/library/parts/42/file.mkv", "h264", "aac", "Le Film", "", None, ""));
        rig.mount_player();
        let session = view(true, None, None);
        rig.frame(&session);
        assert!(rig.bridge.together.player.is_none(), "the leaf's data has not landed: nothing half-known is handed");
        // Its playing leaf lands (what the detail landing installs for a played item).
        rig.bridge.metadata_run(MetadataCmd::SetNowPlaying(Some(plx_data::metadata::NowPlaying {
            is_episode: false, is_real_episode: false, is_movie: true, title: "Le Film".into(), ep_title: String::new(),
            season: 0, index: 0, summary: String::new(), year: 2019, dur_ms: 6_000_000, rating: String::new(),
            thumb: String::new(), detail_rk: FILM.into(),
        })));
        plx_data::metadata::set_playing_for_test(rig.bridge.metadata_mut().state_mut(), rig.sid, FILM, "movie");
        rig.frame(&session);
        let handed = rig.bridge.together.player.clone().expect("the host's playback is the session's");
        assert_eq!(handed.item, film(), "named as the web's sharedOf names it");
        rig.follow(film(), None);
        rig.frame(&session);
        assert_eq!(rig.bridge.together.follow_state(), &FollowState::Idle, "the host's own title: Already");
        assert!(rig.wait.is_none());

        // The player leaves (Stop): nothing is handed.
        super::super::nav_select_tab(&mut rig.pages, AppArg::Home);
        for _ in 0..60 {
            rig.now += 16;
            let tick = plx_ui::fixture::tick(rig.now);
            super::super::frame(&mut rig.pages, &mut rig.bridge, tick, Vec::new());
        }
        rig.frame(&session);
        assert!(rig.bridge.together.player.is_none(), "the session hears the player leave");
        rig.bridge.together_player_step(None, PlayerSide {
            player: &mut rig.player, pa: &mut rig.pa, pages: &mut rig.pages, menu_play_await: &mut rig.wait, now: rig.now,
            held: false,
        });
        assert!(rig.bridge.together.player.is_none() && rig.bridge.together.item.is_none());
    }

    /// The gate decides again when the title's page lands: the Follow came with nothing playing
    /// (Start), the person started their own title while the page loaded, and the landing must not
    /// replace that playback. It is held as the gate's question instead.
    #[test]
    fn a_follow_whose_page_lands_after_the_person_started_their_own_title_asks() {
        let mut rig = Rig::new();
        plx_data::metadata::set_current_for_test(rig.bridge.metadata_mut().state_mut(), None);
        let session = view(false, Some(film()), Some(600.0));
        rig.follow(film(), Some(600.0));
        rig.frame(&session);
        assert_eq!(rig.bridge.together.follow_state(), &FollowState::Opening { item: film() });
        assert!(rig.wait.as_ref().is_some_and(|w| w.is_together()), "the film's page is loading");
        rig.play_own("autre-7");
        plx_data::metadata::set_current_for_test(rig.bridge.metadata_mut().state_mut(), Some(film_page(rig.sid)));
        rig.frame(&session);
        assert_eq!(rig.requested(), Some((rig.sid, "autre-7".to_owned())), "the person's playback stands");
        let FollowState::Asking(ask) = rig.bridge.together.follow_state().clone() else {
            panic!("asked: {:?}", rig.bridge.together.follow_state());
        };
        assert_eq!((ask.item, ask.position), (film(), Some(600.0)));
        assert_eq!(ask.playing.key.as_deref(), Some("autre-7"));
        assert!(rig.wait.is_none(), "the opening is over");
    }

    /// A Follow never takes the place of the person's own pending play (a card's or a link's wait
    /// for its page): it asks. Joining is their consent, and only then is the wait replaced.
    #[test]
    fn a_follow_never_overwrites_the_persons_pending_play_it_asks() {
        let mut rig = Rig::new();
        rig.wait = Some(crate::app::input::MenuPlayAwait::for_link(rig.sid, "serie-9".into(), false, rig.now));
        let session = view(false, Some(film()), Some(600.0));
        rig.follow(film(), Some(600.0));
        rig.frame(&session);
        assert!(rig.wait.as_ref().is_some_and(|w| !w.is_together()), "the person's play still waits for its page");
        let FollowState::Asking(ask) = rig.bridge.together.follow_state().clone() else {
            panic!("asked: {:?}", rig.bridge.together.follow_state());
        };
        assert_eq!(ask.item, film());
        assert_eq!(ask.playing.key.as_deref(), Some("serie-9"), "what the person is waiting for");
        assert!(rig.requested().is_none(), "nothing was opened");
        rig.bridge.together.answer_follow(FollowAnswer::Join);
        rig.frame(&session);
        assert_eq!(rig.requested(), Some((rig.sid, FILM.to_owned())), "joined: the film replaces the pending play");
    }

    /// While the app is hidden, or its parked playback is being brought back, the step neither
    /// performs nor reads: the session's requests wait in their queue, a Follow's opening in flight
    /// goes back to it, and no snapshot is handed. Shown again, the queue is performed in order.
    #[test]
    fn while_held_nothing_is_performed_or_handed_and_the_queue_waits() {
        let mut rig = Rig::new();
        let session = view(true, Some(film()), Some(600.0));
        rig.host_plays_the_film(&session);
        let next = SharedItem { key: Some("film-43".into()), guid: String::new(), title: "La Suite".into(), ..film() };
        rig.follow(next.clone(), Some(30.0));
        rig.frame_as(&session, true);
        assert!(rig.bridge.together.player.is_none(), "a held player is not handed");
        assert_eq!(rig.bridge.together.requests.len(), 1, "the Follow waits in the queue");
        assert!(rig.wait.is_none(), "nothing was opened");
        assert_eq!(rig.bridge.together.follow_state(), &FollowState::Idle);
        assert_eq!(rig.requested(), Some((rig.sid, FILM.to_owned())));

        // Shown again: the Follow goes through the gate (the session's previous title: Switch).
        rig.frame(&session);
        assert!(rig.bridge.together.requests.is_empty());
        assert_eq!(rig.bridge.together.follow_state(), &FollowState::Opening { item: next.clone() });
        assert!(rig.wait.as_ref().is_some_and(|w| w.is_together()), "its page is loading");

        // Hidden again while it loads: the opening goes back to the queue, to be gated afresh.
        rig.frame_as(&session, true);
        assert!(rig.wait.is_none(), "no opening runs behind the foreground lifecycle");
        // (It was opened at the session's place then, 600 s; the next opening reads the place anew.)
        assert!(matches!(rig.bridge.together.requests.as_slice(),
            [TogetherPlayerRequest::Follow { item, position: Some(p) }] if *item == next && *p == 600.0));
        assert_eq!(rig.bridge.together.follow_state(), &FollowState::Idle);
        assert_eq!(rig.requested(), Some((rig.sid, FILM.to_owned())));
    }

    /// The session's seek on a playback it did not open (the host's own Play from Start) leaves
    /// that playback's resume reset owed: only a playback a Follow opened waives it.
    #[test]
    fn a_session_seek_waives_no_reset_on_a_playback_it_did_not_open() {
        let mut rig = Rig::new();
        let session = view(true, Some(film()), Some(600.0));
        rig.host_plays_the_film(&session);
        assert!(!plx_media::route::resume_reset_waived(&rig.player.session));
        #[cfg(feature = "play-feed")]
        let was = (plx_play_feed::shared::FEED_TX.seek_to_ns.load(std::sync::atomic::Ordering::Relaxed),
            plx_play_feed::shared::FEED_SHARED.seeking.load(std::sync::atomic::Ordering::Relaxed));
        rig.bridge.together.requests.push(TogetherPlayerRequest::Seek(1_200.0));
        rig.frame(&session);
        #[cfg(feature = "play-feed")]
        {
            plx_play_feed::shared::FEED_TX.seek_to_ns.store(was.0, std::sync::atomic::Ordering::Relaxed);
            plx_play_feed::shared::FEED_SHARED.seeking.store(was.1, std::sync::atomic::Ordering::Relaxed);
        }
        assert_eq!(rig.player.session.readout().pending_seek(), Some(1_200_000_000_000), "the session's seek was performed");
        assert!(!plx_media::route::resume_reset_waived(&rig.player.session), "the host's Play from Start still owes its reset");
    }

    /// The session ends (the person left, or the host ended it) while the Follow's page loads: the
    /// opening goes with it, and a together wait that outlived it opens nothing when its page
    /// lands, so no playback is started, or waived its reset, for someone no longer in a session.
    #[test]
    fn a_follow_whose_session_ends_while_its_page_loads_opens_nothing() {
        let mut rig = Rig::new();
        plx_data::metadata::set_current_for_test(rig.bridge.metadata_mut().state_mut(), None);
        let session = view(false, Some(film()), Some(600.0));
        rig.follow(film(), Some(600.0));
        rig.frame(&session);
        assert!(rig.wait.as_ref().is_some_and(|w| w.is_together()), "the film's page is loading");
        rig.frame_without_session();
        assert!(rig.wait.is_none(), "the opening went with the session");

        // A together wait that outlived its session (armed, then the session went): its landing
        // asks the gate, which finds no session.
        let open = crate::app::input::TogetherOpen { at_ns: 600_000_000_000, item: film(), position: Some(600.0), replaces: None };
        rig.wait = Some(crate::app::input::MenuPlayAwait::for_together(rig.sid, FILM.into(), open, rig.now));
        plx_data::metadata::set_current_for_test(rig.bridge.metadata_mut().state_mut(), Some(film_page(rig.sid)));
        rig.tick_wait();
        assert!(rig.wait.is_none(), "the landing ended the wait");
        assert!(rig.requested().is_none(), "nothing was opened");
        assert!(!plx_media::route::resume_reset_waived(&rig.player.session), "and nothing waived");
    }

    /// A Follow held while the app was hidden opens, once shown again, at the session's position
    /// then, not at the one it came with: a stale start would cost a second seek.
    #[test]
    fn a_follow_held_while_hidden_opens_at_the_sessions_position_on_return() {
        let mut rig = Rig::new();
        let session = view(true, Some(film()), Some(600.0));
        rig.host_plays_the_film(&session);
        let next = SharedItem { key: Some("film-43".into()), guid: String::new(), title: "La Suite".into(), ..film() };
        rig.follow(next.clone(), Some(30.0));
        rig.frame_as(&view(true, Some(next.clone()), Some(30.0)), true);
        assert!(rig.wait.is_none());
        rig.frame(&view(true, Some(next.clone()), Some(95.0)));
        let open = rig.wait.clone().and_then(crate::app::input::MenuPlayAwait::into_together).expect("its page is loading");
        assert_eq!(open.at_ns, 95_000_000_000, "at the session's place now");
    }
}
