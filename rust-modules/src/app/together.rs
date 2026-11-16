//! **Watch Together's feed** (`plx_data::stores::together`): once a frame, the together store's pump
//! with what the Bridge reads for it, as values (the seated profile and its plex.tv identity, whether
//! it is a Guest boot or a managed profile, the servers this identity sees with their own tokens,
//! the settings, the player snapshot, the write gate and the logical clock), and the jobs it emitted
//! started on the together runner (`adapters::together`).
//!
//! - **A Guest boot announces nothing**: a dev-token boot seats no profile (`Seat::Unseated`) and
//!   carries no plex.tv credential, so the store is told it may not announce. Code sessions work.
//! - **A managed profile announces nothing** and lists no friends' sessions (it has no Friends page,
//!   as on the web); it may still host or join by code.
//! - **The debug-build write gate** (`plex::together::together_write_gate`): announcing, and the
//!   owner's delegation token, need a loopback server, an armed stand-in or the owner's live switch.
//! - **Nothing runs while nothing is asked**: with no session, no request in flight and no command,
//!   the pump is skipped and nothing is read.
//!
//! The UI (commands, notices, the friends' sessions) and the player (its snapshot, the requests the
//! session makes of it) are wired by the lanes that build them; this is their seam.
use plx_data::stores::together::{
    PlayerSnapshot, TogetherAuth, TogetherCmd, TogetherInput, TogetherPlayerRequest, TogetherProfile, TogetherServer, TogetherSettings, TogetherStore,
};
use plx_data::together::relay::{PlexConnection, ServerInfo};
use plx_data::together::session::Notice;

use super::*;

/// The together store, its runner, and what the UI and the player have not taken yet.
#[derive(Default)]
pub(crate) struct TogetherFeed {
    store: TogetherStore,
    runner: crate::app::adapters::together::TogetherRunner,
    settings: TogetherSettings,
    player: Option<PlayerSnapshot>,
    notices: Vec<Notice>,
    requests: Vec<TogetherPlayerRequest>,
}

/// The servers this identity sees, as the together store needs them: identity, the address that
/// answered (its public name is where a friend's relay may be found), ownership, and this identity's
/// own access token for each.
fn together_servers(session: &plx_plex::plex::session::Session) -> Vec<TogetherServer> {
    use plx_plex::plex::probe::Location;
    session
        .sources
        .iter()
        .filter(|s| !s.machine_id.is_empty())
        .map(|s| {
            let base = s.origin().map(|o| o.base());
            let connections = base
                .iter()
                .map(|uri| PlexConnection { uri: uri.clone(), local: s.tier == Some(Location::Local), relay: s.tier == Some(Location::Relay) })
                .collect();
            let token = (!s.token.is_empty()).then(|| s.token.clone());
            TogetherServer::new(ServerInfo { id: s.machine_id.clone(), connections }, s.owned, base, token)
        })
        .collect()
}

impl Bridge {
    /// Once a frame: pump the together store when it has anything to do, and start its jobs.
    pub(crate) fn together_feed(&mut self) {
        let together = &mut self.together;
        if !together.store.busy() && together.store.view().is_none() {
            return;
        }
        let profile = Self::seated_profile();
        let session = plx_plex::plex::session::peek();
        let user = profile.user.as_ref();
        let seated = matches!(profile.seat(), plx_plex::plex::session::Seat::Seated { .. });
        let credential = user.and_then(plx_plex::plex::session::plex_tv_credential).filter(|_| seated);
        let account_id = user.map(|u| u.id.to_string()).filter(|id| id != "0");
        let auth = credential.map(|token| TogetherAuth::new(&session.client_id, account_id.as_deref(), &token));
        let me = user.map(|u| TogetherProfile { name: u.title.clone(), thumb: (!u.thumb.is_empty()).then(|| u.thumb.clone()) });
        let servers = together_servers(&session);
        let server = (!session.server.machine_id.is_empty()).then_some(session.server.machine_id.as_str());
        let base = session.server.origin().base();
        let writes_permitted = plx_plex::plex::together::together_write_gate(plx_plex::plex::together::is_loopback_base(&base));
        let input = TogetherInput {
            profile: me.as_ref(),
            auth: auth.as_ref(),
            guest: !seated,
            managed: self.chrome.managed(),
            writes_permitted,
            settings: &together.settings,
            server,
            servers: &servers,
            player: together.player.as_ref(),
            now_ms: crate::app::clock::now(),
        };
        {
            let _step = plx_base::step::StepScope::enter();
            together.store.pump(&input);
        }
        together.runner.start(together.store.take_jobs());
        let notices = together.store.take_notices();
        let requests = together.store.take_player_requests();
        if !notices.is_empty() || !requests.is_empty() {
            together.notices.extend(notices);
            together.requests.extend(requests);
            plx_machine::idle::invalidate();
        }
    }
}

/// The seam the UI and the player lanes wire: commands in, notices and player requests out.
impl TogetherFeed {
    /// The UI's command to the together store, applied on the next frame's pump.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn command(&mut self, cmd: TogetherCmd) {
        self.store.command(cmd);
        plx_machine::idle::invalidate();
    }

    /// The settings the store reads (the relay, a hosted session's default visibility, the notices).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn set_settings(&mut self, settings: TogetherSettings) {
        self.settings = settings;
    }

    /// What the player plays, for the session to follow (lane 4 hands it each frame).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn set_player(&mut self, snapshot: Option<PlayerSnapshot>) {
        self.player = snapshot;
    }

    /// The notices the UI has not taken yet.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn take_notices(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.notices)
    }

    /// What the session asked of the player and the player has not taken yet.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn take_player_requests(&mut self) -> Vec<TogetherPlayerRequest> {
        std::mem::take(&mut self.requests)
    }

    /// The live session's view, for the UI.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn view(&self) -> Option<plx_data::together::session::View> {
        self.store.view()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seam_queues_a_command_and_hands_out_what_it_holds() {
        let mut feed = TogetherFeed::default();
        assert!(!feed.store.busy());
        feed.command(TogetherCmd::Leave);
        assert!(feed.store.busy(), "a queued command makes the next frame pump");
        feed.set_settings(TogetherSettings { relay: "relais.exemple.fr".into(), ..TogetherSettings::default() });
        assert_eq!(feed.settings.relay, "relais.exemple.fr");
        feed.set_player(None);
        feed.notices.push(Notice::Left);
        feed.requests.push(TogetherPlayerRequest::Pause);
        assert_eq!(feed.take_notices(), [Notice::Left]);
        assert_eq!(feed.take_player_requests(), [TogetherPlayerRequest::Pause]);
        assert!(feed.take_notices().is_empty());
        assert!(feed.view().is_none());
    }
}
