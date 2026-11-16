//! **Watch Together's notices** (`plx_ui::action_notice`): what the session tells the viewer,
//! decided here from values and posted on the application's one transient notice.
//!
//! - **A participant's gesture** (`Notice::Action`): "Camille paused", one passive card that a
//!   following gesture updates in place.
//! - **A friend's new session** (`Notice::FriendSessions`): an actionable card, "Camille is watching
//!   “Dune” together · [▲] Join", only for a room not announced before, only while the viewer's
//!   "Friends' session alerts" setting is on and they are in no session of their own. It is shown
//!   in the player too, because it takes no focus (`action_notice`'s module doc); a room the relay
//!   stops listing withdraws its card. Join enters the session (`TogetherCmd::JoinFriend`), whose
//!   title then opens through the player side's one gate.
//! - **The gate's standing** (`FollowState`): Opening is a sticky progress line until it resolves,
//!   Missing a plain message. Asking is the player page's decision card when a player page is up;
//!   with none (the viewer's own play still waiting on its page) it is an actionable card whose
//!   Join answers the gate; leaving it unanswered leaves the question held, as the gate holds it.
//! - **Failures and changes of hand** (`JoinFailed`, `HostFailed`, `HostChanged`, `Left`): passive.
//! - **The friends' listing is polled** (`TogetherCmd::RefreshFriends`) every [`POLL_MS`] while the
//!   alerts are on or the Friends page is up, never during a session (the web's cadence:
//!   first after [`FIRST_POLL_MS`]); the Friends page's arrival asks at once.
//!
//! Nothing here performs IO: the caller hands the notices, the state and the elapsed time, and
//! carries out what [`TogetherUi::activate`] returns.
use std::collections::BTreeSet;

use plx_data::together::player::{FollowAnswer, FollowState};
use plx_data::together::session::{FriendSession, JoinError, Notice};
use plx_data::together::wire::SyncAction;
use plx_platform::i18n::msg;
use plx_ui::action_notice::{ActionNotice, Answer, Notice as Card, NoticeKey};
use plx_ui::icons::Icon;

/// The friends' listing is asked again this often.
pub(crate) const POLL_MS: u32 = 45_000;
/// …and first this long after it may be.
pub(crate) const FIRST_POLL_MS: u32 = 4_000;

const ID_ACTION: u64 = 1;
const ID_HAND: u64 = 2;
const ID_ERROR: u64 = 3;
const ID_LEFT: u64 = 4;
const ID_FOLLOW: u64 = 5;
const ID_ASK: u64 = 6;
/// A friend's room's card id: this base plus the room's hash.
const ID_FRIEND: u64 = 1 << 32;

/// A friend's live session as the notices and the Friends page show it (no ticket: that stays in the store).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FriendRoom {
    pub(crate) relay: String,
    /// The relay's Plex server (`machineIdentifier`), where `key` is the title's rating key.
    pub(crate) server: String,
    pub(crate) key: Option<String>,
    pub(crate) room: String,
    pub(crate) name: String,
    pub(crate) title: String,
    pub(crate) sub: Option<String>,
    pub(crate) count: u32,
    pub(crate) playing: bool,
}

impl FriendRoom {
    pub(crate) fn of(f: &FriendSession) -> FriendRoom {
        FriendRoom {
            relay: f.relay.clone(),
            server: f.server.clone(),
            key: f.card.key.clone(),
            room: f.room.clone(),
            name: f.card.name.clone(),
            title: f.card.title.clone(),
            sub: f.card.sub.clone(),
            count: f.count,
            playing: f.card.playing,
        }
    }
}

/// What an answered card asks the application to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum NoticeAction {
    JoinFriend { relay: String, room: String },
    Follow(FollowAnswer),
}

/// What the caller knows this frame.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct UiCtx {
    /// A session is live (hosted or joined).
    pub(crate) in_session: bool,
    /// The viewer's "Friends' session alerts" setting.
    pub(crate) friend_alerts: bool,
    /// The Friends page is on screen.
    pub(crate) friends_page: bool,
    /// A player page is mounted (it asks the gate's question itself).
    pub(crate) player_page: bool,
}

/// The notice, the friend rooms already announced, the actions of the cards standing, the poll clock.
#[derive(Default)]
pub(crate) struct TogetherUi {
    pub(crate) notice: ActionNotice,
    announced: BTreeSet<String>,
    actions: Vec<(u64, NoticeAction)>,
    /// Time since the last listing was asked (`None`: never yet).
    since_poll_ms: Option<u32>,
    /// Time the listing may be asked in for, before its first ask.
    waiting_ms: u32,
    /// The gate's standing last shown.
    follow: FollowState,
    /// The friends' sessions last listed, for the Friends page.
    pub(crate) sessions: Vec<FriendRoom>,
    /// A key-down the engaged notice took: its key-up is the notice's too.
    pub(crate) swallow_sym: Option<u32>,
}

fn friend_id(room: &str) -> u64 {
    // FNV-1a: a stable id per room for the card's in-place update and withdrawal.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in room.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    ID_FRIEND | (h & 0xffff_ffff)
}

/// The words of a join failure.
pub(crate) fn join_error_text(e: JoinError) -> &'static str {
    match e {
        JoinError::Code => msg::together_error_code(),
        JoinError::Relay => msg::together_error_relay(),
        JoinError::NotFound => msg::together_error_not_found(),
        JoinError::NoHost => msg::together_error_no_host(),
        JoinError::Failed => msg::together_error_failed(),
    }
}

/// A title as the notices name it.
pub(crate) fn title_of(item: &plx_data::together::wire::SharedItem) -> &str {
    if item.title.is_empty() {
        item.sub.as_deref().unwrap_or("")
    } else {
        &item.title
    }
}

impl TogetherUi {
    fn post(&mut self, card: Card, action: Option<NoticeAction>) {
        self.actions.retain(|(id, _)| *id != card.id);
        if let Some(a) = action {
            self.actions.push((card.id, a));
        }
        self.notice.post(card);
    }

    fn withdraw(&mut self, id: u64) {
        self.actions.retain(|(i, _)| *i != id);
        self.notice.withdraw(id);
    }

    /// One notice of the session.
    pub(crate) fn on_notice(&mut self, n: &Notice, ctx: &UiCtx) {
        match n {
            Notice::Action { name, action: Some(a) } => {
                let text = match a {
                    SyncAction::Pause => msg::together_action_paused(name),
                    SyncAction::Play => msg::together_action_played(name),
                    SyncAction::Seek => msg::together_action_seeked(name),
                };
                self.post(Card::passive(ID_ACTION, text).mark(Icon::Crowd), None);
            }
            Notice::Action { action: None, .. } => {}
            Notice::HostChanged { host: true } => self.post(Card::passive(ID_HAND, msg::together_host_now()).mark(Icon::Crowd), None),
            Notice::HostChanged { host: false } => {}
            Notice::HostFailed => self.post(Card::passive(ID_ERROR, msg::together_error_host()), None),
            Notice::JoinFailed(e) => self.post(Card::passive(ID_ERROR, join_error_text(*e)), None),
            Notice::Left => {
                self.withdraw(ID_FOLLOW);
                self.withdraw(ID_ASK);
                self.post(Card::passive(ID_LEFT, msg::together_left()), None);
            }
            Notice::FriendSessions(list) => {
                let rooms: Vec<FriendRoom> = list.iter().map(FriendRoom::of).collect();
                self.on_friend_sessions(&rooms, ctx);
            }
            Notice::Opened { .. } | Notice::Status(_) | Notice::Item(_) | Notice::Visible(_) | Notice::Listed(_) => {}
        }
    }

    pub(crate) fn on_friend_sessions(&mut self, list: &[FriendRoom], ctx: &UiCtx) {
        // A room no longer listed takes its card with it.
        let listed: BTreeSet<&str> = list.iter().map(|f| f.room.as_str()).collect();
        let gone: Vec<String> = self.announced.iter().filter(|r| !listed.contains(r.as_str())).cloned().collect();
        for room in gone {
            self.withdraw(friend_id(&room));
            self.announced.remove(&room);
        }
        let mut fresh = list.iter().filter(|f| !self.announced.contains(&f.room)).cloned().collect::<Vec<_>>();
        for f in &fresh {
            self.announced.insert(f.room.clone());
        }
        self.sessions = list.to_vec();
        // Discreet: one card at a time, none during a session, none with the alerts off.
        if !ctx.friend_alerts || ctx.in_session || fresh.is_empty() {
            return;
        }
        let first = fresh.remove(0);
        let text = msg::together_friend_started(&first.name, &first.title);
        let id = friend_id(&first.room);
        self.post(
            Card::actionable(id, text, msg::together_join()).mark(Icon::Crowd),
            Some(NoticeAction::JoinFriend { relay: first.relay.clone(), room: first.room.clone() }),
        );
    }

    /// The gate's standing, each frame; only a change posts.
    pub(crate) fn on_follow(&mut self, f: &FollowState, ctx: &UiCtx) {
        if *f == self.follow {
            return;
        }
        self.follow = f.clone();
        match f {
            FollowState::Idle => {
                self.withdraw(ID_FOLLOW);
                self.withdraw(ID_ASK);
            }
            FollowState::Opening { item } => {
                self.withdraw(ID_ASK);
                self.post(Card::passive(ID_FOLLOW, msg::together_opening(title_of(item))).mark(Icon::Crowd).sticky(), None);
            }
            FollowState::Missing { item } => {
                self.withdraw(ID_ASK);
                self.withdraw(ID_FOLLOW);
                self.post(Card::passive(ID_ERROR, msg::together_missing(title_of(item))), None);
            }
            FollowState::Asking(ask) => {
                self.withdraw(ID_FOLLOW);
                if !ctx.player_page {
                    let text = format!("{} {}", msg::together_ask_title(), msg::together_ask_body(title_of(&ask.item)));
                    self.post(Card::actionable(ID_ASK, text, msg::together_join()).mark(Icon::Crowd).sticky(), Some(NoticeAction::Follow(FollowAnswer::Join)));
                }
            }
        }
    }

    /// Whether the friends' listing is due this frame (`elapsed_ms` since the previous call).
    pub(crate) fn poll_due(&mut self, elapsed_ms: u32, ctx: &UiCtx) -> bool {
        let wanted = (ctx.friend_alerts || ctx.friends_page) && !ctx.in_session;
        if !wanted {
            self.waiting_ms = 0;
            return false;
        }
        match self.since_poll_ms.as_mut() {
            None => {
                self.waiting_ms = self.waiting_ms.saturating_add(elapsed_ms);
                if self.waiting_ms >= FIRST_POLL_MS || ctx.friends_page {
                    self.since_poll_ms = Some(0);
                    return true;
                }
                false
            }
            Some(since) => {
                *since = since.saturating_add(elapsed_ms);
                if *since >= POLL_MS {
                    *since = 0;
                    return true;
                }
                false
            }
        }
    }

    /// The Friends page arrived: ask now.
    pub(crate) fn poll_now(&mut self) {
        self.since_poll_ms = Some(POLL_MS);
    }

    /// The card OK answered: what to do.
    pub(crate) fn activate(&mut self, id: u64) -> Option<NoticeAction> {
        let i = self.actions.iter().position(|(a, _)| *a == id)?;
        Some(self.actions.remove(i).1)
    }

    /// A key, offered to the notice before the page.
    pub(crate) fn key(&mut self, key: NoticeKey) -> (Answer, Option<NoticeAction>) {
        match self.notice.key(key) {
            Answer::Activated(id) => (Answer::Activated(id), self.activate(id)),
            other => (other, None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plx_data::together::player::FollowAsk;
    use plx_data::together::wire::SharedItem;

    fn ctx() -> UiCtx {
        UiCtx { in_session: false, friend_alerts: true, friends_page: false, player_page: false }
    }

    fn item(title: &str) -> SharedItem {
        SharedItem { guid: "plex://movie/1".into(), server: Some("srv".into()), key: Some("1".into()), kind: "movie".into(), title: title.into(), sub: None }
    }

    fn session(room: &str, name: &str, title: &str) -> FriendRoom {
        FriendRoom { relay: "wss://relais.exemple.fr".into(), server: String::new(), key: None, room: room.into(), name: name.into(), title: title.into(), sub: None, count: 2, playing: true }
    }

    #[test]
    fn a_participants_gesture_is_one_passive_card_updated_in_place() {
        let mut ui = TogetherUi::default();
        ui.on_notice(&Notice::Action { name: "Camille".into(), action: Some(SyncAction::Pause) }, &ctx());
        ui.notice.step(16);
        ui.on_notice(&Notice::Action { name: "Camille".into(), action: Some(SyncAction::Seek) }, &ctx());
        let cur = ui.notice.current().unwrap();
        assert_eq!(cur.id, ID_ACTION);
        assert_eq!(cur.text, msg::together_action_seeked("Camille"));
        assert!(cur.action.is_none(), "passive: nothing to reach");
    }

    #[test]
    fn a_friends_new_session_is_offered_once_and_join_enters_it() {
        let mut ui = TogetherUi::default();
        let list = vec![session("room-1", "Camille", "Dune")];
        ui.on_friend_sessions(&list, &ctx());
        ui.notice.step(16);
        let cur = ui.notice.current().unwrap().clone();
        assert_eq!(cur.text, msg::together_friend_started("Camille", "Dune"));
        assert_eq!(cur.action.as_deref(), Some(msg::together_join()));
        assert_eq!(ui.key(NoticeKey::Ok).0, Answer::Ignored, "it took no focus");
        assert!(ui.notice.engage(), "the UP overshoot reaches it");
        let (answer, action) = ui.key(NoticeKey::Ok);
        assert_eq!(answer, Answer::Activated(cur.id));
        assert_eq!(action, Some(NoticeAction::JoinFriend { relay: "wss://relais.exemple.fr".into(), room: "room-1".into() }));
        // listed again: not announced twice
        ui.on_friend_sessions(&list, &ctx());
        for _ in 0..200 {
            ui.notice.step(16);
        }
        assert!(!ui.notice.busy(), "no second card for the same room");
    }

    #[test]
    fn no_friend_card_with_the_alerts_off_or_during_a_session() {
        for c in [UiCtx { friend_alerts: false, ..ctx() }, UiCtx { in_session: true, ..ctx() }] {
            let mut ui = TogetherUi::default();
            ui.on_friend_sessions(&[session("r", "Camille", "Dune")], &c);
            assert!(!ui.notice.busy());
            assert_eq!(ui.sessions.len(), 1, "the Friends page still lists it");
        }
    }

    #[test]
    fn a_room_no_longer_listed_withdraws_its_card() {
        let mut ui = TogetherUi::default();
        ui.on_friend_sessions(&[session("r", "Camille", "Dune")], &ctx());
        ui.notice.step(16);
        assert!(ui.notice.visible());
        ui.on_friend_sessions(&[], &ctx());
        for _ in 0..100 {
            ui.notice.step(16);
        }
        assert!(!ui.notice.busy());
        assert!(ui.activate(friend_id("r")).is_none(), "its action went with it");
    }

    #[test]
    fn the_gates_standing_maps_to_progress_a_message_or_a_question() {
        let mut ui = TogetherUi::default();
        ui.on_follow(&FollowState::Opening { item: item("Dune") }, &ctx());
        ui.notice.step(16);
        assert_eq!(ui.notice.current().unwrap().text, msg::together_opening("Dune"));
        for _ in 0..2_000 {
            ui.notice.step(16);
        }
        assert!(ui.notice.visible(), "progress stands until the gate moves on");
        ui.on_follow(&FollowState::Missing { item: item("Dune") }, &ctx());
        for _ in 0..60 {
            ui.notice.step(16);
        }
        assert_eq!(ui.notice.current().unwrap().text, msg::together_missing("Dune"));
        assert!(ui.notice.current().unwrap().action.is_none(), "a plain message");

        // Asking with a player page up: the page asks; without one, the card does, and Join answers.
        let ask = FollowState::Asking(FollowAsk { item: item("Dune"), position: None, playing: item("Mine") });
        let mut ui = TogetherUi::default();
        ui.on_follow(&ask, &UiCtx { player_page: true, ..ctx() });
        assert!(!ui.notice.busy());
        let mut ui = TogetherUi::default();
        ui.on_follow(&ask, &ctx());
        ui.notice.step(16);
        assert!(ui.notice.engage());
        assert_eq!(ui.key(NoticeKey::Ok).1, Some(NoticeAction::Follow(FollowAnswer::Join)));
        ui.on_follow(&FollowState::Idle, &ctx());
        assert!(ui.activate(ID_ASK).is_none());
    }

    #[test]
    fn the_listing_is_polled_on_its_cadence_only_while_wanted() {
        let mut ui = TogetherUi::default();
        assert!(!ui.poll_due(FIRST_POLL_MS - 1, &ctx()));
        assert!(ui.poll_due(1, &ctx()), "first after the delay");
        assert!(!ui.poll_due(POLL_MS - 1, &ctx()));
        assert!(ui.poll_due(1, &ctx()), "then every POLL_MS");
        assert!(!ui.poll_due(POLL_MS, &UiCtx { in_session: true, ..ctx() }), "never during a session");
        assert!(!ui.poll_due(POLL_MS, &UiCtx { friend_alerts: false, ..ctx() }), "nor with nothing to show it");
        assert!(ui.poll_due(POLL_MS, &UiCtx { friend_alerts: false, friends_page: true, ..ctx() }), "the Friends page wants it");
        ui.poll_now();
        assert!(ui.poll_due(0, &UiCtx { friends_page: true, ..ctx() }), "an arrival asks at once");
    }
}
