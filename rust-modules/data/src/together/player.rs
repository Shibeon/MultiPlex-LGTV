//! The player's side of a session, pure: what the player reports becomes the session's
//! observation ([`observation`]), which playback a session may drive ([`attachable`]), and the one
//! gate every [`Follow`](crate::stores::together::TogetherPlayerRequest::Follow) goes through
//! ([`follow_verdict`]). The application reads the player and performs the verdicts; nothing here
//! reads a player, a registry or a clock.
//!
//! **One active playback** (the web's `TogetherGate`). While a session is live, the player plays
//! the session's title or nothing of the session's: a Follow never replaces the person's own
//! playback without asking ([`FollowVerdict::Ask`]), and a participant's player playing another
//! title is never driven by the session ([`attachable`] is false, so the session reads it as not
//! in the player). The gate fails closed: a title it cannot match is a different title, and a
//! title the application cannot find on a server it sees is not opened at all (the application's
//! `FollowState::Missing`).

use plx_playback_contract::readout::PlaybackState;

use super::sync::PlayerObs;
use super::wire::SharedItem;

/// What the player says about itself this frame, read by the application from the shared player
/// surfaces (the same ones the HUD reads), whichever backend plays.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerFacts {
    /// The derived playback state (`plx_media::player::state`).
    pub state: PlaybackState,
    /// The published content position, `None` while the backend has reported none.
    pub position_ns: Option<i64>,
    /// The transport the viewer sees (`viewer_paused_for`).
    pub viewer_paused: bool,
    /// A seek waits for the pump or the backend (`seek_pending`).
    pub seek_pending: bool,
    /// This playback has presented a picture (`seen_frame`); `None` when the backend cannot say
    /// (the URI backend reports no presented frame of its own: its `Playing` status is the proof).
    pub seen_frame: Option<bool>,
    /// The person's play, pause and seek gestures on this playback so far.
    pub gestures: u32,
}

/// The session's observation of the player, or `None` when there is no playback to watch (no
/// engine, or one that failed: the session is told the player left).
///
/// - `ready` once a picture is up and the clock is known: `Playing` (a viewer pause keeps it
///   `Playing`), `Seeking`, and a `Buffering` that follows a presented picture (a seek's prime or a
///   rebuffer). Before that (`Resolving`, `Connecting`, a first `Buffering`, `Paused` with no
///   picture yet) the player is attached but not ready, and nothing it does is a gesture.
/// - `seeking` while a seek waits or resolves, and through a rebuffer: the position is not a
///   landing yet, so the rule neither measures it nor reads it as a jump.
pub fn observation(f: &PlayerFacts) -> Option<PlayerObs> {
    let pictured = f.seen_frame.unwrap_or(matches!(f.state, PlaybackState::Playing | PlaybackState::Seeking));
    let (ready, seeking) = match f.state {
        PlaybackState::Idle | PlaybackState::Error => return None,
        PlaybackState::Resolving | PlaybackState::Connecting | PlaybackState::Paused => (false, false),
        PlaybackState::Buffering => (pictured, true),
        PlaybackState::Seeking => (true, true),
        PlaybackState::Playing => (true, f.seek_pending),
    };
    let ready = ready && f.position_ns.is_some();
    let time = f.position_ns.unwrap_or(0).max(0) as f64 / 1e9;
    Some(PlayerObs { time, paused: f.viewer_paused, ready, seeking, gestures: Some(f.gestures) })
}

/// The same title (the web's `isSessionItem`): the same portable guid, or the same key on the same
/// server. Anything less is a different title, which is the closed side of every gate below.
pub fn same_title(a: &SharedItem, b: &SharedItem) -> bool {
    let guid = !a.guid.is_empty() && a.guid == b.guid;
    let key = a.server.is_some() && a.server == b.server && a.key.as_ref().is_some_and(|k| !k.is_empty()) && a.key == b.key;
    guid || key
}

/// May the session drive a player that plays `playing`? A host's playback is the session's (the
/// session follows the host's title); a participant's only while it plays the session's own title.
pub fn attachable(host: bool, session_item: Option<&SharedItem>, playing: &SharedItem) -> bool {
    host || session_item.is_some_and(|item| same_title(item, playing))
}

/// What a Follow does, given what the player plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FollowVerdict {
    /// Nothing plays: open the session's title at its position.
    Start,
    /// The player already plays it: nothing to open (the sync rule brings it to the session's place).
    Already,
    /// The player plays the session's previous title (it was attached): switch to the new one.
    Switch,
    /// The player plays the person's own title: ask before leaving it ([`FollowAsk`]).
    Ask,
}

/// The one gate a Follow goes through. `playing`: the title the player plays, if any; `attached`:
/// that playback is the session's (the session was attached to it).
pub fn follow_verdict(want: &SharedItem, playing: Option<&SharedItem>, attached: bool) -> FollowVerdict {
    match playing {
        None => FollowVerdict::Start,
        Some(p) if same_title(want, p) => FollowVerdict::Already,
        Some(_) if attached => FollowVerdict::Switch,
        Some(_) => FollowVerdict::Ask,
    }
}

/// A Follow the gate holds: the session plays `item` while the player plays the person's own
/// `playing`. The UI asks "leave the current playback to join?" and answers with
/// [`FollowAnswer`]; until then nothing is opened and the session does not drive the player.
#[derive(Clone, Debug, PartialEq)]
pub struct FollowAsk {
    pub item: SharedItem,
    /// The session's position (s) when the Follow came.
    pub position: Option<f64>,
    pub playing: SharedItem,
}

/// The person's answer to a [`FollowAsk`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FollowAnswer {
    /// Leave the current playback and join the session's title.
    Join,
    /// Keep the current playback: leave the session (one playback while in a session).
    Leave,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(guid: &str, server: &str, key: &str) -> SharedItem {
        SharedItem {
            guid: guid.into(),
            server: (!server.is_empty()).then(|| server.into()),
            key: (!key.is_empty()).then(|| key.into()),
            kind: "movie".into(),
            title: "Le Film".into(),
            sub: None,
        }
    }

    fn facts(state: PlaybackState) -> PlayerFacts {
        PlayerFacts { state, position_ns: Some(42_500_000_000), viewer_paused: false, seek_pending: false, seen_frame: Some(true), gestures: 3 }
    }

    #[test]
    fn a_playing_player_is_ready_and_carries_its_gesture_count() {
        let o = observation(&facts(PlaybackState::Playing)).expect("attached");
        assert_eq!(o, PlayerObs { time: 42.5, paused: false, ready: true, seeking: false, gestures: Some(3) });
        let paused = observation(&PlayerFacts { viewer_paused: true, ..facts(PlaybackState::Playing) }).unwrap();
        assert!(paused.paused && paused.ready, "a viewer pause stays ready");
        let asked = observation(&PlayerFacts { seek_pending: true, ..facts(PlaybackState::Playing) }).unwrap();
        assert!(asked.seeking, "a seek waiting for the pump is not a landing");
    }

    #[test]
    fn no_engine_or_a_failed_one_is_no_player_at_all() {
        assert_eq!(observation(&facts(PlaybackState::Idle)), None);
        assert_eq!(observation(&facts(PlaybackState::Error)), None);
    }

    #[test]
    fn before_the_first_picture_the_player_is_attached_but_not_ready() {
        for state in [PlaybackState::Resolving, PlaybackState::Connecting, PlaybackState::Paused] {
            let o = observation(&facts(state)).expect("attached");
            assert!(!o.ready && !o.seeking, "{state:?}");
        }
        let first = observation(&PlayerFacts { seen_frame: Some(false), ..facts(PlaybackState::Buffering) }).unwrap();
        assert!(!first.ready, "a first Buffering has no picture");
        let rebuffer = observation(&facts(PlaybackState::Buffering)).unwrap();
        assert!(rebuffer.ready && rebuffer.seeking, "a rebuffer after a picture is a position in flight");
        let unknown = observation(&PlayerFacts { position_ns: None, ..facts(PlaybackState::Playing) }).unwrap();
        assert!(!unknown.ready, "no clock, no observation to compare");
    }

    /// The URI backend reports no presented frame: its `Playing` status is the picture.
    #[test]
    fn a_backend_that_cannot_report_frames_is_ready_on_its_playing_status() {
        let o = observation(&PlayerFacts { seen_frame: None, ..facts(PlaybackState::Playing) }).unwrap();
        assert!(o.ready);
        let b = observation(&PlayerFacts { seen_frame: None, ..facts(PlaybackState::Buffering) }).unwrap();
        assert!(!b.ready, "with no frame report a Buffering cannot be told from a first one");
    }

    #[test]
    fn titles_match_by_guid_or_by_server_and_key_never_by_less() {
        assert!(same_title(&item("plex://movie/1", "", ""), &item("plex://movie/1", "srv-2", "9")));
        assert!(same_title(&item("", "srv-1", "42"), &item("plex://movie/1", "srv-1", "42")));
        assert!(!same_title(&item("", "srv-1", "42"), &item("", "srv-2", "42")), "a key names an item on one server");
        assert!(!same_title(&item("", "", ""), &item("", "", "")), "two unknowns are not the same title");
        assert!(!same_title(&item("", "srv-1", ""), &item("", "srv-1", "")));
    }

    #[test]
    fn a_participant_is_driven_only_on_the_sessions_title() {
        let film = item("plex://movie/1", "srv-1", "42");
        let other = item("plex://movie/2", "srv-1", "43");
        assert!(attachable(true, None, &other), "the host's playback is the session's");
        assert!(attachable(false, Some(&film), &film));
        assert!(!attachable(false, Some(&film), &other));
        assert!(!attachable(false, None, &film), "no title yet: nothing is driven");
    }

    #[test]
    fn the_gate_starts_switches_or_asks_and_never_replaces_the_persons_own_playback() {
        let film = item("plex://movie/1", "srv-1", "42");
        let next = item("plex://movie/3", "srv-1", "44");
        assert_eq!(follow_verdict(&film, None, false), FollowVerdict::Start);
        assert_eq!(follow_verdict(&film, Some(&film), true), FollowVerdict::Already);
        assert_eq!(follow_verdict(&film, Some(&film), false), FollowVerdict::Already);
        assert_eq!(follow_verdict(&next, Some(&film), true), FollowVerdict::Switch);
        assert_eq!(follow_verdict(&next, Some(&film), false), FollowVerdict::Ask);
    }
}
