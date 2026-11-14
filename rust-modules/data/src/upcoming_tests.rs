use super::*;
use crate::metadata::{Episode, EpisodeList, EPISODE_PAGE};

fn ep(index: i64) -> Episode {
    Episode { rk: format!("e{index}"), index, ..Default::default() }
}

fn announced(index: i64, aired: Option<&str>) -> ProviderEpisode {
    ProviderEpisode {
        index,
        title: Some(format!("Title {index}")),
        summary: None,
        aired: aired.map(str::to_string),
        thumb: None,
    }
}

fn indices(list: &[UpcomingEpisode]) -> Vec<i64> {
    list.iter().map(|e| e.index).collect()
}

#[test]
fn an_announced_episode_counts_when_the_server_lacks_it_and_it_has_not_aired() {
    let server = ServerSeason::of(&EpisodeList::from(vec![ep(1), ep(2), ep(3)])).expect("a whole season");
    let catalogue = [
        announced(6, Some("2026-12-01")), // future, missing: counts
        announced(2, Some("2027-01-01")), // on the server: never
        announced(4, None),               // no date, missing: counts
        announced(5, Some("2026-11-14")), // airs today: counts
        announced(7, Some("2026-11-13")), // aired yesterday, missing: not upcoming
        announced(4, Some("2026-12-24")), // the same number twice: shown once
    ];
    let out = upcoming_episodes(&catalogue, &server, Some("2026-11-14"));
    assert_eq!(indices(&out), vec![4, 5, 6], "sorted by number, aired and held ones left out");
    assert_eq!(out[0].aired, None);
    assert_eq!(out[1].aired.as_deref(), Some("2026-11-14"));
    assert_eq!(out[2].title, "Title 6");
}

#[test]
fn without_today_only_an_undated_episode_counts() {
    let server = ServerSeason::of(&EpisodeList::from(vec![ep(1)])).unwrap();
    let catalogue = [announced(2, Some("2030-01-01")), announced(3, None), announced(4, Some(""))];
    assert_eq!(indices(&upcoming_episodes(&catalogue, &server, None)), vec![3, 4], "an empty date is no date");
}

#[test]
fn an_empty_server_season_shows_every_coming_episode() {
    let server = ServerSeason::of(&EpisodeList::default()).expect("an empty season is known");
    let out = upcoming_episodes(&[announced(1, None), announced(2, Some("2031-02-03"))], &server, Some("2026-11-14"));
    assert_eq!(indices(&out), vec![1, 2]);
}

#[test]
fn a_partly_loaded_season_counts_only_numbers_past_its_last_episode() {
    // Two pages; only the second (holding the season's last episode) is loaded.
    let total = EPISODE_PAGE + 2;
    let mut list = EpisodeList::with_total(total);
    list.set_page(1, (EPISODE_PAGE as i64 + 1..=total as i64).map(ep).collect());
    let server = ServerSeason::of(&list).expect("the last episode is loaded");
    let last = total as i64;
    let catalogue = [announced(3, None), announced(last + 1, None)];
    assert_eq!(indices(&upcoming_episodes(&catalogue, &server, Some("2026-11-14"))), vec![last + 1],
        "a number the unloaded first page may hold is not announced");

    // Only the first page: the season's end is not known, so nothing is said yet.
    let mut list = EpisodeList::with_total(total);
    list.set_page(0, (1..=EPISODE_PAGE as i64).map(ep).collect());
    assert_eq!(ServerSeason::of(&list), None);
}

#[test]
fn a_placeholder_title_is_told_apart_from_a_real_one() {
    for t in ["", "  ", "TBA", "tba", "TBD", "Episode 7", "episode 12", " EPISODE 3 "] {
        assert!(is_placeholder_title(t), "{t:?} is a placeholder");
    }
    for t in ["Pilot", "Episode", "Episode 7: The Return", "Episode seven", "TBA Part 2", "Épisode 7"] {
        assert!(!is_placeholder_title(t), "{t:?} is a real title");
    }
}
