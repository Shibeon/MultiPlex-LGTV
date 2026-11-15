//! **The Settings pill by keys** (MultiPlex's top bar): the tune icon after the bar's divider, reached
//! the way a person reaches it (Up into the bar, Right along it) and pressed with OK, on every page
//! that wears the bar and answers it (`plx_screens::registry::strip_settings`). The press is a loop
//! request (`LoopReq::OpenSettings(SettingsPage::Root)`), which the loop performs with
//! `bridge::open_settings_at` (`run.rs`); the rig does not drain loop requests, so each test reads
//! the request off the bridge and performs it the loop's way, then reads the surface that came up.
//! The chip beside it has its own rows (`account_menu_route_tests`).

use super::*;
use super::test_support::World;
use plx_screens::family::SettingsPage;
use plx_ui::widgets::{TAB_KEY_CHIP, TAB_KEY_SETTINGS};

/// Up until focus is on the bar, then Right along it to the Settings pill, then OK: the request the
/// press raised, performed as the loop performs it, and the page under it unchanged.
fn press_settings(w: &mut World, page: &AppArg, name: &str) {
    for _ in 0..4 {
        if w.on_nav() { break; }
        w.press(Key::Up);
    }
    assert!(w.on_nav(), "premise: Up reaches the bar on {name}");
    for _ in 0..12 {
        if w.elem() == Some(TAB_KEY_SETTINGS) { break; }
        w.press(Key::Right);
    }
    assert_eq!(w.elem(), Some(TAB_KEY_SETTINGS), "Right along the bar reaches the Settings pill on {name}");
    let _ = w.bridge.take_reqs(); // what the walk raised is not what this grades
    assert_eq!(crate::app::bridge::overlay_word(&w.d), None, "premise: focus alone opens nothing on {name}");

    w.press(Key::Ok);
    let reqs = w.bridge.take_reqs();
    let opens: Vec<_> = reqs.iter().filter(|req| matches!(req, LoopReq::OpenSettings(_))).collect();
    assert_eq!(opens.len(), 1, "OK on the Settings pill asks the loop for Settings once on {name}: {reqs:?}");
    let LoopReq::OpenSettings(at) = opens[0] else { unreachable!() };
    assert!(matches!(at, SettingsPage::Root), "at its root page on {name}: {at:?}");

    crate::app::bridge::open_settings_at(&mut w.d, *at);
    w.settle(30);
    assert_eq!(crate::app::bridge::overlay_word(&w.d), Some("settings"), "the Settings surface is up over {name}");
    assert!(w.on(page), "a surface over the page, not a replacement of it");
}

/// The divider between the Friends pill and Settings is drawn and never a stop: Left from Settings lands on
/// the pill before it, Right on the chip after it.
fn settings_neighbours(w: &mut World) {
    w.press(Key::Right);
    assert_eq!(w.elem(), Some(TAB_KEY_CHIP), "Right from Settings is the profile chip");
    w.press(Key::Left);
    assert_eq!(w.elem(), Some(TAB_KEY_SETTINGS), "and Left from the chip is Settings again");
    w.press(Key::Left);
    assert_eq!(w.elem(), Some(plx_ui::widgets::TAB_KEY_FRIENDS), "Left from Settings skips the divider to Friends");
    w.press(Key::Right);
    assert_eq!(w.elem(), Some(TAB_KEY_SETTINGS), "premise: back on Settings");
}

#[test]
fn ok_on_the_settings_pill_opens_settings_over_home() {
    let mut w = World::home_grid("route-settings-home", 2, 4);
    press_settings(&mut w, &AppArg::Home, "Home");
}

#[test]
fn ok_on_the_settings_pill_opens_settings_over_the_library() {
    let mut w = World::movies_library("route-settings-library", false);
    w.climb_into_header();
    press_settings(&mut w, &AppArg::Library, "the Library");
}

#[test]
fn ok_on_the_settings_pill_opens_settings_over_search() {
    let mut w = World::search_results("route-settings-search", |_| Vec::new());
    press_settings(&mut w, &AppArg::Search, "Search");
}

#[test]
fn the_divider_is_never_a_stop_beside_the_settings_pill() {
    let mut w = World::movies_library("route-settings-divider", false);
    w.climb_into_header();
    w.press(Key::Up);
    for _ in 0..12 {
        if w.elem() == Some(TAB_KEY_SETTINGS) { break; }
        w.press(Key::Right);
    }
    assert_eq!(w.elem(), Some(TAB_KEY_SETTINGS), "premise: on Settings");
    settings_neighbours(&mut w);
}
