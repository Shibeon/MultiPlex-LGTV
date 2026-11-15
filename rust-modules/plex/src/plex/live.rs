//! **What is playing on a server right now**: `GET /status/sessions` (the web app's `LiveNow`). PMS
//! answers it to the server's OWNER only; a shared or managed account gets 401/403, which the
//! Friends page reads as "no section", never as an error.
//!
//! **The privacy rule (the owner's).** The Friends page shows who is watching what, how far and
//! whether it is paused. It does NOT show the device, the player, the platform, the address, the
//! bandwidth or the stream decision: friends' activity must not become tracking. So this module
//! decodes none of them ([`super::SessionPlayer`] carries `state` alone) and a [`LiveSession`] has
//! nowhere to put them.
use super::client::JsonStatusOutcome;
use super::Client;

/// Whether a session plays, is paused or is filling its buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveState {
    Playing,
    Paused,
    Buffering,
}

/// One playback in progress, as the Friends page's live section shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveSession {
    /// The playing account's name (`User.title`), empty when the server sent none.
    pub user: String,
    /// Its picture (`User.thumb`, usually an absolute plex.tv URL), empty when none.
    pub user_thumb: String,
    /// The item's ratingKey on THIS server (OK opens its Detail).
    pub rating_key: String,
    /// `movie`, `episode`, ... as the server spells it.
    pub kind: String,
    pub title: String,
    /// An episode's show.
    pub show: String,
    /// An episode's season and number (0 when not sent).
    pub season: i64,
    pub episode: i64,
    /// The art the tile shows: an episode's show poster, else the item's own (a server path).
    pub thumb: String,
    pub view_offset_ms: i64,
    pub duration_ms: i64,
    pub state: LiveState,
}

/// Why the sessions could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveError {
    /// 401/403: this account does not own the server. The section is not shown.
    Forbidden,
    /// Any other answer, or none.
    Failed,
}

/// The sessions of a decoded container's `Metadata` rows. A row with no ratingKey is dropped.
pub fn sessions_of(rows: &[super::Metadata]) -> Vec<LiveSession> {
    rows.iter()
        .filter(|m| !m.rating_key.is_empty())
        .map(|m| {
            let user = m.session_user.clone().unwrap_or_default();
            let state = match m.session_player.as_ref().map(|p| p.state.to_ascii_lowercase()) {
                Some(s) if s == "paused" => LiveState::Paused,
                Some(s) if s == "buffering" => LiveState::Buffering,
                _ => LiveState::Playing,
            };
            let episode = m.kind == "episode";
            let thumb = if episode && !m.grandparent_thumb.is_empty() { m.grandparent_thumb.clone() } else { m.thumb.clone() };
            LiveSession {
                user: user.title,
                user_thumb: user.thumb,
                rating_key: m.rating_key.clone(),
                kind: m.kind.clone(),
                title: m.title.clone(),
                show: if episode { m.grandparent_title.clone() } else { String::new() },
                season: if episode { m.parent_index.unwrap_or(0) } else { 0 },
                episode: if episode { m.index } else { 0 },
                thumb,
                view_offset_ms: m.view_offset.max(0),
                duration_ms: m.duration.max(0),
                state,
            }
        })
        .collect()
}

impl Client {
    /// BLOCKING: what is playing on this server now. `Err(Forbidden)` for an account that does not
    /// own it. Logs nothing beyond what the door logs (a parse failure's shape).
    pub fn live_sessions(&self) -> Result<Vec<LiveSession>, LiveError> {
        match self.get_json_status("/status/sessions") {
            JsonStatusOutcome::Response { status: 401 | 403, .. } => Err(LiveError::Forbidden),
            JsonStatusOutcome::Response { status, parsed: Some(c) } if (200..300).contains(&status) => Ok(sessions_of(&c.metadata)),
            JsonStatusOutcome::Response { .. } | JsonStatusOutcome::Transport => Err(LiveError::Failed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(json: &str) -> Vec<super::super::Metadata> {
        #[derive(serde::Deserialize)]
        struct C {
            #[serde(rename = "Metadata", default)]
            metadata: Vec<super::super::Metadata>,
        }
        serde_json::from_str::<C>(json).unwrap().metadata
    }

    /// Who, what, how far and the state, and NOTHING of the device: the player's address, device,
    /// platform and the session's bandwidth are on the wire and have nowhere to land.
    #[test]
    fn a_session_keeps_who_what_progress_and_state_and_no_device_detail() {
        let got = sessions_of(&rows(r#"{"Metadata":[
            {"ratingKey":"12","type":"episode","title":"Pilote","grandparentTitle":"Archipel","parentIndex":2,"index":1,
             "grandparentThumb":"/library/metadata/10/thumb/1","thumb":"/library/metadata/12/thumb/1",
             "viewOffset":600000,"duration":2400000,
             "User":{"id":"1","title":"Camille","thumb":"https://plex.tv/users/1/avatar"},
             "Player":{"state":"paused","address":"192.0.2.7","device":"iPhone","platform":"iOS","machineIdentifier":"m"},
             "Session":{"id":"s","bandwidth":12000,"location":"wan"}},
            {"ratingKey":"7","type":"movie","title":"Film","thumb":"/t","Player":{"state":"buffering"}},
            {"type":"movie","title":"no key"}
        ]}"#));
        assert_eq!(got.len(), 2, "a row with no ratingKey is dropped");
        let e = &got[0];
        assert_eq!((e.user.as_str(), e.title.as_str(), e.show.as_str(), e.season, e.episode), ("Camille", "Pilote", "Archipel", 2, 1));
        assert_eq!(e.thumb, "/library/metadata/10/thumb/1", "an episode shows its show's poster");
        assert_eq!((e.view_offset_ms, e.duration_ms, e.state), (600_000, 2_400_000, LiveState::Paused));
        assert_eq!(got[1].state, LiveState::Buffering);
        assert_eq!((got[1].user.as_str(), got[1].show.as_str()), ("", ""));
        let debug = format!("{got:?}");
        for leak in ["192.0.2.7", "iPhone", "iOS", "wan", "12000"] {
            assert!(!debug.contains(leak), "{leak} must not be decoded: {debug}");
        }
    }
}
