//! Upcoming episodes: the episodes Plex's metadata provider announces for a season that the
//! server does not hold yet. The Detail page appends them to the season's episode strip, each
//! with its air date, so a viewer sees what is coming (`GET metadata.provider
//! /library/metadata/<season hex>/children`).
//!
//! This module holds the pure rule ([`upcoming_episodes`]: which announced episodes count, and
//! in what order), the card's title rule ([`is_placeholder_title`]) and the I/O seam the store's
//! job reads through ([`UpcomingIo`]; production is [`LiveUpcomingIo`]).
//! `stores::upcoming::UpcomingStore` owns the state; its read is a job the application's runner
//! starts.

use crate::metadata::EpisodeList;
use crate::watchlist::WatchlistAuth;
use plx_plex::plex::account::AccountClient;
use plx_plex::plex::watchlist::{ProviderEpisode, WatchlistError};

/// One announced episode the server does not hold, as the strip draws it. Strings are empty, not
/// optional, when the provider sent none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UpcomingEpisode {
    /// The episode's number in its season.
    pub index: i64,
    /// The provider's title, empty when it sent none. A placeholder ("TBA", "Episode 7") is kept
    /// as sent; the card asks [`is_placeholder_title`] what to show.
    pub title: String,
    pub summary: String,
    /// `originallyAvailableAt` as `YYYY-MM-DD`; `None` when the provider has no date yet.
    pub aired: Option<String>,
    /// An absolute `http(s)` image URL, empty when none.
    pub thumb: String,
}

/// What the server holds of a season, as far as its loaded pages tell: the rule's other input.
/// `None` from [`ServerSeason::of`] means too little is loaded to say which numbers are missing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerSeason {
    /// The episode numbers loaded from the server.
    held: Vec<i64>,
    /// Every page is loaded, so `held` is the whole season.
    complete: bool,
    /// The number of the season's last episode (episodes come in index order).
    last: Option<i64>,
}

impl ServerSeason {
    /// Read a season's server episodes. A list whose every page is loaded is exact. Otherwise the
    /// list's LAST episode must be loaded: the server sorts a season by index, so a number past
    /// it is certainly missing, and a number before it is taken as held (a gap the unloaded pages
    /// might fill is not announced). Anything less says nothing yet.
    pub fn of(list: &EpisodeList) -> Option<ServerSeason> {
        let complete = list.is_complete();
        let last = match list.len().checked_sub(1) {
            Some(i) => Some(list.get(i)?.index),
            None => None,
        };
        let held = list.iter_loaded().map(|(_, e)| e.index).collect();
        Some(ServerSeason { held, complete, last })
    }

    /// Does the server hold (or might it hold) episode `index`?
    fn holds(&self, index: i64) -> bool {
        self.held.contains(&index) || (!self.complete && self.last.is_some_and(|last| index <= last))
    }
}

/// The announced episodes that are coming: not held by the server (by number), and not aired yet
/// (no date, or a date on or after `today`), sorted by number. `today` is `YYYY-MM-DD`; when it is
/// unknown only an undated episode counts, since a dated one cannot be placed against today.
/// A date compares as text (`YYYY-MM-DD` orders like the date it spells). The same number
/// announced twice is shown once.
pub fn upcoming_episodes(announced: &[ProviderEpisode], server: &ServerSeason, today: Option<&str>) -> Vec<UpcomingEpisode> {
    let mut out: Vec<UpcomingEpisode> = announced
        .iter()
        .filter(|e| !server.holds(e.index))
        .filter(|e| match (e.aired.as_deref().filter(|d| !d.is_empty()), today) {
            (None, _) => true,
            (Some(d), Some(t)) => d >= t,
            (Some(_), None) => false,
        })
        .map(|e| UpcomingEpisode {
            index: e.index,
            title: e.title.clone().unwrap_or_default(),
            summary: e.summary.clone().unwrap_or_default(),
            aired: e.aired.clone().filter(|d| !d.is_empty()),
            thumb: e.thumb.clone().unwrap_or_default(),
        })
        .collect();
    out.sort_by_key(|e| e.index);
    out.dedup_by_key(|e| e.index);
    out
}

/// Is `title` a stand-in the provider uses before an episode is named: empty, "TBA", "TBD" or
/// "Episode <n>" (any case, surrounding spaces ignored)? The card then says the title is still
/// to be announced, in the viewer's language.
pub fn is_placeholder_title(title: &str) -> bool {
    let t = title.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("tba") || t.eq_ignore_ascii_case("tbd") {
        return true;
    }
    match t.get(..8) {
        Some(head) if head.eq_ignore_ascii_case("episode ") => {
            let n = &t[8..];
            !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())
        }
        _ => false,
    }
}

/// Everything the store's job asks of the outside world. Production is [`LiveUpcomingIo`].
pub trait UpcomingIo: Send + Sync {
    /// WORKER THREAD: the season's announced episodes, read as `auth` in `language`.
    fn read(&self, auth: &WatchlistAuth, hex: &str, language: Option<&str>) -> Result<Vec<ProviderEpisode>, WatchlistError>;
}

/// The live read: the account's plex.tv identity over the Watchlist's transport (which keeps the
/// dev-build read refusal), asking the metadata provider.
pub struct LiveUpcomingIo;

impl UpcomingIo for LiveUpcomingIo {
    fn read(&self, auth: &WatchlistAuth, hex: &str, language: Option<&str>) -> Result<Vec<ProviderEpisode>, WatchlistError> {
        let client = AccountClient::new(&auth.client_id, Some(&auth.credential));
        let mut transport = client.watchlist_transport();
        client.season_children(hex, language, &mut transport)
    }
}

#[cfg(test)]
#[path = "upcoming_tests.rs"]
mod upcoming_tests;
