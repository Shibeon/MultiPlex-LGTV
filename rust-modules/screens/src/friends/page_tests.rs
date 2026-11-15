//! The Friends page's tests: the toggle asks for the privacy write ONLY on its explicit press, the
//! filter narrows the shelves, the live shelf is absent when nothing plays, the words of a post and
//! of a day in French, what a card's OK opens, and the strip.
use std::collections::HashMap;
use std::sync::Arc;

use plx_data::stores::community::{FeedState, ShareState};
use plx_data::watchlist::model::ServerMatch;
use plx_machine::machine::{Cx, Effects, EntryId, FocusKey, FocusRead, Fx, InputOwner, Machine, PressRead, Stamped, Tick};
use plx_platform::i18n::{language_on_this_thread_for_test, Preference};
use plx_plex::plex::community::{ActivityKind, Friend, FriendActivity};
use plx_plex::plex::live::{LiveSession, LiveState};
use plx_plex::plex::ServerId;
use plx_ui::fixture::FixtureMeasure;
use plx_ui::screen::ScreenEvent;

use plx_data::stores::community::CommunityCmd;
use plx_data::stores::{StoreCmd, StoreId};
use plx_machine::machine::Host;

use super::page::{FriendsIn, FriendsScreen, FriendsView, SHARE_ELEM};
use crate::registry::{AppFx, AppMsg, ContentArg, ContentReq, HomeTab, PageMemory};

/// A host with no stores: the page reads everything it shows from its own inputs.
struct WatchHost;
impl Host for WatchHost {
    type Arg = crate::registry::SettingsPage;
    type Fx = AppFx;
    type Msg = AppMsg;
    type Elem = u32;
    type Views<'a> = ();
    type Init = crate::registry::NoInit;
    type Memory = PageMemory;
}

/// What the page asked, as these tests compare it: the store commands and the page requests of
/// the shared lanes (a Detail push is read off the effects where a test wants it).
#[derive(Debug, PartialEq)]
enum Asked {
    SetShare(bool),
    Retry,
    OpenCatalog([u8; 12]),
    Tab(HomeTab),
    Account,
    LeaveToHome,
}

const ENTRY: EntryId = EntryId(9);
const DAY: i64 = 86_400;
/// 2026-11-15T12:00:00Z.
const NOON: i64 = 1_794_744_000;
const HEX: &str = "5d776b59ad5437001f79c6f8";

fn friend(id: &str, name: &str) -> Friend {
    Friend { id: id.into(), name: name.into(), username: None, avatar: None }
}

fn movie(id: &str, who: &Friend, at: i64, title: &str) -> FriendActivity {
    FriendActivity {
        id: id.into(), date: String::new(), at: Some(at), user: who.clone(), kind: ActivityKind::Movie,
        title: title.into(), show: None, season: None, episode: None, year: Some(2001), poster: None, art: None,
        catalog_hex: Some(HEX.into()),
    }
}

fn episode(id: &str, who: &Friend, at: i64, show: &str, s: i64, e: i64) -> FriendActivity {
    FriendActivity {
        kind: ActivityKind::Episode, show: Some(show.into()), season: Some(s), episode: Some(e),
        title: format!("Épisode {e}"), catalog_hex: Some("5d9c086fe9d5a1001f4d7a2b".into()),
        ..movie(id, who, at, "")
    }
}

/// Camille binged two episodes and watched a film today; Léo watched a film yesterday; Inès three
/// days ago.
fn feed() -> Vec<FriendActivity> {
    let (a, b, c) = (friend("1", "Camille"), friend("2", "Léo"), friend("3", "Inès"));
    vec![
        episode("e2", &a, NOON - 600, "Archipel", 1, 3),
        episode("e1", &a, NOON - 3_000, "Archipel", 1, 2),
        movie("m1", &a, NOON - 7_200, "Le Film"),
        movie("m2", &b, NOON - DAY, "Hier soir"),
        movie("m3", &c, NOON - 3 * DAY, "Avant"),
    ]
}

fn live_row(user: &str, state: LiveState) -> (ServerId, LiveSession) {
    (ServerId::from_raw(1), LiveSession {
        user: user.into(), user_thumb: String::new(), rating_key: "42".into(), kind: "movie".into(),
        title: "Le Film".into(), show: String::new(), season: 0, episode: 0, thumb: "/library/metadata/42/thumb".into(),
        view_offset_ms: 30 * 60_000, duration_ms: 120 * 60_000, state,
    })
}

fn view(shares: ShareState) -> FriendsView {
    FriendsView { feed: FeedState::Ready(Arc::from(feed())), shares, ..Default::default() }
}

struct H {
    screen: FriendsScreen,
    focus: Option<FocusKey<u32>>,
    ms: u32,
}

impl H {
    fn new(view: FriendsView) -> Self {
        let mut screen = FriendsScreen::loading(ENTRY);
        screen.input(FriendsIn::Clock { now: Some(NOON), offset: 0 });
        screen.input(FriendsIn::View(view));
        let mut h = Self { screen, focus: None, ms: 0 };
        h.step(ScreenEvent::Tick(Tick { ms: 0, dt_us: 16_667 }));
        h
    }

    fn step(&mut self, ev: ScreenEvent<WatchHost>) -> Vec<Stamped<WatchHost>> {
        let mut present = plx_machine::present::Present::new();
        let mut out = Vec::new();
        let cx = Cx {
            views: (),
            tick: Tick { ms: self.ms, dt_us: 16_667 },
            measure: &FixtureMeasure,
            press: PressRead { scale: 1.0, owner: self.focus, ..Default::default() },
            focus: FocusRead { current: self.focus, ..Default::default() },
            owner: InputOwner::Entry(ENTRY),
        };
        let mut fx = Effects::new(&mut out, plx_machine::machine::MachineId::Instance(plx_machine::machine::InstanceId(9)), &mut present);
        Machine::<WatchHost>::step(&mut self.screen, &ev, &cx, &mut fx);
        out
    }

    fn run(&mut self, frames: u32) -> Vec<Asked> {
        let mut asked = Vec::new();
        for _ in 0..frames {
            self.ms += 16;
            asked.extend(requests(self.step(ScreenEvent::Tick(Tick { ms: self.ms, dt_us: 16_667 }))));
        }
        asked
    }

    fn press(&mut self, elem: u32) -> Vec<Stamped<WatchHost>> {
        self.step(ScreenEvent::Activate(elem))
    }
}

fn requests(fx: Vec<Stamped<WatchHost>>) -> Vec<Asked> {
    fx.into_iter()
        .filter_map(|e| match e.fx {
            Fx::App(AppFx::Store(StoreId::Community, StoreCmd::Community(CommunityCmd::SetShare(on)))) => Some(Asked::SetShare(on)),
            Fx::App(AppFx::Store(StoreId::Community, StoreCmd::Community(CommunityCmd::Retry))) => Some(Asked::Retry),
            Fx::App(AppFx::Content(ContentReq::Push(ContentArg::Catalog { hex }))) => Some(Asked::OpenCatalog(hex)),
            Fx::App(AppFx::Content(ContentReq::Tab(tab))) => Some(Asked::Tab(tab)),
            Fx::App(AppFx::Content(ContentReq::Account)) => Some(Asked::Account),
            Fx::App(AppFx::Content(ContentReq::LeaveToHome)) => Some(Asked::LeaveToHome),
            _ => None,
        })
        .collect()
}

/// **The privacy write is asked for on the toggle's press and nowhere else.** Landing the feed,
/// the setting, a failed write, the clock and frames ask for nothing; a press asks for the opposite
/// of what the account says; a press while the setting is not known, or while a save is out, asks
/// for nothing.
#[test]
fn the_share_write_is_asked_for_only_on_the_explicit_press() {
    let mut h = H::new(view(ShareState::Unknown));
    assert_eq!(h.run(30), vec![], "mounting and frames write nothing");
    assert_eq!(requests(h.press(SHARE_ELEM)), vec![], "an unknown setting is not toggled");
    h.screen.input(FriendsIn::View(view(ShareState::Known(false))));
    h.screen.input(FriendsIn::Clock { now: Some(NOON + 60), offset: 3_600 });
    assert_eq!(h.run(30), vec![], "the setting landing writes nothing");
    assert_eq!(requests(h.press(SHARE_ELEM)), vec![Asked::SetShare(true)]);
    h.screen.input(FriendsIn::View(FriendsView { writing: Some(true), ..view(ShareState::Known(false)) }));
    assert_eq!(requests(h.press(SHARE_ELEM)), vec![], "no second write while one is out");
    h.screen.input(FriendsIn::View(FriendsView { write_failed: true, ..view(ShareState::Known(true)) }));
    assert_eq!(h.run(30), vec![], "a failed write is not retried by itself");
    assert_eq!(requests(h.press(SHARE_ELEM)), vec![Asked::SetShare(false)]);
}

/// **The filter row: All first, then the friends, the most active first; a press narrows the
/// shelves to that friend, and All widens them again.**
#[test]
fn a_friend_chip_narrows_the_shelves_and_all_widens_them() {
    let _fr = language_on_this_thread_for_test(Preference::Fr);
    let mut h = H::new(view(ShareState::Known(true)));
    assert_eq!(h.screen.probe_friends(), ["Camille", "Léo", "Inès"]);
    let titles = |h: &H| h.screen.probe_days().into_iter().map(|(_, cards)| cards.into_iter().map(|c| c.0).collect::<Vec<_>>()).collect::<Vec<_>>();
    assert_eq!(titles(&h), [vec!["Archipel", "Le Film"], vec!["Hier soir"], vec!["Avant"]]);
    assert_eq!(requests(h.press(FriendsScreen::chip_elem(2))), vec![], "the filter is the page's own");
    assert_eq!(h.screen.filter(), Some("2"));
    assert_eq!(titles(&h), [vec!["Hier soir"]]);
    h.press(FriendsScreen::chip_elem(0));
    assert_eq!(h.screen.filter(), None);
    assert_eq!(titles(&h).len(), 3);
}

/// **The words, in French**: today, yesterday, then the date; the binge is one post worded as the
/// web words it, with the last episode watched and the relative time.
#[test]
fn days_and_posts_are_worded_as_the_web_words_them() {
    let _fr = language_on_this_thread_for_test(Preference::Fr);
    let h = H::new(view(ShareState::Known(true)));
    let days = h.screen.probe_days();
    let headings: Vec<&str> = days.iter().map(|(h, _)| h.as_str()).collect();
    assert_eq!(&headings[..2], ["Aujourd’hui", "Hier"]);
    assert!(headings[2].contains("12") && headings[2].contains("novembre"), "{:?}", headings[2]);
    let (title, caption) = &days[0].1[0];
    assert_eq!(title, "Archipel");
    assert_eq!(caption, "Camille a regardé 2 épisodes de · S1, E3 · Épisode 3 · il y a 10 min");
    assert_eq!(days[0].1[1].1, "Camille a regardé · il y a 2 h");
}

/// **Nothing playing, no live shelf**; a session shows who, what, how far and whether it is paused,
/// and nothing else (no player, no device, no address: the row carries none).
#[test]
fn the_live_shelf_is_absent_when_nothing_plays() {
    let _fr = language_on_this_thread_for_test(Preference::Fr);
    let mut h = H::new(view(ShareState::Known(true)));
    assert!(h.screen.probe_live().is_empty());
    let live: Arc<[_]> = Arc::from(vec![live_row("Camille", LiveState::Paused), live_row("Léo", LiveState::Playing)]);
    h.screen.input(FriendsIn::View(FriendsView { live, ..view(ShareState::Known(true)) }));
    let rows = h.screen.probe_live();
    assert_eq!(rows.iter().map(|r| r.1.as_str()).collect::<Vec<_>>(), ["Camille · en pause", "Léo regarde"]);
    assert_eq!(rows[0].2, Some(0.25));
    h.screen.input(FriendsIn::View(view(ShareState::Known(true))));
    assert!(h.screen.probe_live().is_empty(), "the shelf goes when the sessions do");
}

/// **OK opens the user's own copy when a server holds the title, else its catalog page.**
#[test]
fn a_held_title_opens_its_detail_and_any_other_its_catalog_page() {
    let mut held = HashMap::new();
    held.insert(format!("plex://movie/{HEX}"), ServerMatch::new(ServerId::from_raw(3), 77));
    let mut h = H::new(FriendsView { held, ..view(ShareState::Known(true)) });
    // Today: the binge (not held), then the film (held).
    let binge = h.screen.probe_day_card(0, 0).unwrap();
    let film = h.screen.probe_day_card(0, 1).unwrap();
    let out = h.press(film);
    let pushes: Vec<_> = out.iter().filter_map(|e| match &e.fx {
        Fx::App(AppFx::Content(ContentReq::Push(ContentArg::Detail { sid, rk }))) => Some((sid.raw(), rk.clone())),
        _ => None,
    }).collect();
    assert_eq!(pushes, [(3, "77".to_string())]);
    let hex = plx_data::watchlist::decode_hex("5d9c086fe9d5a1001f4d7a2b").unwrap();
    assert_eq!(requests(h.press(binge)), vec![Asked::OpenCatalog(hex)]);
}

/// **The strip: the other pills are tab requests, the Friends pill is the page itself.**
#[test]
fn a_strip_press_is_a_tab_request() {
    use plx_ui::widgets::{TAB_KEY_CHIP, TAB_KEY_FRIENDS, TAB_KEY_HOME, TAB_KEY_WATCHLIST};
    let mut h = H::new(view(ShareState::Known(true)));
    for (key, want) in [
        (TAB_KEY_HOME, vec![Asked::Tab(HomeTab::Home)]),
        (TAB_KEY_WATCHLIST, vec![Asked::Tab(HomeTab::Watchlist)]),
        (TAB_KEY_CHIP, vec![Asked::Account]),
        (TAB_KEY_FRIENDS, vec![]),
    ] {
        assert_eq!(requests(h.press(key)), want, "{key:#x}");
    }
}

/// **The read-outs**: no account, unreachable (with Try again), and an empty feed each replace the
/// shelves; the page draws in every one of them without a card.
#[test]
fn the_readouts_replace_the_shelves() {
    for feed in [FeedState::NoAccount, FeedState::Failed, FeedState::Ready(Arc::from(Vec::new())), FeedState::Loading] {
        let h = H::new(FriendsView { feed: feed.clone(), ..Default::default() });
        assert!(h.screen.probe_days().is_empty(), "{feed:?}");
    }
    let mut h = H::new(FriendsView { feed: FeedState::Failed, ..Default::default() });
    assert_eq!(requests(h.press(super::page::RETRY_ELEM)), vec![Asked::Retry]);
}
