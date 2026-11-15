//! **The Friends page's rules** (the web app's `Activity.tsx`): how the friends' activity feed is
//! cut into posts and days, who the filter row offers, and how long ago an entry was. Pure: time is
//! an argument (`now`, unix seconds) and so is the viewer's UTC offset (`offset`, seconds east), which
//! the application reads off the platform's local calendar ([`utc_offset`]) where it reads the clock.
//!
//! `stores::community::CommunityStore` owns the feed, the privacy setting and the live sessions; the
//! screen reads them and words them through these functions.
use plx_plex::plex::community::{Friend, FriendActivity};

/// Seconds in a day.
const DAY: i64 = 86_400;

/// One post of the feed: a friend and a movie, or a show and the episodes they watched of it in a
/// row (the web's `Post`). `episodes` runs newest first, as the feed does; [`Post::watched_in_order`]
/// gives them in the order they were watched.
#[derive(Clone, Debug, PartialEq)]
pub struct Post {
    pub first: FriendActivity,
    pub episodes: Vec<FriendActivity>,
}

/// What the post's verb says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verb {
    /// "a regardé" (a movie, or anything that is not an episode).
    Movie,
    /// "a regardé un épisode de".
    Episode,
    /// "a regardé N épisodes de".
    Episodes(usize),
}

impl Post {
    pub fn verb(&self) -> Verb {
        match self.episodes.len() {
            0 => Verb::Movie,
            1 => Verb::Episode,
            n => Verb::Episodes(n),
        }
    }

    /// The episodes in the order they were watched (the feed's reversed).
    pub fn watched_in_order(&self) -> impl Iterator<Item = &FriendActivity> {
        self.episodes.iter().rev()
    }
}

/// The local day (days since 1970-01-01 in the viewer's zone) an instant falls on; `None` for an
/// entry whose date did not parse.
pub fn local_day(at: Option<i64>, offset: i64) -> Option<i64> {
    at.map(|at| (at + offset).div_euclid(DAY))
}

/// **The web's `toPosts`**: consecutive episodes of the same show, by the same friend, on the same
/// local day, are one post; anything else starts a new one. A movie's post has no episodes.
pub fn to_posts(feed: &[FriendActivity], offset: i64) -> Vec<Post> {
    let mut posts: Vec<Post> = Vec::new();
    for a in feed {
        let joins = posts.last().is_some_and(|last| {
            a.show.is_some()
                && last.first.show == a.show
                && last.first.user.id == a.user.id
                && local_day(a.at, offset).is_some()
                && local_day(last.first.at, offset) == local_day(a.at, offset)
        });
        match posts.last_mut() {
            Some(last) if joins => last.episodes.push(a.clone()),
            _ => posts.push(Post { first: a.clone(), episodes: if a.show.is_some() { vec![a.clone()] } else { Vec::new() } }),
        }
    }
    posts
}

/// One day of the feed and its posts, in the feed's order.
#[derive(Clone, Debug, PartialEq)]
pub struct Day {
    /// The local day ([`local_day`]); `None` gathers entries whose date did not parse.
    pub day: Option<i64>,
    pub posts: Vec<Post>,
}

/// The feed cut into days (a new day starts whenever the post's day changes, as the web's), keeping
/// only `who`'s entries when a friend is chosen (`None`: everyone).
pub fn days_of(feed: &[FriendActivity], who: Option<&str>, offset: i64) -> Vec<Day> {
    let kept: Vec<FriendActivity> = feed.iter().filter(|a| who.is_none_or(|id| a.user.id == id)).cloned().collect();
    let mut days: Vec<Day> = Vec::new();
    for post in to_posts(&kept, offset) {
        let day = local_day(post.first.at, offset);
        match days.last_mut() {
            Some(last) if last.day == day => last.posts.push(post),
            _ => days.push(Day { day, posts: vec![post] }),
        }
    }
    days
}

/// How a day's heading reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DayWord {
    Today,
    Yesterday,
    /// The civil date `[year, month, day]`.
    Date([i32; 3]),
    /// Entries whose date did not parse.
    Unknown,
}

/// The heading of `day` seen from `today` (both [`local_day`]s). A day after today (a clock behind
/// the service's) reads as today, as the web's `Math.max(0, days)`.
pub fn day_word(day: Option<i64>, today: i64) -> DayWord {
    match day {
        None => DayWord::Unknown,
        Some(d) if today - d <= 0 => DayWord::Today,
        Some(d) if today - d == 1 => DayWord::Yesterday,
        Some(d) => DayWord::Date(civil_of_days(d)),
    }
}

/// The civil date of a day count since 1970-01-01 (Howard Hinnant's algorithm).
pub fn civil_of_days(days: i64) -> [i32; 3] {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    [y as i32, m as i32, d as i32]
}

/// Days since 1970-01-01 of a civil date.
fn days_of_civil([y, m, d]: [i32; 3]) -> i64 {
    let (m, d) = (i64::from(m), i64::from(d));
    let y = i64::from(y) - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The friends present in the feed with how many entries each has, the most active first (ties in
/// the order they first appear): the filter row after "All".
pub fn friends_of(feed: &[FriendActivity]) -> Vec<(Friend, usize)> {
    let mut seen: Vec<(Friend, usize)> = Vec::new();
    for a in feed {
        match seen.iter_mut().find(|(f, _)| f.id == a.user.id) {
            Some((_, n)) => *n += 1,
            None => seen.push((a.user.clone(), 1)),
        }
    }
    seen.sort_by(|x, y| y.1.cmp(&x.1));
    seen
}

/// How long ago an entry was (the web's `ago`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ago {
    /// Under an hour: whole minutes, rounded.
    Minutes(i64),
    /// Under a day: whole hours, rounded.
    Hours(i64),
    /// A day or more: the local clock time `[hour, minute]` (the day's heading says which day).
    Clock([i32; 2]),
    Unknown,
}

/// [`Ago`] of an entry at `at`, seen at `now`, in a zone `offset` seconds east of UTC.
pub fn ago(at: Option<i64>, now: i64, offset: i64) -> Ago {
    let Some(at) = at else { return Ago::Unknown };
    let secs = (now - at).max(0);
    if secs < 3_600 {
        return Ago::Minutes((secs + 30) / 60);
    }
    if secs < DAY {
        return Ago::Hours((secs + 1_800) / 3_600);
    }
    let local = (at + offset).rem_euclid(DAY);
    Ago::Clock([(local / 3_600) as i32, (local % 3_600 / 60) as i32])
}

/// The viewer's offset east of UTC in seconds, from what the platform says the local date and clock
/// time are at the instant `now`, rounded to the quarter hour (every zone in use is a multiple).
pub fn utc_offset(now: i64, local_date: [i32; 3], local_hm: [i32; 2]) -> i64 {
    let local = days_of_civil(local_date) * DAY + i64::from(local_hm[0]) * 3_600 + i64::from(local_hm[1]) * 60;
    let raw = local - now.div_euclid(60) * 60;
    (raw + 450).div_euclid(900) * 900
}

#[cfg(test)]
#[path = "community_tests.rs"]
mod community_tests;
