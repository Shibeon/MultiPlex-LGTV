//! **A managed Plex Home profile has no My list** (the owner's decision, shared with the web app):
//! the bar a managed profile wears has no Watchlist pill (`chrome::strip_keys`), focus travels the
//! strip it does have through the shared `FocusEngine` (no hand-routed key), and whatever still
//! asks for the Watchlist page (a press the strip can no longer make, the `/tmp/multiplex-watchlist`
//! boot target) lands on Home (`bridge::nav_tab`). The Watchlist affordances on other pages go too:
//! the Home hero disc and the Detail More-menu row (the screens' own tests), and the hold-menu row,
//! whose face the application builds here, so no list read and no record resolve is asked for it.
//! The owner keeps everything (`settings_pill_route_tests`, `watchlist_press_tests`).

use super::*;
use super::test_support::World;
use plx_ui::widgets::{TAB_KEY_CHIP, TAB_KEY_DIVIDER, TAB_KEY_SETTINGS, TAB_KEY_WATCHLIST};

/// Seat the household's managed member `u-kid` the way a profile switch leaves it: the persisted
/// roster names the tile `restricted`, and the profile is published. Nothing reaches plex.tv.
fn seat_managed(w: &mut World) {
    seat_managed_session();
    w.frames(10);
}

/// [`seat_managed`]'s session half, for a rig that steps its own frames.
fn seat_managed_session() {
    use plx_plex::plex::session::{self, HomeUserRef, UserRef};
    assert!(session::update(|cur| {
        let mut next = cur.clone();
        next.user.uuid = "u-kid".into();
        next.home_users = vec![
            HomeUserRef { uuid: "u-owner".into(), title: "Owner".into(), admin: true, ..Default::default() },
            HomeUserRef { uuid: "u-kid".into(), title: "Kid".into(), restricted: true, ..Default::default() },
        ];
        Some(next)
    }), "the fixture session takes the roster");
    // With its own plex.tv credential, so nothing but the managed rule keeps the Watchlist away.
    session::publish_profile_for_test(Some(UserRef { uuid: "u-kid".into(),
        plex_tv_token: Some("synthetic-credential".into()), ..Default::default() }),
        session::current_gen().wrapping_add(1));
}

/// The published strip's element keys, in order.
fn strip(w: &World) -> Vec<u32> {
    w.d.nav.tabs.strip.iter().map(|member| member.elem).collect()
}

#[test]
fn a_managed_profile_walks_a_bar_without_my_list() {
    let mut w = World::movies_library("route-managed-bar", false);
    seat_managed(&mut w);
    assert!(w.bridge.profile_managed(), "premise: the managed profile is captured");
    let keys = strip(&w);
    assert!(!keys.contains(&TAB_KEY_WATCHLIST), "no My list on a managed profile's bar: {keys:?}");
    assert!(!keys.contains(&TAB_KEY_DIVIDER), "the divider is never a stop");
    let before = keys[keys.iter().position(|&k| k == TAB_KEY_SETTINGS).expect("Settings stays") - 1];
    assert_ne!(before, TAB_KEY_WATCHLIST);

    w.climb_into_header();
    for _ in 0..4 {
        if w.on_nav() { break; }
        w.press(Key::Up);
    }
    assert!(w.on_nav(), "premise: Up reaches the bar");
    let mut seen = Vec::new();
    for _ in 0..12 {
        seen.extend(w.elem());
        if w.elem() == Some(TAB_KEY_CHIP) { break; }
        w.press(Key::Right);
    }
    assert_eq!(w.elem(), Some(TAB_KEY_CHIP), "Right along the bar reaches the chip: {seen:?}");
    assert!(!seen.contains(&TAB_KEY_WATCHLIST), "and never stands on My list: {seen:?}");
    w.press(Key::Left);
    assert_eq!(w.elem(), Some(TAB_KEY_SETTINGS));
    w.press(Key::Left);
    assert_eq!(w.elem(), Some(before), "Left from Settings skips the divider to the last type pill");
}

#[test]
fn the_watchlist_door_lands_a_managed_profile_on_home() {
    let mut w = World::home_grid("route-managed-door", 2, 4);
    seat_managed(&mut w);
    super::nav_tab(&mut w.d, &mut w.bridge, HomeTab::Watchlist, None, None);
    w.frames(60);
    assert!(w.on(&AppArg::Home), "a managed profile asking for the Watchlist stays on Home");
    assert!(!w.d.nav.tabs.stack.entries.iter().any(|entry| entry.arg == AppArg::Watchlist),
        "and no Watchlist page was mounted anywhere");
}

#[test]
fn the_owner_still_opens_the_watchlist() {
    let mut w = World::home_grid("route-owner-door", 2, 4);
    assert!(!w.bridge.profile_managed(), "premise: the rig's profile is not managed");
    assert!(strip(&w).contains(&TAB_KEY_WATCHLIST), "the owner's bar has My list");
    super::nav_tab(&mut w.d, &mut w.bridge, HomeTab::Watchlist, None, None);
    w.frames(60);
    assert!(w.on(&AppArg::Watchlist), "the owner's door opens the Watchlist");
}

/// A managed profile's hold menus carry no Watchlist row: the face the application hands them is
/// `None` for a title with a catalog id and for one that would resolve through its record, and
/// neither the list read nor the record resolve the row would need is asked for.
#[test]
fn a_managed_profile_is_offered_no_watchlist_row_and_reads_no_list() {
    let _serial = plx_base::testlock::serial();
    let script = super::watchlist_press_tests::Script::new(&[1]);
    let (mut d, mut rig) = super::watchlist_press_tests::rig_over(&script);
    let movie = plx_data::pms::PmsMovie { kind: 0, guid: Some([1; 12]), ..Default::default() };
    let unresolved = plx_data::pms::PmsMovie { sid: plx_plex::plex::ServerId::from_raw(1), rk: "11".into(), kind: 0,
        ..Default::default() };
    assert!(rig.watchlist_card_face(&movie).is_some(), "premise: the owner is offered the row");
    seat_managed_session();
    super::frame(&mut d, &mut rig, super::test_support::tick(1), vec![]);
    assert!(rig.screen_inputs.managed, "the screen inputs carry the managed answer");
    assert!(rig.watchlist_card_face(&movie).is_none(), "no row for a title with a catalog id");
    assert!(rig.watchlist_card_face(&unresolved).is_none(), "nor for one resolved through its record");
    rig.watchlist_open_if_idle();
    assert_eq!(rig.stores.watchlist.view().status(), plx_data::stores::watchlist::Status::Idle, "no list read");
    rig.watchlist_pump();
    assert!(rig.store_runner.take_held_watchlist_resolves().is_empty(), "no record resolve asked");
}

/// The Home hero's disc on a managed profile: the page hands the feed no title, so the application
/// reads no membership for it.
#[test]
fn a_managed_home_hero_asks_no_membership() {
    let mut w = World::home_with_disc("route-managed-disc", [9; 12]);
    let home = |w: &World| w.d.nav.tabs.stack.entries.iter().find_map(|e| e.inst.as_ref()
        .and_then(|i| i.screen.as_any()).and_then(|a| a.downcast_ref::<plx_screens::home::HomeScreen>())
        .map(|h| h.watchlist_hex()));
    assert_eq!(home(&w), Some(Some([9; 12])), "premise: the owner's hero stands for the title");
    seat_managed(&mut w);
    assert_eq!(home(&w), Some(None), "a managed profile's hero has no disc to feed");
}

/// The catalog page (a title no server holds, reached from a person's credit) on a managed
/// profile: the record is shown with no pill, the Watchlist is never read, and the write the
/// pill would have asked for (or a stale one) reaches nothing.
#[test]
fn a_managed_catalog_page_has_no_pill_reads_no_list_and_writes_nothing() {
    use plx_screens::catalog::{CatalogOut, CatalogScreen, Load, PILL_ELEM};
    let _serial = plx_base::testlock::serial();
    let script = super::watchlist_press_tests::Script::new(&[1]);
    let (mut d, mut rig) = super::watchlist_press_tests::rig_over(&script);
    seat_managed_session();
    super::frame(&mut d, &mut rig, super::test_support::tick(1), vec![]);
    let hex = [9u8; 12];
    super::open_catalog(&mut d, hex, None);
    for i in 0..40 {
        super::frame(&mut d, &mut rig, super::test_support::tick(10 + i), vec![]);
    }
    super::watchlist_press_tests::settle(&mut d, &mut rig, 60);
    let page = |d: &Dispatcher<AppHost>| d.top_screen().and_then(|s| s.as_any())
        .and_then(|s| s.downcast_ref::<CatalogScreen>()).expect("the catalog page is on top").view_model().clone();
    let vm = page(&d);
    assert!(matches!(vm.load, Load::Loaded { .. }), "the record is still shown: {:?}", vm.load);
    assert!(!vm.offered, "with no pill");
    assert_ne!(w_elem(&d), Some(PILL_ELEM), "and no focus on one");
    assert_eq!(rig.stores.watchlist.view().status(), plx_data::stores::watchlist::Status::Idle, "no list read");
    rig.catalog_effect(CatalogOut::Add { hex });
    super::watchlist_press_tests::settle(&mut d, &mut rig, 120);
    assert!(script.writes().is_empty(), "no write reaches plex.tv");
    assert_eq!(rig.stores.watchlist.view().status(), plx_data::stores::watchlist::Status::Idle);
}

fn w_elem(d: &Dispatcher<AppHost>) -> Option<u32> {
    d.focus().map(|key| key.elem)
}
