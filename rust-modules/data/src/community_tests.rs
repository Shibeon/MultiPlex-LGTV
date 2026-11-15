use super::*;
use plx_plex::plex::community::{ActivityKind, Friend, FriendActivity};

const DAY: i64 = 86_400;
/// 2026-11-15T12:00:00Z.
const NOON: i64 = 1_794_744_000;

fn friend(id: &str, name: &str) -> Friend {
    Friend { id: id.into(), name: name.into(), username: None, avatar: None }
}

fn movie(id: &str, who: &Friend, at: i64, title: &str) -> FriendActivity {
    FriendActivity {
        id: id.into(), date: String::new(), at: Some(at), user: who.clone(), kind: ActivityKind::Movie,
        title: title.into(), show: None, season: None, episode: None, year: Some(2001), poster: None, art: None,
        catalog_hex: Some("5d776b59ad5437001f79c6f8".into()),
    }
}

fn episode(id: &str, who: &Friend, at: i64, show: &str, s: i64, e: i64) -> FriendActivity {
    FriendActivity {
        kind: ActivityKind::Episode, show: Some(show.into()), season: Some(s), episode: Some(e),
        title: format!("Episode {e}"), ..movie(id, who, at, "")
    }
}

/// The web's `toPosts`: consecutive episodes of one show, by one friend, on one LOCAL day, are one
/// post; a movie is a post of its own with no episodes; another friend, show or day breaks the run.
#[test]
fn consecutive_episodes_of_one_show_by_one_friend_on_one_day_are_one_post() {
    let (a, b) = (friend("1", "Camille"), friend("2", "Léo"));
    let feed = vec![
        episode("e3", &a, NOON, "Archipel", 1, 3),
        episode("e2", &a, NOON - 3_600, "Archipel", 1, 2),
        episode("x1", &b, NOON - 4_000, "Archipel", 1, 1),
        movie("m1", &a, NOON - 5_000, "Film"),
        episode("e1", &a, NOON - 6_000, "Archipel", 1, 1),
        episode("e0", &a, NOON - DAY, "Archipel", 1, 0),
    ];
    let posts = to_posts(&feed, 0);
    let shape: Vec<(&str, usize)> = posts.iter().map(|p| (p.first.id.as_str(), p.episodes.len())).collect();
    assert_eq!(shape, [("e3", 2), ("x1", 1), ("m1", 0), ("e1", 1), ("e0", 1)]);
    assert_eq!(posts[0].verb(), Verb::Episodes(2));
    assert_eq!(posts[1].verb(), Verb::Episode);
    assert_eq!(posts[2].verb(), Verb::Movie);
    // In the order they were watched, as the web lists them: the feed runs newest first.
    assert_eq!(posts[0].watched_in_order().map(|e| e.episode).collect::<Vec<_>>(), [Some(2), Some(3)]);
}

/// The day boundary is the viewer's: two episodes 23:30 and 00:30 local are two days, even when
/// they are one UTC day, and an hour apart in the same local day stay one post.
#[test]
fn the_day_a_post_belongs_to_is_the_viewers_local_day() {
    let a = friend("1", "Camille");
    // 2026-11-15T22:30Z and 23:30Z: one UTC day; at UTC+1 the second is 00:30 on the 16th.
    let late = NOON + 10 * 3_600 + 1_800;
    let feed = vec![episode("b", &a, late + 3_600, "Archipel", 1, 2), episode("a", &a, late, "Archipel", 1, 1)];
    assert_eq!(to_posts(&feed, 0).len(), 1);
    assert_eq!(to_posts(&feed, 3_600).len(), 2);
    assert_eq!(local_day(Some(late), 3_600), local_day(Some(NOON), 0));
}

/// Day groups follow the posts' order; a filter keeps one friend's entries (an empty id is "All").
#[test]
fn days_group_posts_and_the_filter_keeps_one_friend() {
    let (a, b) = (friend("1", "Camille"), friend("2", "Léo"));
    let feed = vec![
        movie("m1", &a, NOON, "Un"),
        movie("m2", &b, NOON - 60, "Deux"),
        movie("m3", &a, NOON - DAY, "Trois"),
        movie("m4", &b, NOON - 3 * DAY, "Quatre"),
    ];
    let all = days_of(&feed, None, 0);
    assert_eq!(all.iter().map(|d| d.posts.len()).collect::<Vec<_>>(), [2, 1, 1]);
    let today = local_day(Some(NOON), 0).unwrap();
    assert_eq!(day_word(all[0].day, today), DayWord::Today);
    assert_eq!(day_word(all[1].day, today), DayWord::Yesterday);
    assert_eq!(day_word(all[2].day, today), DayWord::Date([2026, 11, 12]));
    let leo = days_of(&feed, Some("2"), 0);
    assert_eq!(leo.iter().flat_map(|d| d.posts.iter().map(|p| p.first.id.as_str())).collect::<Vec<_>>(), ["m2", "m4"]);
    assert_eq!(day_word(None, today), DayWord::Unknown);
}

/// An entry whose date did not parse keeps its place in a day of its own ("no day"), and never
/// joins a run of episodes.
#[test]
fn an_undated_entry_is_kept_in_no_day() {
    let a = friend("1", "Camille");
    let mut undated = episode("u", &a, 0, "Archipel", 1, 1);
    undated.at = None;
    let feed = vec![episode("d", &a, NOON, "Archipel", 1, 2), undated];
    let days = days_of(&feed, None, 0);
    assert_eq!(days.len(), 2);
    assert_eq!(days[1].day, None);
    assert_eq!(days[1].posts[0].first.id, "u");
}

/// The filter row: the friends present in the feed, the most active first (ties keep the feed's
/// order).
#[test]
fn friends_are_ranked_by_how_much_they_watched() {
    let (a, b, c) = (friend("1", "Camille"), friend("2", "Léo"), friend("3", "Inès"));
    let feed = vec![
        movie("1", &a, NOON, "x"),
        movie("2", &b, NOON, "x"),
        movie("3", &b, NOON, "x"),
        movie("4", &c, NOON, "x"),
        movie("5", &b, NOON, "x"),
    ];
    let friends = friends_of(&feed);
    let ranked: Vec<(&str, usize)> = friends.iter().map(|(f, n)| (f.name.as_str(), *n)).collect();
    assert_eq!(ranked, [("Léo", 3), ("Camille", 1), ("Inès", 1)]);
}

/// The web's `ago`: minutes under an hour, hours under a day, else the local clock time.
#[test]
fn the_relative_time_is_minutes_then_hours_then_the_clock() {
    assert_eq!(ago(Some(NOON - 30), NOON, 0), Ago::Minutes(1), "rounded, as Intl's -0.5 minute");
    assert_eq!(ago(Some(NOON - 10), NOON, 0), Ago::Minutes(0));
    assert_eq!(ago(Some(NOON - 59 * 60), NOON, 0), Ago::Minutes(59));
    assert_eq!(ago(Some(NOON - 3 * 3_600), NOON, 0), Ago::Hours(3));
    assert_eq!(ago(Some(NOON - DAY - 3_600), NOON, 7_200), Ago::Clock([13, 0]));
    assert_eq!(ago(Some(NOON + 120), NOON, 0), Ago::Minutes(0), "a clock skew is never in the future");
    assert_eq!(ago(None, NOON, 0), Ago::Unknown);
}

/// The viewer's UTC offset, from the platform's local date and clock time for a given instant.
#[test]
fn the_utc_offset_is_read_off_the_local_calendar() {
    assert_eq!(utc_offset(NOON, [2026, 11, 15], [13, 0]), 3_600);
    assert_eq!(utc_offset(NOON, [2026, 11, 15], [7, 30]), -16_200);
    assert_eq!(utc_offset(NOON + 11 * 3_600, [2026, 11, 16], [1, 0]), 7_200, "across midnight");
    assert_eq!(utc_offset(NOON + 1_234, [2026, 11, 15], [12, 20]), 0, "seconds round to the quarter hour");
}
