//! **The Friends page** (the web app's *Amis*, `Activity.tsx`): what the account's Plex friends
//! watched, day by day, a filter by friend, the account's "Share what I watch" setting, and, for the
//! owner of a server, what plays on it right now.
//!
//! A `plx_ui::cards::Stack`, like My list: the head (the title block and the one row of controls,
//! [`super::header`]), then the *Watching now* shelf (only while something plays on a server the
//! account owns), then one shelf of posters per day ("Today", "Yesterday", then the date), then the
//! full-screen read-outs. Each card is a POST (the web's): a movie, or a show and the episodes the
//! friend watched of it in a row that day; its poster wears the friend's avatar, and its focused
//! caption says who did what and when ("Camille watched 2 episodes of · S1, E3 · 3 h ago").
//!
//! **The live section shows no device.** A *Watching now* card names who watches what, how far
//! (its progress bar) and whether it is paused. The store reads nothing more (`plex::live`).
//!
//! **Routing.** A screen names no sibling, and the page adds no lane of its own to the app: OK on a
//! title some server of the user's holds pushes its Detail, and OK on one none holds its catalog page
//! (`ContentReq::Push`); the strip, the chip and BACK are the shared page requests
//! (`ContentReq::Tab`, `ContentReq::Account`, `ContentReq::LeaveToHome`); the toggle's press is a
//! command to the community store on the ordered FIFO (`StoreCmd::Community`, `CommunityCmd::SetShare`),
//! the ONLY way the privacy write is asked for, and Try again is `CommunityCmd::Retry`. The filter is
//! the page's own: a press on a friend's plate narrows the shelves at once.
use std::collections::HashMap;
use std::ffi::CString;
use std::sync::Arc;

use plx_data::community::{ago, day_word, days_of, friends_of, local_day, Ago, DayWord, Post, Verb};
use plx_data::stores::community::{FeedState, LiveRow, ShareState};
use plx_data::watchlist::model::ServerMatch;
use plx_machine::machine::{
    Canon, Cx, Delivery, Edge, Effects, Enter, EntryId, FocusKey, FocusTarget, Fx, GroupId, Handled, InputEvent,
    InputKind, Key, LogicalState, Machine, Tick,
};
use plx_machine::present::Provenance;
use plx_plex::plex::community::ActivityKind;
use plx_plex::plex::live::LiveState;
use plx_plex::plex::ServerId;
use plx_ui::cards::{CardEvent, RowStyle, CardSource, Kind, SectionSpec, Stack, StackEvent, StackPage, Tile, TileLabel};
use plx_ui::consts::*;
use plx_ui::page_regions::{Arrival, BackStep, PageRegions};
use plx_ui::screen::{
    AxisMask, DrawFrame, EdgeRule, ElemKind, FocusSource, Focusable, GroupKind, GroupSpec, HitSource, Link,
    RenderStrategy, Screen, ScreenEvent, Seat,
};
use plx_ui::theme;
use plx_ui::tile::{Resume, TileFacts, TileKind};
use plx_ui::widgets::{Art, ProfileChipRead, StatusKind, StatusOverlay};
use plx_ui::{Painter, Rect};

use super::header::{self, ShareFace};
use crate::registry::{AppFx, ContentArg, ContentLike, ContentReq, HomeTab};
use plx_data::stores::community::CommunityCmd;
use plx_data::stores::{StoreCmd, StoreId};

/// What the app hands the page: the store's state, captured when it moved.
#[derive(Clone, Debug, PartialEq)]
pub struct FriendsView {
    pub feed: FeedState,
    pub shares: ShareState,
    /// The value a write in flight asks for.
    pub writing: Option<bool>,
    pub write_failed: bool,
    pub live: Arc<[LiveRow]>,
    /// The copy a press opens, by the feed's guids.
    pub held: HashMap<String, ServerMatch>,
}

impl Default for FriendsView {
    fn default() -> Self {
        Self { feed: FeedState::Loading, shares: ShareState::Unknown, writing: None, write_failed: false,
            live: Arc::from(Vec::new()), held: HashMap::new() }
    }
}

/// What the app tells the page.
#[derive(Clone, Debug, PartialEq)]
pub enum FriendsIn {
    View(FriendsView),
    /// The reachable server whose photo proxy fetches the feed's posters and avatars (absolute
    /// plex.tv URLs). `None`: none is reachable, and the posters draw their skeleton.
    ArtServer(Option<ServerId>),
    /// The time the relative times and day headings are worded against: unix seconds (`None`: not
    /// known, under a replay) and the viewer's UTC offset in seconds. Sent at most once a minute.
    Clock { now: Option<i64>, offset: i64 },
}

/// The toggle's focus element.
pub const SHARE_ELEM: u32 = 2;
/// The unreachable read-out's Try again.
pub const RETRY_ELEM: u32 = 3;
/// Filter chip `i` ("All" is 0).
const CHIP_BASE: u32 = 0x100;
/// The first card's element; cards count up from here and are never reused within a page.
const FIRST_CARD_ELEM: u32 = 0x1000;

const HEADER_GROUP: GroupId = GroupId(1);
const LIVE_GROUP: GroupId = GroupId(2);
const STATUS_GROUP: GroupId = GroupId(3);
/// Day shelf `i`'s group (`DAY_GROUP + i`).
const DAY_GROUP: u32 = 16;

/// The shelves' cards: Home's poster row.
const SHELF_STYLE: RowStyle = RowStyle::HOME;
/// A shelf's heading band, as the Person page's shelves keep it.
const SHELF_LABEL_H: f32 = TITLE_DY + CARD_DY;
/// The friend's avatar on a poster: a disc in its lower-left corner.
const AVATAR_D: f32 = 64.0;
const AVATAR_INSET: f32 = 12.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sec {
    Head,
    Live,
    Day(usize),
    Status,
}

/// What a press on a card opens.
#[derive(Clone, Debug, PartialEq)]
enum Open {
    Detail(ServerId, String),
    Catalog([u8; 12]),
    Nothing,
}

/// One card, derived from a post or a live session: everything its poster, avatar and caption read.
#[derive(Clone, Debug)]
struct Card {
    elem: u32,
    kind: TileKind,
    title: String,
    caption: String,
    src: u16,
    thumb: String,
    year: i32,
    avatar: String,
    initial: CString,
    progress: Option<f32>,
    open: Open,
}

/// A day's shelf.
struct DayShelf {
    heading: CString,
    cards: Vec<Card>,
}

/// Stable element keys: a post or a session keeps its key while it stays on the page.
struct Keys {
    next: u32,
    by_id: HashMap<String, u32>,
}

impl Keys {
    fn new() -> Self {
        Self { next: FIRST_CARD_ELEM, by_id: HashMap::new() }
    }

    fn key(&mut self, id: &str, kept: &mut HashMap<String, u32>) -> u32 {
        let elem = self.by_id.get(id).copied().unwrap_or_else(|| {
            assert!(self.next < plx_ui::geom::NAMESPACE_FLOOR, "friends element-key space exhausted");
            self.next += 1;
            self.next - 1
        });
        kept.insert(id.to_string(), elem);
        elem
    }
}

fn hex12(hex: &str) -> Option<[u8; 12]> {
    plx_data::watchlist::decode_hex(hex)
}

/// "S1, E3 · Title" of an episode, "" when the feed sent no numbers.
fn episode_line(e: &plx_plex::plex::community::FriendActivity) -> String {
    let numbers = match (e.season, e.episode) {
        (Some(s), Some(n)) => Some(plx_platform::i18n::msg::core_episode_ordinal(n, s)),
        _ => None,
    };
    match (numbers, e.title.is_empty()) {
        (Some(n), false) => format!("{n} · {}", e.title),
        (Some(n), true) => n,
        (None, false) => e.title.clone(),
        (None, true) => String::new(),
    }
}

fn ago_words(a: Ago) -> String {
    use plx_platform::i18n::msg;
    match a {
        Ago::Minutes(0) => msg::friends_ago_now().to_string(),
        Ago::Minutes(n) => msg::friends_ago_minutes(n),
        Ago::Hours(n) => msg::friends_ago_hours(n),
        Ago::Clock(hm) => plx_platform::i18n::current().time_of_day(hm).map(|t| msg::friends_ago_at(&t)).unwrap_or_default(),
        Ago::Unknown => String::new(),
    }
}

fn day_heading(w: DayWord) -> String {
    use plx_platform::i18n::msg;
    match w {
        DayWord::Today => msg::friends_day_today().to_string(),
        DayWord::Yesterday => msg::friends_day_yesterday().to_string(),
        DayWord::Unknown => msg::friends_day_earlier().to_string(),
        DayWord::Date([y, m, d]) => plx_platform::i18n::current()
            .month_day(m as u8, d as u8, plx_platform::i18n::MonthWidth::Long)
            .or_else(|| plx_platform::i18n::current().date_medium(y, m as u8, d as u8))
            .unwrap_or_default(),
    }
}

/// The caption of a post: who did what, the episode(s), and when.
fn post_caption(p: &Post, now: Option<i64>, offset: i64) -> String {
    use plx_platform::i18n::msg;
    let name = &p.first.user.name;
    let verb = match p.verb() {
        Verb::Movie => msg::friends_post_movie(name),
        Verb::Episode => msg::friends_post_episode(name),
        Verb::Episodes(n) => msg::friends_post_episodes(n as i64, name),
    };
    let mut parts = vec![verb];
    match p.verb() {
        Verb::Movie => {}
        Verb::Episode => parts.push(episode_line(&p.first)),
        Verb::Episodes(_) => {
            let last = p.watched_in_order().last().map(episode_line).unwrap_or_default();
            parts.push(last);
        }
    }
    if let Some(now) = now {
        parts.push(ago_words(ago(p.first.at, now, offset)));
    }
    parts.retain(|s| !s.is_empty());
    parts.join(" · ")
}

fn initial(name: &str) -> CString {
    CString::new(plx_ui::widgets::initial_of(name)).unwrap_or_default()
}

/// The page's content and the state its sections read; the `Stack` is the screen's other field.
struct Page {
    entry: EntryId,
    view: FriendsView,
    art_server: Option<ServerId>,
    now: Option<i64>,
    offset: i64,
    /// The chosen friend's id (`None`: All). The page's own state.
    who: Option<String>,
    /// The filter row's friends (after All), the most active first.
    friends: Vec<(String, String)>,
    live: Vec<Card>,
    days: Vec<DayShelf>,
    keys: Keys,
    /// Element to (section, index).
    index: HashMap<u32, (Sec, usize)>,
    rev: u64,
    header_capsules: plx_ui::widgets::TabStrip,
    header_pop: plx_ui::widgets::CtlPop<1>,
    handoff: plx_ui::scroll_chrome::Handoff,
    header_rows: plx_ui::page_header::SharedRow<header::Row>,
    header_swap: std::cell::RefCell<plx_ui::page_header::HeaderSwap>,
}

impl Page {
    fn touch(&mut self) {
        self.rev += 1;
    }

    fn share_face(&self) -> ShareFace {
        if self.view.writing.is_some() {
            return ShareFace::Pending;
        }
        match self.view.shares {
            _ if self.view.write_failed => ShareFace::Failed,
            ShareState::Unknown => ShareFace::Pending,
            ShareState::Failed => ShareFace::Failed,
            ShareState::Known(true) => ShareFace::On,
            ShareState::Known(false) => ShareFace::Off,
        }
    }

    fn selected_chip(&self) -> usize {
        self.who.as_ref().and_then(|id| self.friends.iter().position(|(f, _)| f == id)).map_or(0, |i| i + 1)
    }

    fn header_row(&self, measure: &dyn plx_machine::machine::Measure) -> std::rc::Rc<header::Row> {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (std::ptr::from_ref(plx_platform::i18n::current()) as usize).hash(&mut h);
        self.friends.hash(&mut h);
        self.selected_chip().hash(&mut h);
        self.share_face().hash(&mut h);
        let names: Vec<String> = self.friends.iter().map(|(_, n)| n.clone()).collect();
        self.header_rows.get_replacing(
            h.finish(),
            || header::Row::new(measure, &names, self.selected_chip(), self.share_face(), |i| CHIP_BASE + i as u32, SHARE_ELEM),
            |old, new| new.begin_swap(old, &mut self.header_swap.borrow_mut(), &mut plx_ui::widgets::TabStrip::new()),
        )
    }

    /// Re-derive the friends, the live cards and the day shelves from the view, the filter and the
    /// clock.
    fn rebuild(&mut self) {
        let mut kept = HashMap::new();
        self.index.clear();
        let art = self.art_server.map_or(0, |s| s.raw());
        let feed: &[plx_plex::plex::community::FriendActivity] = match &self.view.feed {
            FeedState::Ready(feed) => feed,
            _ => &[],
        };
        self.friends = friends_of(feed).into_iter().take(header::FILTER_FRIENDS).map(|(f, _)| (f.id, f.name)).collect();
        if self.who.as_ref().is_some_and(|id| !self.friends.iter().any(|(f, _)| f == id)) {
            self.who = None;
        }
        self.live.clear();
        for (sid, s) in self.view.live.iter() {
            let id = format!("live:{}:{}:{}", sid.raw(), s.rating_key, s.user);
            let elem = self.keys.key(&id, &mut kept);
            let caption = match s.state {
                LiveState::Paused => plx_platform::i18n::msg::friends_live_paused(&s.user),
                LiveState::Playing | LiveState::Buffering => plx_platform::i18n::msg::friends_live_watching(&s.user),
            };
            let episode = s.kind == "episode";
            let caption = if episode {
                let numbers = plx_platform::i18n::msg::core_episode_ordinal(s.episode, s.season);
                format!("{caption} · {numbers} · {}", s.title)
            } else {
                caption
            };
            let progress = (s.duration_ms > 0).then(|| (s.view_offset_ms as f32 / s.duration_ms as f32).clamp(0.0, 1.0));
            self.index.insert(elem, (Sec::Live, self.live.len()));
            self.live.push(Card {
                elem,
                kind: if episode { TileKind::Show } else { TileKind::Movie },
                title: if episode { s.show.clone() } else { s.title.clone() },
                caption,
                src: sid.raw(),
                thumb: s.thumb.clone(),
                year: 0,
                avatar: s.user_thumb.clone(),
                initial: initial(&s.user),
                progress,
                open: Open::Detail(*sid, s.rating_key.clone()),
            });
        }
        self.days.clear();
        let today = self.now.and_then(|now| local_day(Some(now), self.offset));
        for (d, day) in days_of(feed, self.who.as_deref(), self.offset).into_iter().enumerate() {
            let word = match today {
                Some(today) => day_word(day.day, today),
                None => day.day.map_or(DayWord::Unknown, |d| DayWord::Date(plx_data::community::civil_of_days(d))),
            };
            let mut cards = Vec::new();
            for post in &day.posts {
                let a = &post.first;
                let elem = self.keys.key(&format!("post:{}", a.id), &mut kept);
                let guid = a.guid();
                let held = guid.as_ref().and_then(|g| self.view.held.get(g)).copied();
                let open = match (held, a.catalog_hex.as_deref().and_then(hex12)) {
                    (Some(m), _) => Open::Detail(m.server_id, m.rating_key.to_string()),
                    (None, Some(hex)) => Open::Catalog(hex),
                    (None, None) => Open::Nothing,
                };
                let (src, thumb) = match (held, a.poster.as_deref()) {
                    (Some(m), _) => (m.server_id.raw(), format!("/library/metadata/{}/thumb", m.rating_key)),
                    (None, Some(url)) if art != 0 => (art, url.to_string()),
                    _ => (0, String::new()),
                };
                self.index.insert(elem, (Sec::Day(d), cards.len()));
                cards.push(Card {
                    elem,
                    kind: if a.kind == ActivityKind::Movie { TileKind::Movie } else { TileKind::Show },
                    title: a.show.clone().unwrap_or_else(|| a.title.clone()),
                    caption: post_caption(post, self.now, self.offset),
                    src,
                    thumb,
                    year: if post.episodes.is_empty() { a.year.unwrap_or(0) } else { 0 },
                    avatar: a.user.avatar.clone().filter(|_| art != 0).unwrap_or_default(),
                    initial: initial(&a.user.name),
                    progress: None,
                    open,
                });
            }
            self.days.push(DayShelf { heading: CString::new(day_heading(word)).unwrap_or_default(), cards });
        }
        self.keys.by_id = kept;
        self.touch();
    }

    fn card(&self, elem: u32) -> Option<&Card> {
        let (sec, i) = *self.index.get(&elem)?;
        self.cards_of(sec)?.get(i)
    }

    fn cards_of(&self, sec: Sec) -> Option<&[Card]> {
        match sec {
            Sec::Live => Some(&self.live),
            Sec::Day(d) => self.days.get(d).map(|d| d.cards.as_slice()),
            Sec::Head | Sec::Status => None,
        }
    }

    /// The shelves present, in document order.
    fn shelves(&self) -> Vec<Sec> {
        let mut out = Vec::new();
        if !self.live.is_empty() {
            out.push(Sec::Live);
        }
        out.extend((0..self.days.len()).filter(|d| !self.days[*d].cards.is_empty()).map(Sec::Day));
        out
    }

    fn group_of(sec: Sec) -> GroupId {
        match sec {
            Sec::Head => HEADER_GROUP,
            Sec::Live => LIVE_GROUP,
            Sec::Day(d) => GroupId(DAY_GROUP + d as u32),
            Sec::Status => STATUS_GROUP,
        }
    }

    /// The read-out the page shows instead of the feed, if any.
    fn readout(&self) -> Option<(&'static std::ffi::CStr, StatusKind, bool)> {
        use plx_platform::i18n::msg;
        match &self.view.feed {
            FeedState::NoAccount => Some((msg::friends_empty_no_account_c(), StatusKind::Empty, false)),
            FeedState::Failed => Some((msg::friends_empty_unreachable_c(), StatusKind::Failed, true)),
            FeedState::Ready(feed) if feed.is_empty() => Some((msg::friends_empty_none_c(), StatusKind::Empty, false)),
            FeedState::Loading | FeedState::Ready(_) => None,
        }
    }

    fn overlay(&self) -> Option<StatusOverlay<'static>> {
        let (caption, kind, retry) = self.readout()?;
        let overlay = StatusOverlay::new(Rect::FULL, caption, kind).page(plx_ui::icons::Icon::Crowd);
        Some(if retry { overlay.action(plx_platform::i18n::msg::browse_action_retry_c()) } else { overlay })
    }

    fn regions<'a>(&self, content: &'a [GroupId]) -> PageRegions<'a> {
        PageRegions {
            header: Some(HEADER_GROUP),
            content,
            rail: None,
            back_walks_header: false,
            arrival: Arrival::Content,
            pending: (self.view.feed == FeedState::Loading && content.is_empty()).then_some(GroupId(DAY_GROUP)),
        }
    }

    fn content_groups(&self) -> Vec<GroupId> {
        self.shelves().into_iter().map(Self::group_of).collect()
    }
}

/// One shelf's cards (`CardSource`).
struct Cards<'a> {
    cards: &'a [Card],
}

impl<H: ContentLike> CardSource<H> for Cards<'_> {
    fn len(&self) -> usize {
        self.cards.len()
    }
    fn elem(&self, i: usize) -> u32 {
        self.cards[i].elem
    }
    fn index_of(&self, e: &u32) -> Option<usize> {
        self.cards.iter().position(|c| c.elem == *e)
    }
    fn art(&self, i: usize) -> Art<'_> {
        let c = &self.cards[i];
        Art::Poster(Some(TileFacts {
            owner: c.elem as usize,
            src: c.src,
            kind: c.kind,
            title: &c.title,
            thumb: &c.thumb,
            year: c.year,
            resume: c.progress.map(|frac| Resume { frac, left_ms: 0 }),
            ..Default::default()
        }))
    }
    fn label(&self, i: usize) -> TileLabel {
        let c = &self.cards[i];
        TileLabel::titled(&c.title, &c.caption)
    }
    fn progress(&self, i: usize) -> Option<f32> {
        self.cards.get(i)?.progress
    }
    /// The friend's avatar in the poster's lower-left corner: the shared profile chip's disc (their
    /// plex.tv picture through the art server, else their initial).
    fn overlay(&self, p: Painter, i: usize, tile: &Tile, _measure: &dyn plx_machine::machine::Measure) {
        let Some(c) = self.cards.get(i) else { return };
        let d = AVATAR_D * tile.scale;
        let inset = AVATAR_INSET * tile.scale;
        let rect = Rect::new(tile.rect.x + inset, tile.rect.y + tile.rect.h - inset - d, d, d);
        plx_ui::widgets::profile_chip_with(p, ProfileChipRead { src: c.src, thumb: &c.avatar, initial: &c.initial }, 0.0, rect);
    }
    /// A post has no item menu: a hold does nothing.
    fn holdable(&self, _i: usize) -> bool {
        false
    }
}

impl<H: ContentLike> StackPage<H> for Page {
    type Key = Sec;
    type Cards<'a> = Cards<'a>;

    fn revision(&self, _cx: &Cx<'_, H>) -> u64 {
        self.rev
    }

    fn sections(&self, cx: &Cx<'_, H>, out: &mut Vec<SectionSpec<Sec>>) {
        out.push(SectionSpec::new(Sec::Head, Kind::Custom { height: header::BLOCK_H, focusable: true }, HEADER_GROUP).ranked(1));
        for sec in self.shelves() {
            out.push(SectionSpec::new(sec, Kind::Shelf { style: &SHELF_STYLE, heading: SHELF_LABEL_H }, Page::group_of(sec)));
        }
        let retry = self.overlay().filter(|_| self.view.feed == FeedState::Failed).and_then(|o| o.action_frame_measured(cx.measure));
        out.push(
            SectionSpec::new(Sec::Status, Kind::Overlay { rect: retry.unwrap_or(Rect::FULL), focusable: retry.is_some() }, STATUS_GROUP)
                .ranked(2),
        );
    }

    fn fallback(&self, _cx: &Cx<'_, H>, out: &mut Vec<Sec>) {
        out.extend(self.shelves());
        out.extend([Sec::Status, Sec::Head]);
    }

    fn cards<'a>(&'a self, _cx: &'a Cx<'_, H>, k: Sec) -> Option<Cards<'a>> {
        self.cards_of(k).filter(|c| !c.is_empty()).map(|cards| Cards { cards })
    }

    fn elem_of(&self, k: Sec) -> Option<u32> {
        match k {
            Sec::Head => Some(SHARE_ELEM),
            Sec::Status => (self.view.feed == FeedState::Failed).then_some(RETRY_ELEM),
            Sec::Live | Sec::Day(_) => None,
        }
    }

    fn card_has_menu(&self, _cx: &Cx<'_, H>, _k: Sec, _elem: &u32) -> bool {
        false
    }

    /// Every shelf but the last keeps the label band's air and the section air under it.
    fn shelf_foot(&self, _cx: &Cx<'_, H>, k: Sec) -> f32 {
        match self.shelves().last() {
            Some(&last) if last != k => SHELF_FOOT,
            _ => 0.0,
        }
    }

    fn focus_rect(&self, cx: &Cx<'_, H>, k: Sec, section: Rect) -> Rect {
        match k {
            Sec::Head => self.header_row(cx.measure).plate(SHARE_ELEM).map_or(section, |p| Rect::new(p.x, p.y + section.y, p.w, p.h)),
            _ => section,
        }
    }

    fn plain_group(&self, cx: &Cx<'_, H>, k: Sec, id: GroupId, extent: Rect) -> GroupSpec {
        let header = (k == Sec::Head).then(|| self.header_row(cx.measure));
        match header.as_ref().and_then(|row| row.focus(self.entry, id).group_spec()) {
            Some(spec) => GroupSpec { extent, ..spec },
            None => GroupSpec {
                id,
                kind: GroupKind::Free,
                seat: Seat::First,
                reachable: AxisMask::VERTICAL,
                edge: [EdgeRule::Geometric, EdgeRule::Geometric, EdgeRule::Stop, EdgeRule::Stop],
                extent,
                len: 1,
                elem: ElemKind::Bare,
            },
        }
    }

    fn extra_groups(&self, _cx: &Cx<'_, H>, out: &mut Vec<GroupSpec>) {
        let content = self.content_groups();
        self.regions(&content).declare_pending(out);
    }

    fn draw_heading(&self, k: Sec, f: &mut DrawFrame<'_, '_, H>, r: Rect, lift: f32) {
        let heading = match k {
            Sec::Live => plx_platform::i18n::msg::friends_live_heading_c(),
            Sec::Day(d) => match self.days.get(d) {
                Some(day) => day.heading.as_c_str(),
                None => return,
            },
            Sec::Head | Sec::Status => return,
        };
        let p = f.painter.alpha(f.page_alpha).translate(0.0, r.y);
        plx_ui::cards::draw_heading_c(p, heading, c"", c"", MARGIN_X, -lift, 1.0, f.measure);
    }

    fn custom_draw(&self, k: Sec, f: &mut DrawFrame<'_, '_, H>, r: Rect, _scroll: f32) {
        let p = f.painter.alpha(f.page_alpha);
        match k {
            Sec::Head => {
                let p = p.alpha(self.handoff.head_alpha(r.y));
                let row = self.header_row(f.measure);
                row.draw(p, r.y, &self.header_capsules, self.header_pop.scale_with(0, f.press.scale), &self.header_swap.borrow());
            }
            Sec::Status => {
                if let Some(overlay) = self.overlay() {
                    let on = f.focus.current.is_some_and(|key| key.entry == self.entry && key.elem == RETRY_ELEM);
                    overlay.focused(on).draw_measured(&plx_ui::Env::inert(), p, f.measure);
                }
            }
            Sec::Live | Sec::Day(_) => {}
        }
    }
}

pub struct FriendsScreen {
    page: Page,
    stack: Stack<Sec>,
    /// The page's own arrival seat while nobody has chosen another (My list's rule).
    seat: Option<Option<GroupId>>,
    own_enters: u8,
}

impl FriendsScreen {
    /// The heartbeat word, byte-identical to the route word.
    pub const WORD: &'static str = "friends";

    pub const SHAPE: &'static str = "FriendsScreen{entry:u32,next_elem:u32,feed:{tag:u32,len:u64},who:str,share:u32,live:[u32],days:[[u32]],stack:Stack{scroll:{pos:f32,vel:f32},target:f32,sections:[Shelf{pop,scroll}]},header_capsules:TabStrip{sel:Capsule{x:Spring{pos:f32,vel:f32},w:Spring{pos:f32,vel:f32},a:Spring{pos:f32,vel:f32},at:u32},foc:Capsule{x:Spring{pos:f32,vel:f32},w:Spring{pos:f32,vel:f32},a:Spring{pos:f32,vel:f32},at:u32}},header_pop:{sp:[Spring{pos:f32,vel:f32};1],focused:Option<u32>},handoff:Handoff{tucked:bool,chrome:f32,capsule:f32}}";

    /// The page as it mounts: nothing landed. The application feeds it ([`FriendsIn`]).
    pub fn loading(entry: EntryId) -> Self {
        let mut page = Page {
            entry,
            view: FriendsView::default(),
            art_server: None,
            now: None,
            offset: 0,
            who: None,
            friends: Vec::new(),
            live: Vec::new(),
            days: Vec::new(),
            keys: Keys::new(),
            index: HashMap::new(),
            rev: 0,
            header_capsules: plx_ui::widgets::TabStrip::new(),
            header_pop: plx_ui::widgets::CtlPop::new(),
            handoff: plx_ui::scroll_chrome::Handoff::new(),
            header_rows: plx_ui::page_header::SharedRow::new(),
            header_swap: std::cell::RefCell::new(plx_ui::page_header::HeaderSwap::new()),
        };
        page.rebuild();
        Self { page, stack: Stack::new(entry).home_when_unfocused(true), seat: None, own_enters: 0 }
    }

    pub fn input(&mut self, ev: FriendsIn) {
        match ev {
            FriendsIn::View(view) => {
                if self.page.view == view {
                    return;
                }
                self.page.view = view;
            }
            FriendsIn::ArtServer(server) => {
                if self.page.art_server == server {
                    return;
                }
                self.page.art_server = server;
            }
            FriendsIn::Clock { now, offset } => {
                if (self.page.now, self.page.offset) == (now, offset) {
                    return;
                }
                self.page.now = now;
                self.page.offset = offset;
            }
        }
        self.page.rebuild();
    }

    /// The friend the filter shows (`None`: All).
    pub fn filter(&self) -> Option<&str> {
        self.page.who.as_deref()
    }

    /// The day shelves' headings and each card's title and caption, as drawn: what a test reads.
    #[cfg(any(test, feature = "test-support"))]
    pub fn probe_days(&self) -> Vec<(String, Vec<(String, String)>)> {
        self.page.days.iter().map(|d| (d.heading.to_string_lossy().into_owned(),
            d.cards.iter().map(|c| (c.title.clone(), c.caption.clone())).collect())).collect()
    }

    /// The live shelf's cards (title, caption, progress).
    #[cfg(any(test, feature = "test-support"))]
    pub fn probe_live(&self) -> Vec<(String, String, Option<f32>)> {
        self.page.live.iter().map(|c| (c.title.clone(), c.caption.clone(), c.progress)).collect()
    }

    /// The filter row's names after All.
    #[cfg(any(test, feature = "test-support"))]
    pub fn probe_friends(&self) -> Vec<String> {
        self.page.friends.iter().map(|(_, n)| n.clone()).collect()
    }

    /// The element of filter chip `i` (0: All).
    pub fn chip_elem(i: usize) -> u32 {
        CHIP_BASE + i as u32
    }

    /// The element of the first card of day shelf `d`, if any.
    #[cfg(any(test, feature = "test-support"))]
    pub fn probe_day_card(&self, d: usize, i: usize) -> Option<u32> {
        self.page.days.get(d)?.cards.get(i).map(|c| c.elem)
    }

    /// What a press on `elem` does, as the page decides it.
    fn press(&mut self, elem: u32, fx: &mut Effects<'_, impl ContentLike>) -> Handled {
        if let Some(settings) = crate::registry::strip_settings(elem) {
            fx.push(Fx::App(settings));
            fx.invalidate(Provenance::Input);
            return Handled::Yes;
        }
        if elem >= plx_ui::dispatch::STRIP_BASE {
            return match Self::strip_press(elem) {
                Some(Some(req)) => {
                    self.route(AppFx::Content(req), fx);
                    Handled::Yes
                }
                Some(None) => Handled::Yes,
                None => Handled::No,
            };
        }
        match elem {
            RETRY_ELEM => self.route(AppFx::Store(StoreId::Community, StoreCmd::Community(CommunityCmd::Retry)), fx),
            SHARE_ELEM => {
                // The explicit press: the opposite of what the account says, and nothing while the
                // setting is not known or a save is out.
                if let (ShareState::Known(on), None) = (self.page.view.shares, self.page.view.writing) {
                    self.route(AppFx::Store(StoreId::Community, StoreCmd::Community(CommunityCmd::SetShare(!on))), fx);
                }
            }
            e if (CHIP_BASE..CHIP_BASE + 1 + header::FILTER_FRIENDS as u32).contains(&e) => {
                let i = (e - CHIP_BASE) as usize;
                let who = i.checked_sub(1).and_then(|i| self.page.friends.get(i)).map(|(id, _)| id.clone());
                if who != self.page.who {
                    self.page.who = who;
                    self.page.rebuild();
                    fx.invalidate(Provenance::Input);
                }
            }
            e => match self.page.card(e).map(|c| c.open.clone()) {
                Some(Open::Detail(sid, rk)) => {
                    fx.push(Fx::App(AppFx::Content(ContentReq::Push(ContentArg::Detail { sid, rk }))));
                }
                Some(Open::Catalog(hex)) => self.route(AppFx::Content(ContentReq::Push(ContentArg::Catalog { hex })), fx),
                Some(Open::Nothing) => {}
                None => return Handled::No,
            },
        }
        Handled::Yes
    }

    /// A press on the shared strip: the other pills and the chip are the shared page requests; the
    /// Friends pill is the page the user is on.
    fn strip_press(elem: u32) -> Option<Option<ContentReq>> {
        use plx_ui::widgets::{TAB_KEY_CHIP, TAB_KEY_FRIENDS, TAB_KEY_HOME, TAB_KEY_MOVIES, TAB_KEY_SEARCH, TAB_KEY_SHOWS, TAB_KEY_WATCHLIST};
        match elem {
            TAB_KEY_HOME => Some(Some(ContentReq::Tab(HomeTab::Home))),
            TAB_KEY_MOVIES => Some(Some(ContentReq::Tab(HomeTab::Movies))),
            TAB_KEY_SHOWS => Some(Some(ContentReq::Tab(HomeTab::Shows))),
            TAB_KEY_SEARCH => Some(Some(ContentReq::Tab(HomeTab::Search))),
            TAB_KEY_WATCHLIST => Some(Some(ContentReq::Tab(HomeTab::Watchlist))),
            TAB_KEY_CHIP => Some(Some(ContentReq::Account)),
            TAB_KEY_FRIENDS => Some(None),
            _ => None,
        }
    }

    fn route(&mut self, req: AppFx, fx: &mut Effects<'_, impl ContentLike>) {
        fx.push(Fx::App(req));
        fx.invalidate(Provenance::Input);
    }

    fn with_regions<R>(&self, then: impl FnOnce(&PageRegions<'_>) -> R) -> R {
        let content = self.page.content_groups();
        then(&self.page.regions(&content))
    }

    /// Seat the arrival in the content's head, and follow it there when the shelves land (My list's
    /// rule): while nothing has landed the page seats nothing.
    fn seat_first<H: ContentLike>(&mut self, fx: &mut Effects<'_, H>) {
        let (want, target) = self.with_regions(|r| (r.first_seat(), r.first_seat().map(FocusTarget::FirstInGroupAnimated).or_else(|| r.unseat())));
        self.seat = (want.is_none()).then_some(want);
        if let Some(target) = target {
            self.own_enters = self.own_enters.saturating_add(1);
            fx.push(Fx::Deliver(fx.from(), Delivery::Screen(ScreenEvent::Enter(Enter::Fresh { focus: target }))));
        }
    }

    fn follow_seat<H: ContentLike>(&mut self, fx: &mut Effects<'_, H>) {
        let Some(seated) = self.seat else { return };
        if seated != self.with_regions(|r| r.first_seat()) {
            self.seat_first(fx);
        }
    }

    fn back<H: ContentLike>(&mut self, cx: &Cx<'_, H>, fx: &mut Effects<'_, H>) -> Handled {
        let focus = cx.focus.current.filter(|key| key.entry == self.page.entry);
        let step = self.with_regions(|regions| {
            let at = focus.and_then(|key| regions.region_of(key.elem, Focusable::<H>::group_of(self, &key.elem, cx)));
            regions.back(at, self.stack.target() > 0.5)
        });
        let target = match step {
            BackStep::Top | BackStep::Content => self.with_regions(|r| r.content.first().copied()).map(FocusTarget::ContainerGroup),
            BackStep::Header => Some(FocusTarget::ContainerGroup(HEADER_GROUP)),
            BackStep::Nav => Some(FocusTarget::ContainerGroup(plx_ui::containers::tabs::STRIP)),
            BackStep::Leave => {
                self.route(AppFx::Content(ContentReq::LeaveToHome), fx);
                None
            }
        };
        if let Some(focus) = target {
            self.own_enters = self.own_enters.saturating_add(1);
            fx.push(Fx::Deliver(fx.from(), Delivery::Screen(ScreenEvent::Enter(Enter::Fresh { focus }))));
        }
        Handled::Yes
    }

    fn view<H: ContentLike>(&self) -> impl Focusable<H> + '_ {
        self.stack.view(&self.page)
    }

    fn tick<H: ContentLike>(&mut self, t: Tick, cx: &Cx<'_, H>) {
        self.page.header_row(cx.measure).step(self.page.entry, cx.focus.current, &mut self.page.header_capsules, &mut self.page.header_pop, t.dt());
        self.page.header_swap.get_mut().step(t.dt());
        self.page.handoff.step(t.dt(), self.stack.scroll(), self.stack.target(), SHELF_STYLE.pitch_fixed(), false);
    }
}

plx_ui::focusable_via_view!(FriendsScreen, H: [ContentLike], view);

impl LogicalState for FriendsScreen {
    fn write(&self, c: &mut Canon) {
        let page = &self.page;
        c.u32(page.entry.0).u32(page.keys.next);
        match &page.view.feed {
            FeedState::NoAccount => c.u32(0).u64(0),
            FeedState::Loading => c.u32(1).u64(0),
            FeedState::Failed => c.u32(2).u64(0),
            FeedState::Ready(feed) => c.u32(3).u64(feed.len() as u64),
        };
        c.str(page.who.as_deref().unwrap_or(""));
        c.u32(page.share_face() as u32);
        c.seq(page.live.len());
        for card in &page.live {
            c.u32(card.elem);
        }
        c.seq(page.days.len());
        for day in &page.days {
            c.seq(day.cards.len());
            for card in &day.cards {
                c.u32(card.elem);
            }
        }
        self.stack.write(c);
        page.header_capsules.write_motion(c);
        page.header_pop.write_motion(c);
        page.handoff.write(c);
    }

    fn probe(&self, out: &mut String) {
        let feed = match &self.page.view.feed {
            FeedState::NoAccount => "no-account".to_string(),
            FeedState::Loading => "loading".to_string(),
            FeedState::Failed => "failed".to_string(),
            FeedState::Ready(feed) => format!("ready:{}", feed.len()),
        };
        out.push_str(&format!(
            "friends feed={feed} days={} live={} who={} share={:?}",
            self.page.days.len(),
            self.page.live.len(),
            self.page.who.as_deref().unwrap_or("all"),
            self.page.share_face(),
        ));
    }
}

impl<H: ContentLike> Machine<H> for FriendsScreen {
    type Ev = ScreenEvent<H>;

    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, H>, fx: &mut Effects<'_, H>) -> Handled {
        let card = match self.stack.on(&self.page, ev, cx, fx) {
            Some(StackEvent::Card(_, card)) => Some(card),
            _ => None,
        };
        if let ScreenEvent::Input(InputEvent { kind, .. }) = ev {
            if matches!(kind, InputKind::Key { edge: Edge::Down, .. } | InputKind::Pointer { .. }
                | InputKind::Click { .. } | InputKind::Drag { .. } | InputKind::Wheel { .. }) {
                self.seat = None;
            }
        }
        match ev {
            ScreenEvent::Tick(t) => {
                self.tick(*t, cx);
                self.follow_seat(fx);
                Handled::Yes
            }
            ScreenEvent::Enter(enter) => {
                match enter {
                    Enter::Fresh { .. } if self.own_enters > 0 => self.own_enters -= 1,
                    Enter::Fresh { .. } => self.seat_first(fx),
                    Enter::Restored => self.seat = None,
                }
                fx.invalidate(Provenance::Nav);
                Handled::Yes
            }
            ScreenEvent::Uncover => {
                fx.invalidate(Provenance::Nav);
                Handled::Yes
            }
            ScreenEvent::FocusMoved { .. } | ScreenEvent::StoreChanged(..) => {
                fx.invalidate(Provenance::Input);
                Handled::Yes
            }
            ScreenEvent::Activate(elem) => self.press(*elem, fx),
            ScreenEvent::PressCommit(_) => {
                if let Some(CardEvent::Activate(elem)) = card {
                    self.press(elem, fx);
                } else if let Some(key) = cx.focus.current.filter(|key| key.entry == self.page.entry) {
                    self.press(key.elem, fx);
                }
                Handled::Yes
            }
            ScreenEvent::Input(InputEvent { kind: InputKind::Key { key: Key::Back, edge: Edge::Down, .. }, .. }) => {
                self.back(cx, fx)
            }
            _ => Handled::No,
        }
    }
}

impl<H: ContentLike> Screen<H> for FriendsScreen {
    fn focused_card_rect(&self, cx: &Cx<'_, H>, focus: Option<FocusKey<u32>>, at: plx_ui::screen::At) -> Option<Rect> {
        let key = focus.filter(|k| k.entry == self.page.entry && k.elem >= FIRST_CARD_ELEM)?;
        Focusable::<H>::place(self, &key.elem, cx, at).map(|placed| placed.rect)
    }
    fn redraw_focused(&self, f: &mut DrawFrame<'_, '_, H>, focus: Option<FocusKey<u32>>) {
        self.stack.view(&self.page).redraw_focused(f, focus);
    }
    fn opener_departing(&self, key: FocusKey<u32>) -> bool {
        self.stack.view(&self.page).opener_departing::<H>(key)
    }
    fn glides(&self) -> bool {
        true
    }
    fn name(&self) -> &'static str {
        Self::WORD
    }
    fn chrome_tuck(&self) -> f32 {
        self.page.handoff.tuck()
    }
    fn state(&self) -> &dyn LogicalState {
        self
    }
    fn crumb(&self, _cx: &Cx<'_, H>) -> Option<std::borrow::Cow<'_, str>> {
        None
    }
    fn prepare(&mut self, _budget: &mut plx_ui::frame::Budget, _cx: &Cx<'_, H>) {}
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, H>) {
        plx_gfx::gfx::frame_clear(theme::CLEAR_RGB.0, theme::CLEAR_RGB.1, theme::CLEAR_RGB.2);
        self.stack.view(&self.page).paint(f);
        let tuck = self.page.handoff.tuck();
        if tuck > 0.0 {
            let [r, g, b, _] = theme::SURFACE_APP;
            plx_ui::scroll_capsule::draw_top_fade(f.painter.alpha(f.page_alpha), SCR_W, [r, g, b], tuck, None);
        }
    }
    fn render(&self) -> RenderStrategy {
        RenderStrategy::Page
    }
    fn focus_source(&self) -> FocusSource {
        FocusSource::Engine
    }
    fn hit_source(&self) -> HitSource {
        HitSource::Engine
    }
    /// The bar, the head and the shelves are one column (`PageRegions::links`).
    fn links(&self, out: &mut Vec<Link>) {
        self.with_regions(|regions| regions.links(out));
    }
    fn holds_arrival(&self, dir: plx_ui::screen::Dir) -> bool {
        self.with_regions(|regions| regions.holds(dir))
    }
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}
