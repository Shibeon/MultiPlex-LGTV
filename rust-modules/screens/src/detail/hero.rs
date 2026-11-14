//! Detail hero policy: the action row and its More menu, the credit table, the technical pills,
//! playback facts and their exact width/priority rules. Painting orchestration stays in [`super::DetailScreen`]; the decisions
//! here are explicit-value helpers so host tests need no global focus state.
//!
//! Control selection and geometry take explicit `Detail`/`HeroSet` values. The playback-facts
//! helpers also consult cached application policy and the text renderer; they are not all pure
//! host-only functions. The screen owns the metadata identity and supplies the current item.
//!
//! These helpers are orchestrated by the completed screen's direct `Focusable`/`Screen` impls;
//! they do not own an independent focus cursor or screen lifetime.
//!
//! # The `ElemKey` numbering convention this package establishes (spec §13's own instruction:
//! "reserve a range so the next package can pick a disjoint one")
//!
//! Hero controls occupy the `u32` elem range **`[0, HERO_ELEM_RANGE_END)` = `[0, 64)`**. Every
//! constant below is an IDENTITY, not a POSITION — `ELEM_MARK_WATCHED`/`ELEM_MARK_UNWATCHED` are
//! two DIFFERENT ids for the two faces one control wears, never both present at once, which is
//! what lets [`DetailScreen::reconcile`](super::DetailScreen) carry the focus across a press that
//! flips the toggle (see that impl's doc — the engine calls `reconcile` with whatever key focus
//! currently holds, every frame, so identity-as-elem plus a membership test reproduces
//! the legacy `hero_col` correction without a second cursor). Section helpers use disjoint LOCAL
//! slot ranges starting at [`HERO_ELEM_RANGE_END`]; `DetailScreen` projects repeated items to stable
//! engine identities instead of exposing those positional slots as focus keys.
use std::ffi::{CStr, CString};
use std::os::raw::c_int;

use plx_data::metadata::{Detail, Episode, Extra};
use plx_ui::hstack::{Flow, Item, Span};
use plx_machine::machine::{GroupId, Measure};
use plx_ui::vstack::{Anchor, VStack};
use plx_ui::widgets::PosterMark;
use plx_ui::{theme, Painter, Rect};

/// The hero owns the local ids below the season block; its controls are the `ELEM_*` ids at its
/// bottom.
pub const HERO_ELEM_RANGE_END: u32 = super::SECTION_BLOCK;

pub const ELEM_PLAY: u32 = 0;
pub const ELEM_RESTART: u32 = 1;
pub const ELEM_ALT: u32 = 2;
pub const ELEM_MARK_WATCHED: u32 = 3;
pub const ELEM_MARK_UNWATCHED: u32 = 4;
pub const ELEM_TRAILER: u32 = 5;
/// The More menu's Watchlist row — the Watchlist toggle. ONE identity for both faces (Add /
/// Remove): the face is membership, not a different control.
pub const ELEM_WATCHLIST: u32 = 6;
/// The Play Version disc.
pub const ELEM_VERSION: u32 = 7;
/// The Go to Show disc — present only on an episode's own page (`Detail::kind == "episode"` with a
/// `show_rk`), never on a show's page, whose hero is already about the show.
pub const ELEM_GO_TO_SHOW: u32 = 8;
pub const ELEM_MORE: u32 = 9;

/// The hero's one focus group. Local to this screen — nothing outside `screens::detail` ever names
/// it — so, like `screens::profiles`'s `ROSTER_GROUP`/`FOOTER_GROUP`, any small integer would do;
/// `0` matches this family's own convention of seating group `GroupId(0)` as a screen's primary
/// group.
pub const HERO_GROUP: GroupId = GroupId(0);

// The row's shared geometry (CGAP, CD, the pill-width formula, the row accumulation and the meta
// line) lives in `appkit::hero_layout`, so the catalog page lays out with the same numbers.
pub use plx_appkit::hero_layout::{CD, MAX_CTLS};
#[cfg(test)]
use plx_appkit::hero_layout::CGAP;
use plx_ui::detail_layout::{HERO_TEXT_W, TEXT_X};
/// Names on the credit table's Starring row: a summary. The whole cast is the Cast row further down
/// the page, one headshot per credit with nothing cut (`cards::Cards` counts `Detail::credits_len`).
const STARRING_SUMMARY: usize = 3;
/// Genres on the credit table's Genre row.
const GENRE_SUMMARY: usize = 3;
const FACTS_SEP_PAD: f32 = theme::space::SM;

/// The *Also available* menu row's mark: a disclosure chevron, because the press opens a list (the
/// other sources' copies) rather than acting.
pub const ALT_ICON: plx_ui::icons::Icon = plx_ui::icons::Icon::ChevronDown;

pub fn alt_label() -> &'static CStr { plx_platform::i18n::msg::browse_detail_also_available_c() }

fn mark_watched_label() -> &'static CStr { plx_platform::i18n::msg::browse_detail_mark_watched_c() }
fn mark_unwatched_label() -> &'static CStr { plx_platform::i18n::msg::browse_detail_mark_unwatched_c() }
fn mark_episode_watched_label() -> &'static CStr { plx_platform::i18n::msg::browse_detail_mark_episode_watched_c() }
fn mark_episode_unwatched_label() -> &'static CStr { plx_platform::i18n::msg::browse_detail_mark_episode_unwatched_c() }
fn play_from_start_label() -> &'static CStr { plx_platform::i18n::msg::browse_detail_play_start_c() }
fn trailer_label() -> &'static CStr { plx_platform::i18n::msg::browse_detail_trailer_c() }
fn play_version_label() -> &'static CStr { plx_platform::i18n::msg::browse_detail_play_version_c() }
fn go_to_show_label() -> &'static CStr { plx_platform::i18n::msg::browse_menu_go_show_c() }

/// The Play Version disc's mark: a portrait card with another behind it — several copies of one
/// title, which is what a version list is. It used to be [`plx_ui::icons::Icon::Show`], the item
/// menu's own *Go to Show* glyph; that mark is now the Go to Show disc's, and two identical glyphs
/// in one row would make the only icon-only pair of controls indistinguishable at three metres.
pub const VERSION_ICON: plx_ui::icons::Icon = plx_ui::icons::Icon::Collection;

/// The Go to Show disc's mark: the same stack-of-episodes glyph the item menu's *Go to Show* row
/// wears, so the row and the disc name one destination with one mark.
pub const GO_TO_SHOW_ICON: plx_ui::icons::Icon = plx_ui::icons::Icon::Show;

/// A hero control, named rather than numbered. The action ROW draws two of them ([`hero_ctls`]:
/// Play/Resume and More); every other one is a row of the More menu ([`menu_ctls`]), performed by
/// the same `DetailScreen::activate_hero` when the menu hands its identity back.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HeroCtl {
    /// the Play/Resume pill — always present, always first
    Play,
    /// the ↺ disc, present only while there is a resume point for it to ignore
    Restart,
    /// the Trailer disc, present only while the loaded item has a playable extra
    Trailer,
    /// the *Also available* pill, present only while a second pinned source holds this item
    Alt,
    /// the ✓ face of the watched TOGGLE — worn while the item is not watched (part-watched
    /// included)
    MarkWatched,
    /// the − face of that same toggle — worn once the item IS watched
    MarkUnwatched,
    /// the Watchlist row of the More menu — present only for an item with a Watchlist-takeable
    /// guid; it reads Add, or a destructive Remove while the title is on the list
    Watchlist,
    /// the Go to Show disc — present only on an EPISODE's own page, where the show page is one
    /// press away; it unfurls to "Go to Show" on focus like every other verb disc
    GoToShow,
    /// the Play Version disc, present only while the loaded movie or episode has two or more
    /// versions; it opens the list of them (`screens::versions_panel`)
    Version,
    /// The round ⋯ disc beside Play: the remaining actions ([`menu_ctls`]) through the shared item
    /// menu. Always present.
    More,
}

impl HeroCtl {
    /// This control's fixed `u32` identity (never its position — see the module doc).
    pub const fn elem(self) -> u32 {
        match self {
            HeroCtl::Play => ELEM_PLAY,
            HeroCtl::Restart => ELEM_RESTART,
            HeroCtl::Trailer => ELEM_TRAILER,
            HeroCtl::Alt => ELEM_ALT,
            HeroCtl::MarkWatched => ELEM_MARK_WATCHED,
            HeroCtl::MarkUnwatched => ELEM_MARK_UNWATCHED,
            HeroCtl::Watchlist => ELEM_WATCHLIST,
            HeroCtl::GoToShow => ELEM_GO_TO_SHOW,
            HeroCtl::Version => ELEM_VERSION,
            HeroCtl::More => ELEM_MORE,
        }
    }
    /// The inverse of [`elem`](Self::elem) — `None` for any `u32` outside the ten identities
    /// above, which is every elem a NON-hero group can mint (the completed sections use
    /// their own disjoint range, per the module doc).
    pub fn of_elem(e: u32) -> Option<Self> {
        match e {
            ELEM_PLAY => Some(HeroCtl::Play),
            ELEM_RESTART => Some(HeroCtl::Restart),
            ELEM_TRAILER => Some(HeroCtl::Trailer),
            ELEM_ALT => Some(HeroCtl::Alt),
            ELEM_MARK_WATCHED => Some(HeroCtl::MarkWatched),
            ELEM_MARK_UNWATCHED => Some(HeroCtl::MarkUnwatched),
            ELEM_WATCHLIST => Some(HeroCtl::Watchlist),
            ELEM_GO_TO_SHOW => Some(HeroCtl::GoToShow),
            ELEM_VERSION => Some(HeroCtl::Version),
            ELEM_MORE => Some(HeroCtl::More),
            _ => None,
        }
    }
    /// Is `self` the watched toggle, wearing either face?
    #[cfg(test)]
    pub fn is_watch(self) -> bool {
        matches!(self, HeroCtl::MarkWatched | HeroCtl::MarkUnwatched)
    }
}

/// Which of the conditional actions the More menu offers: the independent bits (Watchlist, Go to
/// Show, Play Version, restart, trailer, Also available), plus the item's watch state, which
/// decides which FACE the watched toggle wears.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct HeroSet {
    /// the item carries a `plex://movie|show` guid the Watchlist takes — the Watchlist row exists only then
    pub watchlist: bool,
    /// this is an EPISODE's own page and it names its show — the Go to Show disc exists only then
    pub go_show: bool,
    /// the loaded movie or episode has two or more versions — the Play Version disc exists only then
    pub version: bool,
    pub restart: bool,
    pub trailer: bool,
    pub alt: bool,
    pub mark: PosterMark,
}

/// The action row, in drawn order: the Play/Resume capsule and the More disc, whatever the item
/// offers. A fixed [`MAX_CTLS`]-slot array plus a live count, so no per-frame allocation; `set` is
/// taken so every enumeration of the row goes through one function.
pub fn hero_ctls(_set: HeroSet) -> ([HeroCtl; MAX_CTLS], usize) {
    let mut v = [HeroCtl::Play; MAX_CTLS];
    v[1] = HeroCtl::More;
    (v, 2)
}

/// The More menu's rows, in order: the plays first (from the start, the trailer, a version), then
/// navigation (Go to Show), then the item's own state (the watched toggle's one face, the
/// Watchlist), then the other source. The watched toggle is always offered, exactly once.
pub fn menu_ctls(set: HeroSet) -> ([HeroCtl; MAX_CTLS], usize) {
    let mut v = [HeroCtl::More; MAX_CTLS];
    let mut n = 0;
    let mut push = |c: HeroCtl| {
        v[n] = c;
        n += 1;
    };
    if set.restart { push(HeroCtl::Restart); }
    if set.trailer { push(HeroCtl::Trailer); }
    if set.version { push(HeroCtl::Version); }
    if set.go_show { push(HeroCtl::GoToShow); }
    push(if set.mark == PosterMark::Watched { HeroCtl::MarkUnwatched } else { HeroCtl::MarkWatched });
    if set.watchlist { push(HeroCtl::Watchlist); }
    if set.alt { push(HeroCtl::Alt); }
    (v, n)
}

/// One More-menu row per [`menu_ctls`] entry: its label (the episode-named watch verbs when the
/// hero presents a leaf; Add or Remove by the Watchlist membership the page reads now), its mark,
/// and whether it is destructive (Remove from Watchlist, as on every other item menu).
pub fn menu_rows(set: HeroSet, name_episode: bool, member: Membership) -> Vec<crate::registry::HeroMenuRow> {
    let (v, n) = menu_ctls(set);
    v[..n]
        .iter()
        .filter_map(|&ctl| {
            let (label, icon, destructive) = match ctl {
                HeroCtl::Watchlist => match member {
                    Membership::On => (
                        plx_platform::i18n::msg::watchlist_action_remove().to_string(),
                        plx_ui::icons::Icon::Close,
                        true,
                    ),
                    Membership::Off | Membership::Unknown => (
                        plx_platform::i18n::msg::watchlist_action_add().to_string(),
                        plx_ui::icons::Icon::Bookmark,
                        false,
                    ),
                },
                HeroCtl::Alt => (alt_label().to_string_lossy().into_owned(), ALT_ICON, false),
                ctl => (menu_verb(ctl, name_episode)?.to_string_lossy().into_owned(), menu_icon(ctl)?, false),
            };
            Some(crate::registry::HeroMenuRow { elem: ctl.elem(), label, icon, destructive })
        })
        .collect()
}

/// The mark a menu row wears: the glyph each control wore as a disc.
fn menu_icon(ctl: HeroCtl) -> Option<plx_ui::icons::Icon> {
    use plx_ui::icons::Icon;
    Some(match ctl {
        HeroCtl::Restart => Icon::Restart,
        HeroCtl::Trailer => Icon::Trailer,
        HeroCtl::MarkWatched => Icon::Check,
        HeroCtl::MarkUnwatched => Icon::Minus,
        HeroCtl::Version => VERSION_ICON,
        HeroCtl::GoToShow => GO_TO_SHOW_ICON,
        HeroCtl::Watchlist => Icon::Bookmark,
        HeroCtl::Alt => ALT_ICON,
        HeroCtl::Play | HeroCtl::More => return None,
    })
}

/// Whether `ctl` may hold — or be enumerated as holding — focus right now, given whether
/// full-trailer mode owns the screen. The ONE predicate [`visible_ctls`] (group extent) and
/// `DetailScreen::reconcile`/`valid` (focus legitimacy) all gate on, so the row cannot offer a
/// control the mode has taken away — the exact class of bug this predicate replaced (two
/// independent "what's visible" checks that had silently drifted apart).
///
/// **Play survives as the ANCHOR, not as a reachable control.** Full-trailer mode fades the whole
/// row out together (`DetailScreen::draw_buttons` draws every control from [`hero_ctls`]
/// regardless of this predicate — see its own doc — and lets the caller's `chrome`/`preview_chrome`
/// alpha carry the fade; `screens::detail::trailer` puts the trailer's transport up instead) — but
/// the engine still needs somewhere legitimate for the focus to stand while the mode is up, and an
/// EMPTY hero group would push it into the sections below, scrolling the page out from under a
/// trailer nobody asked to leave. Keys never reach the row anyway: the mode answers them itself,
/// before the engine.
pub fn focusable(ctl: HeroCtl, full_trailer: bool) -> bool {
    !full_trailer || ctl == HeroCtl::Play
}

/// [`hero_ctls`], filtered through [`focusable`] for full-trailer mode's FOCUS/hit-testing extent
/// only — the transient UI state where the row narrows to its Play anchor alone and everything
/// else (Restart/Trailer/Alt/the watch toggle) stops being reachable the instant `full_trailer()`
/// flips. **This is not what gets drawn.** `DetailScreen::draw_buttons` draws every control
/// [`hero_ctls`] returns, full_trailer or not, so the row fades out together with the rest of the
/// chrome instead of four pills hard-cutting a frame ahead of the one that fades (2026-09-17
/// fix — a control losing its focus must not also lose its paint on the same frame). Deliberately
/// NOT a flag on [`HeroSet`]: that struct describes what the ITEM offers, not a screen's transient
/// presentation mode. Every call site that enumerates the row for focus/hit-testing extent must go
/// through this rather than `hero_ctls` directly, or the two can disagree about which controls
/// exist right now.
pub fn visible_ctls(set: HeroSet, full_trailer: bool) -> ([HeroCtl; MAX_CTLS], usize) {
    let (all, n) = hero_ctls(set);
    if !full_trailer {
        return (all, n);
    }
    let mut v = [HeroCtl::Play; MAX_CTLS];
    let mut count = 0;
    for &c in &all[..n] {
        if focusable(c, full_trailer) {
            v[count] = c;
            count += 1;
        }
    }
    (v, count)
}

/// The control at index `i` in `set`, or `None` for an index the set does not have.
pub fn ctl_at(set: HeroSet, i: usize) -> Option<HeroCtl> {
    let (v, n) = hero_ctls(set);
    (i < n).then(|| v[i])
}

/// Where `ctl` sits in `set`, or `None` when the set does not include it.
pub fn index_of(set: HeroSet, ctl: HeroCtl) -> Option<usize> {
    let (v, n) = hero_ctls(set);
    v[..n].iter().position(|&c| c == ctl)
}

/// Has this show been started AT ALL? PMS puts `S1E1 off=0` on deck for a show nobody has
/// touched, so "there is something on deck" is not the same question as "this show is underway".
/// A locally marked on-deck leaf remains the subject until the server replaces it.
pub fn show_started(d: &Detail) -> bool {
    d.on_deck.as_ref().is_some_and(|e| e.resume_ms > 0 || e.watched)
        || d.seasons.iter().any(|s| s.viewed_leaf_count > 0)
}

/// The episode a show's hero is ABOUT — the server's on-deck episode, once the show has actually
/// been started. `None` means the hero presents the SERIES instead (finished, or never opened).
/// Ported verbatim from `ui/detail.rs::hero_episode`.
pub fn hero_episode(d: &Detail) -> Option<&Episode> {
    let ep = d.on_deck.as_ref()?;
    show_started(d).then_some(ep)
}

/// The resume position (ns) the hero's Play control would ACTUALLY apply. Ported verbatim from
/// `ui/detail.rs::hero_resume_ns`.
pub fn hero_resume_ns(d: &Detail) -> i64 {
    if d.is_show {
        hero_episode(d)
            .map(|e| plx_data::metadata::resume_ns(e.resume_ms, e.dur_ms))
            .unwrap_or(0)
    } else {
        plx_data::metadata::resume_ns(d.resume_ms, d.dur_ms)
    }
}

/// Is there a resume point for the restart disc to ignore?
pub fn has_restart(resume_ns: i64) -> bool {
    resume_ns > 0
}

/// The loaded item's watch state, in the app's one watch-state vocabulary. Ported verbatim from
/// `ui/detail.rs::hero_watch_state`.
pub fn hero_watch_state(
    is_show: bool,
    show_started: bool,
    watched: bool,
    resume_ns: i64,
) -> PosterMark {
    let mid_run = is_show && show_started && !watched;
    if resume_ns > 0 || mid_run {
        PosterMark::InProgress
    } else if watched {
        PosterMark::Watched
    } else {
        PosterMark::None
    }
}

/// Watch state of the episode displayed by the hero, or the series when there is no on-deck subject.
pub fn hero_mark(d: &Detail) -> PosterMark {
    if let Some(ep) = hero_episode(d) {
        hero_watch_state(false, false, ep.watched, plx_data::metadata::resume_ns(ep.resume_ms, ep.dur_ms))
    } else { hero_watch_state(d.is_show, show_started(d), d.watched, hero_resume_ns(d)) }
}

/// Name the episode explicitly when the hero presents a leaf, including a show's on-deck item.
pub fn watch_names_episode(d: &Detail) -> bool {
    d.kind == "episode" || hero_episode(d).is_some()
}

/// The Go to Show disc's target: `(show_rk, season)` for an EPISODE's own page, `None` for a
/// movie and for a show's page (whose hero is already about the show). A season is not its own
/// page here — it is the show page with that season selected — so it has nothing to link to.
pub fn go_to_show_target(d: &Detail) -> Option<(&str, i64)> {
    (d.kind == "episode" && !d.show_rk.is_empty()).then_some((d.show_rk.as_str(), d.season))
}

/// The verb a More-menu row names — `None` for Play and More (the row's own two controls), for
/// Also available (its own label, [`alt_label`]) and for the Watchlist (membership decides).
pub fn menu_verb(ctl: HeroCtl, name_episode: bool) -> Option<&'static CStr> {
    match (ctl, name_episode) {
        (HeroCtl::Restart, _) => Some(play_from_start_label()),
        (HeroCtl::Trailer, _) => Some(trailer_label()),
        (HeroCtl::MarkWatched, false) => Some(mark_watched_label()),
        (HeroCtl::MarkWatched, true) => Some(mark_episode_watched_label()),
        (HeroCtl::MarkUnwatched, false) => Some(mark_unwatched_label()),
        (HeroCtl::MarkUnwatched, true) => Some(mark_episode_unwatched_label()),
        (HeroCtl::Version, _) => Some(play_version_label()),
        (HeroCtl::GoToShow, _) => Some(go_to_show_label()),
        _ => None,
    }
}

// ---- the Watchlist row (docs/watchlist-plan.md §1.8) ---------------------------------------------

use plx_data::catalog::Membership;


/// The 12 hex bytes of a PMS item's `plex://movie|show/<hex>` guid, `None` for any other shape.
pub fn guid_hex(guid: &str) -> Option<[u8; 12]> {
    plx_data::watchlist::decode_hex(guid.rsplit('/').next()?)
}

/// What a press on the disc asks for: Add when the title is not (known to be) on the list, Remove
/// when it is. `None` when the item carries no guid in the form the service takes — the disc then
/// stays drawn but the press does nothing. The Remove arm is keyed by the guid string, like the
/// menu's own.
pub fn press(guid: &str, m: Membership) -> Option<crate::registry::ItemMenuAction> {
    use crate::registry::ItemMenuAction as Action;
    let hex = guid_hex(guid)?;
    Some(match m {
        Membership::On => Action::WatchlistRemove(guid.to_string()),
        Membership::Off | Membership::Unknown => Action::WatchlistAdd { hex },
    })
}

#[cfg(test)]
#[path = "hero_watchlist_tests.rs"]
mod watchlist_tests;

/// The Trailer disc's play fields — extra identity and HUD title, always from start.
/// `None` when the loaded item has no playable trailer (no button, no-op activate).
pub fn trailer_play(d: &Detail) -> Option<(&Extra, &str)> {
    let extra = d.trailer().filter(|e| e.playable())?;
    Some((extra, extra.hud_title(d.title.as_str())))
}

/// The Play pill's label — the word the press will actually perform.
pub fn hero_pill_label(has_restart: bool) -> &'static CStr {
    plx_appkit::hero_layout::play_label(has_restart)
}

pub fn hero_pill_w(measure: &dyn Measure, has_restart: bool) -> f32 {
    plx_appkit::hero_layout::play_width(measure, hero_pill_label(has_restart))
}

/// The Play capsule's width: its measured label, never narrower than the design's wide pill
/// ([`theme::hero::DETAIL_PLAY_W`]), so Play and Resume stand as one broad target.
pub fn play_w(measure: &dyn Measure, has_restart: bool) -> f32 {
    hero_pill_w(measure, has_restart).max(theme::hero::DETAIL_PLAY_W)
}

/// The row's widths in drawn order: the capsule, then the More disc.
fn control_widths(set: HeroSet, play: f32) -> ([f32; MAX_CTLS], usize) {
    let (v, n) = hero_ctls(set);
    let mut widths = [0.0f32; MAX_CTLS];
    for (slot, c) in widths.iter_mut().zip(&v[..n]) {
        *slot = if *c == HeroCtl::Play { play } else { CD };
    }
    (widths, n)
}

/// The rect the whole action row occupies from `y` (page coordinates): from the text column's
/// left edge to the last control's right edge. What the control ground is sampled over.
pub fn hero_row_rect(set: HeroSet, y: f32, play: f32) -> Rect {
    let (_, n) = hero_ctls(set);
    let last = hero_btn_rect_at(set, n.saturating_sub(1), y, play);
    Rect::new(TEXT_X, y, last.x + last.w - TEXT_X, CD)
}

/// The drawn frame of control `i`, using the shared row spacing from the text column.
pub fn hero_btn_rect_at(set: HeroSet, i: usize, y: f32, play: f32) -> Rect {
    let (widths, n) = control_widths(set, play);
    plx_appkit::hero_layout::row_rect_in(TEXT_X, TEXT_X + HERO_TEXT_W, &widths[..n], i, y)
}

/// The credit table's crew row: Created by (a show's writers) or Directed by.
pub fn hero_credit(d: &Detail) -> Option<(&'static str, Vec<&str>)> {
    if d.is_show {
        let names: Vec<&str> = d
            .crew
            .iter()
            .filter(|credit| credit.role.contains("Writer"))
            .map(|credit| credit.tag.as_str())
            .collect();
        return (!names.is_empty()).then_some((plx_platform::i18n::msg::browse_detail_created_by(), names));
    }
    let names: Vec<&str> = d.directors.iter().map(String::as_str).collect();
    (!names.is_empty()).then_some((plx_platform::i18n::msg::browse_detail_directed_by(), names))
}

/// The credit table's rows, top to bottom: Genre, Directed by / Created by, Starring — each an
/// uppercase label and a value line; a row the item has no names for is absent.
pub fn credit_rows(d: &Detail) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    let genres: Vec<&str> = d.genres.iter().take(GENRE_SUMMARY).map(String::as_str).collect();
    if !genres.is_empty() {
        rows.push((plx_platform::i18n::msg::browse_library_genre().to_uppercase(), genres.join(", ")));
    }
    if let Some((label, names)) = hero_credit(d) {
        rows.push((label.to_uppercase(), names.join(", ")));
    }
    let cast: Vec<&str> = d.cast.iter().take(STARRING_SUMMARY).map(|c| c.tag.as_str()).collect();
    if !cast.is_empty() {
        rows.push((plx_platform::i18n::msg::browse_detail_starring().to_uppercase(), cast.join(", ")));
    }
    rows
}

/// The rows [`credit_rows`] can yield: Genre, the crew row and Starring.
pub const CREDIT_ROWS: usize = 3;

/// Where the credit table's rule and values stand for these `rows`: the label column is as wide
/// as its widest label, so every rule stands on one vertical line. `(rule_x, value_x)`.
pub fn credit_columns(rows: &[(String, String)], measure: &dyn Measure) -> (f32, f32) {
    use plx_ui::detail_layout::{CREDIT_RULE_GAP, CREDIT_RULE_W};
    let label_w = rows
        .iter()
        .map(|(label, _)| measure.width_str(label, theme::size::CAPTION, true))
        .fold(0.0, f32::max);
    let rule_x = TEXT_X + label_w + CREDIT_RULE_GAP;
    (rule_x, rule_x + CREDIT_RULE_W + CREDIT_RULE_GAP)
}

/// Draw the credit table from `y`: the uppercase labels in the label ink, a thin vertical rule,
/// then the values one line each, elided at the column's end (server text, so by character).
pub fn draw_credits(p: Painter, rows: &[(String, String)], y: f32, measure: &dyn Measure) {
    use plx_ui::detail_layout::{CREDIT_INK, CREDIT_LABEL_INK, CREDIT_LEAD, CREDIT_RULE_W};
    if rows.is_empty() {
        return;
    }
    let (rule_x, value_x) = credit_columns(rows, measure);
    let value_w = (TEXT_X + HERO_TEXT_W - value_x).max(0.0);
    let items = [Item::new(CREDIT_LEAD); CREDIT_ROWS];
    let stack = VStack::new(&items[..rows.len().min(CREDIT_ROWS)], 0.0, Anchor::Top(y));
    for (span, (label, value)) in stack.placed().zip(rows) {
        let cy = span.y + span.h * 0.5;
        if let Ok(label) = CString::new(label.as_str()) {
            let ly = plx_gfx::text::text_vcenter_y(theme::size::CAPTION, 1, cy);
            p.text(label.as_ptr(), TEXT_X, ly, theme::size::CAPTION, CREDIT_LABEL_INK, 0, 1);
        }
        let value = plx_gfx::text::elide_by(value, value_w, true, |t| measure.width_str(t, theme::size::LABEL, false));
        if let Ok(value) = CString::new(value) {
            let vy = plx_gfx::text::text_vcenter_y(theme::size::LABEL, 0, cy);
            p.text(value.as_ptr(), value_x, vy, theme::size::LABEL, CREDIT_INK, 0, 0);
        }
    }
    let top = stack.span(0).map_or(y, |(y, _)| y);
    let foot = stack.placed().last().map_or(y, |s| s.y + s.h);
    p.rect(Rect::new(rule_x, top, CREDIT_RULE_W, foot - top), 0.0, theme::HAIRLINE, theme::HAIRLINE, 0.0);
}

/// The technical pills, left to right: the picture (resolution, then Dolby Vision or HDR) in the
/// video ink, the primary audio track (codec and layout) in the audio ink, then the accessibility
/// chips (CC, SDH, AD) in their own.
pub fn tech_pills(d: &Detail) -> Vec<(String, [f32; 4])> {
    let mut out = Vec::new();
    if let Some(res) = plx_ui::fmt::resolution(&d.video_resolution, d.width, d.height) {
        out.push((res, theme::TECH_VIDEO_INK));
    }
    if d.dovi.present {
        out.push(("DV".to_string(), theme::TECH_VIDEO_INK));
    } else if d.hdr {
        out.push(("HDR".to_string(), theme::TECH_VIDEO_INK));
    }
    let track = d.audio.first();
    let codec = track.map_or(d.acodec.as_str(), |s| s.codec.as_str());
    if !codec.is_empty() {
        let layout = track.map(|s| plx_ui::fmt::channel_layout(&s.layout, s.channels)).unwrap_or_default();
        let codec = codec.to_uppercase();
        out.push((if layout.is_empty() { codec } else { format!("{codec} {layout}") }, theme::TECH_AUDIO_INK));
    }
    for (present, label) in [
        (!d.subs.is_empty(), plx_platform::i18n::msg::widgets_badge_cc()),
        (d.subs.iter().any(|s| s.sdh), plx_platform::i18n::msg::widgets_badge_sdh()),
        (d.audio.iter().any(|s| s.ad), plx_platform::i18n::msg::widgets_badge_ad()),
    ] {
        if present {
            out.push((label.to_string(), theme::TECH_ACCESS_INK));
        }
    }
    out
}

/// Draw the pills on the row whose top is `y`, stopping before one would cross the column's end.
pub fn draw_tech(p: Painter, pills: &[(String, [f32; 4])], y: f32, measure: &dyn Measure) {
    let cy = y + plx_ui::widgets::BADGE_H * 0.5;
    let mut flow = Flow::new(TEXT_X, plx_ui::detail_layout::TECH_GAP);
    for (label, ink) in pills {
        let w = plx_ui::widgets::badge_w(label, None, measure);
        if flow.next_x() + w > TEXT_X + HERO_TEXT_W {
            break;
        }
        let style = plx_ui::widgets::BadgeStyle::Ring { ink: *ink };
        flow.place(|x| Item::new(plx_ui::widgets::badge(p, x, cy, label, None, style, measure)));
    }
}

fn hero_facts(d: &Detail) -> (String, Option<String>) {
    plx_appkit::hero_layout::meta_facts(d, hero_episode(d))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PlayNote {
    Quiet,
    Soft,
    Warn,
}

fn play_note(
    preview: plx_media::route::Preview,
    hdr: bool,
    subscription: plx_plex::plex::serverinfo::Subscription,
) -> PlayNote {
    let converts = preview == plx_media::route::Preview::Converts;
    let no_pass = subscription == plx_plex::plex::serverinfo::Subscription::No;
    if converts && no_pass && hdr {
        PlayNote::Warn
    } else if converts && no_pass {
        PlayNote::Soft
    } else {
        PlayNote::Quiet
    }
}

fn item_subscription(d: &Detail) -> plx_plex::plex::serverinfo::Subscription {
    plx_plex::plex::serverinfo::subscription_of(d.sid)
}

const FACTS_GLYPH_D: f32 = theme::size::CAPTION as f32;
fn converts_on_server_c() -> &'static CStr { plx_platform::i18n::msg::browse_detail_converts_c() }

#[derive(Clone, Copy)]
enum Bit {
    Word(&'static CStr, [f32; 4], c_int),
    /// The separator dot, `FACTS_SEP_PAD` from the run on each side (the flow's own gap).
    Sep,
    Glyph,
    /// The gap after the bit before it, in place of the flow's own (`Item::after`).
    Air(f32),
    Capsule,
}

const FACTS_BITS: usize = 8;

fn play_mode_bits(d: &Detail, after: bool) -> ([Bit; FACTS_BITS], usize) {
    match plx_media::route::playback_preview(d) {
        Some(plx_media::route::Preview::DirectPlay | plx_media::route::Preview::Remux) => ([Bit::Air(0.0); FACTS_BITS], 0),
        Some(preview) => play_mode_bits_for(preview, d.hdr, item_subscription(d), after),
        None => ([Bit::Air(0.0); FACTS_BITS], 0),
    }
}

/// [`play_mode_bits`] once the item's route has been read: the line's play-mode clause, behind a
/// separator of its own when something already stands in front of it (`after`).
fn play_mode_bits_for(
    preview: plx_media::route::Preview,
    hdr: bool,
    subscription: plx_plex::plex::serverinfo::Subscription,
    after: bool,
) -> ([Bit; FACTS_BITS], usize) {
    let mut bits = [Bit::Air(0.0); FACTS_BITS];
    let mut n = 0;
    let mut push = |bit: Bit| {
        bits[n] = bit;
        n += 1;
    };
    if after {
        push(Bit::Sep);
    }
    match play_note(preview, hdr, subscription) {
        PlayNote::Quiet => push(Bit::Word(
            match preview {
                plx_media::route::Preview::DirectPlay => plx_platform::i18n::msg::browse_detail_direct_play_c(),
                plx_media::route::Preview::Remux => plx_platform::i18n::msg::browse_detail_direct_stream_c(),
                plx_media::route::Preview::OriginalAudioConverted => {
                    plx_platform::i18n::msg::browse_detail_original_audio_converted_c()
                }
                plx_media::route::Preview::Converts => converts_on_server_c(),
            },
            plx_ui::detail_layout::FACTS_INK,
            0,
        )),
        PlayNote::Soft => {
            push(Bit::Word(converts_on_server_c(), plx_ui::detail_layout::FACTS_INK, 0));
            push(Bit::Sep);
            push(Bit::Word(
                plx_platform::i18n::msg::browse_detail_hardware_needs_c(),
                theme::TEXT_SECONDARY,
                0,
            ));
            push(Bit::Air(theme::space::SM));
            push(Bit::Capsule);
        }
        PlayNote::Warn => {
            push(Bit::Glyph);
            push(Bit::Air(theme::space::SM));
            push(Bit::Word(c"HDR \u{2192} SDR", theme::TEXT_SECONDARY, 1));
            push(Bit::Sep);
            push(Bit::Word(plx_platform::i18n::msg::browse_detail_tonemap_needs_c(), theme::TEXT_SECONDARY, 0));
            push(Bit::Air(theme::space::SM));
            push(Bit::Capsule);
        }
    }
    (bits, n)
}

/// **The play-mode clause's one layout**, from `x`: each bit with the span it draws, as a [`Flow`]
/// whose own gap is `FACTS_SEP_PAD` (word, dot, word) and in which an [`Bit::Air`] is the previous
/// bit's [`Item::after`]. A clause that opens with its dot stands that dot `FACTS_SEP_PAD` after
/// `x`, the end of the run in front, which a zero-width item for that run provides. Returns the
/// width, `x` to the last bit's trailing edge.
fn bits_flow(
    bits: &[Bit],
    x: f32,
    measure: &dyn plx_machine::machine::Measure,
    mut each: impl FnMut(Bit, Span),
) -> f32 {
    let mut flow = Flow::new(x, FACTS_SEP_PAD);
    if matches!(bits.first(), Some(Bit::Sep)) {
        flow.push(Item::new(0.0));
    }
    for (i, bit) in bits.iter().enumerate() {
        let w = match *bit {
            Bit::Word(text, _, bold) => measure.width(text, theme::size::CAPTION, bold != 0),
            Bit::Sep => measure.width(c"\u{b7}", theme::size::CAPTION, false),
            Bit::Glyph => FACTS_GLYPH_D,
            Bit::Capsule => plx_ui::widgets::pass_capsule_w(measure),
            Bit::Air(_) => continue,
        };
        let item = match bits.get(i + 1) {
            Some(Bit::Air(gap)) => Item::new(w).after(*gap),
            _ => Item::new(w),
        };
        if let Some(span) = flow.push(item) {
            each(*bit, span);
        }
    }
    flow.end() - x
}

fn play_mode_w(d: &Detail, after: bool, measure: &dyn plx_machine::machine::Measure) -> f32 {
    let (bits, n) = play_mode_bits(d, after);
    bits_flow(&bits[..n], 0.0, measure, |_, _| {})
}

fn draw_play_mode(
    p: Painter,
    d: &Detail,
    x: f32,
    y: f32,
    after: bool,
    measure: &dyn plx_machine::machine::Measure,
) -> f32 {
    let (bits, n) = play_mode_bits(d, after);
    paint_bits(p, &bits[..n], x, y, measure)
}

/// Paint the play-mode clause's `bits` from `x` on the line whose text top is `y`; the width drawn.
fn paint_bits(p: Painter, bits: &[Bit], x: f32, y: f32, measure: &dyn plx_machine::machine::Measure) -> f32 {
    let (top, baseline) = plx_gfx::text::text_cap_band(theme::size::CAPTION, 0);
    let cy = y + (top + baseline) * 0.5;
    bits_flow(bits, x, measure, |bit, span| match bit {
        Bit::Word(text, color, bold) => {
            p.text(text.as_ptr(), span.x, y, theme::size::CAPTION, color, 0, bold);
        }
        Bit::Sep => {
            p.text(c"\u{b7}".as_ptr(), span.x, y, theme::size::CAPTION, theme::TEXT_SEPARATOR, 0, 0);
        }
        Bit::Glyph => plx_ui::icons::draw(
            p,
            plx_ui::icons::Icon::Alert,
            Rect::new(span.x, cy - FACTS_GLYPH_D * 0.5, FACTS_GLYPH_D, FACTS_GLYPH_D),
            theme::TEXT_SECONDARY,
        ),
        Bit::Capsule => {
            plx_ui::widgets::pass_capsule(p, span.x, cy, false, measure);
        }
        Bit::Air(_) => {}
    })
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct FactsFit {
    extent: bool,
    credit: bool,
    elide: bool,
}

// Not `ui::fit::two_runs`: this is a drop cascade (extent, then credit, then elide) over several
// runs, not one primary/secondary pair.
fn facts_fit(has_credit: bool, budget: f32, mut width: impl FnMut(FactsFit) -> f32) -> FactsFit {
    let mut fit = FactsFit {
        extent: true,
        credit: has_credit,
        elide: false,
    };
    if width(fit) <= budget {
        return fit;
    }
    if fit.credit {
        fit.credit = false;
        if width(fit) <= budget {
            return fit;
        }
    }
    fit.extent = false;
    fit.elide = width(fit) > budget;
    fit
}

fn facts_flow(
    parts: &[&str],
    credit: &str,
    mut run: impl FnMut(&str, f32, c_int, [f32; 4]) -> f32,
    mode: impl FnOnce(f32, bool) -> f32,
) -> f32 {
    // The date, the extent and the dots between them are one `Flow` whose gap is the dot's pad; the
    // play-mode clause carries its own leading dot (`mode` is told whether something precedes it),
    // so the credit's flow starts at the clause's end with a zero-width item standing for it.
    let dot = |run: &mut dyn FnMut(&str, f32, c_int, [f32; 4]) -> f32, x: f32| {
        Item::new(run("\u{b7}", x, theme::size::CAPTION, theme::TEXT_SEPARATOR))
    };
    let mut flow = Flow::new(0.0, FACTS_SEP_PAD);
    let mut any = false;
    for part in parts.iter().filter(|part| !part.is_empty()) {
        if any {
            flow.place(|x| dot(&mut run, x));
        }
        flow.place(|x| Item::new(run(part, x, theme::size::CAPTION, plx_ui::detail_layout::FACTS_INK)));
        any = true;
    }
    // the clause abuts the run before it: it stands its own dot `FACTS_SEP_PAD` out
    let mode_w = mode(flow.end(), any);
    any |= mode_w > 0.0;
    let mut flow = Flow::new(flow.end() + mode_w, FACTS_SEP_PAD);
    if !credit.is_empty() {
        if any {
            flow.push(Item::new(0.0)); // the run in front (the last run or the clause)
            flow.place(|x| dot(&mut run, x));
        }
        flow.place(|x| Item::new(run(credit, x, theme::size::CAPTION, plx_ui::detail_layout::FACTS_INK)));
    }
    flow.end()
}

/// **The hero's two names**: the text the title band stands for (the logo's fallback when there is
/// no clearLogo) and the subtitle line under it ([`plx_ui::detail_layout::HeroRows::subtitle`]).
/// An episode's or a season's page whose show is known stands under the SHOW's name (its logo is
/// the show's) with its own title on the line below, Plex mobile's detail page: "The Show" /
/// "The Episode". Every other page, and one whose own title is its show's, names itself once.
pub fn hero_names(d: &Detail) -> (&str, Option<&str>) {
    band_names(matches!(d.kind.as_str(), "episode" | "season"), &d.title, &d.show_title)
}

/// The title band's name for the SELECTION a page was opened on, before its [`Detail`] lands: the
/// same rule as [`hero_names`]`.0`, read off the browse row (`PmsMovie::kind`/`show_title`), so an
/// episode whose show has no clearLogo names the show from the first frame instead of swapping its
/// own title for the show's when the item lands.
pub fn selection_band_name(m: &plx_data::pms::PmsMovie) -> &str {
    use plx_data::pms::{KIND_EPISODE, KIND_SEASON};
    band_names(matches!(m.kind, KIND_EPISODE | KIND_SEASON), &m.title, &m.show_title).0
}

/// [`hero_names`]' rule on its three inputs: an item that stands under a show (`under_show_kind`)
/// whose show is known and whose own title is non-empty and not the show's is named by the show,
/// with its own title as the subtitle; anything else names itself once.
fn band_names<'a>(under_show_kind: bool, title: &'a str, show_title: &'a str) -> (&'a str, Option<&'a str>) {
    let under_show = under_show_kind && !show_title.is_empty();
    match under_show.then_some(title).filter(|own| !own.is_empty() && *own != show_title) {
        Some(own) => (show_title, Some(own)),
        None => (title, None),
    }
}

/// What leads the meta line before the date: the edition, and on an episode's own page its season
/// and its episode number as two runs ("S1 · E1"). The show is not repeated here: the title band
/// above names it ([`hero_names`]).
fn facts_lead(d: &Detail) -> Vec<String> {
    use plx_platform::i18n::msg;
    let mut lead = Vec::new();
    if !d.edition.is_empty() {
        lead.push(d.edition.clone());
    }
    if d.kind == "episode" {
        // a number that is not positive is no number (`fmt::episode_ordinal`'s rule)
        if d.season > 0 {
            lead.push(msg::core_season(d.season));
        }
        if d.index > 0 {
            lead.push(msg::core_episode(d.index));
        }
    }
    lead
}

/// Draw the subtitle line ([`hero_names`]) at `y` in the text column: one HEADLINE bold line in the
/// title's ink, elided to the reading column.
pub fn draw_subtitle(p: Painter, text: &str, y: f32, measure: &dyn plx_machine::machine::Measure) {
    let size = theme::hero::SUBTITLE_SIZE;
    let line = plx_gfx::text::elide_by(text, HERO_TEXT_W, false, |t| measure.width_str(t, size, true));
    if let Ok(line) = CString::new(line) {
        plx_ui::label::Label::new(line.as_ptr(), size, theme::TEXT_PRIMARY)
            .bold()
            .draw(p, Rect::new(TEXT_X, y, HERO_TEXT_W, theme::hero::SUBTITLE_H));
    }
}

/// The meta line's runs in order; an empty one is skipped by [`facts_flow`].
fn facts_line<'a>(lead: &'a [String], date: &'a str, extent: &'a str, rating: &'a str, fit: FactsFit) -> Vec<&'a str> {
    let mut v: Vec<&str> = lead.iter().map(String::as_str).collect();
    v.push(date);
    if fit.extent {
        v.push(extent);
    }
    v.push(rating);
    v
}

/// Draw the meta line at `y` in the text column: (edition · Sx · Ey ·) date · runtime or extent
/// · content rating, then how it plays and who shares it. Returns the width drawn from the column.
pub fn draw_facts(p: Painter, d: &Detail, y: f32, measure: &dyn plx_machine::machine::Measure) -> f32 {
    let (date, extent) = hero_facts(d);
    let extent = extent.as_deref().unwrap_or("");
    let credit = plx_ui::fmt::shared_by(&d.source()).unwrap_or_default();
    let lead = facts_lead(d);
    let column = Rect::new(TEXT_X, y, HERO_TEXT_W, theme::hero::FACTS_H);
    if play_mode_w(d, false, measure) == 0.0 && lead.is_empty() && d.rating.is_empty() {
        let resumes = hero_resume_ns(d) > 0;
        return plx_ui::widgets::hero_facts(p, column, &date, extent, resumes, &credit, measure);
    }
    let line = |date, fit| facts_line(&lead, date, extent, &d.rating, fit);
    let width = |fit: FactsFit| {
        facts_flow(
            &line(date.as_str(), fit),
            if fit.credit { &credit } else { "" },
            |text, _, size, _| measure.width_str(text, size, false),
            |_, after| play_mode_w(d, after, measure),
        )
    };
    let fit = facts_fit(!credit.is_empty(), column.w, width);
    let elided;
    let date = if fit.elide {
        let rest = facts_flow(&line("", fit), "", |text, _, size, _| measure.width_str(text, size, false), |_, after| {
            play_mode_w(d, after, measure)
        });
        let budget = (column.w - rest - FACTS_SEP_PAD * 2.0).max(0.0);
        elided = plx_gfx::text::elide_by(&date, budget, false, |t| {
            measure.width_str(t, theme::size::CAPTION, false)
        });
        elided.as_str()
    } else {
        date.as_str()
    };
    facts_flow(
        &line(date, fit),
        if fit.credit { &credit } else { "" },
        |text, dx, size, color| {
            CString::new(text)
                .ok()
                .map(|text| p.text(text.as_ptr(), TEXT_X + dx, y, size, color, 0, 0))
                .unwrap_or(0.0)
        },
        |dx, after| draw_play_mode(p, d, TEXT_X + dx, y, after, measure),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // TEST ONLY: this module's few call sites still read/write metadata as free functions, a
    // shape written for the old crate-global statics. A thread-confined store (same pattern as
    // `screens::detail::tests`'s `TEST_METADATA`) gives them a real, per-owner `MetadataStore`
    // without threading one through every helper here.
    thread_local! {
        static TEST_METADATA: std::cell::UnsafeCell<plx_data::stores::metadata::MetadataStore> =
            std::cell::UnsafeCell::new(plx_data::stores::metadata::MetadataStore::default());
    }

    fn test_store() -> &'static mut plx_data::stores::metadata::MetadataStore {
        TEST_METADATA.with(|cell| unsafe { &mut *cell.get() })
    }

    fn set_full(restart: bool, alt: bool, mark: PosterMark, trailer: bool) -> HeroSet {
        HeroSet {

            watchlist: true,
            go_show: false,
            version: false,
            restart,
            trailer,
            alt,
            mark,
        }
    }

    /// **The More menu offers exactly ONE watched toggle, never both faces at once** — the
    /// property `ui/detail.rs`'s own module doc calls out by name ("this row briefly drew the two
    /// faces side by side for a part-watched item, and that was wrong").
    #[test]
    fn the_menu_offers_exactly_one_watched_toggle() {
        for mark in [
            PosterMark::None,
            PosterMark::InProgress,
            PosterMark::Watched,
        ] {
            for restart in [false, true] {
                for alt in [false, true] {
                    for trailer in [false, true] {
                        let s = set_full(restart, alt, mark, trailer);
                        let (v, n) = menu_ctls(s);
                        let watch_ctls: Vec<_> = v[..n].iter().filter(|c| c.is_watch()).collect();
                        assert_eq!(watch_ctls.len(), 1, "set={s:?}");
                    }
                }
            }
        }
    }

    /// **Full-trailer mode always narrows the row's FOCUS/pointer extent to exactly `[Play]`,
    /// whatever the item's own facts offer** — swept over every `HeroSet` this row can take. The
    /// property that keeps the mode's focus anchor single and legitimate. Drawing is the OPPOSITE:
    /// `DetailScreen::draw_buttons` always paints the full `hero_ctls` row, full-trailer or not
    /// (2026-09-17 fix — the row fades out together with the rest of the chrome instead of four
    /// pills hard-cutting a frame ahead of Play), so this also pins that the two predicates
    /// genuinely diverge whenever the set has more than Play alone.
    #[test]
    fn full_trailer_mode_collapses_focus_to_play_only_but_leaves_drawing_alone() {
        for mark in [
            PosterMark::None,
            PosterMark::InProgress,
            PosterMark::Watched,
        ] {
            for restart in [false, true] {
                for alt in [false, true] {
                    for trailer in [false, true] {
                        let s = set_full(restart, alt, mark, trailer);
                        let (v, n) = visible_ctls(s, true);
                        assert_eq!(n, 1, "set={s:?}");
                        assert_eq!(v[0], HeroCtl::Play, "set={s:?}");
                        // And the non-full-trailer path must be byte-identical to `hero_ctls` —
                        // `visible_ctls` is a strict narrowing, never a second row model.
                        assert_eq!(visible_ctls(s, false), hero_ctls(s), "set={s:?}");
                        // Drawing does not narrow with `visible_ctls` — `draw_buttons` iterates
                        // `hero_ctls` directly — so drawing always sees strictly more than
                        // full-trailer mode makes focusable/pointer-reachable: the More disc
                        // (unconditional in `hero_ctls`) already outnumbers the `[Play]` focus set.
                        let (_, drawn) = hero_ctls(s);
                        assert!(
                            drawn > n,
                            "set={s:?}: drawing must stay decoupled from the focus narrowing"
                        );
                    }
                }
            }
        }
    }

    /// A finished-then-restarted item reads `InProgress`, never `Watched` — the viewer is
    /// part-way through a re-watch, and "not watched" is the honest state for the toggle to read.
    #[test]
    fn a_restarted_finished_item_reads_in_progress_not_watched() {
        // `watched=true` but a live resume point beats it.
        assert_eq!(
            hero_watch_state(false, false, true, 4_000_000_000),
            PosterMark::InProgress
        );
    }

    /// A show mid-run (started, not finished) reads `InProgress` even with no resume point of its
    /// own — `mid_run` is `is_show && show_started && !watched`.
    #[test]
    fn a_show_mid_run_reads_in_progress_with_no_resume_point_of_its_own() {
        assert_eq!(
            hero_watch_state(true, true, false, 0),
            PosterMark::InProgress
        );
        assert_eq!(
            hero_watch_state(true, true, true, 0),
            PosterMark::Watched,
            "…but a FINISHED show is simply watched"
        );
        assert_eq!(
            hero_watch_state(true, false, false, 0),
            PosterMark::None,
            "an unstarted show is simply unstarted"
        );
    }

    /// `ctl_at`/`index_of` are exact inverses over every index the set actually has.
    #[test]
    fn ctl_at_and_index_of_round_trip() {
        for restart in [false, true] {
            for alt in [false, true] {
                for trailer in [false, true] {
                    for mark in [
                        PosterMark::None,
                        PosterMark::InProgress,
                        PosterMark::Watched,
                    ] {
                        let s = set_full(restart, alt, mark, trailer);
                        let (_, n) = hero_ctls(s);
                        for i in 0..n {
                            let ctl = ctl_at(s, i).unwrap();
                            assert_eq!(index_of(s, ctl), Some(i));
                        }
                        assert_eq!(
                            ctl_at(s, n),
                            None,
                            "one past the end is None, never a panic"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_two_spellings_of_the_conversion_notice_are_the_same_bytes() {
        assert_eq!(
            converts_on_server_c().to_str().unwrap(),
            plx_ui::fmt::converts_on_server()
        );
    }

    #[test]
    fn how_it_plays_resolves_the_full_docs_truth_table() {
        use plx_plex::plex::serverinfo::Subscription::{No, Unknown, Yes};
        use plx_media::route::Preview::{Converts, DirectPlay, OriginalAudioConverted, Remux};
        for preview in [DirectPlay, Remux, OriginalAudioConverted, Converts] {
            for hdr in [false, true] {
                for subscription in [Unknown, No, Yes] {
                    let expected = match (preview, hdr, subscription) {
                        (Converts, true, No) => PlayNote::Warn,
                        (Converts, false, No) => PlayNote::Soft,
                        _ => PlayNote::Quiet,
                    };
                    assert_eq!(play_note(preview, hdr, subscription), expected);
                }
            }
        }
    }

    #[test]
    fn the_pass_note_judges_the_items_own_server_not_the_browsed_one() {
        let _guard = plx_base::testlock::serial();
        plx_plex::plex::reset_servers_for_test();
        let own = plx_plex::plex::register_for_test("own", "127.0.0.1", 1, "t", "c1");
        let shared = plx_plex::plex::register_for_test("shared", "127.0.0.2", 2, "t", "c2");
        plx_plex::plex::serverinfo::store_for_test(
            own,
            plx_plex::plex::serverinfo::Subscription::Yes,
            "1",
        );
        plx_plex::plex::serverinfo::store_for_test(
            shared,
            plx_plex::plex::serverinfo::Subscription::No,
            "1",
        );
        let borrowed = Detail {
            sid: shared,
            ..Default::default()
        };
        let ours = Detail {
            sid: own,
            ..Default::default()
        };
        assert_eq!(
            item_subscription(&borrowed),
            plx_plex::plex::serverinfo::Subscription::No
        );
        assert_eq!(
            item_subscription(&ours),
            plx_plex::plex::serverinfo::Subscription::Yes
        );
        plx_plex::plex::reset_servers_for_test();
    }

    #[test]
    fn an_item_on_your_own_server_gets_no_source_run_at_all() {
        #[derive(Debug, PartialEq)]
        enum Event {
            Run(String),
            Mode,
        }
        let events = std::cell::RefCell::new(Vec::new());
        facts_flow(
            &["13 Jun 2024", "1h 57m"],
            "",
            |text, _, _, _| {
                events.borrow_mut().push(Event::Run(text.into()));
                text.len() as f32
            },
            |_, _| {
                events.borrow_mut().push(Event::Mode);
                5.0
            },
        );
        assert_eq!(events.into_inner().last(), Some(&Event::Mode));
    }

    /// Q2, painted: the facts line is flowed and PAINTED through a recording painter, and every
    /// dot on it, the one before the play-mode clause included, stands `FACTS_SEP_PAD` after the
    /// run before it and `FACTS_SEP_PAD` before the run after it. With nothing in front of the
    /// clause there is no dot.
    #[test]
    fn a_painted_facts_line_pads_the_play_mode_dot_like_every_other_dot() {
        use plx_media::route::Preview;
        use plx_plex::plex::serverinfo::Subscription;
        let _g = plx_base::testlock::serial();
        let measure = plx_ui::widgets::LegacyMeasure;
        let paint = |parts: &[&str]| {
            let mut runs = plx_ui::draw_census::capture_runs(|| {
                let p = Painter::recording();
                facts_flow(
                    parts,
                    "",
                    |text, dx, size, color| {
                        let text = CString::new(text).unwrap();
                        p.text(text.as_ptr(), 100.0 + dx, 50.0, size, color, 0, 0)
                    },
                    |dx, after| {
                        let (bits, n) = play_mode_bits_for(Preview::DirectPlay, false, Subscription::No, after);
                        paint_bits(p, &bits[..n], 100.0 + dx, 50.0, &measure)
                    },
                );
            });
            runs.sort_by(|a, b| a.at.x.total_cmp(&b.at.x));
            runs
        };
        let line = paint(&["2021", "1h 40m"]);
        let dots: Vec<usize> = (0..line.len()).filter(|&i| line[i].text == "\u{b7}").collect();
        assert_eq!(dots.len(), 2, "one dot between the facts and one before the clause: {line:?}");
        for i in dots {
            let (before, after) = (&line[i - 1], &line[i + 1]);
            let lead = line[i].at.x - (before.at.x + before.at.w);
            let trail = after.at.x - (line[i].at.x + line[i].at.w);
            assert!((lead - FACTS_SEP_PAD).abs() < 0.5, "run {} -> dot: {lead}", i - 1);
            assert!((trail - FACTS_SEP_PAD).abs() < 0.5, "dot -> run {}: {trail}", i + 1);
        }
        let bare = paint(&["", ""]);
        assert!(bare.iter().all(|r| r.text != "\u{b7}"), "nothing in front of the clause, no dot: {bare:?}");
    }

    #[test]
    fn the_source_credit_is_the_last_run_on_the_line_after_the_play_mode_fragment() {
        let events = std::cell::RefCell::new(Vec::new());
        facts_flow(
            &["date", "extent"],
            "Shared by friend",
            |text, _, _, _| {
                events.borrow_mut().push(text.to_string());
                text.len() as f32
            },
            |_, _| {
                events.borrow_mut().push("<mode>".into());
                5.0
            },
        );
        assert_eq!(
            events.into_inner().last().map(String::as_str),
            Some("Shared by friend")
        );
    }

    #[test]
    fn a_credit_with_nothing_in_front_of_it_opens_the_line_bare() {
        let placed = std::cell::RefCell::new(Vec::new());
        facts_flow(
            &["", ""],
            "Shared by friend",
            |text, x, _, _| {
                placed.borrow_mut().push((text.to_string(), x));
                text.len() as f32
            },
            |_, _| 0.0,
        );
        assert_eq!(placed.into_inner(), vec![("Shared by friend".into(), 0.0)]);
    }

    #[test]
    fn the_trailing_credit_is_the_first_member_the_row_gives_up() {
    let width = |fit: FactsFit| {
            100.0 + if fit.extent { 100.0 } else { 0.0 } + if fit.credit { 150.0 } else { 0.0 }
        };
        assert_eq!(
            facts_fit(true, 349.0, width),
            FactsFit {
                extent: true,
                credit: false,
                elide: false
            }
        );
        assert_eq!(
            facts_fit(true, 199.0, width),
            FactsFit {
                extent: false,
                credit: false,
                elide: false
            }
        );
        assert!(facts_fit(true, 99.0, width).elide);
    }

    #[test]
    fn the_crew_credit_names_the_right_job_for_the_kind_of_item() {
        fn credit(name: &str, role: &str) -> plx_data::metadata::Cast {
            plx_data::metadata::Cast {
                tag: name.into(),
                role: role.into(),
                thumb: String::new(),
                id: 0,
                tag_key: String::new(),
            }
        }
        let movie = Detail {
            directors: vec!["Jane Doe".into()],
            ..Default::default()
        };
        assert_eq!(hero_credit(&movie), Some(("Directed by", vec!["Jane Doe"])));
        let show = Detail {
            is_show: true,
            crew: vec![
                credit("Pilot Director", "Director"),
                credit("Kerry Ehrin", "Writer"),
            ],
            ..Default::default()
        };
        assert_eq!(
            hero_credit(&show),
            Some(("Created by", vec!["Kerry Ehrin"]))
        );
    }

    #[test]
    fn a_shows_hero_is_about_the_servers_on_deck_episode_or_the_series() {
        let mut d = Detail {
            is_show: true,
            seasons: vec![plx_data::metadata::Season {
                rk: "s1".into(),
                index: 1,
                title: "Season 1".into(),
                leaf_count: 3,
                viewed_leaf_count: 0,
                guid: String::new(),
            }],
            on_deck: Some(Episode {
                rk: "e1".into(),
                resume_ms: 0,
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(
            hero_episode(&d).is_none(),
            "an untouched show is about the series"
        );
        d.seasons[0].viewed_leaf_count = 1;
        assert_eq!(hero_episode(&d).map(|ep| ep.rk.as_str()), Some("e1"));
        d.seasons[0].viewed_leaf_count = 0;
        d.on_deck.as_mut().unwrap().watched = true;
        assert_eq!(hero_episode(&d).map(|ep| ep.rk.as_str()), Some("e1"),
            "an optimistic watched flip retains the displayed leaf until the server replaces OnDeck");
        assert_eq!(hero_mark(&d), PosterMark::Watched);
    }

    #[test]
    fn the_watch_state_resolver_answers_leaf_and_container_by_their_own_rules() {
        assert_eq!(hero_watch_state(false, false, false, 0), PosterMark::None);
        assert_eq!(hero_watch_state(false, false, true, 0), PosterMark::Watched);
        assert_eq!(
            hero_watch_state(true, true, false, 0),
            PosterMark::InProgress
        );
        assert_eq!(hero_watch_state(true, true, true, 0), PosterMark::Watched);
    }

    #[test]
    fn the_optimistic_flip_settles_a_leaf_at_once_and_a_container_a_round_trip_late() {
        let _guard = plx_base::testlock::serial();
        let sid = plx_plex::plex::ServerId::UNSET;
        plx_data::metadata::set_current_for_test(test_store().state_mut(), Some(Detail {
            sid,
            rk: "movie".into(),
            resume_ms: 1_800_000,
            dur_ms: 7_200_000,
            ..Default::default()
        }));
        assert!(test_store().run(
            plx_data::stores::metadata::MetadataCmd::ApplyView {
                at: plx_data::viewstate::ViewAt::leaf(sid, "movie"),
                edit: plx_data::viewstate::ViewEdit::Watched(true),
            }
        ));
        let movie = test_store().view().current().unwrap();
        assert_eq!(hero_mark(movie), PosterMark::Watched);
        assert_eq!(
            movie.resume_ms, 0,
            "a leaf's restart disc disappears immediately"
        );

        plx_data::metadata::set_current_for_test(test_store().state_mut(), Some(Detail {
            sid,
            rk: "show".into(),
            is_show: true,
            seasons: vec![plx_data::metadata::Season {
                rk: "s1".into(),
                index: 1,
                title: "Season 1".into(),
                leaf_count: 2,
                viewed_leaf_count: 1,
                guid: String::new(),
            }],
            on_deck: Some(Episode {
                resume_ms: 600_000,
                dur_ms: 2_700_000,
                ..Default::default()
            }),
            ..Default::default()
        }));
        assert!(test_store().run(
            plx_data::stores::metadata::MetadataCmd::ApplyView {
                at: plx_data::viewstate::ViewAt::leaf(sid, "show"),
                edit: plx_data::viewstate::ViewEdit::Watched(true),
            }
        ));
        assert_eq!(
            hero_mark(test_store().view().current().unwrap()),
            PosterMark::InProgress,
            "container progress remains server evidence until the re-read lands"
        );
        plx_data::metadata::set_current_for_test(test_store().state_mut(), Some(Detail {
            sid,
            rk: "show".into(),
            is_show: true,
            watched: true,
            ..Default::default()
        }));
        assert_eq!(
            hero_mark(test_store().view().current().unwrap()),
            PosterMark::Watched
        );
        plx_data::metadata::set_current_for_test(test_store().state_mut(), None);
    }

    fn trailer_extra(rk: &str, part: &str, title: &str) -> Extra {
        Extra {
            rk: rk.into(),
            part: part.into(),
            title: title.into(),
            vcodec: "h264".into(),
            acodec: "aac".into(),
            subtype: "trailer".into(),
            extra_type: 1,
            dur_ms: 120_000,
            bitrate: 2_500,
            thumb: String::new(),
        }
    }

    #[test]
    fn trailer_play_uses_the_extra_not_the_parent_or_on_deck() {
        let extra = trailer_extra("9", "/library/parts/trailer", "Official Trailer");
        let movie = Detail {
            rk: "m".into(),
            title: "Movie".into(),
            part: "/library/parts/movie".into(),
            extras: vec![extra.clone()],
            ..Default::default()
        };
        let (got, title) = trailer_play(&movie).unwrap();
        assert_eq!(got.rk, "9");
        assert_eq!(got.part, "/library/parts/trailer");
        assert_eq!(title, "Official Trailer");
        assert_ne!(got.part, movie.part);

        let show = Detail {
            is_show: true,
            title: "Show".into(),
            on_deck: Some(Episode {
                rk: "ep".into(),
                part: "/library/parts/ep".into(),
                ..Default::default()
            }),
            extras: vec![extra],
            ..Default::default()
        };
        let (got, title) = trailer_play(&show).unwrap();
        assert_eq!(got.rk, "9");
        assert_eq!(got.part, "/library/parts/trailer");
        assert_eq!(title, "Official Trailer");
        assert_ne!(got.rk, show.on_deck.as_ref().unwrap().rk);
        assert!(trailer_play(&Detail::default()).is_none());
        assert!(
            trailer_play(&Detail {
                extras: vec![trailer_extra("", "/p", "T")],
                ..Default::default()
            })
            .is_none(),
            "a part without rk is not playable"
        );
        assert!(
            trailer_play(&Detail {
                extras: vec![trailer_extra("9", "", "T")],
                ..Default::default()
            })
            .is_none(),
            "an rk without part is not playable"
        );
        let untitled = Detail {
            title: "Parent".into(),
            extras: vec![trailer_extra("9", "/p", "")],
            ..Default::default()
        };
        assert_eq!(trailer_play(&untitled).unwrap().1, "Parent");
    }

    /// The action row is the wide Play capsule and the More disc, whatever the item offers: every
    /// other action is a row of the More menu.
    #[test]
    fn the_row_is_play_and_more_whatever_the_item_offers() {
        let full = HeroSet {
            watchlist: true,
            go_show: true,
            version: true,
            restart: true,
            trailer: true,
            alt: true,
            mark: PosterMark::InProgress,
        };
        for set in [HeroSet::default(), full] {
            let (row, n) = hero_ctls(set);
            assert_eq!(&row[..n], &[HeroCtl::Play, HeroCtl::More]);
            let (menu, m) = menu_ctls(set);
            assert!(!menu[..m].contains(&HeroCtl::Play) && !menu[..m].contains(&HeroCtl::More));
        }
        assert_eq!(index_of(full, HeroCtl::More), Some(1));
        assert_eq!(index_of(full, HeroCtl::Restart), None, "a menu action is not a row control");
    }

    /// The menu's order: the plays, then navigation, then the item's own state, then the other
    /// source; an action the item does not offer is absent and leaves no gap.
    #[test]
    fn the_menu_lists_the_plays_then_navigation_then_state() {
        let full = HeroSet {
            watchlist: true,
            go_show: true,
            version: true,
            restart: true,
            trailer: true,
            alt: true,
            mark: PosterMark::Watched,
        };
        let (v, n) = menu_ctls(full);
        assert_eq!(
            &v[..n],
            &[
                HeroCtl::Restart,
                HeroCtl::Trailer,
                HeroCtl::Version,
                HeroCtl::GoToShow,
                HeroCtl::MarkUnwatched,
                HeroCtl::Watchlist,
                HeroCtl::Alt,
            ]
        );
        let (v, n) = menu_ctls(HeroSet::default());
        assert_eq!(&v[..n], &[HeroCtl::MarkWatched], "the watched toggle is always on offer");
    }

    /// Each menu row names its own verb: the watch toggle's depends on its face AND on whether the
    /// hero presents an episode; the words are the item menu's own.
    #[test]
    fn each_menu_row_names_its_own_verb() {
        let word = |ctl, ep| menu_verb(ctl, ep).map(|c| c.to_str().unwrap().to_string());
        assert_eq!(word(HeroCtl::Restart, false).as_deref(), Some("Play from Start"));
        assert_eq!(word(HeroCtl::Restart, true), word(HeroCtl::Restart, false));
        assert_eq!(word(HeroCtl::MarkWatched, false).as_deref(), Some("Mark as Watched"));
        assert_eq!(menu_verb(HeroCtl::MarkWatched, true), Some(mark_episode_watched_label()));
        assert_eq!(menu_verb(HeroCtl::MarkUnwatched, false), Some(mark_unwatched_label()));
        assert_eq!(word(HeroCtl::MarkUnwatched, true).as_deref(), Some("Mark episode as unwatched"));
        assert_eq!(menu_verb(HeroCtl::Trailer, false), Some(trailer_label()));
        assert_eq!(menu_verb(HeroCtl::Version, false), Some(play_version_label()));
        assert_eq!(menu_verb(HeroCtl::GoToShow, false), Some(go_to_show_label()));
        for ctl in [HeroCtl::Play, HeroCtl::More, HeroCtl::Alt, HeroCtl::Watchlist] {
            assert_eq!(menu_verb(ctl, false), None, "{ctl:?} has no verb of its own");
        }
    }

    /// The menu's rows carry the control's identity, and the Watchlist row reads the membership:
    /// Add while the title is off the list (or unknown), a destructive Remove while it is on.
    #[test]
    fn the_menu_rows_carry_identity_and_read_the_watchlist_membership() {
        let set = HeroSet { watchlist: true, alt: true, ..Default::default() };
        let rows = menu_rows(set, false, Membership::Off);
        let elems: Vec<u32> = rows.iter().map(|r| r.elem).collect();
        assert_eq!(elems, [ELEM_MARK_WATCHED, ELEM_WATCHLIST, ELEM_ALT]);
        assert!(rows.iter().all(|r| !r.label.is_empty()));
        let add = &rows[1];
        assert_eq!(add.label, plx_platform::i18n::msg::watchlist_action_add());
        assert!(!add.destructive);
        let on = menu_rows(set, false, Membership::On);
        assert_eq!(on[1].label, plx_platform::i18n::msg::watchlist_action_remove());
        assert!(on[1].destructive, "Remove is destructive, as on every item menu");
        assert_eq!(menu_rows(set, false, Membership::Unknown)[1].label, add.label);
        assert_eq!(rows[2].label, alt_label().to_str().unwrap());
    }

    /// The row stands in the text column: Play at its left edge, never narrower than the design's
    /// wide pill, and the More disc one shared gap after Play's focus pop.
    #[test]
    fn the_row_starts_the_text_column_with_a_wide_play_and_a_round_more() {
        let measure = plx_ui::fixture::FixtureMeasure;
        let w = play_w(&measure, false);
        assert!(w >= theme::hero::DETAIL_PLAY_W);
        assert!(play_w(&measure, true) >= w, "Resume is never narrower than Play");
        let set = HeroSet::default();
        let play = hero_btn_rect_at(set, 0, 100.0, w);
        let more = hero_btn_rect_at(set, 1, 100.0, w);
        assert_eq!((play.x, play.y, play.w, play.h), (TEXT_X, 100.0, w, CD));
        assert_eq!((more.w, more.h), (CD, CD), "More is a circle");
        let popped = play.scaled(plx_ui::widgets::CTRL_FOCUS_SCALE);
        assert!((more.x - (popped.x + popped.w) - CGAP).abs() < 0.01);
        let row = hero_row_rect(set, 100.0, w);
        assert_eq!(row.x, TEXT_X);
        assert!((row.x + row.w - (more.x + more.w)).abs() < 0.01);
        assert!(row.x + row.w <= TEXT_X + HERO_TEXT_W);
    }

    /// The credit table: Genre, then Created by for a show (its writers) or Directed by, then
    /// Starring (a summary of the cast); a row with no names is absent.
    #[test]
    fn the_credit_table_names_genre_crew_and_cast() {
        let _g = plx_base::testlock::serial();
        let credit = |tag: &str, role: &str| plx_data::metadata::Cast {
            tag: tag.into(),
            role: role.into(),
            thumb: String::new(),
            id: 0,
            tag_key: String::new(),
        };
        let movie = Detail {
            genres: vec!["Drama".into(), "Comedy".into(), "Crime".into(), "Mystery".into()],
            directors: vec!["A Director".into()],
            cast: vec![credit("One", "a"), credit("Two", "b"), credit("Three", "c"), credit("Four", "d")],
            ..Default::default()
        };
        let rows = credit_rows(&movie);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], ("GENRE".to_string(), "Drama, Comedy, Crime".to_string()));
        assert_eq!(rows[1], ("DIRECTED BY".to_string(), "A Director".to_string()));
        assert_eq!(rows[2], ("STARRING".to_string(), "One, Two, Three".to_string()));
        let show = Detail { is_show: true, crew: vec![credit("A Writer", "Writer")], ..Default::default() };
        assert_eq!(credit_rows(&show), [("CREATED BY".to_string(), "A Writer".to_string())]);
        assert!(credit_rows(&Detail::default()).is_empty());
        let measure = plx_ui::fixture::FixtureMeasure;
        let (rule_x, value_x) = credit_columns(&rows, &measure);
        assert!(rule_x > TEXT_X && value_x > rule_x);
        let runs = plx_ui::draw_census::capture_runs(|| draw_credits(Painter::recording(), &rows, 200.0, &measure));
        let labels: Vec<_> = runs.iter().filter(|r| r.at.x == TEXT_X).map(|r| r.text.as_str()).collect();
        assert_eq!(labels, ["GENRE", "DIRECTED BY", "STARRING"]);
        assert!(runs.iter().filter(|r| r.at.x != TEXT_X).all(|r| (r.at.x - value_x).abs() < 0.01));
    }

    /// The technical pills: the picture in the video ink, the primary audio track in the audio
    /// ink, the accessibility chips in their own.
    #[test]
    fn the_technical_pills_name_picture_sound_and_access() {
        let _g = plx_base::testlock::serial();
        let audio = plx_data::metadata::Stream { codec: "ac3".into(), channels: 6, layout: "5.1(side)".into(), ..Default::default() };
        let d = Detail {
            video_resolution: "1080".into(),
            audio: vec![audio],
            subs: vec![plx_data::metadata::Stream::default()],
            ..Default::default()
        };
        let pills = tech_pills(&d);
        assert_eq!(
            pills,
            [
                ("1080p".to_string(), theme::TECH_VIDEO_INK),
                ("AC3 5.1".to_string(), theme::TECH_AUDIO_INK),
                (plx_platform::i18n::msg::widgets_badge_cc().to_string(), theme::TECH_ACCESS_INK),
            ]
        );
        let dv = Detail { hdr: true, dovi: plx_data::metadata::Dovi { present: true, ..Default::default() }, ..Default::default() };
        assert_eq!(tech_pills(&dv), [("DV".to_string(), theme::TECH_VIDEO_INK)], "Dolby Vision outranks HDR");
        let hdr = Detail { hdr: true, acodec: "aac".into(), ..Default::default() };
        assert_eq!(
            tech_pills(&hdr),
            [("HDR".to_string(), theme::TECH_VIDEO_INK), ("AAC".to_string(), theme::TECH_AUDIO_INK)]
        );
        assert!(tech_pills(&Detail::default()).is_empty());
    }

    /// Go to Show is an EPISODE's own page's action: a show page is already the show, and an
    /// episode that names no show has nowhere to go.
    #[test]
    fn go_to_show_is_an_episodes_action() {
        let episode = Detail { kind: "episode".into(), show_rk: "show".into(), season: 4, ..Default::default() };
        assert_eq!(go_to_show_target(&episode), Some(("show", 4)));
        assert!(go_to_show_target(&Detail { kind: "show".into(), show_rk: "show".into(), ..Default::default() }).is_none());
        assert!(go_to_show_target(&Detail { kind: "episode".into(), ..Default::default() }).is_none());
        assert_eq!(HeroCtl::of_elem(ELEM_GO_TO_SHOW), Some(HeroCtl::GoToShow));
        let (v, n) = menu_ctls(HeroSet { go_show: true, ..Default::default() });
        assert_eq!(&v[..n], &[HeroCtl::GoToShow, HeroCtl::MarkWatched]);
    }
}
