//! **R15, the Library switch**, key-driven on a real `Dispatcher<AppHost>` over a real store: the
//! header's library chip after a library switch settles.

use super::test_support::{World, FILM_CLUB};

/// The section the STORE holds as current: the truth the page's published snapshot follows a frame
/// later.
fn store_current(r: &World) -> Option<usize> {
    let mut snapshot = plx_data::stores::browse::DirectorySnapshot::default();
    r.bridge.stores.browse.borrow().observe_pin_write();
    let mut keys = plx_plex::plex::ClientKeys::default();
    crate::app::adapters::stores::refill_client_keys(&mut keys);
    let input = plx_data::browse::BrowseRosterInput { tenure: 0,
        keys: &keys, session: &plx_plex::plex::session::peek(),
        visible_generation: plx_plex::plex::session::visible_generation(),
        settled: plx_plex::plex::session::peek_settled().is_some(),
        profile_key: &plx_plex::plex::session::current_profile_key(),
    };
    r.bridge.stores.browse.borrow_mut().capture_directory(&mut snapshot, &input);
    snapshot.view().current()
}

/// **The library chip names the library that was switched to** once the switch settles.
///
/// The page reads which library is "on view" from its own `pending` target while the grid fades out
/// and from the directory's current library afterwards (`view_section`). `flush` consumes `pending`
/// in the frame the store is told to switch, and the page's published snapshot follows the store a
/// frame later, so there is a window in which `pending` is gone and the snapshot still names the OLD
/// library. A fixture with no store never leaves that window (its directory never changes), so the
/// header (then its selection plate, now the chip) went back to the old library from the fifth frame
/// on (lane F2). With the real store the directory follows: this pins that it does, on EVERY frame,
/// the window included.
#[test]
fn the_library_chip_names_the_library_that_was_switched_to_once_the_switch_settles() {
    let mut r = World::movies_library("route-plate-after-switch", true);
    let chip_names = |r: &World| r.library().probe_libraries().iter().map(|(_, section)| *section).collect::<Vec<_>>();
    assert_eq!(chip_names(&r), [0], "premise: the chip names Movies");

    // OK on Film Club's row in the chip's list switches; watch every frame until the store has
    // followed and a while after.
    r.pick_library(FILM_CLUB);
    let mut landed_at = None;
    for frame in 0..90 {
        let (chip, store, published) = (chip_names(&r), store_current(&r), r.bridge.browse_directory().current());
        if landed_at.is_none() && chip == [FILM_CLUB] { landed_at = Some(frame); }
        if landed_at.is_some() {
            assert_eq!(chip, [FILM_CLUB], "frame {frame}: the chip went back to {chip:?} after naming Film Club (store {store:?}, published {published:?})");
        }
        r.step(vec![]);
    }
    assert!(landed_at.is_some(), "the OK switched the chip");
    assert_eq!(store_current(&r), Some(FILM_CLUB), "premise: the switch settled, `flush` consumed `pending` and the store followed");
    assert_eq!(r.bridge.browse_directory().current(), Some(FILM_CLUB), "the page's snapshot followed the store");
}

/// **The title's count pill only ever counts the library on view.** From the OK that switches
/// libraries until the new library's listing lands, the published listing (and the totals noted
/// from it) are the library left's, so the pill shows nothing rather than the old count; once the
/// new listing lands it counts that one (review of 88e35ab).
#[test]
fn the_count_pill_never_shows_the_library_left_across_a_switch() {
    let mut r = World::movies_library("route-count-across-switch", true);
    r.frames(30);
    let pill = |r: &World| r.library().probe_title().1;
    let movies = pill(&r);
    assert!(!movies.is_empty(), "premise: the pill counts Movies");
    let film_club = {
        let view = r.bridge.directory.view();
        let section = &view.sections()[FILM_CLUB];
        (section.sid.expect("premise: Film Club has an address"), section.key)
    };
    r.pick_library(FILM_CLUB);
    let listed = |r: &World| r.bridge.listing.view().id().map(|id| (id.sid, id.section));
    for frame in 0..30 {
        let count = pill(&r);
        assert!(count.is_empty() || (count != movies && listed(&r) == Some(film_club)),
            "frame {frame}: the pill reads {count:?} (Movies counted {movies:?}) for the listing {:?}", listed(&r));
        r.step(vec![]);
    }
    r.bridge.stores.browse.borrow_mut().seed_items_for_test(37);
    for frame in 0..60 {
        let count = pill(&r);
        assert!(count.is_empty() || (count != movies && listed(&r) == Some(film_club)),
            "frame {frame} after the answer: the pill reads {count:?} (Movies counted {movies:?}) for the listing {:?}", listed(&r));
        r.step(vec![]);
    }
    assert_eq!(listed(&r), Some(film_club), "premise: Film Club's listing landed");
    assert_eq!(pill(&r), plx_platform::i18n::msg::library_header_items(37), "the pill counts the library switched to");
}

/// **OK on the ticked library in the chip's list changes nothing.** The list opens focused on the
/// library on view; picking it must not switch to it again: no grid dissolve and return, no
/// re-request of the same section, the store's current library untouched (review of 88e35ab: the
/// menu's select reached `switch_library` with no "already on view" check).
#[test]
fn picking_the_ticked_library_in_the_chips_list_reloads_nothing() {
    let mut r = World::movies_library("route-pick-ticked", true);
    r.frames(30);
    let (page_a, grid_a, _, key, tiles) = r.library().probe_content();
    assert_eq!((page_a, grid_a), (1.0, 1.0), "premise: the page is settled");
    assert!(tiles > 0, "premise: the grid holds the listing");
    r.pick_library(0);
    for frame in 0..60 {
        let (page, grid, _, now, _) = r.library().probe_content();
        assert_eq!((page, grid), (1.0, 1.0), "frame {frame}: the page or the grid faded for a pick of the library on view");
        assert_eq!(now, key, "frame {frame}: the section changed");
        r.step(vec![]);
    }
    assert_eq!(store_current(&r), Some(0), "the store still holds the library on view");
    assert!(r.on(&plx_screens::registry::AppArg::Library), "the list closed onto the Library");
}
