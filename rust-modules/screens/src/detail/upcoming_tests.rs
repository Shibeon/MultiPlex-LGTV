//! The strip's upcoming cells (`episodes::UpcomingCell`, fed by `DetailScreen::set_upcoming`):
//! drawn after the season's own episodes, focusable like any cell, and inert on OK and on a hold.

use super::*;
use plx_data::upcoming::UpcomingEpisode;
use std::sync::Arc;

fn announced() -> Arc<[UpcomingEpisode]> {
    vec![
        UpcomingEpisode { index: 3, title: "TBA".into(), aired: Some("2031-02-03".into()), ..Default::default() },
        UpcomingEpisode { index: 4, title: "Finale".into(), summary: "It ends.".into(), ..Default::default() },
    ]
    .into()
}

/// The screen over the two-episode season of `detail`, handed the two upcoming episodes for the
/// list it shows, with the tick that keys them run.
fn screen_with_upcoming(guard: &plx_base::testlock::Serial) -> DetailScreen {
    let mut screen = bare(guard, ServerId::UNSET, "show");
    let list = test_store().view().current().unwrap().episodes.id();
    screen.set_upcoming(list, &announced());
    let _ = step(&mut screen, &ScreenEvent::Tick(plx_machine::machine::Tick { ms: 16, dt_us: 16_667 }), None);
    screen
}

#[test]
fn upcoming_cells_are_drawn_after_the_season_own_episodes() {
    let guard = install(detail(ServerId::UNSET, "show"));
    let screen = screen_with_upcoming(&guard);
    let meta = test_store().view();
    let d = screen.detail(meta).unwrap();
    assert_eq!(episodes::len(d, screen.upcoming_of(d)), 4, "two from the server, two announced");
    let measure = plx_ui::fixture::FixtureMeasure;
    let runs = plx_ui::draw_census::capture_text(|| {
        episodes::draw(plx_ui::Painter::recording(), d, screen.upcoming_of(d), 0.0, 0.0, None,
            |_| 1.0, |_| 1.0, |_| false, |_| Default::default(), &measure, meta);
    });
    let x_of = |text: &str| runs.iter().find(|(t, _)| t == text).map(|(_, r)| r.x)
        .unwrap_or_else(|| panic!("{text:?} is drawn: {runs:?}"));
    let tba = plx_platform::i18n::msg::browse_detail_upcoming_title_tba();
    let airs = plx_platform::i18n::msg::browse_detail_upcoming_airs(&plx_ui::fmt::pretty_date("2031-02-03", 0));
    let unknown = plx_platform::i18n::msg::browse_detail_upcoming_air_date_unknown();
    assert!(x_of("Episode 2") < x_of(tba), "the placeholder title is worded, after the server's last");
    assert!(x_of(tba) < x_of("Finale"));
    assert!(runs.iter().filter(|(t, _)| *t == airs).count() >= 1, "the air line is drawn: {runs:?}");
    // The undated cell has no image: its still says the date is unknown, and so does its air line.
    assert_eq!(runs.iter().filter(|(t, _)| t == unknown).count(), 2, "{runs:?}");
    assert!(!runs.iter().any(|(t, _)| t == "TBA"), "a placeholder is never drawn as sent");
    clear();
}

#[test]
fn an_upcoming_cell_is_reached_with_right_and_the_strip_ends_after_the_last() {
    let guard = install(detail(ServerId::UNSET, "show"));
    let screen = screen_with_upcoming(&guard);
    let measure = plx_ui::fixture::FixtureMeasure;
    let mut at = episode_key(&screen, 1, episodes::Row::Still).expect("the server's last episode is keyed");
    for i in 2..4 {
        let context = cx(&measure, Some(at));
        let Step::Move(next) = Focusable::<TestHost>::neighbour(&screen, FocusKey { entry: EntryId(7), elem: at }, Dir::Right, &context)
            else { panic!("Right from cell {} stopped", i - 1) };
        assert!(screen.locate(next.elem, test_store().view()) == Some(Located::Episode(i, episodes::Row::Still)));
        at = next.elem;
    }
    let context = cx(&measure, Some(at));
    assert!(matches!(
        Focusable::<TestHost>::neighbour(&screen, FocusKey { entry: EntryId(7), elem: at }, Dir::Right, &context),
        Step::Edge
    ), "the strip ends after the last upcoming cell");
    // Down reaches the upcoming cell's text stop, as on any cell.
    let Step::Move(text) = Focusable::<TestHost>::neighbour(&screen, FocusKey { entry: EntryId(7), elem: at }, Dir::Down, &context)
        else { panic!("Down from an upcoming still stopped") };
    assert!(screen.locate(text.elem, test_store().view()) == Some(Located::Episode(3, episodes::Row::Text)));
    clear();
}

#[test]
fn ok_and_a_hold_on_an_upcoming_cell_do_nothing() {
    let guard = install(detail(ServerId::UNSET, "show"));
    let mut screen = screen_with_upcoming(&guard);
    for row in [episodes::Row::Still, episodes::Row::Text] {
        let elem = episodes::elem(2, row).unwrap();
        let (_, effects) = step(&mut screen, &ScreenEvent::Activate(elem), Some(elem));
        assert!(!effects.iter().any(|e| matches!(e.fx, Fx::App(_))), "OK on an upcoming {row:?} asked for nothing");
        let (handled, effects) = step(&mut screen, &ScreenEvent::PressHold(plx_machine::machine::PressId(9)), Some(elem));
        assert!(!effects.iter().any(|e| matches!(e.fx, Fx::App(_))), "a hold on an upcoming {row:?} opens no menu");
        assert_eq!(handled, Handled::No);
    }
    // The server's own episode still plays or opens: the inertness is the upcoming cell's alone.
    let elem = episodes::elem(1, episodes::Row::Text).unwrap();
    let (_, effects) = step(&mut screen, &ScreenEvent::Activate(elem), Some(elem));
    assert!(effects.iter().any(|e| matches!(e.fx, Fx::App(_))), "the server's episode opens");
    clear();
}

#[test]
fn upcoming_cells_for_another_list_are_not_drawn() {
    let guard = install(detail(ServerId::UNSET, "show"));
    let mut screen = bare(&guard, ServerId::UNSET, "show");
    let list = test_store().view().current().unwrap().episodes.id();
    screen.set_upcoming(list.wrapping_add(1), &announced());
    let meta = test_store().view();
    let d = screen.detail(meta).unwrap();
    assert!(screen.upcoming_of(d).is_empty(), "a strip computed for another season's list is ignored");
    clear();
}
