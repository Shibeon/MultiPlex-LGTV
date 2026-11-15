//! **A managed Plex Home profile has no My list** (the owner's decision, shared with the web app):
//! the bar a managed profile wears has no Watchlist pill (`chrome::strip_keys`), focus travels the
//! strip it does have through the shared `FocusEngine` (no hand-routed key), and whatever still
//! asks for the Watchlist page (a press the strip can no longer make, the `/tmp/multiplex-watchlist`
//! boot target) lands on Home (`bridge::nav_tab`). The owner keeps everything (`settings_pill_route_tests`,
//! `watchlist_press_tests`).

use super::*;
use super::test_support::World;
use plx_ui::widgets::{TAB_KEY_CHIP, TAB_KEY_DIVIDER, TAB_KEY_SETTINGS, TAB_KEY_WATCHLIST};

/// Seat the household's managed member `u-kid` the way a profile switch leaves it: the persisted
/// roster names the tile `restricted`, and the profile is published. Nothing reaches plex.tv.
fn seat_managed(w: &mut World) {
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
    session::publish_profile_for_test(Some(UserRef { uuid: "u-kid".into(), ..Default::default() }),
        session::current_gen().wrapping_add(1));
    w.frames(10);
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
