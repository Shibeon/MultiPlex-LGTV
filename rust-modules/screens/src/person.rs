//! The owned person page for restructure phase 7. Under its measured header the page is one
//! horizontal shelf per category, as Home is: **Available on MultiPlex** (the person's films and
//! shows on the viewer's own servers, the merged Movies then Shows of every source), then the
//! credits plex.tv knows of, one row each for Acting, Appearances, Producing, Directing and Other
//! credits ([`plx_data::person::credit_rows`]). Every row is a `cards::Shelf` in the page's
//! `cards::Stack`: ONE line whatever its length, walked by Left/Right, Up/Down between rows and to
//! the header, the engine remembering the card per row. A credit a server holds opens its Detail
//! page; one no server holds opens its catalog page. Shelves, biography panel and animation share
//! the UI system, while navigation and focus are explicit contracts:
//! the engine is the only cursor, card keys preserve item identity across store reshapes, and page
//! changes leave through `ContentReq` effects. The Person owner observes the
//! visible session generation and retires credential-bound metadata on identity changes; its
//! normal store landing rebuilds this page without reopening it.
//!
//! The Filmography route (`screens::filmography`, `ContentArg::Filmography`) is no longer offered
//! here: its departments are this page's rows. It stays only because replay anchor
//! `12-filmography-detail-return` (`tests/focusfp.sh` flow 12) and the dev `filmography` trigger
//! open it directly; removing it means re-pointing flow 12 at a credit-row card and recording that
//! anchor afresh.
//!
//! **The biography sheet left in phase 10** (`screens::person_bio`, `ContentPanel::Bio`): it is a
//! `Style::Alert` surface on the container tree, so its input, its phase and its teardown are the
//! container's and this page neither intercepts keys for it nor hides it. What stays here is the
//! GATE — the panel is offered exactly when the `MORE` mark is drawn, and both read
//! [`PersonScreen::bio_more`]: the answer `header_flow` measured over this header's own column
//! width, never a fresh wrap per frame.

use std::ffi::CString;

use plx_data::person::{Credit, Person, CREDIT_ROWS, NSHELF};
use plx_plex::plex::ServerId;
use plx_data::pms::PmsMovie;
use plx_data::stores::person::PersonCmd;
use plx_ui::cards::{RowStyle, TileLabel};
use plx_ui::cards::{CardEvent, CardSource, Kind, SectionSpec, Stack, StackEvent, StackPage};
use plx_ui::consts::*;
use plx_ui::label::{Label, VAlign};
use plx_machine::machine::{
    Canon, Cx, Edge, Effects, EntryId, GroupId, Handled, InputEvent, InputKind, Key, Leave,
    LogicalState, Machine, Measure, Tick,
};
use plx_machine::present::Provenance;
use plx_ui::screen::{
    At, AxisMask, By, Dir, DrawFrame, EdgeRule, ElemKind, FocusSource, FocusedCard, Focusable, GroupKind,
    GroupSpec, HitSource, Link, RenderStrategy, Screen, ScreenEvent, Seat,
};
#[cfg(test)]
use plx_ui::screen::Enter;
#[cfg(test)]
use plx_data::person::CreditRow;
use plx_ui::text_view::TextView;
use plx_ui::hstack::Item;
use plx_ui::theme;
use plx_ui::vstack::{Anchor, VStack};
use plx_ui::widgets::{Art, PageGround, StatusKind, StatusOverlay};
use plx_ui::{Env, Painter, Rect, View};

use super::registry::{
    tile_facts, AppFx, CardIdentity, CardKeys, CardPageMemory, ContentArg, ContentLike, ContentReq, PageMemory,
    PersonLike,
};

// -------------------------------------------------------------------------------------------
// element-key + group-id namespace (module doc: "Element-key namespace")
// -------------------------------------------------------------------------------------------

const HEADER_ELEM: u32 = 0;
const FIRST_CARD_ELEM: u32 = 0x1000;
/// How many credit rows the page can stack ([`CREDIT_ROWS`]).
const NCREDIT: usize = CREDIT_ROWS.len();
/// Every shelf the page can show: the local row and the credit rows.
const NROWS: usize = 1 + NCREDIT;

/// The page's sections, in document order, and the focus group each is known by. The ids are the
/// page's and fixed per section kind (the engine remembers a cursor per `(entry, group)`, and a
/// fresh mount targets `GroupId(0)`, the header): a shelf keeps its id whichever other shelves
/// exist. `groups()` lists them in document order: header, then the shelves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sec {
    /// The header band: portrait, name, facts and biography (one focus element). Drawn by the page,
    /// in the flow from the top of the document.
    Head,
    /// The skeleton / empty read-out in the shelves' place, while no shelf has content.
    State,
    /// The air between the header band and the first shelf. The air under a shelf is its own foot
    /// ([`StackPage::shelf_foot`]), so two shelves are adjacent sections.
    Gap,
    /// Available on MultiPlex: the person's titles on the viewer's own servers.
    Local,
    /// The credit row at this index of [`CREDIT_ROWS`].
    Credits(usize),
}

const HEADER_GROUP: GroupId = GroupId(0);
/// Group of the Available on MultiPlex shelf.
const LOCAL_GROUP: GroupId = GroupId(1);
/// Group of each credit row, by its index in [`CREDIT_ROWS`].
const CREDIT_GROUP: [GroupId; NCREDIT] = [GroupId(2), GroupId(3), GroupId(4), GroupId(5), GroupId(6)];
/// Sections with no focus element still carry a distinct id ([`SectionSpec::group`]); none is listed.
const STATE_GROUP: GroupId = GroupId(8);
const GAP_GROUP: GroupId = GroupId(9);

#[cfg(test)]
pub(crate) mod cards_harness; // Tier 2 card conformance (cards_conformance_tests.rs)

/// Where an element key falls in this screen's own row model — the one place the `u32` namespace
/// is decoded, so `on_header`/`focus_pos`'s legacy jobs (deriving "which row" from a struct field)
/// become "which row" derived from the key the engine handed back instead.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Located {
    Header,
    /// Card `i` of the Available on MultiPlex shelf (Movies, then Shows).
    Local(usize),
    /// Card `i` of credit row `row` ([`CREDIT_ROWS`] index).
    Credit(usize, usize),
}

// -------------------------------------------------------------------------------------------
// geometry constants — the shared person header, biography and compact Filmography action
// -------------------------------------------------------------------------------------------

const PORTRAIT_EXP: f32 = 320.0;
const PORTRAIT_BARE: f32 = 220.0;
const PORTRAIT_RES: (std::os::raw::c_int, std::os::raw::c_int) = (300, 300);
// The owner's reference mock, 2026-09-19: the portrait's top sits on the page's ordinary top
// margin, not 42px below it — the header is the top of the page's content, not a band offset
// from it.
const HEADER_TOP: f32 = plx_ui::consts::MARGIN_Y;
const BAND_GAP: f32 = theme::space::XL;
// The owner's reference mock, 2026-09-19: name→roles measures one rung tighter than the shipped
// `LG` — the header reads as one tight block rather than a loose stack.
const META_GAP: f32 = theme::space::MD;
// The owner's reference mock, 2026-09-19: roles→Born/Died measures the SAME rung as the line
// above it (`MD`), not the tighter `SM` the two lines shipped with — they no longer read as one
// fact split across two lines, so they no longer sit closer than the gap above them.
const LIFE_GAP: f32 = theme::space::MD;
// The highlight BOX, not the prose, keeps `space::MD` (24) from the facts line above it (design:
// the bio highlight sits `--space-md` below the dates line) — and the box itself pads the prose by
// `HL_PAD_Y` on every side, so the cap-to-prose gap this measures to has to carry both.
const BIO_GAP: f32 = theme::space::MD + HL_PAD_Y;
const fn text_w_const(d: f32) -> f32 {
    SCR_W - MARGIN_X - (MARGIN_X + d + BAND_GAP)
}
const BIO_W: f32 = text_w_const(PORTRAIT_EXP);
const BIO_LINES: usize = 3;
const BIO_LEAD: f32 = 40.0;
const HL_PAD_X: f32 = 26.0;
const HL_PAD_Y: f32 = 24.0;
const BIO_MORE_GAP: f32 = theme::space::LG;
/// The air between the header block and the first shelf's heading: one shelf section to the next
/// is [`theme::space::SECTION`], and the header is the page's first section.
const BAND_GAP_TO_SHELF: f32 = theme::space::SECTION;
/// The air under a shelf that has another under it ([`StackPage::shelf_foot`]): the label band's
/// air and the section air, which keeps the pitch Home's shelves have ([`SECTION_AIR`]).
const SHELF_GAP: f32 = SHELF_FOOT;
const SHELF_LABEL_H: f32 = TITLE_DY + CARD_DY;
const SHELF_STYLE: RowStyle = RowStyle::HOME;


// -------------------------------------------------------------------------------------------
// pure store-shape predicates (ported from `ui/person.rs`, `Scene`-independent already there)
// -------------------------------------------------------------------------------------------

/// How many titles the Available on MultiPlex shelf holds: the Movies, then the Shows.
fn local_len(p: &Person) -> usize {
    (0..NSHELF).map(|k| p.shelf(k).len()).sum()
}

/// Card `i` of the Available on MultiPlex shelf as `(kind, index in that kind's shelf)`.
fn local_at(p: &Person, i: usize) -> Option<(usize, usize)> {
    let mut i = i;
    for k in 0..NSHELF {
        let n = p.shelf(k).len();
        if i < n {
            return Some((k, i));
        }
        i -= n;
    }
    None
}

fn local_item(p: &Person, i: usize) -> Option<&PmsMovie> {
    local_at(p, i).and_then(|(k, j)| p.shelf(k).get(j))
}

/// The engine identity of a credit card: its row and its catalog id, interned with the page's
/// card keys under no server (a credit is plex.tv's, not a server's). A title credited in two rows
/// is two cards; [`plx_data::person::credit_rows`] keeps one card per title within a row.
fn credit_key(row: usize, c: &Credit) -> String {
    if c.catalog_id.is_empty() {
        format!("credit/{row}/title/{}/{}", c.year, c.title)
    } else {
        format!("credit/{row}/{}", c.catalog_id)
    }
}

/// The shelves that have content, in page order: Available on MultiPlex, then each credit row.
fn shelves_present(local: usize, credits: &[Vec<Credit>; NCREDIT]) -> ([Sec; NROWS], usize) {
    let mut v = [Sec::Local; NROWS];
    let mut n = 0;
    if local > 0 {
        v[n] = Sec::Local;
        n += 1;
    }
    for (r, row) in credits.iter().enumerate() {
        if !row.is_empty() {
            v[n] = Sec::Credits(r);
            n += 1;
        }
    }
    (v, n)
}

// -------------------------------------------------------------------------------------------
// Header measurement uses explicit meta/life presence flags, so the pure layout remains
// testable without a PersonScreen or server data. The measured entry frame belongs to this
// same flow and is reused for painting, focus and hit geometry.
// -------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Default)]
struct HeaderFlow {
    bio_truncated: bool,
    exp_h: f32,
    exp_d: f32,
    portrait_y: f32,
    name_y: f32,
    meta_y: Option<f32>,
    life_y: Option<f32>,
    bio_y: Option<f32>,
}

/// Pure over the facts it needs, so it is testable with no `Person`/`PersonScreen` in scope (the
/// PENDING header state, which this decides the placeholder geometry for, has no live `Person`
/// seam to exercise it through).
fn header_flow(
    pending: bool,
    bio: &str,
    has_roles: bool,
    has_life: bool,
    measure: &dyn Measure,
) -> HeaderFlow {
    let mut f = HeaderFlow::default();
    let has_meta = pending || has_roles;
    let has_life = pending || has_life;
    let has_bio = pending || !bio.is_empty();
    let bio_h = if !has_bio {
        0.0
    } else if pending && bio.is_empty() {
        BIO_LEAD * BIO_LINES as f32
    } else {
        let view = bio_view(bio, 1.0, measure);
        f.bio_truncated = view.truncates(BIO_W);
        view.measure_h(BIO_W)
    };
    // One stack, so a missing line takes its gap with it. The name and meta lines own the gap BELOW
    // them (`after`): the name leaves META_GAP to whatever follows it (the meta line, or the life
    // line when there is no meta) and the meta line leaves LIFE_GAP to the life line; the
    // biography keeps BIO_GAP whichever lines above it are absent. The
    // life line draws at `CAPTION` (see `draw_header`'s loop), not `LABEL` — advancing by the wrong
    // rung's cap height left a few px of drift between the pending skeleton's reserved band and the
    // loaded line's actual one.
    let items = [
        Item::new(measure.cap_h(theme::size::DISPLAY)).after(META_GAP),
        presence(has_meta, measure.cap_h(theme::size::LABEL)).after(LIFE_GAP),
        presence(has_life, measure.cap_h(theme::size::CAPTION)),
        presence(has_bio, bio_h).before(BIO_GAP),
    ];
    let stack = VStack::new(&items, 0.0, Anchor::Top(0.0));
    // The bottom of the last line as the spans stand (`y + h`), not `stack.height()`: the sum the
    // callers compare `exp_h` against, with no ulp between them.
    let y = stack.placed().last().map_or(0.0, |s| s.y + s.h);
    f.name_y = 0.0;
    f.meta_y = stack.span(1).map(|(y, _)| y);
    f.life_y = stack.span(2).map(|(y, _)| y);
    f.bio_y = stack.span(3).map(|(y, _)| y);
    let bare = !pending && f.meta_y.is_none() && f.life_y.is_none() && f.bio_y.is_none();
    f.exp_d = if bare { PORTRAIT_BARE } else { PORTRAIT_EXP };
    f.exp_h = y.max(f.exp_d);
    if bare {
        f.portrait_y = (f.exp_h - f.exp_d) * 0.5;
        let ty = (f.exp_h - y) * 0.5;
        f.name_y = ty;
        for v in [&mut f.meta_y, &mut f.life_y, &mut f.bio_y]
            .into_iter()
            .flatten()
        {
            *v += ty;
        }
    }
    f
}

/// An item `h` tall when `present`, else one that takes no room and no gap.
fn presence(present: bool, h: f32) -> Item {
    if present { Item::new(h) } else { Item::absent() }
}

fn bio_view<'a>(bio: &'a str, a: f32, measure: &'a dyn Measure) -> TextView<'a> {
    TextView::new(
        bio,
        theme::size::BODY,
        theme::with_a(theme::TEXT_READING, a),
    )
    .with_measure(measure)
    .leading(BIO_LEAD)
    .max_lines(BIO_LINES)
    .fade_for_more(BIO_MORE_GAP)
}

fn text_w(d: f32) -> f32 {
    SCR_W - MARGIN_X - col_x(d)
}

fn col_x(d: f32) -> f32 {
    MARGIN_X + d + BAND_GAP
}

fn cstr_elide(s: &str, w: f32, sz: std::os::raw::c_int, bold: std::os::raw::c_int, measure: &dyn Measure) -> CString {
    if s.is_empty() {
        return CString::default();
    }
    let elided = plx_gfx::text::elide_by(s, w, false, |t| measure.width_str(t, sz, bold != 0));
    CString::new(elided).unwrap_or_default()
}

// -------------------------------------------------------------------------------------------
// the shelf's content, for `plx_ui::cards::Shelf`
// -------------------------------------------------------------------------------------------

/// One shelf's cards, read over the store's person (or the page's credit rows) and the page's
/// element interning. The engine element of a card is stable for its identity ([`CardKeys`]), so a
/// landing may move an item along or between shelves without moving the engine cursor off it: each
/// shelf's `index_of` simply stops (or starts) answering for it.
enum ShelfCards<'a> {
    /// Available on MultiPlex: the person's Movies then Shows on the viewer's servers.
    Local { person: &'a Person, cards: &'a CardKeys },
    /// Credit row `row` ([`CREDIT_ROWS`] index): plex.tv's credits, the art through the ORIGIN
    /// server's photo transcoder (`sid`), as the Filmography route's preview fetched it.
    Credits { row: usize, credits: &'a [Credit], sid: ServerId, cards: &'a CardKeys },
}

/// A credit card's state line: the role (a character or a job) and the year, whichever are known.
fn credit_state(c: &Credit) -> Option<String> {
    match (c.role.is_empty(), c.year) {
        (true, 0) => None,
        (true, y) => Some(y.to_string()),
        (false, 0) => Some(c.role.clone()),
        (false, y) => Some(format!("{} · {y}", c.role)),
    }
}

impl<H: ContentLike> CardSource<H> for ShelfCards<'_> {
    fn len(&self) -> usize {
        match self {
            Self::Local { person, .. } => local_len(person),
            Self::Credits { credits, .. } => credits.len(),
        }
    }
    fn elem(&self, i: usize) -> u32 {
        match self {
            Self::Local { person, cards } => local_item(person, i).and_then(|m| cards.elem_for(m.sid, &m.rk)),
            Self::Credits { row, credits, cards, .. } => {
                credits.get(i).and_then(|c| cards.elem_for(ServerId::UNSET, &credit_key(*row, c)))
            }
        }
        .unwrap_or(HEADER_ELEM)
    }
    fn index_of(&self, e: &u32) -> Option<usize> {
        match self {
            Self::Local { person, cards } => {
                let id = cards.get(*e)?;
                (0..NSHELF).flat_map(|k| person.shelf(k).iter()).position(|m| {
                    plx_plex::plex::same_item((m.sid, m.rk.as_str()), (id.sid, id.rk.as_str()))
                })
            }
            Self::Credits { row, credits, cards, .. } => {
                let id = cards.get(*e)?;
                if id.sid != ServerId::UNSET {
                    return None;
                }
                credits.iter().position(|c| credit_key(*row, c) == id.rk)
            }
        }
    }
    fn art(&self, i: usize) -> Art<'_> {
        match self {
            Self::Local { person, .. } => Art::Poster(local_item(person, i).map(tile_facts::of)),
            Self::Credits { credits, sid, .. } => match credits.get(i).filter(|c| !c.thumb.is_empty()) {
                Some(c) => Art::Thumb { sid: sid.raw(), key: &c.thumb, res: plx_ui::widgets::POSTER_RES },
                None => Art::Poster(None),
            },
        }
    }
    /// Every poster here follows the Under posters setting through the one builder. The character
    /// the credit plays is the PAGE's own state for the tile (the way a Continue Watching tile's time
    /// left is), so it is the details line under every setting but Nothing; a local title with no
    /// character (every crew credit, and every credit before the roles batch lands) reads the picked
    /// fact like any other poster. A credit row's tile states its role and its year.
    fn label(&self, i: usize) -> TileLabel {
        let policy = crate::registry::captions::policy();
        match self {
            Self::Local { person, .. } => match local_at(person, i) {
                Some((kind, j)) => {
                    let m = &person.shelf(kind)[j];
                    let role = Some(person.role(kind, j)).filter(|role| !role.is_empty()).map(str::to_owned);
                    crate::registry::captions::label(&policy, &tile_facts::caption_facts(m), role)
                }
                None => TileLabel::default(),
            },
            Self::Credits { credits, .. } => match credits.get(i) {
                Some(c) => {
                    let facts = plx_ui::caption_policy::CaptionFacts {
                        title: Some(c.title.clone()),
                        year: (c.year > 0).then_some(c.year),
                        ..Default::default()
                    };
                    crate::registry::captions::label(&policy, &facts, credit_state(c))
                }
                None => TileLabel::default(),
            },
        }
    }
    fn progress(&self, i: usize) -> Option<f32> {
        match self {
            Self::Local { person, .. } => local_item(person, i).and_then(|m| m.resume_frac()),
            Self::Credits { .. } => None,
        }
    }
}

// -------------------------------------------------------------------------------------------
// the screen
// -------------------------------------------------------------------------------------------

/// Which groups the page links, as of the store's last landing: the shelves that have content, in
/// page order.
#[derive(Clone, Copy)]
struct Reach {
    rows: [Sec; NROWS],
    n: usize,
}

impl Default for Reach {
    fn default() -> Self {
        Self { rows: [Sec::Local; NROWS], n: 0 }
    }
}

impl Reach {
    fn rows(&self) -> &[Sec] {
        &self.rows[..self.n]
    }
}

/// The page's content and the state its sections read. The `Stack` is the screen's other field so
/// that `self.stack.on(&self.page, ..)` borrows the two apart; every section reads only this.
struct Page {
    entry: EntryId,
    // ---- identity, fixed at construction; re-issued to the store on `Enter` (module doc: "an
    // Enter, fresh or restored, re-opens" — this Bridge's PersonStore holds one person, so
    // returning to a covered instance must re-claim it through an addressed effect) ----
    sid: ServerId,
    key: String,
    guid: String,
    name: String,
    thumb: String,

    // ---- logical state the engine cannot derive ----
    /// Set the moment an explicit D-pad press lands on (or stays on) the header — never by a data
    /// landing that merely leaves the header as the only row. See [`bio_mark_visible`].
    header_marked: bool,
    /// Stable item-key interning. This maps identities to engine elements; it never stores which
    /// element is focused.
    cards: CardKeys,
    /// One-shot return hydration. While set, a known engine card key may remain unpublished until
    /// that card's own source answers; the engine remains the sole owner of the key itself.
    return_pending: bool,

    // ---- render cache: ambient ground and skeleton clock (never hashed; the page's own scroll and card springs are the stack's, which IS in the canon) ----
    amb: PageGround,
    amb_seeded: bool,
    /// Skeleton spinner clock, in ms — cached each tick from [`spin_phase`](Self::spin_phase)'s
    /// `advance`.
    spin_ms: f32,
    /// The underlying clock for [`spin_ms`](Self::spin_ms) (`motion::Phase`, phase 12 D4): reports
    /// `Motion` from inside its own `advance` rather than the raw `+= dt` this used to be, with
    /// `fx.note(Motion)` a separate line further down `tick`.
    spin_phase: plx_machine::motion::Phase,

    // ---- render cache: baked text runs + measured flow, rebuilt only when the store lands ----
    name_c: CString,
    /// The life line's parts, in flow order (`Born …`, the birthplace, `Died …`, each absent
    /// rather than blank) — drawn through `widgets::dotted_run`, which owns the `·` ink itself, so
    /// this is never joined into one string the way `name_c`/the shelf-count runs are.
    life_parts: Vec<String>,
    /// Each shelf's heading and count runs, by [`Page::row_slot`]: Available on MultiPlex, then
    /// the credit rows.
    row_title_c: [CString; NROWS],
    row_count_c: [CString; NROWS],
    /// The credit rows ([`plx_data::person::credit_rows`]), rebuilt when the store's revision moves:
    /// the availability join under them changes as sources land.
    credits: [Vec<Credit>; NCREDIT],
    header: HeaderFlow,
    header_dirty: bool,
    /// What the page's links are made of; the screen turns it into group ids ([`PersonScreen::links`]).
    reach: Reach,
    /// The store revision ([`PersonView::revision`](plx_data::person::PersonView::revision)) the
    /// card interning and `links_c` were last derived at; `None` until a person is there. The
    /// content counter, not the request epoch and not a shelf length: a same-length shelf
    /// replacement moves it.
    synced: Option<u64>,
    covered_ready_c: bool,
    /// The biography's focus lift ([`Self::bio_marked`]). Presentation, not logical state.
    bio_lift: plx_ui::text_lift::TextLift,
    /// Moves whenever the page derives something its sections read that the store's revision does
    /// not carry (a header remeasure). Derived, not logical state.
    layout_gen: u64,
}

/// The person / actor page (restructure spec §13, phase 7).
pub struct PersonScreen {
    page: Page,
    /// The page's layout, one scroll, groups, reconcile and the shelves (`ui::cards::Stack`).
    /// Presentation, not logical state — its motion is in the canon, its content is the store's.
    stack: Stack<Sec>,
    /// This page has already asked the Person store to close the slot it owned. §3.4's
    /// `pop_sequence` delivers `WillLeave(ForGood)` AND `Unmount` to the same body, and both land
    /// in the teardown arm below; an un-latched `self.person(cx).is_some()` guard there reads a
    /// slot this page has already disclaimed by the time the second event arrives, because the
    /// first `Close` crosses `AppFx::Store` and is still queued rather than applied — and so
    /// emits a SECOND non-idempotent `Close`. Not hashed: it is teardown bookkeeping for one
    /// event pair, never a property of the page's logical shape. Mirrors
    /// `screens/detail/mod.rs`'s `teardown_cleared` exactly (#119, #126).
    teardown_closed: bool,
}

impl LogicalState for PersonScreen {
    fn write(&self, w: &mut Canon) {
        let page = &self.page;
        w.u32(page.entry.0)
            .u32(page.sid.raw() as u32)
            .str(&page.key)
            .str(&page.guid)
            .str(&page.name)
            .str(&page.thumb)
            .bool(page.header_marked)
            .bool(page.return_pending)
            .u32(page.cards.next)
            .u32(page.cards.len() as u32);
        for card in &page.cards.keys {
            w.u32(card.sid.raw() as u32).str(&card.rk).u32(card.elem);
        }
        self.stack.write(w);
    }

    fn probe(&self, out: &mut String) {
        let page = &self.page;
        out.push_str(&format!(
            "person sid={} key={} marked={} return_pending={} card_keys={} next_elem={}",
            page.sid.raw(),
            page.key,
            page.header_marked,
            page.return_pending,
            page.cards.len(),
            page.cards.next
        ));
    }
}

impl PersonScreen {
    pub const SHAPE: &'static str = "PersonScreen{entry:EntryId,sid:ServerId,key:String,guid:String,name:String,thumb:String,header_marked:bool,return_pending:bool,next_card_elem:u32,card_keys:[{sid:ServerId,rk:String,elem:u32}],stack:Stack{scroll:{pos:f32,vel:f32},target:f32,sections:[Shelf{motion}]}}";

    /// Mount a person from the header a cast row (or, later, the Filmography route) handed in —
    /// mirrors `PersonCmd::Open`'s fields exactly. Issues the store command directly; see the
    /// module doc for why an `Enter` (fresh OR restored) does so again rather than assuming the
    /// single-slot store still holds this instance's data.
    pub fn new(
        entry: EntryId,
        sid: ServerId,
        key: String,
        guid: String,
        name: String,
        thumb: String,
    ) -> Self {
        Self {
            page: Page::new(entry, sid, key, guid, name, thumb),
            // A page that leaves focus (a menu over it, a pointer gone) rests at the top.
            stack: Stack::new(entry).hold_hint().home_when_unfocused(true),
            teardown_closed: false,
        }
    }

    /// The page's `Focusable` (see `focusable_via_view!` below): the stack's view over the page.
    fn view<H: ContentLike + PersonLike>(&self) -> impl Focusable<H> + '_ {
        self.stack.view(&self.page)
    }

    pub fn restore(&mut self, memory: &CardPageMemory) {
        self.page.restore(memory);
    }
}

impl Page {
    fn new(entry: EntryId, sid: ServerId, key: String, guid: String, name: String, thumb: String) -> Self {
        Self {
            entry,
            sid,
            key,
            guid,
            name,
            thumb,
            header_marked: false,
            cards: CardKeys::new(FIRST_CARD_ELEM),
            return_pending: false,
            amb: PageGround::new(),
            amb_seeded: false,
            spin_ms: 0.0,
            spin_phase: plx_machine::motion::Phase::default(),
            name_c: CString::default(),
            life_parts: Vec::new(),
            row_title_c: Default::default(),
            row_count_c: Default::default(),
            credits: Default::default(),
            header: HeaderFlow::default(),
            header_dirty: true,
            reach: Reach::default(),
            synced: None,
            covered_ready_c: false,
            bio_lift: plx_ui::text_lift::TextLift::new(),
            layout_gen: 0,
        }
    }

    /// Claim this Bridge's Person owner for this identity through the addressed store effect.
    fn request_store<H: ContentLike + PersonLike>(&mut self, fx: &mut Effects<'_, H>) {
        fx.push(plx_machine::machine::Fx::App(AppFx::Store(
            plx_data::stores::StoreId::Person,
            plx_data::stores::StoreCmd::Person(PersonCmd::Open {
            sid: self.sid,
            key: self.key.clone(),
            guid: self.guid.clone(),
            name: self.name.clone(),
            thumb: self.thumb.clone(),
            }),
        )));
        self.header_dirty = true;
        self.amb_seeded = false;
    }

    fn person<'a, H: PersonLike>(&self, cx: &Cx<'a, H>) -> Option<&'a Person> {
        H::person(cx).current().filter(|p| {
            plx_plex::plex::same_item((p.sid, p.key.as_str()), (self.sid, self.key.as_str()))
        })
    }

    fn sync_card_keys<H: PersonLike>(&mut self, cx: &Cx<'_, H>) {
        let Some(p) = self.person(cx) else {
            return;
        };
        let shelved = (0..NSHELF).flat_map(|kind| p.shelf(kind).iter());
        self.cards.intern_all(shelved.map(|item| (item.sid, item.rk.as_str())), "person");
        let credited: Vec<String> = self.credits.iter().enumerate()
            .flat_map(|(row, credits)| credits.iter().map(move |c| credit_key(row, c)))
            .collect();
        self.cards.intern_all(credited.iter().map(|k| (ServerId::UNSET, k.as_str())), "person");
    }

    fn refresh_store_cache<H: PersonLike>(&mut self, cx: &Cx<'_, H>) {
        self.covered_ready_c = false;
        let Some(person) = self.person(cx) else {
            self.synced = None;
            self.reach = Reach::default();
            return;
        };
        self.covered_ready_c = person.credited && !H::person(cx).loading();
        let revision = H::person(cx).revision();
        if self.synced == Some(revision) {
            return;
        }
        self.synced = Some(revision);
        self.layout_gen += 1;
        self.credits = plx_data::person::credit_rows(person);
        self.sync_card_keys(cx);
        let (rows, n) = shelves_present(local_len(person), &self.credits);
        self.reach = Reach { rows, n };
    }

    pub fn restore(&mut self, memory: &CardPageMemory) {
        self.synced = None;
        self.cards.merge(&memory.cards, FIRST_CARD_ELEM, "person");
        self.header_marked |= memory.header_marked;
    }

    fn memory(&self) -> CardPageMemory {
        CardPageMemory { cards: self.cards.clone(), header_marked: self.header_marked, ..Default::default() }
    }

    #[cfg(test)]
    fn elem_for(&self, item: &PmsMovie) -> Option<u32> {
        self.cards.elem_for(item.sid, &item.rk)
    }

    fn card_for_elem(&self, elem: u32) -> Option<&CardIdentity> {
        self.cards.get(elem)
    }

    /// Retire return hydration only after the engine's saved card has an answer from its own
    /// source. A page-level `landed` is intentionally not consulted: another server may already
    /// have populated a shelf while this card's server is still resolving or retrying.
    fn settle_return_pending<H: ContentLike + PersonLike>(&mut self, cx: &Cx<'_, H>) {
        if !self.return_pending {
            return;
        }
        let Some(elem) = cx
            .focus
            .current
            .filter(|focus| focus.entry == self.entry)
            .map(|focus| focus.elem)
        else {
            return;
        };
        let Some(sid) = self.card_for_elem(elem).map(|card| card.sid) else {
            self.return_pending = false;
            return;
        };
        let Some(person) = self.person(cx) else {
            return;
        };
        if self.locate(person, elem).is_some() || !plx_data::person::media_resolving(person, sid) {
            self.return_pending = false;
        }
    }

    fn locate(&self, p: &Person, elem: u32) -> Option<Located> {
        if elem == HEADER_ELEM {
            return Some(Located::Header);
        }
        let id = self.cards.get(elem)?;
        if id.sid == ServerId::UNSET && id.rk.starts_with("credit/") {
            return self.credits.iter().enumerate().find_map(|(row, credits)| {
                credits.iter().position(|c| credit_key(row, c) == id.rk).map(|i| Located::Credit(row, i))
            });
        }
        (0..NSHELF)
            .flat_map(|k| p.shelf(k).iter())
            .position(|m| plx_plex::plex::same_item((m.sid, m.rk.as_str()), (id.sid, id.rk.as_str())))
            .map(Located::Local)
    }

    /// The focus key of card `col` of the Available on MultiPlex shelf.
    #[cfg(test)]
    fn local_key(&self, p: &Person, col: usize) -> plx_machine::machine::FocusKey<u32> {
        let elem = local_item(p, col).and_then(|m| self.elem_for(m)).unwrap_or(HEADER_ELEM);
        plx_machine::machine::FocusKey { entry: self.entry, elem }
    }

    /// The focus key of card `col` of credit row `row`.
    #[cfg(test)]
    fn credit_card_key(&self, row: CreditRow, col: usize) -> plx_machine::machine::FocusKey<u32> {
        let elem = self.credits[row.index()]
            .get(col)
            .and_then(|c| self.cards.elem_for(ServerId::UNSET, &credit_key(row.index(), c)))
            .unwrap_or(HEADER_ELEM);
        plx_machine::machine::FocusKey { entry: self.entry, elem }
    }

    /// Where shelf `k`'s heading runs live in [`Self::row_title_c`] / [`Self::row_count_c`].
    fn row_slot(k: Sec) -> Option<usize> {
        match k {
            Sec::Local => Some(0),
            Sec::Credits(r) => Some(1 + r),
            _ => None,
        }
    }

    fn refresh_runs(&mut self, p: &Person, measure: &dyn Measure) {
        let w = text_w(PORTRAIT_EXP);
        self.name_c = cstr_elide(&p.name, w, theme::size::DISPLAY, 1, measure);

        // Roles and life are drawn through `widgets::dotted_run` now (see `draw_header`), which
        // owns the `·` ink itself and elides nothing — `MAX_ROLES`/the fixed life-fact count are
        // this line's width safety, the way the cap on any other fixed-vocabulary run is, so the
        // per-character `cstr_elide` this used to run is no longer needed. Birthplace is its own
        // part now (design: "Born …" · birthplace · "Died …"), not glued onto Born with a comma —
        // it no longer depends on a birth date being known.
        self.life_parts = Vec::new();
        let born = plx_ui::fmt::pretty_date(&p.born, 0);
        if !born.is_empty() {
            self.life_parts.push(plx_platform::i18n::msg::browse_person_born(&born));
        }
        if !p.birthplace.is_empty() {
            self.life_parts.push(p.birthplace.clone());
        }
        let died = plx_ui::fmt::pretty_date(&p.died, 0);
        if !died.is_empty() {
            self.life_parts.push(plx_platform::i18n::msg::browse_person_died(&died));
        }

        let count = |n: usize| match n {
            0 => CString::default(),
            n => CString::new(plx_platform::i18n::current().number(n as i64)).unwrap_or_default(),
        };
        self.row_title_c[0] = CString::new(plx_platform::i18n::msg::browse_person_available()).unwrap_or_default();
        self.row_count_c[0] = count((0..NSHELF).map(|k| p.total(k)).sum());
        for (r, row) in CREDIT_ROWS.iter().enumerate() {
            self.row_title_c[1 + r] = CString::new(row.title()).unwrap_or_default();
            self.row_count_c[1 + r] = count(self.credits[r].len());
        }
    }

    fn remeasure_header(&mut self, p: &Person, measure: &dyn Measure) {
        self.refresh_runs(p, measure);
        self.header = header_flow(
            plx_data::person::facts_pending(p),
            &p.bio,
            !p.roles.is_empty(),
            !self.life_parts.is_empty(),
            measure,
        );
        self.header_dirty = false;
        self.layout_gen += 1;
    }

    fn focused_movie_in<'a>(&self, p: &'a Person, elem: Option<u32>) -> Option<&'a PmsMovie> {
        match elem.and_then(|e| self.locate(p, e))? {
            Located::Local(col) => local_item(p, col),
            _ => None,
        }
    }

    pub fn focused_item<'a, H: PersonLike>(
        &self,
        focus: Option<plx_machine::machine::FocusKey<u32>>,
        cx: &Cx<'a, H>,
    ) -> Option<&'a PmsMovie> {
        let p = self.person(cx)?;
        self.focused_movie_in(p, focus.filter(|k| k.entry == self.entry).map(|k| k.elem))
    }

    /// The zero-area geometric anchor. Keeping this separate from [`Self::header_rect`] lets DOWN
    /// project to the first shelf tile without making the drawn biography impossible to click.
    fn header_anchor(&self) -> Rect {
        Rect::new(MARGIN_X, HEADER_TOP, 0.0, self.header.exp_h.max(1.0))
    }

    /// The actual drawn/clickable header band. `place` and the hit-map stop both return this exact
    /// rectangle.
    fn header_rect(&self) -> Rect {
        Rect::new(
            MARGIN_X,
            HEADER_TOP,
            SCR_W - 2.0 * MARGIN_X,
            self.header.exp_h.max(1.0),
        )
    }

    /// The Tick work that precedes the stack's layout: the store cache, the header measure and
    /// the ambient ground. False while no person is on show (nothing is drawn, nothing animates).
    fn pre_tick<H: ContentLike + PersonLike>(&mut self, dt: f32, cx: &Cx<'_, H>) -> bool {
        let cur = cx.focus.current.map(|k| k.elem);
        self.refresh_store_cache(cx);
        let Some(p) = self.person(cx) else {
            // Nothing is drawn while `person()` is `None` (`Screen::draw` returns before touching
            // the skeleton), so there is nothing on screen for the clock to animate — freeze
            // rather than advance-and-report for no visible reason.
            return false;
        };
        if self.header_dirty {
            self.remeasure_header(p, cx.measure);
        }

        let focused_movie = self.focused_movie_in(p, cur);
        let k = PageGround::page_target(focused_movie.map(tile_facts::of));
        if self.amb_seeded {
            self.amb.key_target(k, dt);
        } else {
            self.amb.jump_target(k);
            self.amb_seeded = true;
        }
        true
    }

    /// The Tick work that follows the stack's: the skeleton clock and the biography's lift.
    fn post_tick<H: ContentLike + PersonLike>(&mut self, t: Tick, cx: &Cx<'_, H>, fx: &mut Effects<'_, H>) {
        let Some(p) = self.person(cx) else { return };
        if plx_data::person::facts_pending(p) || H::person(cx).loading() {
            self.spin_ms = self.spin_phase.advance(t, &mut fx.present());
        }
        let cur = cx.focus.current.map(|k| k.elem);
        self.bio_lift.step(self.bio_marked(cur), t.dt());
    }

    /// OK on the header: opens the biography panel when the bio is truncated. Mirrors
    /// `header_ok`'s tail (the overlay guard lives in `step`'s `Input` arm now — see the module
    /// doc for why the raw key is intercepted before the engine ever turns it into `Activate`).
    fn activate_header<H: ContentLike + PersonLike>(&mut self, cx: &Cx<'_, H>, fx: &mut Effects<'_, H>) {
        self.header_marked = true;
        if self.person(cx).is_some() && self.bio_more() {
            // The GATE is the page's, and stays the page's: the panel exists exactly when the
            // `MORE` mark is drawn, and both read `bio_more` — which depends on this
            // header's own column width. A surface asked to re-derive it would be how the mark and
            // the sheet came to disagree about whether there is more to read.
            fx.push(plx_machine::machine::Fx::App(AppFx::Content(ContentReq::Panel(
                crate::registry::ContentPanel::Bio,
            ))));
            fx.invalidate(Provenance::Input);
        }
    }

    /// **Can the biography sheet be offered at all?** The page's answer about the page's own
    /// person, and the same predicate the `MORE` mark is drawn from — so the mark and the sheet
    /// cannot disagree about whether there is more to read.
    ///
    /// `pub` for one caller, `dev::scenarios`' `/tmp/multiplex-bio`: a headless boot presents
    /// the sheet through the same door the OK press uses, and asking the page first is what stops
    /// the trigger opening a panel an interactive press would have refused. `DetailScreen::
    /// tracks_available` is the precedent and the reason.
    /// The scenario has no frame capability; it reads the header's last measured answer and
    /// waits for the next measure after a store invalidation, just as the painted header does.
    pub fn bio_available(&self, view: plx_data::person::PersonView<'_>) -> bool {
        view.current().is_some_and(|person| plx_plex::plex::same_item(
            (person.sid, person.key.as_str()), (self.sid, self.key.as_str())))
            && self.bio_more()
    }

    /// **Is there more biography than the header shows?** The one predicate the `MORE` mark, the
    /// OK gate and `bio_available` read. It is the answer `header_flow` measured when the header
    /// was last laid out — not a fresh wrap: the set's person-page stack profile (2026-09-19)
    /// caught the draw path re-wrapping the whole bio (`TTF_SizeUTF8` per word) every frame to
    /// ask this. A header awaiting its remeasure answers `false` until it has one.
    fn bio_more(&self) -> bool {
        !self.header_dirty && self.header.bio_truncated
    }

    /// The bio earns its marked/lifted treatment when the header holds focus, is marked open, and
    /// the bio truncates.
    fn bio_marked(&self, focus: Option<u32>) -> bool {
        focus == Some(HEADER_ELEM) && self.header_marked && self.bio_more()
    }

    /// A press committed on card `elem`: leave for its Detail page — a credit a server holds opens
    /// that server's copy (the Filmography route's rule, [`Credit::local`]) — or, for a credit no
    /// server holds, for its catalog page (`ContentArg::Catalog`, the page a Watchlist title no
    /// server holds opens). A credit with no catalog id opens nothing.
    fn open_card<H: ContentLike + PersonLike>(&self, elem: u32, cx: &Cx<'_, H>, fx: &mut Effects<'_, H>) {
        let Some(p) = self.person(cx) else { return };
        let arg = match self.locate(p, elem) {
            Some(Located::Local(col)) => local_item(p, col).map(|m| ContentArg::Detail { sid: m.sid, rk: m.rk.clone() }),
            Some(Located::Credit(row, col)) => self.credits[row].get(col).and_then(|c| match &c.local {
                Some((sid, rk)) => Some(ContentArg::Detail { sid: *sid, rk: rk.clone() }),
                None => plx_data::watchlist::decode_hex(&c.catalog_id).map(|hex| ContentArg::Catalog { hex }),
            }),
            _ => None,
        };
        if let Some(arg) = arg {
            fx.push(plx_machine::machine::Fx::App(AppFx::Content(ContentReq::Push(arg))));
        }
    }

    // ---- draw ----

    fn draw_header(&self, p: Painter, person: &Person, focus_elem: Option<u32>, measure: &dyn plx_machine::machine::Measure) {
        let flow = self.header;
        let d = flow.exp_d;
        let portrait = Rect::new(MARGIN_X, flow.portrait_y, d, d);
        plx_ui::widgets::card(
            p,
            portrait,
            Art::Person {
                sid: person.sid.raw(),
                key: &person.thumb,
                res: PORTRAIT_RES,
            },
            d * 0.5,
            false,
            1.0,
            0.0,
        );

        let col_x_ = col_x(d);
        let truncated = self.bio_more();
        let marked = self.bio_marked(focus_elem);
        let pending = plx_data::person::facts_pending(person);
        // The bio block (plate, shadow and text) is drawn FIRST so its plate sits under the name,
        // roles and life lines above it, as the old fixed highlight did. A resting bio never
        // measures for a plate: the lift block is skipped until the lift has a factor.
        if let Some(by) = flow.bio_y.filter(|_| !pending) {
            let bio = bio_view(&person.bio, 1.0, measure);
            let bh = bio.measure_h(BIO_W);
            let draw_bio = |p: Painter| {
                bio.draw(p, Rect::new(col_x_, by, BIO_W, 0.0));
                if truncated {
                    bio.draw_more(p, col_x_, by, BIO_W, bh, marked);
                }
            };
            if self.bio_lift.factor() > 0.0 {
                let ink = bio.last_line_cap_y(by, bh) - by + measure.cap_h(theme::size::BODY);
                let plate = Rect::new(
                    col_x_ - HL_PAD_X,
                    by - HL_PAD_Y,
                    BIO_W + 2.0 * HL_PAD_X,
                    ink + 2.0 * HL_PAD_Y,
                );
                plx_ui::text_lift::draw_focused(
                    p,
                    plate,
                    plx_ui::widgets::TEXT_BLOCK_HL_RAD,
                    &self.bio_lift,
                    plx_ui::text_lift::CENTRE,
                    draw_bio,
                );
            } else {
                draw_bio(p);
            }
        }

        Label::new(
            self.name_c.as_ptr(),
            theme::size::DISPLAY,
            theme::TEXT_PRIMARY,
        )
        .bold()
        .v(VAlign::CapTop)
        .draw(p, Rect::new(col_x_, flow.name_y, 0.0, 0.0));

        let phase = plx_ui::widgets::skeleton_phase(self.spin_ms as u32);
        // Roles and life-facts both draw through `widgets::dotted_run` — words in
        // `TEXT_SECONDARY`, the `·` in `TEXT_SEPARATOR` — rather than one pre-joined `Label`, so
        // the dot can carry its own ink (design: "Actor · Writer · Producer",
        // "Born … · <birthplace> · Died …"). `theme::space::XS` is the same pad idiom
        // `person_bio.rs`'s identity line and `detail.rs`'s facts row already pass it.
        for (y, parts, sz, w) in [
            (flow.meta_y, &person.roles, theme::size::LABEL, 0.42),
            (flow.life_y, &self.life_parts, theme::size::CAPTION, 0.68),
        ] {
            let Some(y) = y else { continue };
            if pending {
                let h = measure.cap_h(sz);
                plx_ui::widgets::skeleton_bar(p, Rect::new(col_x_, y, BIO_W * w, h), phase);
            } else {
                let refs: Vec<&str> = parts.iter().map(String::as_str).collect();
                let (cap_top, _) = plx_gfx::text::text_cap_band(sz, 0);
                plx_ui::widgets::dotted_run(
                    p,
                    &refs,
                    col_x_,
                    y - cap_top,
                    sz,
                    theme::TEXT_SECONDARY,
                    theme::space::XS,
                );
            }
        }
        if pending {
            if let Some(by) = flow.bio_y {
                let h = measure.cap_h(theme::size::BODY);
                for (i, w) in [1.0, 1.0, 0.58].into_iter().enumerate() {
                    let ly = by + i as f32 * BIO_LEAD;
                    plx_ui::widgets::skeleton_bar(p, Rect::new(col_x_, ly, BIO_W * w, h), phase);
                }
            }
        }
    }

    /// Shelf `slot`'s heading and count ([`Self::row_slot`]), drawn in the shelf block's own space
    /// (`lift` is how far the focused tile makes it rise); the tiles are the stack's.
    fn draw_shelf_heading(&self, p: Painter, slot: usize, lift: f32, measure: &dyn plx_machine::machine::Measure) {
        plx_ui::cards::draw_heading_c(
            p,
            &self.row_title_c[slot],
            &self.row_count_c[slot],
            c"",
            MARGIN_X,
            -lift,
            1.0,
            measure,
        );
    }

    /// The skeleton / empty read-out where the first shelf would be, `y` being that shelf's top.
    fn draw_shelf_state(&self, p: Painter, env: &Env, person: &Person, y: f32) {
        if self.reach.n > 0 {
            return;
        }
        let band = Rect::new(0.0, y, SCR_W, CARD_H);
        if !person.landed || !(person.credited || person.guid.is_empty()) {
            let phase = plx_ui::widgets::skeleton_phase(self.spin_ms as u32);
            plx_ui::widgets::skeleton_bar(
                p,
                Rect::new(MARGIN_X, y + TITLE_DY - 32.0, 214.0, 32.0),
                phase,
            );
            let cy = y + TITLE_DY + CARD_DY;
            for i in 0..5 {
                let cx_ = MARGIN_X + i as f32 * (CARD_W + GAP);
                plx_ui::widgets::skeleton_sheet(
                    p,
                    Rect::new(cx_, cy, CARD_W, CARD_H),
                    theme::CARD_RING_RAD,
                    phase,
                );
            }
            return;
        }
        StatusOverlay::new(
            band,
            plx_platform::i18n::msg::browse_person_empty_c(),
            StatusKind::Empty,
        )
        .draw(env, p);
    }

}

// -------------------------------------------------------------------------------------------
// the sections / Machine / Screen
// -------------------------------------------------------------------------------------------

impl<H: ContentLike + PersonLike> StackPage<H> for Page {
    type Key = Sec;
    type Cards<'a> = ShelfCards<'a>;

    /// Everything `sections` reads: the store's content (its revision, and the shelves the page
    /// derived from it), the header's measure (a remeasure moves `layout_gen`) and whether a person
    /// is on show.
    fn revision(&self, cx: &Cx<'_, H>) -> u64 {
        let flags = self.person(cx).map_or(0, |p| {
            1 | (self.reach.n as u64) << 1 | u64::from(p.credited) << 5 | u64::from(p.landed) << 6
        });
        H::person(cx).revision().wrapping_mul(1_000_003).wrapping_add(self.layout_gen).wrapping_mul(128).wrapping_add(flags)
    }

    /// The remembered card of a Back has not landed yet (or the whole person has not).
    fn pending(&self, cx: &Cx<'_, H>, want: &u32) -> bool {
        if !self.return_pending {
            return false;
        }
        let Some(card) = self.card_for_elem(*want) else { return false };
        match self.person(cx) {
            None => true,
            Some(p) => plx_data::person::media_resolving(p, card.sid),
        }
    }

    fn sections(&self, cx: &Cx<'_, H>, out: &mut Vec<SectionSpec<Sec>>) {
        let person = self.person(cx);
        let rows = if person.is_some() { self.reach.rows() } else { &[] };
        out.push(SectionSpec::new(Sec::Head, Kind::Custom { height: HEADER_TOP + self.header.exp_h, focusable: person.is_some() }, HEADER_GROUP));
        out.push(SectionSpec::new(Sec::State, Kind::Custom { height: 0.0, focusable: false }, STATE_GROUP));
        if !rows.is_empty() {
            out.push(SectionSpec::new(Sec::Gap, Kind::Custom { height: BAND_GAP_TO_SHELF, focusable: false }, GAP_GROUP));
        }
        for &k in rows {
            let group = match k {
                Sec::Credits(r) => CREDIT_GROUP[r],
                _ => LOCAL_GROUP,
            };
            out.push(SectionSpec::new(k, Kind::Shelf { style: &SHELF_STYLE, heading: SHELF_LABEL_H }, group));
        }
    }

    /// Every shelf but the last keeps the label band's air and the section air under it, so the
    /// next shelf stands at Home's pitch; the last ends where its block does (the page rests it one
    /// margin above the bottom edge). The shelves present are read the way `sections` reads them.
    fn shelf_foot(&self, _cx: &Cx<'_, H>, k: Sec) -> f32 {
        match self.reach.rows().last() {
            Some(&last) if last != k && matches!(k, Sec::Local | Sec::Credits(_)) => SHELF_GAP,
            _ => 0.0,
        }
    }

    /// The shelf-gone-under-focus fallback: the first PRESENT shelf's own head, else the header.
    fn fallback(&self, _cx: &Cx<'_, H>, out: &mut Vec<Sec>) {
        out.push(Sec::Local);
        out.extend((0..NCREDIT).map(Sec::Credits));
        out.push(Sec::Head);
    }

    /// Every shelf here is a row of posters: they follow the Under posters setting.
    fn caption_policy(&self, k: Sec) -> Option<plx_ui::caption_policy::CaptionPolicy> {
        matches!(k, Sec::Local | Sec::Credits(_)).then(crate::registry::captions::policy)
    }

    fn cards<'a>(&'a self, cx: &'a Cx<'_, H>, k: Sec) -> Option<ShelfCards<'a>> {
        match k {
            Sec::Local => self.person(cx).map(|person| ShelfCards::Local { person, cards: &self.cards }),
            Sec::Credits(row) => self.person(cx).map(|_| ShelfCards::Credits {
                row,
                credits: &self.credits[row],
                sid: self.sid,
                cards: &self.cards,
            }),
            _ => None,
        }
    }

    fn elem_of(&self, k: Sec) -> Option<u32> {
        match k {
            Sec::Head => Some(HEADER_ELEM),
            _ => None,
        }
    }

    /// Only a title a server holds has an item menu: a credit row's card is plex.tv's.
    fn card_has_menu(&self, cx: &Cx<'_, H>, _k: Sec, elem: &u32) -> bool {
        let key = plx_machine::machine::FocusKey { entry: self.entry, elem: *elem };
        self.focused_item(Some(key), cx).is_some_and(crate::registry::item_has_menu)
    }

    fn focus_rect(&self, _cx: &Cx<'_, H>, k: Sec, section: Rect) -> Rect {
        match k {
            Sec::Head => self.header_rect(),
            _ => section,
        }
    }

    fn plain_group(&self, _cx: &Cx<'_, H>, k: Sec, id: GroupId, extent: Rect) -> GroupSpec {
        let (extent, edge) = match k {
            // the zero-width anchor: DOWN projects to the first shelf tile without making the
            // drawn biography impossible to click
            Sec::Head => (self.header_anchor(), [EdgeRule::Stop, EdgeRule::Geometric, EdgeRule::Stop, EdgeRule::Stop]),
            _ => (extent, [EdgeRule::Geometric, EdgeRule::Stop, EdgeRule::Stop, EdgeRule::Stop]),
        };
        GroupSpec { id, kind: GroupKind::Free, seat: Seat::First, reachable: AxisMask::VERTICAL, edge, extent, len: 1, elem: ElemKind::Bare }
    }

    fn draw_heading(&self, k: Sec, f: &mut DrawFrame<'_, '_, H>, r: Rect, lift: f32) {
        if let Some(slot) = Self::row_slot(k) {
            let p = f.painter.alpha(f.page_alpha).translate(0.0, r.y);
            self.draw_shelf_heading(p, slot, lift, f.measure);
        }
    }

    fn custom_draw(&self, k: Sec, f: &mut DrawFrame<'_, '_, H>, r: Rect, _scroll: f32) {
        let Some(person) = self.person(f.cx) else { return };
        let p = f.painter.alpha(f.page_alpha);
        let cur = f.focus.current.map(|k| k.elem);
        match k {
            Sec::Head => {
                // the focused band is never culled, as the column's own cull has it
                let top = r.y + HEADER_TOP;
                let held = matches!(cur.and_then(|e| self.locate(person, e)), Some(Located::Header));
                if held || plx_ui::on_axis(top, self.header.exp_h, SCR_H, 0.0) {
                    self.draw_header(p.translate(0.0, top), person, cur, f.measure);
                }
            }
            Sec::State => self.draw_shelf_state(p, &Env::inert(), person, r.y + BAND_GAP_TO_SHELF),
            Sec::Gap | Sec::Local | Sec::Credits(_) => {}
        }
    }
}

plx_ui::focusable_via_view!(PersonScreen, H: [ContentLike + PersonLike], view);

impl PersonScreen {
    pub fn focused_item<'a, H: PersonLike>(
        &self,
        focus: Option<plx_machine::machine::FocusKey<u32>>,
        cx: &Cx<'a, H>,
    ) -> Option<&'a PmsMovie> {
        self.page.focused_item(focus, cx)
    }

    pub fn bio_available(&self, view: plx_data::person::PersonView<'_>) -> bool {
        self.page.bio_available(view)
    }
}

impl<H: ContentLike + PersonLike> Machine<H> for PersonScreen {
    type Ev = ScreenEvent<H>;
    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, H>, fx: &mut Effects<'_, H>) -> Handled {
        // The content the stack reads is current before it reads it for this event.
        // The rows are derived from the store at its revision (`refresh_store_cache` returns at
        // once when that has not moved), so any event, a remount's first included, lays the stack
        // over the rows the store holds now.
        let mut live = false;
        match ev {
            ScreenEvent::Tick(t) => live = self.page.pre_tick(t.dt(), cx),
            ScreenEvent::StoreChanged(ord, _) if *ord == plx_data::stores::StoreId::Person.ord() => {
                self.page.header_dirty = true;
                self.page.refresh_store_cache(cx);
            }
            _ => self.page.refresh_store_cache(cx),
        }
        let card = match self.stack.on(&self.page, ev, cx, fx) {
            Some(StackEvent::Card(_, card)) => Some(card),
            _ => None,
        };
        match ev {
            ScreenEvent::RestoreMemory(PageMemory::Person(memory)) => {
                self.page.restore(memory);
                self.page.return_pending = true;
                Handled::Yes
            }
            ScreenEvent::Tick(t) => {
                if live {
                    self.page.post_tick(*t, cx, fx);
                }
                Handled::Yes
            }
            ScreenEvent::Enter(_) | ScreenEvent::Uncover => {
                // Unconditional, not gated on `self.person(cx).is_none()` (review round 1, P1):
                // `Close` and `Open` are both deferred `AppFx::Store` effects applied AFTER this
                // event is delivered, so a same-identity `Close` queued by an evicted/left body
                // earlier in this SAME drain (e.g. a stack eviction's `Unmount`, or a `PopTo`
                // that leaves a same-identity page) can land after this `Enter`/`Uncover` — and a
                // guard reading the store NOW would see the about-to-be-closed identity and skip
                // `Open`, leaving the store empty under a page with no pending re-request.
                // `person::open` is idempotent on the identity it already holds (an early return
                // before any generation bump / fetch-claim clear / rebuild — see its doc), so
                // requesting it every time is a genuine no-op whenever the store is already
                // correctly settled on `(self.sid, self.key)`, and issues the real request
                // whenever it is not.
                self.page.request_store(fx);
                fx.invalidate(Provenance::Nav);
                self.page.settle_return_pending(cx);
                Handled::Yes
            }
            ScreenEvent::FocusMoved { to, by, .. } => {
                let page = &mut self.page;
                if matches!(by, By::Dir | By::Pointer)
                    || page.person(cx).is_some_and(|person| page.locate(person, to.elem).is_some())
                {
                    page.return_pending = false;
                }
                if matches!(page.person(cx).and_then(|p| page.locate(p, to.elem)), Some(Located::Header))
                    && matches!(by, By::Dir | By::Pointer)
                {
                    page.header_marked = true;
                }
                fx.invalidate(Provenance::Input);
                Handled::Yes
            }
            ScreenEvent::Activate(e) => {
                if let Some(Located::Header) = self.page.person(cx).and_then(|p| self.page.locate(p, *e)) {
                    self.page.activate_header(cx, fx);
                }
                Handled::Yes
            }
            ScreenEvent::PressCommit(_) => {
                if let Some(CardEvent::Activate(elem)) = card {
                    self.page.open_card(elem, cx, fx);
                }
                Handled::Yes
            }
            ScreenEvent::PressHold(_) => {
                if let Some(CardEvent::Hold(_)) = card {
                    fx.push(plx_machine::machine::Fx::App(AppFx::Content(
                        ContentReq::ItemMenu,
                    )));
                    return Handled::Yes;
                }
                Handled::No
            }
            ScreenEvent::Input(InputEvent {
                kind:
                    InputKind::Key {
                        key,
                        edge: Edge::Down,
                        ..
                    },
                ..
            }) => {
                if matches!(key, Key::Up | Key::Down | Key::Left | Key::Right) {
                    self.page.return_pending = false;
                }
                if *key == Key::Back {
                    fx.push(plx_machine::machine::Fx::App(AppFx::Content(
                        ContentReq::Back,
                    )));
                    return Handled::Yes;
                }
                Handled::No
            }
            ScreenEvent::Input(InputEvent {
                kind: InputKind::Click { .. },
                ..
            }) => {
                self.page.return_pending = false;
                Handled::No
            }
            ScreenEvent::Input(InputEvent {
                kind: InputKind::Pointer { .. },
                ..
            }) => {
                Handled::No
            }
            ScreenEvent::StoreChanged(ord, _) if *ord == plx_data::stores::StoreId::Person.ord() => {
                self.page.settle_return_pending(cx);
                Handled::Yes
            }
            ScreenEvent::WillLeave(Leave::ForGood) | ScreenEvent::Unmount => {
                // Cancel this body's Open even while the store is empty. The bridge keeps its
                // sender address, so a retired page cannot close a replacement. One `Close`, even
                // though §3.4 delivers this arm twice and the queued command has not run yet.
                if !self.teardown_closed {
                    self.teardown_closed = true;
                    fx.push(plx_machine::machine::Fx::App(AppFx::Store(
                        plx_data::stores::StoreId::Person,
                        plx_data::stores::StoreCmd::Person(PersonCmd::Close),
                    )));
                }
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
}

impl<H: ContentLike + PersonLike> Screen<H> for PersonScreen {
    fn focused_card<'a>(&self, cx: &Cx<'a, H>, focus: Option<plx_machine::machine::FocusKey<u32>>, at: Option<At>) -> Option<FocusedCard<'a>> {
        let item = self.focused_item(focus, cx)?;
        let placed = focus.zip(at).and_then(|(key, at)| Focusable::<H>::place(self, &key.elem, cx, at));
        Some(FocusedCard::new(item, placed))
    }
    fn redraw_focused(&self, f: &mut DrawFrame<'_, '_, H>, focus: Option<plx_machine::machine::FocusKey<u32>>) {
        self.stack.view(&self.page).redraw_focused(f, focus);
    }
    fn glides(&self) -> bool { true }
    fn name(&self) -> &'static str {
        // Filmography deliberately returns the same word: the manifest records that opaque modal
        // as the Person route with a separate `filmography=1` state bit.
        super::registry::word::PERSON
    }
    fn state(&self) -> &dyn LogicalState {
        self
    }
    fn crumb(&self, _cx: &Cx<'_, H>) -> Option<std::borrow::Cow<'_, str>> {
        None
    }
    fn prepare(&mut self, _b: &mut plx_ui::frame::Budget, _cx: &Cx<'_, H>) {}
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, H>) {
        let p = f.painter.alpha(f.page_alpha);
        if self.page.person(f.cx).is_none() {
            return;
        }
        self.page.amb.draw(p, Rect::FULL);
        // Stops register in z order, section by section in document order: the header band, then
        // each shelf's tiles (a shelf wholly off the screen registers none).
        self.stack.view(&self.page).paint(f);
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
    fn covered_surfaces_ready(&self) -> bool {
        self.page.covered_ready_c
    }
    fn links(&self, out: &mut Vec<Link>) {
        let reach = self.page.reach;
        let rows = reach.rows();
        let mut link = |from: Sec, dir: Dir, to: Sec| {
            if let (Some(from), Some(to)) = (self.stack.group(from), self.stack.group(to)) {
                out.push(Link { from, dir, to });
            }
        };
        if let Some(&first) = rows.first() {
            link(Sec::Head, Dir::Down, first);
            link(first, Dir::Up, Sec::Head);
        }
        for pair in rows.windows(2) {
            link(pair[0], Dir::Down, pair[1]);
            link(pair[1], Dir::Up, pair[0]);
        }
    }
    fn memory_at(&self, _focus: Option<plx_machine::machine::FocusKey<u32>>) -> PageMemory {
        PageMemory::Person(self.page.memory())
    }
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plx_ui::fixture::FixtureMeasure;
    use plx_ui::cards as ui_cards;
    use plx_ui::cards::StackMemory;
    use plx_ui::screen::Step;
    use plx_machine::machine::{FocusRead, Host, InputOwner, PressRead, Tick};

    struct PersonHost;

    impl Host for PersonHost {
        type Arg = super::super::family::SettingsPage;
        type Fx = AppFx;
        type Msg = super::super::registry::AppMsg;
        type Elem = u32;
        type Views<'a> = plx_data::person::PersonView<'a>;
        type Init = super::super::family::NoInit;
        type Memory = PageMemory;
    }

    impl PersonLike for PersonHost {
        fn person<'a>(cx: &Cx<'a, Self>) -> plx_data::person::PersonView<'a> { cx.views }
    }

    fn item(rk: &str) -> PmsMovie {
        item_on(ServerId::UNSET, rk)
    }

    fn item_on(sid: ServerId, rk: &str) -> PmsMovie {
        PmsMovie {
            sid,
            rk: rk.to_string(),
            ..Default::default()
        }
    }

    fn cx<'a>(m: &'a dyn Measure, person: plx_data::person::PersonView<'a>) -> Cx<'a, PersonHost> {
        Cx {
            views: person,
            tick: Tick::default(),
            measure: m,
            press: PressRead::default(),
            focus: FocusRead::default(),
            owner: InputOwner::Entry(EntryId(0)),
        }
    }

    fn cx_at<'a>(m: &'a FixtureMeasure, person: plx_data::person::PersonView<'a>, focus: plx_machine::machine::FocusKey<u32>) -> Cx<'a, PersonHost> {
        Cx {
            focus: FocusRead {
                current: Some(focus),
            ..Default::default() },
            ..cx(m, person)
        }
    }

    /// Mounts through [`PersonScreen::new`] (which issues the real `PersonCmd::Open`) and then
    /// seeds the shelves the way a landing does — `install_for_test` ALSO settles `credited =
    /// true`, which is why every test below that wants a genuinely PENDING entry row reasons about
    /// [`entry_reachable_of`] directly instead (see that function's own doc for why no test seam
    /// can force `credited` back to `false` on a live `Person` from outside `plx_data::person`).
    fn seed(movies: usize, shows: usize) -> (plx_data::stores::person::PersonStore, PersonScreen) {
        let mut store = plx_data::stores::person::PersonStore::default();
        crate::registry::run_person_fixture(&mut store, PersonCmd::Open {
            sid: ServerId::UNSET,
            key: "161".into(),
            guid: "5d77682aeb5d26001f1de4b0".into(),
            name: "Idina Menzel".into(),
            thumb: String::new(),
        });
        let mut s = PersonScreen::new(
            EntryId(0),
            ServerId::UNSET,
            "161".to_string(),
            "5d77682aeb5d26001f1de4b0".to_string(),
            "Idina Menzel".to_string(),
            String::new(),
        );
        store.install_for_test(
            (0..movies).map(|i| item(&format!("m{i}"))).collect(),
            (0..shows).map(|i| item(&format!("s{i}"))).collect(),
        );
        s.page.refresh_store_cache(&cx(&FixtureMeasure, store.view()));
        lay(&mut s, &store, &FixtureMeasure);
        (store, s)
    }

    /// The focus key of card `col` of the person's `kind` titles (0 Movies, 1 Shows) on the one
    /// Available on MultiPlex shelf, where the Shows follow the Movies.
    fn focus_of(s: &PersonScreen, store: &plx_data::stores::person::PersonStore, kind: usize, col: usize) -> plx_machine::machine::FocusKey<u32> {
        let p = store.view().current().unwrap();
        let at = (0..kind).map(|k| p.shelf(k).len()).sum::<usize>() + col;
        s.page.local_key(p, at)
    }

    /// The focus key of card `col` of credit row `row`.
    fn credit_of(s: &PersonScreen, row: CreditRow, col: usize) -> plx_machine::machine::FocusKey<u32> {
        s.page.credit_card_key(row, col)
    }

    /// Install `groups` as plex.tv's credits and let the page and its stack take them in.
    fn credit(s: &mut PersonScreen, store: &mut plx_data::stores::person::PersonStore, groups: &[(&str, usize)]) {
        store.install_credits_for_test(groups);
        s.page.refresh_store_cache(&cx(&FixtureMeasure, store.view()));
        lay(s, store, &FixtureMeasure);
    }

    /// Feed `ev` to the screen with the engine focus on `focus`.
    fn feed(
        s: &mut PersonScreen,
        store: &plx_data::stores::person::PersonStore,
        m: &dyn Measure,
        focus: Option<plx_machine::machine::FocusKey<u32>>,
        ev: ScreenEvent<PersonHost>,
    ) {
        let mut present = plx_machine::present::Present::new();
        let mut buf: Vec<plx_machine::machine::Stamped<PersonHost>> = Vec::new();
        let cxv = Cx { focus: FocusRead { current: focus, ..Default::default() }, ..cx(m, store.view()) };
        let mut fx = Effects::new(
            &mut buf,
            plx_machine::machine::MachineId::Instance(plx_machine::machine::InstanceId(0)),
            &mut present,
        );
        Machine::<PersonHost>::step(s, &ev, &cxv, &mut fx);
    }

    /// Let the stack lay its sections out over what the page now holds (the stack lays out on the
    /// first event it sees, and again on the next one after the page's content moved).
    fn lay(s: &mut PersonScreen, store: &plx_data::stores::person::PersonStore, m: &dyn Measure) {
        feed(s, store, m, None, ScreenEvent::Cover);
    }

    /// Tick the page `frames` times with the engine focus on `focus`, so a focused tile has grown
    /// to its settled pop and the page has scrolled to its target.
    fn settle_shelves(
        s: &mut PersonScreen,
        store: &plx_data::stores::person::PersonStore,
        focus: plx_machine::machine::FocusKey<u32>,
        frames: u32,
    ) {
        for _ in 0..frames {
            feed(s, store, &FixtureMeasure, Some(focus), ScreenEvent::Tick(Tick { ms: 0, dt_us: 16_667 }));
        }
    }

    /// Every stop the page registers, in z order — what `Screen::draw` registers without painting.
    fn page_stops(s: &PersonScreen, f: &mut DrawFrame<'_, '_, PersonHost>) {
        s.stack.view(&s.page).record_stops(f);
    }

    /// Put the page's scroll at `scroll`, without gliding there.
    fn jump_to(s: &mut PersonScreen, scroll: f32) {
        s.stack.restore(&StackMemory { scroll, shelves: Vec::new() });
    }

    /// The hold hint (`ui::hold_hint`) on a Person shelf card: it stands after the dwell on a
    /// settled card, not on the header (not a card), not while a menu owns focus, and
    /// not a second time this run.
    #[test]
    fn the_hold_hint_stands_on_a_resting_shelf_card_once_per_install() {
        let _g = plx_base::testlock::serial();
        plx_ui::hold_hint::reset_learned_for_test();
        plx_ui::hold_hint::reset_shown_for_test();
        let (store, mut s) = seed(6, 0);
        let card = focus_of(&s, &store, 0, 1);
        let mut ms = 100;
        let mut rest = |s: &mut PersonScreen, focus: plx_machine::machine::FocusKey<u32>, secs: f32| {
            for _ in 0..(secs * 60.0) as u32 {
                ms += 17;
                feed(s, &store, &FixtureMeasure, Some(focus), ScreenEvent::Tick(Tick { ms, dt_us: 16_667 }));
            }
        };
        rest(&mut s, card, 1.2);
        assert!(!s.stack.hint_visible(), "not before the dwell");
        // negatives first: a stand spends the run's latch and would make them vacuous
        rest(&mut s, plx_machine::machine::FocusKey { entry: EntryId(900), elem: 0 }, 4.0);
        assert!(!s.stack.hint_visible() && !plx_ui::hold_hint::shown_this_run(), "never while a menu owns focus");
        rest(&mut s, plx_machine::machine::FocusKey { entry: EntryId(0), elem: HEADER_ELEM }, 4.0);
        assert!(!s.stack.hint_visible() && !plx_ui::hold_hint::shown_this_run(), "never on the header");
        rest(&mut s, card, 3.6);
        assert!(s.stack.hint_visible(), "after the dwell, on a settled card");
        rest(&mut s, plx_machine::machine::FocusKey { entry: EntryId(900), elem: 0 }, 1.0);
        assert!(!s.stack.hint_visible(), "a menu taking focus hides it");
        rest(&mut s, card, 4.0);
        assert!(!s.stack.hint_visible(), "and not a second time on a Person page this run");
    }

    #[test]
    #[cfg(feature = "devtriggers")]
    fn populated_person_geometry_uses_recorded_metrics() {
        let _serial = plx_base::testlock::serial();
        // Armed in a private trigger root: the shared runtime root is the bare `/tmp` on a host test
        // run, so the old `assert!(!path.exists(), "needs an isolated runtime root")` was only as
        // true as the other processes on the machine were quiet.
        let (mut store, mut s) = plx_base::devtrig::with_private_triggers(|| {
            let path = plx_base::devtrig::path("personbio");
            std::fs::write(&path, "A populated biography whose words must pass through the recorded measurement capability. ".repeat(60)).unwrap();
            let (mut store, s) = seed(3, 2);
            store.install_credits_for_test(&[("Actor", 9)]);
            (store, s)
        });
        assert!(!store.view().current().unwrap().bio.is_empty());
        plx_ui::rec::assert_measured_geometry(|measure| {
            s.page.remeasure_header(store.view().current().unwrap(), measure);
            lay(&mut s, &store, measure);
            let context = cx(measure, store.view());
            let mut groups = Vec::new();
            Focusable::<PersonHost>::groups(&s, &context, &mut groups);
            assert_eq!(groups.len(), 3, "the header, Available on MultiPlex and Acting");
            let mut bits = Vec::new();
            for g in groups {
                bits.extend([g.extent.x, g.extent.y, g.extent.w, g.extent.h].map(f32::to_bits));
            }
            for key in [HEADER_ELEM, focus_of(&s, &store, 0, 0).elem, focus_of(&s, &store, 1, 0).elem, credit_of(&s, CreditRow::Acting, 0).elem] {
                for at in [At::Drawn, At::SpringTarget] {
                    let p = Focusable::<PersonHost>::place(&s, &key, &context, at).unwrap();
                    bits.extend([p.rect.x, p.rect.y, p.rect.w, p.rect.h].map(f32::to_bits));
                }
            }
            bits
        });
        store.run(PersonCmd::Close);
    }

    /// **The frozen-animator regression class, closed for the header skeleton's spinner (phase 12
    /// D4).** `spin_ms` used to be a raw `+= dt` accumulator with a separate, easy-to-forget
    /// `fx.note(Motion)` a few lines below it. Now it is `motion::Phase`, which reports from
    /// inside its own `advance`. A screen fresh off `PersonScreen::new` (no `install_for_test`,
    /// unlike `seed`) has `person()` answering `Some` with `profiled`/`profile_tried` both false —
    /// `facts_pending` — which is exactly the header-skeleton state the spinner animates, so this
    /// drives it through the real `Machine::step` `Tick` path.
    #[test]
    fn the_header_skeleton_spinner_reports_motion_while_facts_are_pending() {
        let _serial = plx_base::testlock::serial();
        let mut store = plx_data::stores::person::PersonStore::default();
        crate::registry::run_person_fixture(&mut store, PersonCmd::Open {
            sid: ServerId::UNSET, key: "161".into(),
            guid: "5d77682aeb5d26001f1de4b0".into(), name: "Idina Menzel".into(),
            thumb: String::new(),
        });
        let mut s = PersonScreen::new(
            EntryId(0),
            ServerId::UNSET,
            "161".to_string(),
            "5d77682aeb5d26001f1de4b0".to_string(),
            "Idina Menzel".to_string(),
            String::new(),
        );
        let p = store.view().current().expect("Open seeds a pending Person synchronously");
        assert!(
            plx_data::person::facts_pending(p),
            "a fresh mount must start pending, or this test is not exercising the skeleton clock"
        );
        let m = FixtureMeasure;
        let cxv = cx(&m, store.view());
        let mut present = plx_machine::present::Present::new();
        let _ = present.take(0);
        let mut buf: Vec<plx_machine::machine::Stamped<PersonHost>> = Vec::new();
        for ms in [16, 32, 48] {
            let mut fx = Effects::new(
                &mut buf,
                plx_machine::machine::MachineId::Instance(plx_machine::machine::InstanceId(0)),
                &mut present,
            );
            let ev = ScreenEvent::Tick(Tick { ms, dt_us: 16_667 });
            Machine::<PersonHost>::step(&mut s, &ev, &cxv, &mut fx);
            assert!(
                present.take(ms),
                "a pending header skeleton must present every frame it is on screen (ms={ms})"
            );
        }
        store.run(PersonCmd::Close);
    }

    #[test]
    fn enter_and_leave_emit_addressed_person_store_commands() {
        let mut screen = PersonScreen::new(
            EntryId(0), ServerId::from_raw(2), "161".into(), "person-guid".into(),
            "Person Name".into(), "thumb".into());
        let measure = FixtureMeasure;
        let mut present = plx_machine::present::Present::new();
        let mut out = Vec::new();
        {
            let context = cx(&measure, plx_data::person::PersonView::default());
            let mut fx = Effects::new(&mut out, plx_machine::machine::MachineId::Instance(
                plx_machine::machine::InstanceId(0)), &mut present);
            Machine::<PersonHost>::step(
                &mut screen, &ScreenEvent::Enter(Enter::Restored), &context, &mut fx);
        }
        assert!(matches!(&out[0].fx,
            plx_machine::machine::Fx::App(AppFx::Store(plx_data::stores::StoreId::Person,
                plx_data::stores::StoreCmd::Person(PersonCmd::Open { sid, key, guid, name, thumb })))
                if *sid == ServerId::from_raw(2) && key == "161" && guid == "person-guid"
                    && name == "Person Name" && thumb == "thumb"));

        let mut store = plx_data::stores::person::PersonStore::default();
        crate::registry::run_person_fixture(&mut store, PersonCmd::Open { sid: ServerId::from_raw(2), key: "161".into(),
            guid: "person-guid".into(), name: "Person Name".into(), thumb: "thumb".into() });
        out.clear();
        {
            let context = cx(&measure, store.view());
            let mut fx = Effects::new(&mut out, plx_machine::machine::MachineId::Instance(
                plx_machine::machine::InstanceId(0)), &mut present);
            Machine::<PersonHost>::step(
                &mut screen, &ScreenEvent::WillLeave(Leave::ForGood), &context, &mut fx);
        }
        assert!(matches!(&out[0].fx,
            plx_machine::machine::Fx::App(AppFx::Store(plx_data::stores::StoreId::Person,
                plx_data::stores::StoreCmd::Person(PersonCmd::Close)))));
        assert!(store.view().current().is_some(),
            "the screen emits; only the addressed Bridge is allowed to apply the command");
    }

    /// §3.4's `pop_sequence` delivers `WillLeave(ForGood)` AND `Unmount` to the same body before
    /// either event's queued effects have run (`AppFx::Store` is deferred to the addressed
    /// Bridge). Mirrors the `#119`/`teardown_cleared` regression in `screens/detail/mod.rs`: an
    /// un-latched `self.person(cx).is_some()` guard reads the store as still populated on the
    /// second event and emits a SECOND non-idempotent `Close` (#126).
    #[test]
    fn teardown_closes_the_person_store_exactly_once() {
        let mut screen = PersonScreen::new(
            EntryId(0), ServerId::from_raw(2), "161".into(), "person-guid".into(),
            "Person Name".into(), "thumb".into());
        let measure = FixtureMeasure;
        let mut store = plx_data::stores::person::PersonStore::default();
        crate::registry::run_person_fixture(&mut store, PersonCmd::Open { sid: ServerId::from_raw(2), key: "161".into(),
            guid: "person-guid".into(), name: "Person Name".into(), thumb: "thumb".into() });
        let mut present = plx_machine::present::Present::new();
        let mut out = Vec::new();
        let context = cx(&measure, store.view());
        {
            let mut fx = Effects::new(&mut out, plx_machine::machine::MachineId::Instance(
                plx_machine::machine::InstanceId(0)), &mut present);
            Machine::<PersonHost>::step(
                &mut screen, &ScreenEvent::WillLeave(Leave::ForGood), &context, &mut fx);
            // The queued Close has not been applied to `store` yet — it is still populated when
            // Unmount arrives, exactly as `pop_sequence` delivers it.
            Machine::<PersonHost>::step(
                &mut screen, &ScreenEvent::Unmount, &context, &mut fx);
        }
        let closes = out.iter().filter(|s| matches!(&s.fx,
            plx_machine::machine::Fx::App(AppFx::Store(plx_data::stores::StoreId::Person,
                plx_data::stores::StoreCmd::Person(PersonCmd::Close))))).count();
        assert_eq!(closes, 1,
            "WillLeave(ForGood) then Unmount must close the Person store exactly once, not twice");
    }

    /// A bare `Unmount` (no preceding `WillLeave`) must also close the store exactly once — the
    /// latch must not suppress the only teardown event when there is no pair.
    #[test]
    fn unmount_alone_closes_the_person_store_once() {
        let mut screen = PersonScreen::new(
            EntryId(0), ServerId::from_raw(2), "161".into(), "person-guid".into(),
            "Person Name".into(), "thumb".into());
        let measure = FixtureMeasure;
        let mut store = plx_data::stores::person::PersonStore::default();
        crate::registry::run_person_fixture(&mut store, PersonCmd::Open { sid: ServerId::from_raw(2), key: "161".into(),
            guid: "person-guid".into(), name: "Person Name".into(), thumb: "thumb".into() });
        let mut present = plx_machine::present::Present::new();
        let mut out = Vec::new();
        let context = cx(&measure, store.view());
        {
            let mut fx = Effects::new(&mut out, plx_machine::machine::MachineId::Instance(
                plx_machine::machine::InstanceId(0)), &mut present);
            Machine::<PersonHost>::step(
                &mut screen, &ScreenEvent::Unmount, &context, &mut fx);
        }
        let closes = out.iter().filter(|s| matches!(&s.fx,
            plx_machine::machine::Fx::App(AppFx::Store(plx_data::stores::StoreId::Person,
                plx_data::stores::StoreCmd::Person(PersonCmd::Close))))).count();
        assert_eq!(closes, 1, "a bare Unmount must close the Person store exactly once");
    }

    #[test]
    fn empty_person_teardown_cancels_an_undelivered_open_exactly_once() {
        for bare_unmount in [false, true] {
            let mut screen = PersonScreen::new(EntryId(0), ServerId::UNSET, "pending".into(),
                String::new(), "Pending".into(), String::new());
            let measure = FixtureMeasure;
            let context = cx(&measure, plx_data::person::PersonView::default());
            let mut present = plx_machine::present::Present::new();
            let mut out = Vec::new();
            {
                let mut fx = Effects::new(&mut out, plx_machine::machine::MachineId::Instance(
                    plx_machine::machine::InstanceId(0)), &mut present);
                if !bare_unmount {
                    Machine::<PersonHost>::step(&mut screen, &ScreenEvent::WillLeave(Leave::ForGood), &context, &mut fx);
                }
                Machine::<PersonHost>::step(&mut screen, &ScreenEvent::Unmount, &context, &mut fx);
                Machine::<PersonHost>::step(&mut screen, &ScreenEvent::Unmount, &context, &mut fx);
            }
            let closes = out.iter().filter(|s| matches!(&s.fx,
                plx_machine::machine::Fx::App(AppFx::Store(plx_data::stores::StoreId::Person,
                    plx_data::stores::StoreCmd::Person(PersonCmd::Close))))).count();
            assert_eq!(closes, 1, "an empty store may still owe this page an Open");
        }
    }

    /// **`focused_item`/`focused_target` split** — mining `ui/person.rs`'s own regression: the
    /// header holds no card at all, so a shelf tile's identity is the only thing OK
    /// should ever navigate to.
    #[test]
    fn focused_movie_answers_only_for_a_shelf_row() {
        let _serial = plx_base::testlock::serial();
        let (mut store, mut s) = seed(2, 0);
        assert!(s
            .focused_item(Some(plx_machine::machine::FocusKey {
                entry: s.page.entry,
                elem: HEADER_ELEM
            }), &cx(&FixtureMeasure, store.view()))
            .is_none());
        credit(&mut s, &mut store, &[("Actor", 2)]);
        assert!(
            s.focused_item(Some(credit_of(&s, CreditRow::Acting, 0)), &cx(&FixtureMeasure, store.view())).is_none(),
            "a credit row's card is plex.tv's: no server's item"
        );
        assert_eq!(
            s.focused_item(Some(focus_of(&s, &store, 0, 0)), &cx(&FixtureMeasure, store.view()))
                .map(|m| m.rk.as_str()),
            Some("m0")
        );
        assert_eq!(
            s.focused_item(Some(focus_of(&s, &store, 0, 1)), &cx(&FixtureMeasure, store.view()))
                .map(|m| m.rk.as_str()),
            Some("m1")
        );
        let _ = &mut s;
        store.run(PersonCmd::Close);
    }

    /// A shelf that has vanished under the focus re-seats by IDENTITY first (the merge-redivide
    /// case), and only falls back to a plain index clamp when no identity is held — the two-step
    /// order `reconcile` must keep (module doc's worked trace).
    #[test]
    fn reconcile_reseats_by_identity_before_falling_back_to_index_clamp() {
        let _serial = plx_base::testlock::serial();
        let (mut store, mut s) = seed(4, 0);
        let want = focus_of(&s, &store, 0, 2);
        // the row is rebuilt with two items inserted ahead — "m2" is now at index 4
        let rebuilt: Vec<PmsMovie> = ["x0", "x1", "m0", "m1", "m2", "m3"]
            .iter()
            .map(|rk| item(rk))
            .collect();
        store.install_for_test(rebuilt, Vec::new());
        let m = FixtureMeasure;
        let got = Focusable::<PersonHost>::reconcile(&s, want, &cx(&m, store.view()));
        assert_eq!(
            got, want,
            "the engine key follows the card without a screen-owned cursor"
        );
        assert_eq!(
            s.page.locate(store.view().current().unwrap(), got.elem),
            Some(Located::Local(4))
        );

        // the identity is gone entirely: falls back to clamping the SAME kind's own bounds
        store.install_for_test(vec![item("only")], Vec::new());
        let got = Focusable::<PersonHost>::reconcile(&s, want, &cx(&m, store.view()));
        assert_eq!(got, focus_of(&s, &store, 0, 0));
        let _ = &mut s;
        store.run(PersonCmd::Close);
    }

    /// A shelf that disappears ENTIRELY (not merely shrinks) hands focus to the other present
    /// shelf, never leaving it pointed at a kind with nothing in it.
    #[test]
    fn a_vanished_shelf_kind_falls_back_to_the_other_present_shelf() {
        let _serial = plx_base::testlock::serial();
        let (mut store, mut s) = seed(2, 3);
        let want = focus_of(&s, &store, 1, 2);
        store.install_for_test(vec![item("m0")], Vec::new()); // shows vanished
        let m = FixtureMeasure;
        let got = Focusable::<PersonHost>::reconcile(&s, want, &cx(&m, store.view()));
        assert_eq!(
            got,
            focus_of(&s, &store, 0, 0),
            "movies is the only present shelf left"
        );
        let _ = &mut s;
        store.run(PersonCmd::Close);
    }

    /// **Header pending vs answered-with-nothing vs answered-fully** — `header_flow`'s three
    /// states, mined from `ui/person.rs`'s own module doc ("Every header line below the name is
    /// optional… a header line is drawn only when it has content").
    #[test]
    fn header_flow_distinguishes_pending_from_answered_empty_from_answered_full() {
        let m = FixtureMeasure;
        // answered, with nothing: no reserved space for the meta/life/bio lines at all.
        let flow = header_flow(false, "", false, false, &m);
        assert!(flow.meta_y.is_none() && flow.life_y.is_none() && flow.bio_y.is_none());

        // pending: not yet asked at all — reserves the full placeholder stack even though every
        // cached run is still empty.
        let pending_flow = header_flow(true, "", false, false, &m);
        assert!(
            pending_flow.meta_y.is_some()
                && pending_flow.life_y.is_some()
                && pending_flow.bio_y.is_some()
        );
        assert!(
            pending_flow.exp_h > flow.exp_h,
            "the placeholder reserves real room, not the bare header's"
        );

        // answered, WITH content: the runs actually drive the reserved space.
        let full_flow = header_flow(false, "", true, false, &m);
        assert!(full_flow.meta_y.is_some());
        assert!(full_flow.life_y.is_none(), "no life facts were given");
    }

    /// **The `MORE` gate is the header's measured answer, not a per-frame wrap.** The set's
    /// person-page stack profile (2026-09-19) caught `draw_header` asking `bio_is_truncated` —
    /// a fresh `TextView` wrap of the whole bio, `TTF_SizeUTF8` per word — on every frame. The
    /// mark, OK's gate and `bio_available` now all read `bio_more`, which is `header_flow`'s
    /// `bio.truncates(BIO_W)` from the last remeasure, and it takes no measure capability at all.
    #[test]
    fn the_more_gate_reads_the_header_measure_not_a_fresh_wrap() {
        let _serial = plx_base::testlock::serial();
        let (mut store, mut s) = seed(1, 0);
        let m = FixtureMeasure;
        let long = "A biography far longer than the header's three lines can hold. ".repeat(40);
        for (bio, want) in [(long.as_str(), true), ("One short line.", false)] {
            let flow = header_flow(false, bio, false, false, &m);
            assert_eq!(flow.bio_truncated, bio_view(bio, 1.0, &m).truncates(BIO_W));
            assert_eq!(flow.bio_truncated, want, "fixture sanity for {:.20}", bio);
            s.page.header = flow;
            s.page.header_dirty = false;
            assert_eq!(s.page.bio_more(), want);
        }
        s.page.header = header_flow(false, &long, false, false, &m);
        s.page.header_dirty = true;
        assert!(!s.page.bio_more(), "a header awaiting its remeasure offers no panel yet");
        store.run(PersonCmd::Close);
    }

    /// A page with NO shelves at all holds the header alone, and OK on it does nothing to the
    /// (nonexistent) catalog focus — `focused_movie` must answer `None` throughout.
    #[test]
    fn a_page_with_no_shelves_answers_no_focused_movie_anywhere() {
        let _serial = plx_base::testlock::serial();
        let (mut store, s) = seed(0, 0);
        assert!(s
            .focused_item(Some(plx_machine::machine::FocusKey {
                entry: s.page.entry,
                elem: HEADER_ELEM
            }), &cx(&FixtureMeasure, store.view()))
            .is_none());
        assert!(s
            .focused_item(Some(plx_machine::machine::FocusKey {
                entry: s.page.entry,
                elem: FIRST_CARD_ELEM
            }), &cx(&FixtureMeasure, store.view()))
            .is_none());
        let _ = &s;
        store.run(PersonCmd::Close);
    }

    /// `PressCommit` leaves immediately through the content contract; there is no pending latch.
    #[test]
    fn press_commit_on_a_card_pushes_detail_as_an_effect() {
        let _serial = plx_base::testlock::serial();
        let (mut store, mut s) = seed(2, 0);
        let m = FixtureMeasure;
        let cxv = cx(&m, store.view());
        let mut present = plx_machine::present::Present::new();
        let mut buf: Vec<plx_machine::machine::Stamped<PersonHost>> = Vec::new();
        {
            let mut fx = Effects::new(
                &mut buf,
                plx_machine::machine::MachineId::Instance(plx_machine::machine::InstanceId(0)),
                &mut present,
            );
            Machine::<PersonHost>::step(&mut s, &ScreenEvent::PressCommit(plx_machine::machine::PressId(1)), &cxv, &mut fx);
        }
        assert!(buf.is_empty(), "no focus means no navigation effect");
        // Feed the identical `Cx` shape but with focus parked on the first movie tile.
        let cxv2 = Cx {
            views: store.view(),
            tick: Tick::default(),
            measure: &m,
            press: PressRead::default(),
            focus: FocusRead {
                current: Some(focus_of(&s, &store, 0, 0)),
            ..Default::default() },
            owner: InputOwner::Entry(EntryId(0)),
        };
        {
            let mut fx = Effects::new(
                &mut buf,
                plx_machine::machine::MachineId::Instance(plx_machine::machine::InstanceId(0)),
                &mut present,
            );
            Machine::<PersonHost>::step(&mut s, &ScreenEvent::PressCommit(plx_machine::machine::PressId(1)), &cxv2, &mut fx);
        }
        assert!(buf.iter().any(|st| matches!(
            &st.fx,
            plx_machine::machine::Fx::App(AppFx::Content(ContentReq::Push(ContentArg::Detail { sid, rk })))
                if *sid == ServerId::UNSET && rk == "m0"
        )));
        store.run(PersonCmd::Close);
    }

    /// An explicit D-pad/pointer arrival on the header marks it (the bio truncation mark may show
    /// from then on); a landing that merely leaves the header as the resolved position (`Restore`)
    /// must not.
    #[test]
    fn focus_moved_marks_the_header_only_on_an_explicit_arrival() {
        let _serial = plx_base::testlock::serial();
        let (mut store, mut s) = seed(1, 0);
        let header_key = plx_machine::machine::FocusKey {
            entry: EntryId(0),
            elem: HEADER_ELEM,
        };
        let m = FixtureMeasure;
        let cxv = cx(&m, store.view());
        let mut present = plx_machine::present::Present::new();
        let mut buf: Vec<plx_machine::machine::Stamped<PersonHost>> = Vec::new();
        let mut fx = Effects::new(
            &mut buf,
            plx_machine::machine::MachineId::Instance(plx_machine::machine::InstanceId(0)),
            &mut present,
        );
        assert!(!s.page.header_marked);
        Machine::<PersonHost>::step(
            &mut s,
            &ScreenEvent::FocusMoved {
                from: None,
                to: header_key,
                by: By::Restore,
            },
            &cxv,
            &mut fx,
        );
        assert!(
            !s.page.header_marked,
            "a restore/landing is not an explicit arrival"
        );
        Machine::<PersonHost>::step(
            &mut s,
            &ScreenEvent::FocusMoved {
                from: None,
                to: header_key,
                by: By::Dir,
            },
            &cxv,
            &mut fx,
        );
        assert!(
            s.page.header_marked,
            "an explicit D-pad press onto the header marks it"
        );
        store.run(PersonCmd::Close);
    }

    fn hash(s: &PersonScreen) -> u64 {
        let mut canon = Canon::new();
        LogicalState::write(s, &mut canon);
        canon.finish()
    }

    /// The interner and its future allocation point are machine decisions, even when two pages
    /// currently draw the same person. A summary-only hash would miss both differences.
    #[test]
    fn logical_state_hash_includes_the_full_card_registry_and_counter() {
        let _serial = plx_base::testlock::serial();
        let (mut store, mut a) = seed(1, 0);
        let mut b = PersonScreen::new(
            EntryId(0),
            ServerId::UNSET,
            "161".to_string(),
            "5d77682aeb5d26001f1de4b0".to_string(),
            "Idina Menzel".to_string(),
            String::new(),
        );
        b.page.cards = a.page.cards.clone();
        lay(&mut b, &store, &FixtureMeasure);
        assert_eq!(
            hash(&a),
            hash(&b),
            "identical machine state hashes identically"
        );

        b.page.cards.next += 1;
        assert_ne!(hash(&a), hash(&b), "the future allocator is part of state");
        b.page.cards.next = a.page.cards.next;
        b.page.cards.keys[0].elem += 1;
        assert_ne!(hash(&a), hash(&b), "the identity registry is part of state");
        b.page.cards = a.page.cards.clone();
        b.page.return_pending = true;
        assert_ne!(
            hash(&a),
            hash(&b),
            "return hydration changes future reconciliation even when the page draws identically"
        );
        let _ = &mut a;
        store.run(PersonCmd::Close);
    }

    /// Entry eviction preserves the identity interner in PageMemory. A reordered landing after
    /// remount therefore resolves the old engine element to the same movie, not merely the same
    /// numeric slot.
    #[test]
    fn evict_remount_with_reordered_shelf_preserves_movie_identity() {
        let _serial = plx_base::testlock::serial();
        let (mut store, original) = seed(2, 0);
        let old_focus = focus_of(&original, &store, 0, 1);
        assert_eq!(
            original
                .focused_item(Some(old_focus), &cx(&FixtureMeasure, store.view()))
                .map(|m| m.rk.as_str()),
            Some("m1")
        );
        let PageMemory::Person(memory) =
            Screen::<PersonHost>::memory_at(&original, Some(old_focus))
        else {
            panic!("Person must persist its interner through PageMemory::Person");
        };

        let mut remounted = PersonScreen::new(
            EntryId(0),
            ServerId::UNSET,
            "161".to_string(),
            "5d77682aeb5d26001f1de4b0".to_string(),
            "Idina Menzel".to_string(),
            String::new(),
        );
        remounted.restore(&memory);
        store.install_for_test(vec![item("m1"), item("m0")], Vec::new());
        remounted.page.sync_card_keys(&cx(&FixtureMeasure, store.view()));
        let measure = FixtureMeasure;
        let restored = Focusable::<PersonHost>::reconcile(
            &remounted, old_focus, &cx(&measure, store.view()));
        assert_eq!(restored.elem, old_focus.elem);
        assert_eq!(
            remounted
                .focused_item(Some(restored), &cx(&FixtureMeasure, store.view()))
                .map(|m| m.rk.as_str()),
            Some("m1")
        );
        store.run(PersonCmd::Close);
    }

    #[test]
    fn restoring_a_frozen_registry_never_rewinds_keys_minted_after_the_snapshot() {
        let _serial = plx_base::testlock::serial();
        let (mut store, mut screen) = seed(1, 0);
        let frozen = screen.page.memory();
        store.install_for_test(vec![item("m0"), item("newer")], Vec::new());
        screen.page.sync_card_keys(&cx(&FixtureMeasure, store.view()));
        let newer_elem = screen
            .page.elem_for(&store.view().current().unwrap().shelf(0)[1])
            .expect("the live body interned the later landing");
        let live_next = screen.page.cards.next;

        screen.restore(&frozen);

        assert_eq!(
            screen.page.elem_for(&store.view().current().unwrap().shelf(0)[1]),
            Some(newer_elem),
            "request-time memory merges into a live interner instead of replacing it"
        );
        assert_eq!(screen.page.cards.next, live_next);
        store.run(PersonCmd::Close);
    }

    fn pending_share_return(
        measure: &FixtureMeasure,
    ) -> (
        plx_data::stores::person::PersonStore,
        PersonScreen,
        plx_machine::machine::FocusKey<u32>,
        ServerId,
        ServerId,
    ) {
        plx_plex::plex::reset_servers_for_test();
        let origin =
            plx_plex::plex::register_for_test("person-pending-origin", "127.0.0.1", 1, "a", "cid");
        let share =
            plx_plex::plex::register_for_test("person-pending-share", "127.0.0.1", 2, "b", "cid");
        let mut store = plx_data::stores::person::PersonStore::default();
        crate::registry::run_person_fixture(&mut store, PersonCmd::Open { sid: origin, key: "161".into(),
            guid: "5d77682aeb5d26001f1de4b0".into(), name: "Idina Menzel".into(),
            thumb: String::new() });
        let mut original = PersonScreen::new(
            EntryId(0),
            origin,
            "161".to_string(),
            "5d77682aeb5d26001f1de4b0".to_string(),
            "Idina Menzel".to_string(),
            String::new(),
        );
        store.install_source_for_test(share, vec![item_on(share, "wanted")], Vec::new());
        original.page.sync_card_keys(&cx(measure, store.view()));
        let old_focus = focus_of(&original, &store, 0, 0);
        let PageMemory::Person(memory) =
            Screen::<PersonHost>::memory_at(&original, Some(old_focus))
        else {
            panic!("Person must persist its interner through PageMemory::Person");
        };

        store.run(PersonCmd::Close);
        let mut returned = PersonScreen::new(
            EntryId(0),
            origin,
            "161".to_string(),
            "5d77682aeb5d26001f1de4b0".to_string(),
            "Idina Menzel".to_string(),
            String::new(),
        );
        returned.restore(&memory);
        let mut present = plx_machine::present::Present::new();
        let mut out = Vec::new();
        let mut fx = Effects::new(
            &mut out,
            plx_machine::machine::MachineId::Instance(plx_machine::machine::InstanceId(0)),
            &mut present,
        );
        Machine::<PersonHost>::step(
            &mut returned,
            &ScreenEvent::RestoreMemory(PageMemory::Person(memory)),
            &cx_at(measure, store.view(), old_focus),
            &mut fx,
        );
        Machine::<PersonHost>::step(
            &mut returned,
            &ScreenEvent::Enter(Enter::Restored),
            &cx_at(measure, store.view(), old_focus),
            &mut fx,
        );
        crate::registry::run_person_fixture(&mut store, PersonCmd::Open { sid: origin, key: "161".into(),
            guid: "5d77682aeb5d26001f1de4b0".into(), name: "Idina Menzel".into(),
            thumb: String::new() });
        store.install_source_for_test(
            origin,
            vec![item_on(origin, "available")],
            Vec::new(),
        );
        Machine::<PersonHost>::step(
            &mut returned,
            &ScreenEvent::StoreChanged(plx_data::stores::StoreId::Person.ord(), 1),
            &cx_at(measure, store.view(), old_focus),
            &mut fx,
        );
        assert!(returned.page.return_pending);
        assert_eq!(
            Focusable::<PersonHost>::reconcile(&returned, old_focus, &cx_at(measure, store.view(), old_focus)),
            old_focus
        );
        (store, returned, old_focus, origin, share)
    }

    #[test]
    fn a_direction_abandons_an_unavailable_return_card_and_allows_fallback() {
        let _serial = plx_base::testlock::serial();
        let measure = FixtureMeasure;
        let (mut store, mut returned, old_focus, _origin, _share) = pending_share_return(&measure);
        let mut present = plx_machine::present::Present::new();
        let mut out = Vec::new();
        let mut fx = Effects::new(
            &mut out,
            plx_machine::machine::MachineId::Instance(plx_machine::machine::InstanceId(0)),
            &mut present,
        );
        let right = ScreenEvent::Input(InputEvent {
            at: Tick::default(),
            source: plx_machine::machine::Source::Replay,
            kind: InputKind::Key {
                key: Key::Right,
                sym: 0,
                wcode: 0,
                edge: Edge::Down,
                at_edge: false,
            },
        });
        assert_eq!(
            Machine::<PersonHost>::step(
                &mut returned,
                &right,
                &cx_at(&measure, store.view(), old_focus),
                &mut fx,
            ),
            Handled::No,
            "the engine still owns directional movement"
        );
        assert!(!returned.page.return_pending);
        assert_eq!(
            Focusable::<PersonHost>::reconcile(&returned, old_focus, &cx_at(&measure, store.view(), old_focus)),
            focus_of(&returned, &store, 0, 0),
            "the same frame's dispatcher reconcile can seat the available card"
        );
        store.run(PersonCmd::Close);
        plx_plex::plex::reset_servers_for_test();
    }

    #[test]
    fn a_click_abandons_an_unavailable_return_card() {
        let _serial = plx_base::testlock::serial();
        let measure = FixtureMeasure;
        let (mut store, mut returned, old_focus, _origin, _share) = pending_share_return(&measure);
        let available = focus_of(&returned, &store, 0, 0);
        let mut present = plx_machine::present::Present::new();
        let mut out = Vec::new();
        let mut fx = Effects::new(
            &mut out,
            plx_machine::machine::MachineId::Instance(plx_machine::machine::InstanceId(0)),
            &mut present,
        );
        let click = ScreenEvent::Input(InputEvent {
            at: Tick::default(),
            source: plx_machine::machine::Source::Replay,
            kind: InputKind::Click {
                x: 0.0,
                y: 0.0,
                hit: Some(available.elem),
            },
        });
        assert_eq!(
            Machine::<PersonHost>::step(
                &mut returned,
                &click,
                &cx_at(&measure, store.view(), old_focus),
                &mut fx,
            ),
            Handled::No
        );
        assert!(!returned.page.return_pending);
        store.run(PersonCmd::Close);
        plx_plex::plex::reset_servers_for_test();
    }

    #[test]
    fn a_successful_empty_source_answer_releases_the_return_card_to_fallback() {
        let _serial = plx_base::testlock::serial();
        let measure = FixtureMeasure;
        let (mut store, mut returned, old_focus, _origin, share) = pending_share_return(&measure);
        store.install_source_for_test(share, Vec::new(), Vec::new());
        let mut present = plx_machine::present::Present::new();
        let mut out = Vec::new();
        let mut fx = Effects::new(
            &mut out,
            plx_machine::machine::MachineId::Instance(plx_machine::machine::InstanceId(0)),
            &mut present,
        );
        Machine::<PersonHost>::step(
            &mut returned,
            &ScreenEvent::StoreChanged(plx_data::stores::StoreId::Person.ord(), 2),
            &cx_at(&measure, store.view(), old_focus),
            &mut fx,
        );
        assert!(
            !returned.page.return_pending,
            "a successful empty media answer is terminal for this saved source"
        );
        assert_eq!(
            Focusable::<PersonHost>::reconcile(&returned, old_focus, &cx_at(&measure, store.view(), old_focus)),
            focus_of(&returned, &store, 0, 0)
        );
        store.run(PersonCmd::Close);
        plx_plex::plex::reset_servers_for_test();
    }

    /// Returning to a retained Person A first reclaims the single-slot store from Person B. That
    /// request is asynchronous: until A's own shelves land, the old engine key is deliberately
    /// absent from the published rows. Reconciliation must hold that known identity instead of
    /// rewriting the engine cursor to Header; the reordered landing can then resolve the same key
    /// back to the same movie without a screen-local focus copy.
    #[test]
    fn retained_back_holds_the_known_card_key_until_the_requested_person_lands() {
        let _serial = plx_base::testlock::serial();
        plx_plex::plex::reset_servers_for_test();
        let origin =
            plx_plex::plex::register_for_test("person-return-origin", "127.0.0.1", 1, "a", "cid");
        let share =
            plx_plex::plex::register_for_test("person-return-share", "127.0.0.1", 2, "b", "cid");
        let mut store = plx_data::stores::person::PersonStore::default();
        crate::registry::run_person_fixture(&mut store, PersonCmd::Open { sid: origin, key: "161".into(),
            guid: "5d77682aeb5d26001f1de4b0".into(), name: "Idina Menzel".into(),
            thumb: String::new() });
        let mut first = PersonScreen::new(
            EntryId(0),
            origin,
            "161".to_string(),
            "5d77682aeb5d26001f1de4b0".to_string(),
            "Idina Menzel".to_string(),
            String::new(),
        );
        store.install_source_for_test(
            share,
            vec![item_on(share, "m0"), item_on(share, "m1")],
            Vec::new(),
        );
        first.page.sync_card_keys(&cx(&FixtureMeasure, store.view()));
        let old_focus = focus_of(&first, &store, 0, 1);
        assert_eq!(
            first
                .focused_item(Some(old_focus), &cx(&FixtureMeasure, store.view()))
                .map(|movie| movie.rk.as_str()),
            Some("m1")
        );
        let PageMemory::Person(memory) = Screen::<PersonHost>::memory_at(&first, Some(old_focus))
        else {
            panic!("Person must persist its interner through PageMemory::Person");
        };

        let _other = PersonScreen::new(
            EntryId(8),
            origin,
            "other".to_string(),
            "other-guid".to_string(),
            "Other Person".to_string(),
            String::new(),
        );
        crate::registry::run_person_fixture(&mut store, PersonCmd::Open { sid: origin, key: "other".into(), guid: "other-guid".into(),
            name: "Other Person".into(), thumb: String::new() });
        assert!(first.page.person(&cx(&FixtureMeasure, store.view())).is_none(), "Person B displaced Person A");

        let measure = FixtureMeasure;
        let mut present = plx_machine::present::Present::new();
        let mut out = Vec::new();
        let mut fx = Effects::new(
            &mut out,
            plx_machine::machine::MachineId::Instance(plx_machine::machine::InstanceId(0)),
            &mut present,
        );
        Machine::<PersonHost>::step(
            &mut first,
            &ScreenEvent::RestoreMemory(PageMemory::Person(memory)),
            &cx_at(&measure, store.view(), old_focus),
            &mut fx,
        );
        Machine::<PersonHost>::step(
            &mut first,
            &ScreenEvent::StoreChanged(plx_data::stores::StoreId::Person.ord(), 1),
            &cx_at(&measure, store.view(), old_focus),
            &mut fx,
        );
        assert!(
            first.page.return_pending,
            "Person B's store notice is not an answer about A"
        );
        assert_eq!(
            Focusable::<PersonHost>::reconcile(&first, old_focus, &cx_at(&measure, store.view(), old_focus)),
            old_focus,
            "a wrong-person notice cannot consume return hydration"
        );
        Machine::<PersonHost>::step(
            &mut first,
            &ScreenEvent::Enter(Enter::Restored),
            &cx_at(&measure, store.view(), old_focus),
            &mut fx,
        );
        crate::registry::run_person_fixture(&mut store, PersonCmd::Open { sid: origin, key: "161".into(),
            guid: "5d77682aeb5d26001f1de4b0".into(), name: "Idina Menzel".into(),
            thumb: String::new() });
        assert!(first.page.person(&cx(&FixtureMeasure, store.view())).is_some(), "return reclaims Person A's store");
        assert!(
            first.page.person(&cx(&FixtureMeasure, store.view())).unwrap().shelf(0).is_empty(),
            "the addressed shelves have not landed yet"
        );
        assert_eq!(
            Focusable::<PersonHost>::reconcile(&first, old_focus, &cx_at(&measure, store.view(), old_focus)),
            old_focus,
            "a known return identity must survive the empty resolving interval"
        );

        store.install_source_for_test(
            origin,
            vec![item_on(origin, "other-source-card")],
            Vec::new(),
        );
        Machine::<PersonHost>::step(
            &mut first,
            &ScreenEvent::StoreChanged(plx_data::stores::StoreId::Person.ord(), 2),
            &cx_at(&measure, store.view(), old_focus),
            &mut fx,
        );
        assert!(
            first.page.person(&cx(&FixtureMeasure, store.view())).unwrap().landed,
            "another source has enough content to settle the page-level spinner"
        );
        assert_eq!(
            Focusable::<PersonHost>::reconcile(&first, old_focus, &cx_at(&measure, store.view(), old_focus)),
            old_focus,
            "page-level landed must not settle the saved card's still-resolving source"
        );

        store.install_source_for_test(
            share,
            vec![item_on(share, "m1"), item_on(share, "m0")],
            Vec::new(),
        );
        Machine::<PersonHost>::step(
            &mut first,
            &ScreenEvent::StoreChanged(plx_data::stores::StoreId::Person.ord(), 3),
            &cx_at(&measure, store.view(), old_focus),
            &mut fx,
        );
        assert!(
            !first.page.return_pending,
            "the matching card landing retires hydration"
        );
        let restored =
            Focusable::<PersonHost>::reconcile(&first, old_focus, &cx_at(&measure, store.view(), old_focus));
        assert_eq!(restored, old_focus);
        assert_eq!(
            first
                .focused_item(Some(restored), &cx(&FixtureMeasure, store.view()))
                .map(|movie| movie.rk.as_str()),
            Some("m1"),
            "the reordered landing resolves the preserved identity"
        );
        store.run(PersonCmd::Close);
        plx_plex::plex::reset_servers_for_test();
    }

    /// The same delayed interval exists after the dispatcher's body cap evicts the Person screen:
    /// PageMemory restores the interner before `Enter(Restored)`, while the new body has already
    /// opened an empty A store. The saved engine key must remain intact until A's shelves arrive.
    #[test]
    fn cold_remount_holds_the_memory_interner_key_until_reordered_shelves_land() {
        let _serial = plx_base::testlock::serial();
        plx_plex::plex::reset_servers_for_test();
        let sid = plx_plex::plex::register_for_test("person-cold-return", "127.0.0.1", 1, "a", "cid");
        let mut store = plx_data::stores::person::PersonStore::default();
        crate::registry::run_person_fixture(&mut store, PersonCmd::Open { sid, key: "161".into(),
            guid: "5d77682aeb5d26001f1de4b0".into(), name: "Idina Menzel".into(),
            thumb: String::new() });
        let mut original = PersonScreen::new(
            EntryId(0),
            sid,
            "161".to_string(),
            "5d77682aeb5d26001f1de4b0".to_string(),
            "Idina Menzel".to_string(),
            String::new(),
        );
        store.install_source_for_test(
            sid,
            vec![item_on(sid, "m0"), item_on(sid, "m1")],
            Vec::new(),
        );
        original.page.sync_card_keys(&cx(&FixtureMeasure, store.view()));
        let old_focus = focus_of(&original, &store, 0, 1);
        let PageMemory::Person(memory) =
            Screen::<PersonHost>::memory_at(&original, Some(old_focus))
        else {
            panic!("Person must persist its interner through PageMemory::Person");
        };

        store.run(PersonCmd::Close);
        let mut remounted = PersonScreen::new(
            EntryId(0),
            sid,
            "161".to_string(),
            "5d77682aeb5d26001f1de4b0".to_string(),
            "Idina Menzel".to_string(),
            String::new(),
        );
        remounted.restore(&memory);

        let measure = FixtureMeasure;
        let mut present = plx_machine::present::Present::new();
        let mut out = Vec::new();
        let mut fx = Effects::new(
            &mut out,
            plx_machine::machine::MachineId::Instance(plx_machine::machine::InstanceId(0)),
            &mut present,
        );
        Machine::<PersonHost>::step(
            &mut remounted,
            &ScreenEvent::RestoreMemory(PageMemory::Person(memory)),
            &cx_at(&measure, store.view(), old_focus),
            &mut fx,
        );
        Machine::<PersonHost>::step(
            &mut remounted,
            &ScreenEvent::Enter(Enter::Restored),
            &cx_at(&measure, store.view(), old_focus),
            &mut fx,
        );
        crate::registry::run_person_fixture(&mut store, PersonCmd::Open { sid, key: "161".into(),
            guid: "5d77682aeb5d26001f1de4b0".into(), name: "Idina Menzel".into(),
            thumb: String::new() });
        assert!(remounted.page.person(&cx(&FixtureMeasure, store.view())).unwrap().shelf(0).is_empty());
        assert_eq!(
            Focusable::<PersonHost>::reconcile(&remounted, old_focus, &cx_at(&measure, store.view(), old_focus)),
            old_focus,
            "CAP eviction must not turn the saved card identity into Header while A reloads"
        );

        store.install_source_for_test(
            sid,
            vec![item_on(sid, "m1"), item_on(sid, "m0")],
            Vec::new(),
        );
        Machine::<PersonHost>::step(
            &mut remounted,
            &ScreenEvent::StoreChanged(plx_data::stores::StoreId::Person.ord(), 2),
            &cx_at(&measure, store.view(), old_focus),
            &mut fx,
        );
        assert!(!remounted.page.return_pending);
        let restored =
            Focusable::<PersonHost>::reconcile(&remounted, old_focus, &cx_at(&measure, store.view(), old_focus));
        assert_eq!(restored, old_focus);
        assert_eq!(
            remounted
                .focused_item(Some(restored), &cx(&FixtureMeasure, store.view()))
                .map(|movie| movie.rk.as_str()),
            Some("m1")
        );
        store.run(PersonCmd::Close);
        plx_plex::plex::reset_servers_for_test();
    }

    #[test]
    fn restore_reclaims_only_a_displaced_store_and_preserves_shell_scroll() {
        let _serial = plx_base::testlock::serial();
        let (mut store, mut first) = seed(2, 0);
        jump_to(&mut first, 173.0);
        let _other = PersonScreen::new(
            EntryId(8),
            ServerId::UNSET,
            "other".to_string(),
            "other-guid".to_string(),
            "Other Person".to_string(),
            String::new(),
        );
        crate::registry::run_person_fixture(&mut store, PersonCmd::Open { sid: ServerId::UNSET, key: "other".into(),
            guid: "other-guid".into(), name: "Other Person".into(), thumb: String::new() });
        assert!(first.page.person(&cx(&FixtureMeasure, store.view())).is_none());

        let measure = FixtureMeasure;
        let mut present = plx_machine::present::Present::new();
        let mut out = Vec::new();
        let mut fx = Effects::new(
            &mut out,
            plx_machine::machine::MachineId::Instance(plx_machine::machine::InstanceId(0)),
            &mut present,
        );
        Machine::<PersonHost>::step(
            &mut first,
            &ScreenEvent::Enter(Enter::Restored),
            &cx(&measure, store.view()),
            &mut fx,
        );
        crate::registry::run_person_fixture(&mut store, PersonCmd::Open { sid: ServerId::UNSET, key: "161".into(),
            guid: "5d77682aeb5d26001f1de4b0".into(), name: "Idina Menzel".into(),
            thumb: String::new() });
        assert!(first.page.person(&cx(&FixtureMeasure, store.view())).is_some());
        assert_eq!(first.stack.scroll(), 173.0);

        // A second restore while the matching store is already current is a true no-op.
        let before_gen = store.gen();
        Machine::<PersonHost>::step(
            &mut first,
            &ScreenEvent::Enter(Enter::Restored),
            &cx(&measure, store.view()),
            &mut fx,
        );
        assert_eq!(
            store.gen(),
            before_gen
        );
        assert_eq!(first.stack.scroll(), 173.0);
        store.run(PersonCmd::Close);
    }

    /// OK on the header, Hold on a card and Back leave through their content effects with no
    /// latch; the header no longer offers the Filmography route (its departments are the rows).
    #[test]
    fn back_and_hold_emit_their_content_effects_and_the_header_opens_no_filmography() {
        let _serial = plx_base::testlock::serial();
        let (mut store, mut s) = seed(1, 0);
        store.install_credits_for_test(&[("Actor", 7)]);
        let measure = FixtureMeasure;
        let mut present = plx_machine::present::Present::new();
        let mut out = Vec::new();

        let mut run = |screen: &mut PersonScreen,
                       event: ScreenEvent<PersonHost>,
                       focus: Option<plx_machine::machine::FocusKey<u32>>| {
            let context = Cx {
                views: store.view(),
                tick: Tick::default(),
                measure: &measure,
                press: PressRead::default(),
                focus: FocusRead { current: focus , ..Default::default() },
                owner: InputOwner::Entry(screen.page.entry),
            };
            let mut fx = Effects::new(
                &mut out,
                plx_machine::machine::MachineId::Instance(plx_machine::machine::InstanceId(0)),
                &mut present,
            );
            Machine::<PersonHost>::step(screen, &event, &context, &mut fx)
        };

        assert_eq!(
            run(&mut s, ScreenEvent::Activate(HEADER_ELEM), None),
            Handled::Yes
        );
        let card = focus_of(&s, &store, 0, 0);
        assert_eq!(
            run(
                &mut s,
                ScreenEvent::PressHold(plx_machine::machine::PressId(2)),
                Some(card)
            ),
            Handled::Yes
        );
        let back = ScreenEvent::Input(InputEvent {
            at: Tick::default(),
            source: plx_machine::machine::Source::Replay,
            kind: InputKind::Key {
                key: Key::Back,
                sym: 0,
                wcode: 0,
                edge: Edge::Down,
                at_edge: false,
            },
        });
        assert_eq!(run(&mut s, back, Some(card)), Handled::Yes);
        drop(run);

        assert!(!out.iter().any(|st| matches!(
            &st.fx,
            plx_machine::machine::Fx::App(AppFx::Content(ContentReq::Present(ContentArg::Filmography { .. })))
        )), "nothing on the page presents the Filmography route any more");
        assert!(out.iter().any(|st| matches!(
            &st.fx,
            plx_machine::machine::Fx::App(AppFx::Content(ContentReq::ItemMenu))
        )));
        assert!(out.iter().any(|st| matches!(
            &st.fx,
            plx_machine::machine::Fx::App(AppFx::Content(ContentReq::Back))
        )));
        store.run(PersonCmd::Close);
    }

    #[test]
    fn header_geometric_anchor_is_not_its_pointer_hit_rectangle() {
        let _serial = plx_base::testlock::serial();
        let (mut store, s) = seed(1, 0);
        let anchor = s.page.header_anchor();
        let hit = s.page.header_rect();
        assert_eq!(anchor.x, MARGIN_X);
        assert_eq!(anchor.w, 0.0, "the navigation projection stays left-pinned");
        assert!(
            hit.w > 0.0 && hit.h > 0.0,
            "the drawn biography band is clickable"
        );

        let measure = FixtureMeasure;
        let placed = Focusable::<PersonHost>::place(
            &s, &HEADER_ELEM, &cx(&measure, store.view()), At::Drawn)
            .expect("the header is a real engine element");
        assert_eq!(
            (placed.rect.x, placed.rect.y, placed.rect.w, placed.rect.h),
            (hit.x, hit.y, hit.w, hit.h),
            "place and the recorded stop share the hit geometry"
        );
        store.run(PersonCmd::Close);
    }

    /// A landed page entered the way the container enters a fresh mount (`FirstInGroup(GroupId(0))`)
    /// opens on the header: group 0 is the header's, whatever rows the page holds.
    #[test]
    fn a_fresh_mount_of_a_landed_page_seats_the_header() {
        let _serial = plx_base::testlock::serial();
        let (mut store, mut s) = seed(2, 1);
        store.install_credits_for_test(&[("Actor", 3)]);
        s.page.refresh_store_cache(&cx(&FixtureMeasure, store.view()));
        lay(&mut s, &store, &FixtureMeasure);
        let context = cx(&FixtureMeasure, store.view());
        let mut engine = plx_ui::focus::FocusEngine::new();
        let plx_ui::focus::Outcome::Moved { to, .. } = engine.enter(
            context.owner,
            &s,
            plx_machine::machine::FocusTarget::FirstInGroup(GroupId(0)),
            None,
            &context,
        ) else {
            panic!("a landed page must seat");
        };
        assert_eq!(to, plx_machine::machine::FocusKey { entry: EntryId(0), elem: HEADER_ELEM });
        store.run(PersonCmd::Close);
    }

    /// A shelf's group id belongs to its row, not to which other rows the page holds: the engine
    /// remembers a cursor per `(entry, group)`. Movies and Shows are one row.
    #[test]
    fn a_shelf_keeps_its_group_id_whichever_other_shelves_exist() {
        let _serial = plx_base::testlock::serial();
        let group_of = |movies: usize, shows: usize, credits: &[(&str, usize)], card: &dyn Fn(&PersonScreen, &plx_data::stores::person::PersonStore) -> plx_machine::machine::FocusKey<u32>| {
            let (mut store, mut s) = seed(movies, shows);
            if !credits.is_empty() {
                credit(&mut s, &mut store, credits);
            }
            let context = cx(&FixtureMeasure, store.view());
            let g = Focusable::<PersonHost>::group_of(&s, &card(&s, &store).elem, &context);
            drop(context);
            store.run(PersonCmd::Close);
            g
        };
        let movie = |s: &PersonScreen, st: &plx_data::stores::person::PersonStore| focus_of(s, st, 0, 0);
        let show = |s: &PersonScreen, st: &plx_data::stores::person::PersonStore| focus_of(s, st, 1, 0);
        let acting = |s: &PersonScreen, _: &plx_data::stores::person::PersonStore| credit_of(s, CreditRow::Acting, 0);
        let directing = |s: &PersonScreen, _: &plx_data::stores::person::PersonStore| credit_of(s, CreditRow::Directing, 0);
        assert_eq!(group_of(1, 1, &[], &movie), Some(LOCAL_GROUP));
        assert_eq!(group_of(1, 1, &[], &show), Some(LOCAL_GROUP), "Shows share the Movies' row");
        assert_eq!(group_of(0, 1, &[], &show), Some(LOCAL_GROUP), "Shows without Movies");
        assert_eq!(group_of(1, 0, &[("Actor", 2)], &acting), group_of(0, 0, &[("Actor", 2)], &acting), "Acting with and without local titles");
        assert_eq!(group_of(0, 0, &[("Director", 1)], &directing), group_of(0, 0, &[("Actor", 1), ("Director", 1)], &directing));
        assert_ne!(group_of(0, 0, &[("Actor", 1), ("Director", 1)], &acting), group_of(0, 0, &[("Actor", 1), ("Director", 1)], &directing));
    }

    /// With no person to show, reconcile answers the header (never the bare `want`).
    #[test]
    fn reconcile_with_no_person_answers_the_header() {
        let _serial = plx_base::testlock::serial();
        let store = plx_data::stores::person::PersonStore::default();
        let mut s = PersonScreen::new(EntryId(0), ServerId::UNSET, "161".to_string(), String::new(), String::new(), String::new());
        lay(&mut s, &store, &FixtureMeasure);
        let want = plx_machine::machine::FocusKey { entry: EntryId(0), elem: 0x7777 };
        let got = Focusable::<PersonHost>::reconcile(&s, want, &cx(&FixtureMeasure, store.view()));
        assert_eq!(got, plx_machine::machine::FocusKey { entry: EntryId(0), elem: HEADER_ELEM });
    }

    /// Owner decision 3 (one recovery rule on every card screen): a focused card that disappears
    /// hands focus to the card now at the same position, clamped within its own section.
    #[test]
    fn a_removed_focused_card_hands_focus_to_the_same_position_clamped() {
        let _serial = plx_base::testlock::serial();
        let (mut store, mut s) = seed(3, 1);
        let m = FixtureMeasure;
        let want = focus_of(&s, &store, 0, 1);
        settle_shelves(&mut s, &store, want, 3);
        store.install_for_test(vec![item("m0"), item("m2")], vec![item("s0")]);
        s.page.refresh_store_cache(&cx(&m, store.view()));
        settle_shelves(&mut s, &store, want, 1);
        let got = Focusable::<PersonHost>::reconcile(&s, want, &cx_at(&m, store.view(), want));
        assert_eq!(got, focus_of(&s, &store, 0, 1), "the card that slid into position 1 takes focus");
        store.install_for_test(vec![item("m0")], vec![item("s0")]);
        s.page.refresh_store_cache(&cx(&m, store.view()));
        settle_shelves(&mut s, &store, got, 1);
        let got = Focusable::<PersonHost>::reconcile(&s, got, &cx_at(&m, store.view(), got));
        assert_eq!(got, focus_of(&s, &store, 1, 0), "past the end, the last card left in the SAME row (its Shows follow its Movies)");
        store.run(PersonCmd::Close);
    }

    #[test]
    fn focus_walks_only_the_shelves_that_exist() {
        let _serial = plx_base::testlock::serial();
        let (mut store, mut s) = seed(3, 0);
        let measure = FixtureMeasure;
        let context = cx(&measure, store.view());
        let mut groups = Vec::new();
        Focusable::<PersonHost>::groups(&s, &context, &mut groups);
        let group = |s: &PersonScreen, k: Sec| s.stack.group(k).unwrap();
        assert!(groups.iter().any(|g| g.id == group(&s, Sec::Local) && g.len == 3));
        assert!(s.stack.group(Sec::Credits(0)).is_none(), "no credits in that row, no section");
        let last = focus_of(&s, &store, 0, 2);
        assert!(matches!(
            Focusable::<PersonHost>::neighbour(&s, last, Dir::Right, &context),
            Step::Edge
        ));
        drop(context);
        credit(&mut s, &mut store, &[("Actor", 3), ("Director", 2)]);
        let mut links = Vec::new();
        Screen::<PersonHost>::links(&s, &mut links);
        let linked = |from: Sec, dir: Dir, to: Sec| links.iter().any(|l| l.from == group(&s, from) && l.dir == dir && l.to == group(&s, to));
        let (acting, directing) = (Sec::Credits(CreditRow::Acting.index()), Sec::Credits(CreditRow::Directing.index()));
        assert!(linked(Sec::Head, Dir::Down, Sec::Local) && linked(Sec::Local, Dir::Up, Sec::Head), "the header and the first row");
        assert!(linked(Sec::Local, Dir::Down, acting) && linked(acting, Dir::Up, Sec::Local));
        assert!(linked(acting, Dir::Down, directing) && linked(directing, Dir::Up, acting), "an absent row is skipped");
        assert_eq!(links.len(), 6, "nothing links to a row that is not there: {links:?}");
        store.run(PersonCmd::Close);
    }

    /// A landing may move the focused item from the Movies to the Shows, and so along the row. The
    /// engine element is the item's identity, so focus stays on it and it keeps the full pop at its
    /// new place (no frame shows two lifted tiles, none shows it collapsed).
    #[test]
    fn an_item_that_changes_kind_in_a_landing_keeps_focus_and_is_lifted_at_its_new_place() {
        let _serial = plx_base::testlock::serial();
        let (mut store, mut s) = seed(2, 1);
        let m = FixtureMeasure;
        let moved = focus_of(&s, &store, 0, 0);
        settle_shelves(&mut s, &store, moved, 120);
        let lift = |s: &PersonScreen, store: &plx_data::stores::person::PersonStore, kind: usize, elem: u32| {
            let p = store.view().current().unwrap();
            let _ = (p, kind);
            s.stack.view(&s.page).scale_of(&cx_at(&m, store.view(), moved), &elem)
        };
        assert_eq!(lift(&s, &store, 0, moved.elem), Some(SHELF_STYLE.focus_scale));

        // "m0" leaves the movies and joins the shows.
        store.install_for_test(vec![item("m1")], vec![item("s0"), item("m0")]);
        s.page.refresh_store_cache(&cx(&m, store.view()));
        assert_eq!(s.page.locate(store.view().current().unwrap(), moved.elem), Some(Located::Local(2)));
        let now = Focusable::<PersonHost>::reconcile(&s, moved, &cx_at(&m, store.view(), moved));
        assert_eq!(now, moved, "the cursor stays on the item across shelves");
        settle_shelves(&mut s, &store, moved, 1);
        assert_eq!(lift(&s, &store, 1, moved.elem), Some(SHELF_STYLE.focus_scale), "lifted whole at its new place");
        let other = focus_of(&s, &store, 0, 0);
        assert_eq!(lift(&s, &store, 0, other.elem), Some(1.0), "the card that took its old place is not lifted");
        settle_shelves(&mut s, &store, moved, 120);
        assert!(Focusable::<PersonHost>::place(&s, &moved.elem, &cx_at(&m, store.view(), moved), At::Drawn).is_some());
        store.run(PersonCmd::Close);
    }

    /// A page whose header has its measured band (or `exp_h`, to pin the other portrait), with two
    /// shelves (Available on MultiPlex and Acting), the stack laid out over it.
    fn shelved_page(exp_h: f32) -> (plx_data::stores::person::PersonStore, PersonScreen) {
        let (mut store, mut s) = seed(3, 3);
        credit(&mut s, &mut store, &[("Actor", 3)]);
        let m = FixtureMeasure;
        s.page.remeasure_header(store.view().current().unwrap(), &m);
        s.page.header.exp_h = exp_h;
        s.page.layout_gen += 1;
        lay(&mut s, &store, &m);
        (store, s)
    }

    #[test]
    fn the_first_shelf_fits_at_rest() {
        let _serial = plx_base::testlock::serial();
        for band in [PORTRAIT_BARE, PORTRAIT_EXP] {
            let (mut store, mut s) = shelved_page(band);
            let key = focus_of(&s, &store, 0, 0);
            settle_shelves(&mut s, &store, key, 240);
            assert_eq!(s.stack.scroll(), 0.0, "a {band}px header: the first shelf fits without scrolling");
            store.run(PersonCmd::Close);
        }
    }

    #[test]
    fn reaching_the_second_shelf_scrolls_it_fully_into_view_and_no_further() {
        let _serial = plx_base::testlock::serial();
        let (mut store, mut s) = shelved_page(PORTRAIT_EXP);
        let key = credit_of(&s, CreditRow::Acting, 0);
        settle_shelves(&mut s, &store, key, 480);
        assert!(s.stack.scroll() > 0.0);
        let placed = Focusable::<PersonHost>::place(&s, &key.elem, &cx_at(&FixtureMeasure, store.view(), key), At::SpringTarget)
            .unwrap();
        let tile_top = placed.rest_rect.cy() - CARD_H / 2.0;
        let block_top = tile_top - SHELF_LABEL_H;
        let block_bottom = tile_top + CARD_H + ui_cards::under_band_for(plx_ui::caption_policy::band_h(&crate::registry::captions::policy()), 1.0);
        assert!(block_bottom <= SCR_H, "the block's bottom edge is on screen: {block_bottom}");
        assert!(block_top >= HEADER_TOP - 0.5, "and its top keeps the page margin: {block_top}");
        // and no further: the last shelf rests exactly one page margin above the bottom edge
        assert!((block_bottom - (SCR_H - MARGIN_Y)).abs() < 0.5, "{block_bottom}");
        store.run(PersonCmd::Close);
    }

    #[test]
    fn a_shelf_here_pitches_like_a_shelf_on_home() {
        // the fixed part of the pitch; the band under it is the Under posters setting's, as Home's
        assert_eq!(SHELF_GAP + SHELF_LABEL_H + CARD_H, ui_cards::ROW_PITCH_FIXED);
        assert_eq!(SHELF_LABEL_H, TITLE_DY + CARD_DY);
    }

    /// **The first shelf's caption stops short of the heading of the shelf under it.** A Down from
    /// Available on MultiPlex onto Acting lifts the Acting heading while the first shelf's caption
    /// is still leaving, and an Up back does the reverse; at the 112 px band no caption line of the
    /// first shelf ends below the Acting heading's drawn top (`SectionFrame::below_rise`).
    #[test]
    fn a_shelfs_caption_keeps_clear_of_the_next_shelfs_lifted_heading() {
        use crate::registry::captions::TEST_UNDER_POSTERS;
        use plx_plex::plex::session::{UnderPosters, UnderPostersLadder as Details};
        let _serial = plx_base::testlock::serial();
        TEST_UNDER_POSTERS.with(|cell| cell.set(Some(UnderPosters { title: true, details: Details::YearLength.movie_parts(), show_details: Details::YearLength.show_parts(), sort_value: false })));
        let (mut store, mut s) = shelved_page(PORTRAIT_EXP);
        let (movies, shows) = (focus_of(&s, &store, 0, 0), credit_of(&s, CreditRow::Acting, 0));
        settle_shelves(&mut s, &store, movies, 120);
        let (mut worst, mut lines, mut from) = (f32::MIN, 0, movies);
        for frame in 0..130 {
            let to = match frame { 0 => shows, 60 => movies, _ => from };
            if to != from {
                feed(&mut s, &store, &FixtureMeasure, Some(to), ScreenEvent::FocusMoved { from: Some(from), to, by: By::Dir });
                from = to;
            }
            settle_shelves(&mut s, &store, from, 1);
            let cxv = cx_at(&FixtureMeasure, store.view(), from);
            let f = DrawFrame::new(&cxv, Painter::root());
            let (first, second) = (Sec::Local, Sec::Credits(CreditRow::Acting.index()));
            let above = s.stack.shelf_frame(&s.page, &cxv, first).expect("the Movies shelf");
            let heading = s.stack.shelf_frame(&s.page, &cxv, second).expect("the Shows shelf").y - SHELF_LABEL_H
                - s.stack.shelf(second).expect("the Shows shelf").heading_lift();
            let src = StackPage::<PersonHost>::cards(&s.page, &cxv, first).expect("the Movies cards");
            let drawn = s.stack.shelf(first).expect("the Movies shelf").caption_lines_probe(&f, &src, above);
            lines += drawn.len();
            worst = drawn.iter().fold(worst, |worst, &(_, _, bottom, _)| worst.max(bottom - heading));
        }
        TEST_UNDER_POSTERS.with(|cell| cell.set(None));
        store.run(PersonCmd::Close);
        assert!(lines > 0, "the walk saw no caption line");
        assert!(worst <= 0.0, "a caption line of the Movies shelf ends {worst:.1} px below the drawn top of the Shows heading");
    }


    /// Q4, painted: a shelf's heading is the one heading + count idiom. The title, the dot and the
    /// count are PAINTED through a recording painter and read back run by run: each stands
    /// `HEADING_SOURCE_PAD` after the one before it; a shelf with no count draws its title alone.
    #[test]
    fn a_painted_shelf_heading_keeps_the_pad_between_title_dot_and_count() {
        let _g = plx_base::testlock::serial();
        let (_store, mut s) = seed(2, 2);
        s.page.row_count_c = Default::default();
        s.page.row_count_c[0] = std::ffi::CString::new("22").unwrap();
        let paint = |kind: usize| {
            let mut runs = plx_ui::draw_census::capture_runs(|| {
                s.page.draw_shelf_heading(Painter::recording(), kind, 0.0, &plx_ui::widgets::LegacyMeasure);
            });
            runs.sort_by(|a, b| a.at.x.total_cmp(&b.at.x));
            runs
        };
        let counted = paint(0);
        assert_eq!(counted.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(), [s.page.row_title_c[0].to_str().unwrap(), "\u{b7}", "22"]);
        assert_eq!(counted[0].at.x, MARGIN_X);
        for pair in counted.windows(2) {
            let gap = pair[1].at.x - (pair[0].at.x + pair[0].at.w);
            assert!((gap - plx_ui::cards::HEADING_SOURCE_PAD).abs() < 0.5, "{} -> {}: {gap}", pair[0].text, pair[1].text);
        }
        assert_eq!(counted[2].size, theme::size::BODY, "the count is a BODY note, not a caption");
        assert_eq!(paint(1).len(), 1, "a shelf with no count draws its title alone");
    }

    /// The header's gaps ride the item, not its neighbour: with no life line the biography keeps
    /// BIO_GAP under the meta line, and with no biography the header ends at the life line.
    #[test]
    fn header_gaps_survive_an_absent_neighbour() {
        let m = FixtureMeasure;
        let label = m.cap_h(theme::size::LABEL);
        let no_life = header_flow(false, "A biography.", true, false, &m);
        assert_eq!(no_life.life_y, None);
        let (meta, bio) = (no_life.meta_y.unwrap(), no_life.bio_y.unwrap());
        assert!((bio - meta - label - BIO_GAP).abs() < 0.01);
        let no_bio = header_flow(false, "", true, true, &m);
        assert_eq!(no_bio.bio_y, None);
        let life = no_bio.life_y.unwrap();
        assert!((no_bio.exp_h - (life + m.cap_h(theme::size::CAPTION))).abs() < 0.01 || no_bio.exp_h == no_bio.exp_d,
            "the life line is the last line of the text column");
    }

    /// **Every row is ONE line, whatever its length.** A hundred and twenty Acting credits stand on
    /// one row: every card's resting place shares the row's top, Right walks from each card to the
    /// next all the way to the last, and only the last answers the row's edge. Under the header the
    /// rows read in the page's order: Available on MultiPlex, then Acting, Appearances, Producing,
    /// Directing and Other credits, each present row a shelf of its own.
    #[test]
    fn every_row_is_one_line_however_long_and_walks_to_its_last_card() {
        let _serial = plx_base::testlock::serial();
        let (mut store, mut s) = seed(2, 1);
        credit(&mut s, &mut store, &[("Other", 2), ("Director", 1), ("Actor", 120), ("Producer", 3), ("Self", 4)]);
        assert_eq!(
            s.page.reach.rows(),
            [Sec::Local, Sec::Credits(0), Sec::Credits(1), Sec::Credits(2), Sec::Credits(3), Sec::Credits(4)],
            "the rows in the page's order, whatever order plex.tv sent the departments in"
        );
        let context = cx(&FixtureMeasure, store.view());
        let mut groups = Vec::new();
        Focusable::<PersonHost>::groups(&s, &context, &mut groups);
        let acting = groups.iter().find(|g| g.id == CREDIT_GROUP[CreditRow::Acting.index()]).expect("the Acting row");
        assert_eq!(acting.len, 120, "every credit is a card of the one row");
        let first = credit_of(&s, CreditRow::Acting, 0);
        let top = Focusable::<PersonHost>::place(&s, &first.elem, &context, At::SpringTarget).unwrap().rest_rect.y;
        let mut at = first;
        for i in 1..120 {
            let Step::Move(next) = Focusable::<PersonHost>::neighbour(&s, at, Dir::Right, &context) else {
                panic!("Right from card {} stopped short of the end", i - 1);
            };
            assert_eq!(next, credit_of(&s, CreditRow::Acting, i), "Right is the next card of the row");
            let placed = Focusable::<PersonHost>::place(&s, &next.elem, &context, At::SpringTarget).unwrap();
            assert_eq!(placed.rest_rect.y, top, "card {i} stands on the row's one line");
            at = next;
        }
        assert!(matches!(Focusable::<PersonHost>::neighbour(&s, at, Dir::Right, &context), Step::Edge), "the last card is the row's end");
        drop(context);
        store.run(PersonCmd::Close);
    }

    /// **Up and Down walk the rows as Home's do.** Down from the header lands on the first row,
    /// Right walks it, Down goes to the card of the next row under it and Up comes back to the card
    /// the first row was left on (each row keeps its own scroll, and the shared engine projects a
    /// vertical move between card rows), then Up again reaches the header: the page's links and the
    /// stack's groups, nothing of its own.
    #[test]
    fn up_and_down_walk_the_rows_and_come_back_to_the_card_left() {
        let _serial = plx_base::testlock::serial();
        let (mut store, mut s) = seed(6, 0);
        credit(&mut s, &mut store, &[("Actor", 6)]);
        let mut links = Vec::new();
        Screen::<PersonHost>::links(&s, &mut links);
        let mut engine = plx_ui::focus::FocusEngine::new();
        let owner = InputOwner::Entry(EntryId(0));
        let go = |engine: &mut plx_ui::focus::FocusEngine<u32>, s: &PersonScreen, dir: Dir| {
            let context = Cx { focus: engine.read(owner), ..cx(&FixtureMeasure, store.view()) };
            match engine.move_dir(owner, s, &links, dir, &context) {
                plx_ui::focus::Outcome::Moved { to, .. } => Some(to),
                _ => None,
            }
        };
        let context = cx(&FixtureMeasure, store.view());
        engine.enter(owner, &s, plx_machine::machine::FocusTarget::FirstInGroup(GroupId(0)), None, &context);
        drop(context);
        assert_eq!(engine.current(owner).map(|k| k.elem), Some(HEADER_ELEM), "the page opens on the header");
        let row_of = |k: Option<plx_machine::machine::FocusKey<u32>>| {
            let context = cx(&FixtureMeasure, store.view());
            k.and_then(|k| Focusable::<PersonHost>::group_of(&s, &k.elem, &context))
        };
        let landed = go(&mut engine, &s, Dir::Down);
        assert_eq!(row_of(landed), Some(LOCAL_GROUP), "Down from the header: the first row");
        while go(&mut engine, &s, Dir::Left).is_some() {}
        go(&mut engine, &s, Dir::Right);
        assert_eq!(go(&mut engine, &s, Dir::Right), Some(focus_of(&s, &store, 0, 2)), "Right walks the row");
        assert_eq!(go(&mut engine, &s, Dir::Down), Some(credit_of(&s, CreditRow::Acting, 2)), "Down: the next row's card under it");
        assert_eq!(go(&mut engine, &s, Dir::Right), Some(credit_of(&s, CreditRow::Acting, 3)));
        assert_eq!(go(&mut engine, &s, Dir::Left), Some(credit_of(&s, CreditRow::Acting, 2)));
        assert_eq!(go(&mut engine, &s, Dir::Up), Some(focus_of(&s, &store, 0, 2)), "Up: back to the card the row was left on");
        assert_eq!(go(&mut engine, &s, Dir::Up), Some(plx_machine::machine::FocusKey { entry: EntryId(0), elem: HEADER_ELEM }), "Up from the first row: the header");
        store.run(PersonCmd::Close);
    }

    /// A card leaves for its page: a local title for its Detail, a credit a server holds for that
    /// server's Detail, a credit no server holds for its catalog page, and a credit with no catalog
    /// id for nothing.
    #[test]
    fn a_card_opens_its_detail_or_its_catalog_page() {
        let _serial = plx_base::testlock::serial();
        let (mut store, mut s) = seed(1, 0);
        credit(&mut s, &mut store, &[("Actor", 3)]);
        let sid = ServerId::from_raw(3);
        s.page.credits[0][0].local = Some((sid, "4242".into()));
        s.page.credits[0][1].catalog_id = "5d776b59ad5437001f79c6f8".into();
        s.page.credits[0][2].catalog_id = String::new();
        s.page.sync_card_keys(&cx(&FixtureMeasure, store.view()));
        let opened = |s: &PersonScreen, key: plx_machine::machine::FocusKey<u32>| {
            let mut present = plx_machine::present::Present::new();
            let mut out: Vec<plx_machine::machine::Stamped<PersonHost>> = Vec::new();
            let mut fx = Effects::new(&mut out, plx_machine::machine::MachineId::Instance(plx_machine::machine::InstanceId(0)), &mut present);
            s.page.open_card(key.elem, &cx(&FixtureMeasure, store.view()), &mut fx);
            drop(fx);
            out.into_iter().filter_map(|st| match st.fx {
                plx_machine::machine::Fx::App(AppFx::Content(ContentReq::Push(arg))) => Some(arg),
                _ => None,
            }).collect::<Vec<_>>()
        };
        assert!(matches!(&opened(&s, focus_of(&s, &store, 0, 0))[..], [ContentArg::Detail { rk, .. }] if rk == "m0"));
        assert!(matches!(&opened(&s, credit_of(&s, CreditRow::Acting, 0))[..], [ContentArg::Detail { sid: at, rk }] if *at == sid && rk == "4242"),
            "a credit a server holds opens that server's copy");
        let hex = plx_data::watchlist::decode_hex("5d776b59ad5437001f79c6f8").unwrap();
        assert!(matches!(&opened(&s, credit_of(&s, CreditRow::Acting, 1))[..], [ContentArg::Catalog { hex: h }] if *h == hex),
            "a credit no server holds opens its catalog page");
        assert!(opened(&s, credit_of(&s, CreditRow::Acting, 2)).is_empty(), "no catalog id, nowhere to go");
        store.run(PersonCmd::Close);
    }

    /// Every row registers the band the Magic Remote wheel scrolls it by (`plx_ui::wheel`): a row
    /// that fits on screen says it has nowhere to go, a long one says how far it can.
    #[test]
    fn every_row_registers_its_wheel_band() {
        let _serial = plx_base::testlock::serial();
        let (mut store, mut s) = seed(3, 0);
        credit(&mut s, &mut store, &[("Actor", 40)]);
        let key = credit_of(&s, CreditRow::Acting, 0);
        settle_shelves(&mut s, &store, key, 240);
        let context = cx_at(&FixtureMeasure, store.view(), key);
        let mut frame = DrawFrame::new(&context, Painter::root());
        page_stops(&s, &mut frame);
        let acting = Focusable::<PersonHost>::place(&s, &key.elem, &context, At::Drawn).unwrap().rect;
        let zone = plx_ui::wheel::zone_at(frame.pans(), acting.cx(), acting.cy()).expect("the Acting row's band is under its card");
        assert_eq!(zone.max, SHELF_STYLE.max_scroll(40), "a long row can go as far as its last card");
        assert!(zone.max > 0.0);
        assert!(frame.pans().iter().any(|z| z.max == 0.0), "the three local titles fit: that row has nowhere to go");
        drop(context);
        store.run(PersonCmd::Close);
    }
}
