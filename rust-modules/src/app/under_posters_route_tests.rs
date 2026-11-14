//! **R13 at the app tier: the Under posters setting reaches the Library's posters** (`routes.md`),
//! on a real `Dispatcher<AppHost>` over a real store.
//!
//! The Settings page's walk (`screens/src/settings_nav_structure_tests.rs`) is key-driven up to the
//! write door: OK on the Title switch goes out as a `PreferenceCmd::Choose`. The door's confirmed
//! write publishes the live cell from the storage worker, which the test lock refuses to let touch
//! a process global (`plx_base::testlock`), so this test publishes the way the door ends
//! (`restore_under_posters`, the cell's own door) and grades the other half: the grid under the
//! Settings surface words its captions from the new value.

use super::*;
use super::test_support::World;
use plx_plex::plex::session::{UnderPosters, UnderPostersLadder};

/// Holds the live setting on its default for the test and puts it back after it.
struct Cell;
impl Cell {
    fn default() -> Self {
        plx_media::route::restore_under_posters(UnderPosters::DEFAULT);
        Cell
    }
}
impl Drop for Cell {
    fn drop(&mut self) { plx_media::route::restore_under_posters(UnderPosters::DEFAULT); }
}

/// The caption lines the poster at `index` of the library grid DREW (`draw_census::label_lines` over
/// the label runs under that card: the block itself, read off the frame, not asked of the label
/// builder). A Library page names every card, so the runs are kept to the one card's column.
fn caption_under_poster(w: &mut World, index: usize) -> Vec<String> {
    let cell = w.poster_cell(index);
    let runs: Vec<_> = w.drawn().runs.into_iter()
        .filter(|run| run.at.y >= cell.y + cell.h && run.at.y < cell.y + cell.h + plx_ui::cards::EVERY_CAPTION_ROW_GAP
            && run.at.x < cell.x + cell.w && run.at.x + run.at.w > cell.x)
        .collect();
    plx_ui::draw_census::label_lines(&runs)
}

#[test]
fn the_under_posters_setting_changes_the_caption_under_the_library_grids_posters() {
    let mut w = World::movies_library("r13-caption", false);
    let _cell = Cell::default();
    w.bridge.stores.browse.borrow_mut().seed_items_for_test(120);
    w.frames(30);
    assert_eq!(w.grid_cell(), Some((0, 0)), "premise: focus is on the first poster");
    // A Library page names every card with its title (the web library's grid), whatever the Title
    // switch says. (The seeded films carry no year or length, so the details line says what they are.)
    for index in [0, 1] {
        assert_eq!(caption_under_poster(&mut w, index), [format!("Item {index}").as_str(), "Movie"],
            "premise: the default names poster {index} and says what it is");
    }

    // Settings > Libraries > Under posters opens over the grid; the details are set to Nothing, and
    // the grid, back on top, words its captions from the new value: the title alone.
    open_settings_at(&mut w.d, plx_screens::family::SettingsPage::UnderPosters);
    w.frames(40);
    assert_eq!(w.elem(), Some(1), "premise: the page is seated on the Title switch");
    plx_media::route::restore_under_posters(UnderPosters { title: false, details: UnderPostersLadder::None.movie_parts(), show_details: UnderPostersLadder::None.show_parts(), sort_value: false });
    w.frames(30);
    w.press(Key::Back);
    w.frames(60);
    assert_eq!(w.grid_cell(), Some((0, 0)), "premise: back on the grid, on the same poster");
    assert_eq!(caption_under_poster(&mut w, 0), ["Item 0"], "Nothing takes the details line from under the poster");
    assert_eq!(caption_under_poster(&mut w, 1), ["Item 1"], "and from under the unfocused one too");

    // And back: the details line returns with the setting, the title stays.
    plx_media::route::restore_under_posters(UnderPosters { title: true, details: UnderPostersLadder::YearLength.movie_parts(), show_details: UnderPostersLadder::YearLength.show_parts(), sort_value: false });
    w.frames(30);
    assert_eq!(caption_under_poster(&mut w, 0), ["Item 0", "Movie"], "the details line comes back under the poster");
}

/// **The Collections page names every collection, by the ONE collection rule.** On the Collections
/// page the face sets the collection's name (a baked fan's live name, the neutral tile's), and a
/// long one ends in an ellipsis there. A Library page names every card with its title whatever the
/// Title switch says, so a name the face cut is always told on the title line (the rule that carries
/// it on the details line applies where the title line is off: `registry::captions`); the details
/// line is the member count under every kind but Nothing.
#[test]
fn a_name_the_face_cuts_rides_the_caption_by_the_one_collection_rule() {
    const FAN: &str = "/library/collections/9/composite/1700000000";
    let long = "The Complete Extended Director's Cut Anniversary Edition Saga Collection Box Set";
    let mut w = World::collections("s23-caption", false);
    let _cell = Cell::default();
    let longer = [long; 3].join(" ");
    w.bridge.stores.browse.borrow_mut().seed_named_collections_for_test(&[
        (long, FAN),
        ("Marvel", FAN),
        (long, "/library/metadata/5/thumb/1"),
        (&longer, ""),
    ]);
    w.frames(30);
    w.press(Key::Down);
    w.frames(30);
    // (name, member count, does the face cut it)
    let tiles: [(&str, &str, bool); 4] = [(long, "2 films", true), ("Marvel", "3 films", false),
        (long, "4 films", false), (&longer, "5 films", true)];
    for (i, (name, count, cut)) in tiles.into_iter().enumerate() {
        for (title, details) in [(false, UnderPostersLadder::YearLength), (false, UnderPostersLadder::None),
            (true, UnderPostersLadder::YearLength), (true, UnderPostersLadder::None)] {
            plx_media::route::restore_under_posters(UnderPosters { title, details: details.movie_parts(), show_details: details.show_parts(), sort_value: false });
            w.frames(60);
            let _ = cut;
            let want: Vec<String> = match details {
                UnderPostersLadder::None => vec![name.to_owned()],
                _ => vec![name.to_owned(), count.to_owned()],
            };
            let lines = caption_under_poster(&mut w, i);
            // The focused card's block glides a name that does not fit; an unfocused one elides it.
            assert_eq!(lines.len(), want.len(), "collection {i}: title {title}, details {details:?}: {lines:?}");
            assert!(lines.iter().zip(&want).all(|(drawn, want)| drawn == want || want.starts_with(drawn.trim_end_matches('\u{2026}'))),
                "collection {i}: title {title}, details {details:?}: {lines:?} against {want:?}");
        }
        w.press(Key::Right);
        w.frames(30);
    }
}
