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
//!   the pump is skipped (`TogetherStore::due`), and only the seated profile's snapshot is read for
//!   its scope counter (`CurrentProfile::generation`), which moves when a profile is seated,
//!   detached or signed out. Holding the seated identity's friends' sessions or friend list does
//!   not keep the pump running: the frame the scope moves pumps once, and that is the frame the
//!   store forgets the previous identity.
//! - **A server's relay is found on what plex.tv publishes for it** (`SourceRef::published`, written
//!   by discovery), not only on the address that answered, which at home is a LAN or `plex.direct`
//!   name no relay can sit behind.
//!
//! The UI (commands, notices, the friends' sessions) is wired by the lane that builds it; this is its
//! seam. The player's side (its snapshot, the requests the session makes of it, the one gate every
//! Follow goes through) is [`player`], run once a frame beside the launch wait.
use plx_data::stores::together::{
    PlayerSnapshot, TogetherAuth, TogetherCmd, TogetherInput, TogetherPlayerRequest, TogetherProfile, TogetherServer, TogetherSettings, TogetherStore,
};
use plx_data::together::player::{FollowAnswer, FollowState};
use plx_data::together::relay::{PlexConnection, ServerInfo};
use plx_data::together::session::Notice;
use plx_data::together::wire::SharedItem;

use super::*;

#[path = "together_player.rs"]
pub(crate) mod player;
#[path = "together_ui.rs"]
pub(crate) mod ui;

/// The together store, its runner, and what the UI and the player have not taken yet.
#[derive(Default)]
pub(crate) struct TogetherFeed {
    store: TogetherStore,
    runner: crate::app::adapters::together::TogetherRunner,
    settings: TogetherSettings,
    player: Option<PlayerSnapshot>,
    notices: Vec<Notice>,
    requests: Vec<TogetherPlayerRequest>,
    /// The gate's standing with the last Follow (`player`), for the UI.
    follow: FollowState,
    /// The UI's answer to the gate's question, performed on the next frame.
    answer: Option<FollowAnswer>,
    /// The playing title as the session names it, identified once per playback.
    item: Option<((plx_plex::plex::ServerId, String), SharedItem)>,
    /// The session drives the player: the last unheld frame handed it a snapshot (`player`).
    drives: bool,
    /// The player side's last frame saw a live session (`player`): a Follow's wait opens nothing
    /// without one.
    live: bool,
    /// What the session tells the viewer, on the application's one transient notice ([`ui`]).
    pub(crate) ui: ui::TogetherUi,
    /// The viewer's "Friends' session alerts" setting (default on).
    friend_alerts: bool,
    /// The clock of the last UI frame (`ui_frame`).
    ui_clock: Option<u32>,
    /// The live session's view as this frame's publication (`ui_frame`), which the screens read
    /// (`AppViews::together`).
    published: Option<plx_data::together::session::View>,
    /// What the player page was last told of the gate's question, and which page (`player_ask`).
    ask_told: Option<(plx_machine::machine::InstanceId, Option<String>)>,
}

/// The servers this identity sees, as the together store needs them: identity, the connections
/// plex.tv publishes for each (a server's public names are where its relay, and a friend's, may be
/// found: the web's `relayCandidates` reads the same list), ownership, the address that answered
/// (the owner's delegation request goes there), and this identity's own access token for each.
///
/// The published list is what discovery wrote down (`SourceRef::published`); the address that
/// answered joins it when the list does not name it (a dev boot, a session file written before the
/// list was kept). A `local` or Plex-relay connection, an IP and a `plex.direct` name are never a
/// relay candidate (`relay::relay_candidates`), so a server reached on its LAN is found by its
/// published public name, or not at all.
pub(crate) fn together_servers(session: &plx_plex::plex::session::Session) -> Vec<TogetherServer> {
    use plx_plex::plex::probe::Location;
    session
        .sources
        .iter()
        .filter(|s| !s.machine_id.is_empty())
        .map(|s| {
            let base = s.origin().map(|o| o.base());
            let mut connections: Vec<PlexConnection> =
                s.published.iter().map(|c| PlexConnection { uri: c.uri.clone(), local: c.local, relay: c.relay }).collect();
            if let Some(uri) = base.as_ref().filter(|uri| !connections.iter().any(|c| c.uri.trim_end_matches('/') == uri.trim_end_matches('/'))) {
                connections.push(PlexConnection { uri: uri.clone(), local: s.tier == Some(Location::Local), relay: s.tier == Some(Location::Relay) });
            }
            let token = (!s.token.is_empty()).then(|| s.token.clone());
            TogetherServer::new(ServerInfo { id: s.machine_id.clone(), connections }, s.owned, base, token)
        })
        .collect()
}

impl Bridge {
    /// Once a frame: pump the together store when it has anything to do, and start its jobs.
    pub(crate) fn together_feed(&mut self) {
        let together = &mut self.together;
        let profile = Self::seated_profile();
        if !together.store.due(profile.generation) {
            return;
        }
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
            scope: profile.generation,
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

/// The seam the UI and the player side ([`player`]) wire: commands in, notices and player requests out.
impl TogetherFeed {
    /// The UI's command to the together store, applied on the next frame's pump.
    pub(crate) fn command(&mut self, cmd: TogetherCmd) {
        self.store.command(cmd);
        plx_machine::idle::invalidate();
    }

    /// The settings the store reads (the relay, a hosted session's default visibility, the notices).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn set_settings(&mut self, settings: TogetherSettings) {
        self.settings = settings;
    }

    /// What the player plays, for the session to follow (`player` hands it each frame).
    pub(crate) fn set_player(&mut self, snapshot: Option<PlayerSnapshot>) {
        self.player = snapshot;
    }

    /// The notices the UI has not taken yet.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn take_notices(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.notices)
    }

    /// What the session asked of the player and the player has not taken yet.
    pub(crate) fn take_player_requests(&mut self) -> Vec<TogetherPlayerRequest> {
        std::mem::take(&mut self.requests)
    }

    /// The live session's view, for the UI and the player's side.
    pub(crate) fn view(&self) -> Option<plx_data::together::session::View> {
        self.store.view()
    }

    /// The viewer's two Watch Together settings: a hosted session's default visibility to their
    /// friends and the friends' session alerts.
    pub(crate) fn set_preferences(&mut self, visible_default: bool, friend_alerts: bool) {
        self.settings.visible_default = visible_default;
        self.friend_alerts = friend_alerts;
    }

    /// Once a frame, after the player side: the notices and the gate's standing onto the
    /// notice, the friends' listing asked on its cadence, and the notice stepped. `friends_page`:
    /// the Friends page is on top; `player_page`: a player page is mounted (it asks the gate's
    /// question itself).
    pub(crate) fn ui_frame(&mut self, now_ms: u32, friends_page: bool, player_page: bool) {
        let elapsed = self.ui_clock.map_or(0, |t| now_ms.wrapping_sub(t));
        self.ui_clock = Some(now_ms);
        let view = self.store.view();
        if view != self.published {
            self.published = view;
            plx_machine::idle::invalidate();
        }
        let ctx = ui::UiCtx { in_session: self.published.is_some(), friend_alerts: self.friend_alerts, friends_page, player_page };
        for n in std::mem::take(&mut self.notices) {
            self.ui.on_notice(&n, &ctx);
        }
        let follow = self.follow.clone();
        self.ui.on_follow(&follow, &ctx);
        if self.ui.poll_due(elapsed, &ctx) {
            self.command(TogetherCmd::RefreshFriends);
        }
        if self.ui.notice.busy() {
            self.ui.notice.step(elapsed);
            plx_machine::idle::invalidate();
        }
    }

    /// The gate's question as the player page `page` should be asking it (`Some(title)`) or not,
    /// when that differs from what this page was last told: the page's `AppMsg::TogetherAsk`.
    pub(crate) fn player_ask(&mut self, page: Option<plx_machine::machine::InstanceId>) -> Option<(plx_machine::machine::InstanceId, Option<String>)> {
        let page = page?;
        let want = match &self.follow {
            FollowState::Asking(ask) => Some(ui::title_of(&ask.item).to_owned()),
            _ => None,
        };
        if self.ask_told.as_ref().is_some_and(|(p, w)| *p == page && *w == want) {
            return None;
        }
        self.ask_told = Some((page, want.clone()));
        Some((page, want))
    }

    /// The live session's view as this frame's publication.
    pub(crate) fn published(&self) -> Option<&plx_data::together::session::View> {
        self.published.as_ref()
    }

    /// Host a session on what plays (the More menu's *Create a session*): the title as the
    /// player side identified it, else the store takes the player's snapshot.
    pub(crate) fn host_playing(&mut self) {
        let item = self.item.as_ref().map(|(_, item)| item.clone());
        self.command(TogetherCmd::Host { item });
    }

    /// What an answered notice asked.
    pub(crate) fn perform(&mut self, action: ui::NoticeAction) {
        match action {
            ui::NoticeAction::JoinFriend { relay, room } => self.command(TogetherCmd::JoinFriend { relay, room }),
            ui::NoticeAction::Follow(answer) => self.answer_follow(answer),
        }
    }

    /// The UP overshoot this frame's focus model reported: it reaches a notice that offers an action.
    pub(crate) fn overshoot(&mut self, dir: Option<plx_ui::screen::Dir>) {
        if dir == Some(plx_ui::screen::Dir::Up) && self.ui.notice.engage() {
            plx_machine::idle::invalidate();
        }
    }

    /// A key, offered to the notice before anything else: `true` when the notice took it (its
    /// key-up is then the notice's too).
    pub(crate) fn notice_key(&mut self, sym: u32, down: bool, key: plx_ui::consts::Key) -> bool {
        use plx_ui::action_notice::{Answer, NoticeKey};
        if !down {
            if self.ui.swallow_sym == Some(sym) {
                self.ui.swallow_sym = None;
                return true;
            }
            return false;
        }
        if !self.ui.notice.engaged() {
            return false;
        }
        let k = match key {
            plx_ui::consts::Key::Ok => NoticeKey::Ok,
            plx_ui::consts::Key::Back => NoticeKey::Back,
            plx_ui::consts::Key::Up => NoticeKey::Up,
            plx_ui::consts::Key::Down => NoticeKey::Down,
            plx_ui::consts::Key::Left { .. } => NoticeKey::Left,
            plx_ui::consts::Key::Right { .. } => NoticeKey::Right,
            _ => NoticeKey::Other,
        };
        let (answer, action) = self.ui.key(k);
        if answer == Answer::Ignored {
            return false;
        }
        self.ui.swallow_sym = Some(sym);
        if let Some(a) = action {
            self.perform(a);
        }
        plx_machine::idle::invalidate();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seam_queues_a_command_and_hands_out_what_it_holds() {
        let mut feed = TogetherFeed::default();
        assert!(!feed.store.due(0));
        feed.command(TogetherCmd::Leave);
        assert!(feed.store.due(0), "a queued command makes the next frame pump");
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

    /// The usual home: the server answers on its LAN address through a `plex.direct` name, which
    /// no relay can sit behind, and publishes its public name on plex.tv. The relay is found on
    /// that published name (as the web's `relayCandidates` reads plex.tv's connections), never on
    /// a local, `plex.direct` or Plex-relay connection.
    #[test]
    fn a_server_reached_on_its_lan_name_is_found_by_its_published_public_name() {
        let source: plx_plex::plex::session::SourceRef = serde_json::from_value(serde_json::json!({
            "machine_id": "srv-1", "name": "nas", "owned": true, "address": "192.168.1.10", "port": 32400, "token": "server-token",
            "origin": "https://192-168-1-10.0123456789abcdef.plex.direct:32400",
            "published": [
                {"uri": "https://192-168-1-10.0123456789abcdef.plex.direct:32400", "local": true, "relay": false},
                {"uri": "https://203-0-113-9.0123456789abcdef.plex.direct:32400", "local": false, "relay": false},
                {"uri": "https://plex.exemple.fr:443", "local": false, "relay": false},
                {"uri": "https://146-0-0-1.0123456789abcdef.plex.direct:8443", "local": false, "relay": true}
            ]
        }))
        .unwrap();
        let session = plx_plex::plex::session::Session { sources: vec![source], ..Default::default() };
        let servers = together_servers(&session);
        assert_eq!(servers.len(), 1);
        let infos: Vec<ServerInfo> = servers.iter().map(|s| s.info.clone()).collect();
        let relays = plx_data::together::relay::relay_candidates(&infos);
        assert_eq!(relays, ["wss://plex.exemple.fr/multiplex-relay", "wss://plex.exemple.fr"], "only the published public name");
        assert!(plx_data::together::relay::relay_discoverable("wss://plex.exemple.fr/multiplex-relay", &infos));
    }
}
