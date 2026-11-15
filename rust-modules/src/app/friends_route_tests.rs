//! **The Friends page, end to end on the host rig**: the owner's bar carries the pill after My list
//! and its door opens the page, whose reads (the feed, the privacy setting, the live sessions) are
//! asked only while it is on top; the privacy WRITE is asked for only by the page's
//! `CommunityCmd::SetShare` (its toggle's press, a store command on the FIFO); and a managed Plex Home profile has no pill, no page and no read.
//! Every job parks in the `Held` runner: nothing reaches plex.tv or a server.

use super::*;
use super::test_support::World;
use plx_data::stores::community::{CommunityCmd, CommunityReq};
use plx_data::stores::StoreJob;
use plx_ui::widgets::{TAB_KEY_FRIENDS, TAB_KEY_SETTINGS, TAB_KEY_WATCHLIST};

/// What the community store asked for, as the parked jobs name it.
fn asked(w: &mut World) -> Vec<&'static str> {
    let mut out = Vec::new();
    let mut keep = Vec::new();
    for job in w.bridge.store_runner.take_held() {
        match &job {
            StoreJob::Community(job, _) => out.push(match job.req {
                CommunityReq::Feed { .. } => "feed",
                CommunityReq::Privacy { .. } => "privacy",
                CommunityReq::SetPrivacy { .. } => "set-privacy",
                CommunityReq::Live { .. } => "live",
                CommunityReq::Match { .. } => "match",
            }),
            _ => keep.push(job),
        }
    }
    drop(keep);
    out
}

fn strip(w: &World) -> Vec<u32> {
    w.d.nav.tabs.strip.iter().map(|member| member.elem).collect()
}

/// Seat the household's managed member, with a plex.tv credential of its own, so nothing but the
/// managed rule keeps the page away (`managed_profile_route_tests`' seating).
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
    }));
    session::publish_profile_for_test(Some(UserRef { uuid: "u-kid".into(),
        plex_tv_token: Some("synthetic-credential".into()), ..Default::default() }),
        session::current_gen().wrapping_add(1));
    w.frames(10);
}

#[test]
fn the_owner_bar_carries_friends_after_my_list_and_its_door_opens_the_page() {
    let mut w = World::home_grid("route-friends-owner", 2, 4);
    w.bridge.seat_watchlist_account_for_test(true);
    w.frames(4);
    let keys = strip(&w);
    let at = |key| keys.iter().position(|&k| k == key);
    assert_eq!(at(TAB_KEY_FRIENDS), at(TAB_KEY_WATCHLIST).map(|i| i + 1), "Friends right after My list: {keys:?}");
    assert_eq!(at(TAB_KEY_SETTINGS), at(TAB_KEY_FRIENDS).map(|i| i + 1), "then the divider (no stop) and Settings");
    assert_eq!(asked(&mut w), Vec::<&str>::new(), "nothing is read while the page is not on screen");

    super::nav_tab(&mut w.d, &mut w.bridge, HomeTab::Friends, None, None);
    w.frames(60);
    assert!(w.on(&AppArg::Friends), "the door opens the Friends page");
    let reads = asked(&mut w);
    for want in ["feed", "privacy", "live"] {
        assert!(reads.contains(&want), "the visible page asks for its {want}: {reads:?}");
    }
    assert!(!reads.contains(&"set-privacy"), "showing the page writes nothing: {reads:?}");
    w.frames(120);
    assert!(!asked(&mut w).contains(&"set-privacy"), "nor do frames");

    // The page's toggle press, as the store step performs it: exactly one write.
    w.bridge.community_run(CommunityCmd::SetShare(true));
    assert_eq!(asked(&mut w), ["set-privacy"]);
    assert!(w.bridge.community_for_test().writing() == Some(true), "the store holds the write in flight");
}

#[test]
fn a_managed_profile_has_no_friends_pill_no_page_and_no_read() {
    let mut w = World::home_grid("route-friends-managed", 2, 4);
    seat_managed(&mut w);
    assert!(w.bridge.profile_managed(), "premise: the managed profile is captured");
    assert!(!strip(&w).contains(&TAB_KEY_FRIENDS), "no Friends pill on a managed profile's bar");
    super::nav_tab(&mut w.d, &mut w.bridge, HomeTab::Friends, None, None);
    w.frames(60);
    assert!(w.on(&AppArg::Home), "a managed profile asking for the Friends page stays on Home");
    assert!(!w.d.nav.tabs.stack.entries.iter().any(|entry| entry.arg == AppArg::Friends));
    // Even a stray toggle request writes nothing for it.
    w.bridge.community_run(CommunityCmd::SetShare(true));
    w.frames(10);
    assert_eq!(asked(&mut w), Vec::<&str>::new(), "nothing is read or written for a managed profile");
}
