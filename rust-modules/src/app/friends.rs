//! **The Friends page's feed** (`screens::friends`, the web app's *Amis*): once a frame, the
//! community store's pump with what the Bridge reads for it (the seated profile and its plex.tv
//! identity, whether that profile is managed, whether the page is on screen, the captured server
//! keys), the jobs it emitted started on their runner, and the page handed what moved.
//!
//! **Nothing is read for a managed Plex Home profile** (the owner's decision, the web app's too):
//! the store empties itself and emits nothing (`CommunityStore::pump`), and the bar has no Friends
//! pill to reach the page by (`chrome::strip_keys`, `nav_tab`).
//!
//! **The privacy write leaves only from [`Bridge::community_run`]**, which only the page's
//! `CommunityCmd::SetShare` reaches (its toggle's explicit press, on the ordered FIFO as a store
//! command); the runner's account client then goes through the dev-build live-write gate
//! (`plex::watchlist::account_write_gate`).
use plx_data::stores::community::{art_server, CommunityCmd, CommunityInput};
use plx_screens::friends::page::{FriendsIn, FriendsScreen, FriendsView, TogetherRoom};

use super::*;

/// What the feed last handed the page, so a frame with nothing new hands it nothing.
#[derive(Default)]
pub(crate) struct Feed {
    /// The mounted page the rest was sent to; a new instance is sent everything again.
    instance: Option<InstanceId>,
    /// The store generation the view was captured at.
    generation: Option<u32>,
    /// The art server as the page was last told (outer `None`: not told yet).
    art_server: Option<Option<plx_plex::plex::ServerId>>,
    /// The key generations the art server was chosen at.
    keys_gen: Option<(u32, u32)>,
    /// The minute (unix seconds / 60) the page's clock was last set at.
    minute: Option<Option<i64>>,
    /// The friends' Watch Together sessions as the page was last told them.
    together: Option<Vec<TogetherRoom>>,
}

/// **The friends' Watch Together sessions as the Friends page shows them** (none while this TV is
/// in a session of its own): each with its poster on the relay's own Plex server when the account
/// reaches it (found by its machine identifier among the captured keys), else none.
fn together_rooms(rooms: &[crate::app::bridge::together::ui::FriendRoom], in_session: bool, keys: &plx_plex::plex::ClientKeys) -> Vec<TogetherRoom> {
    if in_session {
        return Vec::new();
    }
    rooms.iter().map(|r| {
        let server = keys.slots().find(|sid| keys.facts(*sid).is_some_and(|f| !r.server.is_empty() && f.machine_id == r.server));
        let (src, thumb) = match (server, r.key.as_deref().filter(|k| !k.is_empty() && k.bytes().all(|b| b.is_ascii_digit()))) {
            (Some(sid), Some(key)) => (sid.raw(), format!("/library/metadata/{key}/thumb")),
            _ => (0, String::new()),
        };
        TogetherRoom {
            relay: r.relay.clone(),
            room: r.room.clone(),
            name: r.name.clone(),
            title: r.title.clone(),
            sub: r.sub.clone(),
            count: r.count,
            playing: r.playing,
            src,
            thumb,
        }
    }).collect()
}

impl Bridge {
    /// The page mounted on the stack, if any.
    fn friends_page<'a>(d: &'a mut Dispatcher<AppHost>) -> Option<(InstanceId, &'a mut FriendsScreen)> {
        let id = d.nav.tabs.stack.entries.iter().find(|e| e.arg == AppArg::Friends)?.id;
        let inst = d.nav.entry_mut(id)?.inst.as_mut()?;
        let instance = inst.id;
        let screen = inst.screen.as_any_mut()?.downcast_mut::<FriendsScreen>()?;
        Some((instance, screen))
    }

    /// Once a frame, route-unconditional: pump the community store (it asks for something only
    /// while the page is on top), start what it emitted, and hand the page what changed.
    pub(crate) fn friends_feed(&mut self, d: &mut Dispatcher<AppHost>) {
        let now_ms = crate::app::clock::now();
        crate::app::adapters::stores::refill_client_keys(&mut self.client_keys);
        let visible = d.top_arg() == Some(&AppArg::Friends);
        let watch = Self::watchlist_input();
        let input = CommunityInput { watch: &watch, managed: self.profile_managed(), visible, keys: &self.client_keys };
        let changed = {
            let _step = plx_base::step::StepScope::enter();
            self.stores.community.pump(&input, now_ms)
        };
        self.start_store_jobs_of(StoreId::Community);
        if changed {
            plx_machine::idle::invalidate();
        }
        let keys_gen = (self.client_keys.roster_gen(), self.client_keys.facts_gen());
        let Some((instance, screen)) = Self::friends_page(d) else {
            self.friends = Feed::default();
            return;
        };
        let feed = &mut self.friends;
        if feed.instance != Some(instance) {
            *feed = Feed { instance: Some(instance), ..Feed::default() };
        }
        let mut moved = false;
        if feed.keys_gen != Some(keys_gen) {
            feed.keys_gen = Some(keys_gen);
            let server = art_server(&self.client_keys);
            if feed.art_server != Some(server) {
                feed.art_server = Some(server);
                screen.input(FriendsIn::ArtServer(server));
                moved = true;
            }
        }
        // The clock the relative times and the day headings are worded against: the wall clock
        // (`None` under a replay), re-read once a minute, and the viewer's UTC offset at it.
        let secs = crate::app::clock::unix_ms().map(|ms| ms.div_euclid(1000));
        let minute = secs.map(|s| s.div_euclid(60));
        if feed.minute != Some(minute) {
            feed.minute = Some(minute);
            let offset = secs
                .and_then(|s| Some(plx_data::community::utc_offset(s, plx_platform::i18n::local_calendar_date(s)?, plx_platform::i18n::local_clock_time(s)?)))
                .unwrap_or(0);
            screen.input(FriendsIn::Clock { now: secs, offset });
            moved = true;
        }
        let rooms = together_rooms(&self.together.ui.sessions, self.together.published().is_some(), &self.client_keys);
        if feed.together.as_ref() != Some(&rooms) {
            screen.input(FriendsIn::Together(rooms.clone()));
            feed.together = Some(rooms);
            moved = true;
        }
        let store = &self.stores.community;
        if feed.generation != Some(store.gen()) {
            feed.generation = Some(store.gen());
            let held = match store.feed() {
                plx_data::stores::community::FeedState::Ready(rows) => rows.iter()
                    .filter_map(|a| a.guid())
                    .filter_map(|guid| store.held(&guid).map(|m| (guid, m)))
                    .collect(),
                _ => Default::default(),
            };
            screen.input(FriendsIn::View(FriendsView {
                feed: store.feed().clone(),
                shares: store.shares(),
                writing: store.writing(),
                write_failed: store.write_failed(),
                live: std::sync::Arc::clone(store.live()),
                held,
            }));
            moved = true;
        }
        if moved {
            plx_machine::idle::invalidate();
        }
    }

    /// **The page's command to the community store**, stepped off the ordered FIFO
    /// (`StoreCmd::Community`).
    pub(crate) fn community_run(&mut self, command: CommunityCmd) {
        match command {
            CommunityCmd::SetShare(share) => self.friends_set_share(share),
            CommunityCmd::Retry => self.friends_retry(),
        }
    }

    /// **The toggle's explicit press** (`CommunityCmd::SetShare`): the one caller of the store's
    /// write. Nothing for a managed profile (the store refuses it too).
    fn friends_set_share(&mut self, share: bool) {
        if self.profile_managed() {
            return;
        }
        crate::app::adapters::stores::refill_client_keys(&mut self.client_keys);
        let watch = Self::watchlist_input();
        let input = CommunityInput { watch: &watch, managed: false, visible: true, keys: &self.client_keys };
        if self.stores.community.set_share(share, &input) {
            self.start_store_jobs_of(StoreId::Community);
            plx_machine::idle::invalidate();
        }
    }

    fn friends_retry(&mut self) {
        self.stores.community.retry();
        plx_machine::idle::invalidate();
    }

    #[cfg(test)]
    pub(crate) fn community_for_test(&mut self) -> &mut plx_data::stores::community::CommunityStore {
        &mut self.stores.community
    }
}

#[cfg(test)]
mod together_rooms_tests {
    use super::together_rooms;
    use crate::app::bridge::together::ui::FriendRoom;
    use plx_plex::plex::{ClientFacts, ClientKeys, ServerId};

    fn room(server: &str, key: Option<&str>) -> FriendRoom {
        FriendRoom {
            relay: "wss://relais.exemple.fr".into(), server: server.into(), key: key.map(str::to_owned), room: "r1".into(),
            name: "Camille".into(), title: "Le Film".into(), sub: None, count: 2, playing: true,
        }
    }

    fn keys() -> ClientKeys {
        let facts = |machine: &str| ClientFacts { machine_id: machine.into(), described: None, source: Some((None, None)), stream_allowed: true };
        ClientKeys::captured(ServerId::from_raw(1), 1, 1, [(ServerId::from_raw(1), None, facts("aaa")), (ServerId::from_raw(2), None, facts("bbb"))])
    }

    /// **A friend's session's poster comes from the relay's own Plex server, found by its machine
    /// identifier** among the servers the account reaches; a server it does not reach, or a key that
    /// is not a rating key, draws none. While this TV is in a session of its own the page lists none.
    #[test]
    fn the_poster_is_the_relays_server_and_none_while_in_a_session() {
        let rooms = [room("bbb", Some("42")), room("zzz", Some("42")), room("bbb", Some("../x")), room("bbb", None)];
        let got: Vec<(u16, String)> = together_rooms(&rooms, false, &keys()).into_iter().map(|r| (r.src, r.thumb)).collect();
        assert_eq!(got, vec![(2, "/library/metadata/42/thumb".to_string()), (0, String::new()), (0, String::new()), (0, String::new())]);
        assert!(together_rooms(&rooms, true, &keys()).is_empty(), "none while in a session");
    }
}
