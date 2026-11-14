//! The Watchlist action: where it sits in the More menu, and what a press asks for. Pure values
//! only — the menu's presentation and the effect push are `DetailScreen`'s.

use super::*;
use plx_data::catalog::Membership;

const HEX: &str = "5d776b1d96b655001f2a9c3e";
const GUID: &str = "plex://movie/5d776b1d96b655001f2a9c3e";

fn sets() -> Vec<HeroSet> {
    let mut v = Vec::new();
    for mark in [PosterMark::None, PosterMark::InProgress, PosterMark::Watched] {
        for version in [false, true] {
            for restart in [false, true] {
                for trailer in [false, true] {
                    for alt in [false, true] {
                        for go_show in [false, true] {
                            v.push(HeroSet { watchlist: true, go_show, version, restart, trailer, alt, mark });
                        }
                    }
                }
            }
        }
    }
    v
}

/// The Watchlist row stands after the watched toggle in every menu the hero can offer, exactly
/// once, and the rest keep their relative order.
#[test]
fn the_watchlist_row_follows_the_watch_toggle_in_every_menu() {
    for s in sets() {
        let (v, n) = menu_ctls(s);
        assert_eq!(v[..n].iter().filter(|c| **c == HeroCtl::Watchlist).count(), 1, "{s:?}");
        let mut want = Vec::new();
        if s.restart { want.push(HeroCtl::Restart); }
        if s.trailer { want.push(HeroCtl::Trailer); }
        if s.version { want.push(HeroCtl::Version); }
        if s.go_show { want.push(HeroCtl::GoToShow); }
        want.push(if s.mark == PosterMark::Watched { HeroCtl::MarkUnwatched } else { HeroCtl::MarkWatched });
        want.push(HeroCtl::Watchlist);
        if s.alt { want.push(HeroCtl::Alt); }
        assert_eq!(&v[..n], &want[..], "{s:?}");
        assert!(n <= MAX_CTLS);
    }
}

#[test]
fn the_elem_id_is_fresh_in_the_hero_range_and_round_trips() {
    let all = [
        ELEM_PLAY, ELEM_RESTART, ELEM_ALT, ELEM_MARK_WATCHED, ELEM_MARK_UNWATCHED, ELEM_TRAILER,
        ELEM_WATCHLIST, ELEM_GO_TO_SHOW, ELEM_VERSION, ELEM_MORE,
    ];
    let mut seen = std::collections::BTreeSet::new();
    for e in all {
        assert!(e < HERO_ELEM_RANGE_END);
        assert!(seen.insert(e), "elem {e} is minted twice");
        assert_eq!(HeroCtl::of_elem(e).map(HeroCtl::elem), Some(e));
    }
    assert_eq!(HeroCtl::Watchlist.elem(), ELEM_WATCHLIST);
    assert_eq!(HeroCtl::of_elem(HERO_ELEM_RANGE_END), None);
}

/// Full-trailer mode still narrows the row to Play alone: More is not an anchor.
#[test]
fn full_trailer_mode_drops_more_from_the_focus_extent() {
    for s in sets() {
        let (v, n) = visible_ctls(s, true);
        assert_eq!((v[0], n), (HeroCtl::Play, 1), "{s:?}");
        assert!(!focusable(HeroCtl::More, true));
        assert!(focusable(HeroCtl::More, false));
        assert_eq!(visible_ctls(s, false).1, hero_ctls(s).1);
    }
}

/// Membership x press: Add from Off and Unknown, Remove from On; no usable guid, no press.
#[test]
fn the_press_table() {
    use crate::registry::ItemMenuAction as Action;
    let hex = plx_data::watchlist::decode_hex(HEX).unwrap();
    assert_eq!(guid_hex(GUID), Some(hex));
    assert_eq!(press(GUID, Membership::Off), Some(Action::WatchlistAdd { hex }));
    assert_eq!(press(GUID, Membership::Unknown), Some(Action::WatchlistAdd { hex }));
    assert_eq!(press(GUID, Membership::On), Some(Action::WatchlistRemove(GUID.into())));
    for bad in ["", "plex://movie/", "plex://movie/zz", "/library/metadata/42"] {
        assert_eq!(press(bad, Membership::Off), None, "{bad:?}");
        assert_eq!(press(bad, Membership::On), None, "{bad:?}");
    }
}

/// Without a Watchlist-takeable guid the menu has no Watchlist row at all.
#[test]
fn an_item_without_a_watchlist_guid_has_no_watchlist_row() {
    for s in sets() {
        let s = HeroSet { watchlist: false, ..s };
        let (v, n) = menu_ctls(s);
        assert!(!v[..n].contains(&HeroCtl::Watchlist), "{s:?}");
        assert!(menu_rows(s, false, Membership::On).iter().all(|r| r.elem != ELEM_WATCHLIST));
    }
}
