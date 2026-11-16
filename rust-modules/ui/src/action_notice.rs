//! `ActionNotice` — the app's shared **transient notice**: a standing glass card in the top-right
//! corner that says one thing and, optionally, offers ONE action the remote can reach without the
//! notice ever taking focus on its own.
//!
//! It is the one place the app posts a message that is not an answer to the viewer's own press:
//! a Watch Together participant pausing, a friend starting a session the viewer may join, the
//! session's title being opened. A screen does not draw one; the application owns the single
//! instance, posts to it, steps it from its frame tick and draws it LAST, over every page and
//! surface (the card is part of no page).
//!
//! # Focus: the notice never takes it, the viewer goes up to it
//!
//! A notice arrives in the corner and leaves on its own ([`LIFE_MS`], [`ACTION_LIFE_MS`]); it never
//! seats the focus itself, so a viewer pressing keys on a page or in the player keeps doing exactly
//! what they were doing (the owner's rule: a notice must not steal focus from playback). An
//! actionable notice says how to reach it on its second line — the UP key cap and the action's
//! name ("[▲] Join") — because that is how it is reached: the owner publishes the card to the
//! dispatcher every frame ([`crate::dispatch::Dispatcher::set_notice`]), which composes it as the
//! focus group [`crate::containers::tabs::NOTICE`] after the TOP page's own and declares the one
//! link into it, UP off the page's top: off the bar on a page that wears one, off the first row of a
//! page that does not, off the player's topmost transport group (whose own answer to that UP would
//! hide the transport). A surface's rows never reach it. Focused ([`ActionNotice::set_focused`]),
//! the card shows its action as a focused pill and stops counting its life; the dispatcher answers
//! its keys — OK takes the action, BACK or DOWN gives the focus back where it came from, the other
//! directions stop at its walls — and passes every other key (play, pause, the seeks) to the page.
//! A focused notice nobody answers asks for the focus back after [`FOCUSED_IDLE_MS`]
//! ([`ActionNotice::wants_release`]). The card is also a pointer stop over everything: a click on it
//! takes the action from the top page and is swallowed over a surface, never falling through to the
//! control under it.
//!
//! # Queue
//!
//! One card stands at a time. A post with the id of the card standing REPLACES its words in place
//! (no second entrance: a count that moves, the same friend's session listed again); any other post
//! waits its turn, at most [`MAX_QUEUED`] deep (the oldest waiting passive notice is dropped first;
//! a waiting actionable one is never dropped for a passive one). [`ActionNotice::withdraw`] removes a
//! notice whose subject went away (a session that ended, a title that finished opening): a standing
//! card leaves, a waiting one is forgotten. A [`Life::Sticky`] notice stands until withdrawn (a
//! progress line: "Joining “Dune”…").
//!
//! # Motion and material
//!
//! It fades in on the shared appear stiffness ([`crate::popover::K_APPEAR`]) and slides
//! [`SLIDE_PX`] in from the right on the same spring, inside `idle::decorative` (it stands over the
//! page and moves nothing under it). The card stands on the top bar's flat track material (the
//! fallback ground the hold hint's glass falls back to), never the live blur: the blur path is
//! chrome-only (`widgets::glass_budget_tests`), and the card also stands over the player, where the
//! framebuffer under it is the video plane's hole, not a picture to blur. Time is the `dt` it is
//! handed: it reads no clock.
//!
//! Sans-IO: values in, the standing notice's id out; the action itself is the application's to perform.

use std::collections::VecDeque;
use std::ffi::CString;

use plx_machine::machine::Measure;

use crate::icons::Icon;
use crate::widgets::{self, CapFace, CapLook, CapMetrics};
use crate::{theme, Painter, Rect, Spring};

/// A passive notice stands this long once in.
pub const LIFE_MS: u32 = 4_000;
/// An actionable notice stands this long once in (unfocused): long enough to read, decide and press.
pub const ACTION_LIFE_MS: u32 = 10_000;
/// A focused notice nobody answers asks for the focus back after this long.
pub const FOCUSED_IDLE_MS: u32 = 12_000;
/// At most this many notices wait behind the one standing.
pub const MAX_QUEUED: usize = 4;
/// The card's widest; a message longer than this elides (it carries server text: names, titles).
pub const MAX_W: f32 = 760.0;
/// The card's right edge from the right of the frame (the safe area's own margin).
pub const RIGHT: f32 = crate::consts::MARGIN_X;
/// The card's top on a page: under the page header's whole block (the title and the row of chips
/// under it, `page_header::TOP` + title + gap + block), so it covers none of a page's header
/// controls (the Friends page's Share and Join-with-code chips, the Library's sort).
pub const PAGE_TOP: f32 = crate::page_header::TOP + crate::page_header::TITLE_H + crate::page_header::TITLE_ROW_GAP
    + crate::page_header::BLOCK_H;
/// The card's top over the player: under the clock and the title band of the player's head.
pub const PLAYER_TOP: f32 = 170.0;
/// The card slides this far in from the right as it appears.
pub const SLIDE_PX: f32 = 24.0;
/// Padding inside the card.
const PAD_X: f32 = 28.0;
const PAD_Y: f32 = 20.0;
/// The leading mark's box and the space after it.
const MARK: f32 = 36.0;
const MARK_GAP: f32 = 18.0;
/// Line pitch of the two lines.
const LINE_H: f32 = 38.0;
/// The action line's pill height and padding when focused.
const PILL_H: f32 = 44.0;
const PILL_PAD: f32 = 20.0;
/// Space between the key cap and the action's name.
const CAP_GAP: f32 = 12.0;
/// The longest step a spring takes at once (a long idle gap is not one huge integration step).
const MAX_SPRING_DT_MS: u32 = 50;
/// Below this opacity nothing is drawn.
const VISIBLE: f32 = 0.004;
/// The opacity at which the entrance counts as complete and the life starts counting.
const IN: f32 = 0.98;

/// How long a notice stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Life {
    /// [`LIFE_MS`] without an action, [`ACTION_LIFE_MS`] with one.
    Timed,
    /// Until [`ActionNotice::withdraw`]n.
    Sticky,
}

/// One notice as posted.
#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    /// The poster's own identity for it: a repost with the same id updates the standing card in place.
    pub id: u64,
    /// The message (may carry server text, so it may elide).
    pub text: String,
    /// The action's name ("Join"), when the notice offers one.
    pub action: Option<String>,
    /// The leading mark, if any.
    pub mark: Option<Icon>,
    pub life: Life,
}

impl Notice {
    pub fn passive(id: u64, text: impl Into<String>) -> Notice {
        Notice { id, text: text.into(), action: None, mark: None, life: Life::Timed }
    }

    pub fn actionable(id: u64, text: impl Into<String>, action: impl Into<String>) -> Notice {
        Notice { id, text: text.into(), action: Some(action.into()), mark: None, life: Life::Timed }
    }

    pub fn mark(mut self, mark: Icon) -> Notice {
        self.mark = Some(mark);
        self
    }

    pub fn sticky(mut self) -> Notice {
        self.life = Life::Sticky;
        self
    }

    fn life_ms(&self) -> Option<u32> {
        match self.life {
            Life::Sticky => None,
            Life::Timed if self.action.is_some() => Some(ACTION_LIFE_MS),
            Life::Timed => Some(LIFE_MS),
        }
    }
}

/// The notice's state: what stands, what waits, and the two springs. One per application.
pub struct ActionNotice {
    current: Option<Notice>,
    queue: VecDeque<Notice>,
    /// The standing card is leaving (expired, withdrawn, answered).
    leaving: bool,
    /// The dispatcher holds the focus on the card ([`Self::set_focused`]).
    focused: bool,
    /// Time stood fully in, unfocused (ms).
    stood_ms: u32,
    /// Time focused without an answer (ms).
    idle_ms: u32,
    fade: Spring,
}

impl Default for ActionNotice {
    fn default() -> Self {
        Self::new()
    }
}

impl ActionNotice {
    pub const fn new() -> Self {
        Self { current: None, queue: VecDeque::new(), leaving: false, focused: false, stood_ms: 0, idle_ms: 0, fade: Spring::at(0.0) }
    }

    /// Post a notice (see the module doc's "Queue").
    pub fn post(&mut self, n: Notice) {
        if let Some(cur) = self.current.as_mut().filter(|c| c.id == n.id) {
            // The same subject, new words: in place, and a timed card's life starts over.
            *cur = n;
            if !self.leaving {
                self.stood_ms = 0;
            }
            return;
        }
        if let Some(waiting) = self.queue.iter_mut().find(|q| q.id == n.id) {
            *waiting = n;
            return;
        }
        if self.queue.len() >= MAX_QUEUED {
            let drop = self.queue.iter().position(|q| q.action.is_none()).or(if n.action.is_none() { None } else { Some(0) });
            match drop {
                Some(i) => {
                    self.queue.remove(i);
                }
                // Every waiting notice is actionable and this one is not: it is the one dropped.
                None => return,
            }
        }
        self.queue.push_back(n);
    }

    /// Take notice `id` away: a standing card leaves (and stops being a stop), a waiting one is forgotten.
    pub fn withdraw(&mut self, id: u64) {
        self.queue.retain(|q| q.id != id);
        if self.current.as_ref().is_some_and(|c| c.id == id) {
            self.leave();
        }
    }

    fn leave(&mut self) {
        self.leaving = true;
    }

    /// Advance by `dt_ms`, the time since the previous step (the owner's clock difference, so an
    /// idle loop that steps rarely still counts the whole life; the springs take at most
    /// [`MAX_SPRING_DT_MS`] of it per step).
    pub fn step(&mut self, dt_ms: u32) {
        let dt = dt_ms.min(MAX_SPRING_DT_MS) as f32 / 1000.0;
        if self.current.is_none() {
            match self.queue.pop_front() {
                Some(n) => {
                    self.current = Some(n);
                    self.leaving = false;
                    self.stood_ms = 0;
                    self.idle_ms = 0;
                }
                None => return,
            }
        }
        let target = if self.leaving { 0.0 } else { 1.0 };
        plx_machine::idle::decorative(|| self.fade.step(target, crate::popover::K_APPEAR, dt));
        if self.leaving {
            if self.fade.pos <= VISIBLE {
                self.fade.jump(0.0);
                self.current = None;
                self.leaving = false;
            }
            return;
        }
        if self.focused {
            self.idle_ms = self.idle_ms.saturating_add(dt_ms);
            return;
        }
        if self.fade.pos >= IN {
            self.stood_ms = self.stood_ms.saturating_add(dt_ms);
        }
        if let Some(life) = self.current.as_ref().and_then(Notice::life_ms) {
            if self.stood_ms >= life {
                self.leave();
            }
        }
    }

    /// The card offers its action and is not on its way out: the owner publishes it as actionable
    /// (`Dispatcher::set_notice`), so the focus can go up to it.
    pub fn actionable(&self) -> bool {
        self.current.as_ref().is_some_and(|c| c.action.is_some()) && !self.leaving && self.visible()
    }

    /// The dispatcher's focus is (or is no longer) on the card, as the owner reads it every frame
    /// (`Dispatcher::notice_focused`). Focused, the card shows its action as the focused pill and its
    /// life stands still; each new focusing restarts the idle count.
    pub fn set_focused(&mut self, focused: bool) {
        if focused && !self.focused {
            self.idle_ms = 0;
        }
        self.focused = focused;
    }

    pub fn focused(&self) -> bool {
        self.focused
    }

    /// A focused card nobody answered for [`FOCUSED_IDLE_MS`]: the owner gives the focus back
    /// (`Dispatcher::release_notice`).
    pub fn wants_release(&self) -> bool {
        self.focused && self.idle_ms >= FOCUSED_IDLE_MS
    }

    /// The action of the standing card was taken (OK, or a click): the card leaves, and the owner
    /// performs what its id names. `None` when nothing actionable stands.
    pub fn take_action(&mut self) -> Option<u64> {
        let id = self.current.as_ref().filter(|c| c.action.is_some() && !self.leaving)?.id;
        self.leave();
        Some(id)
    }

    pub fn visible(&self) -> bool {
        self.current.is_some() && self.fade.pos > VISIBLE
    }

    /// The notice standing (or entering, or leaving).
    pub fn current(&self) -> Option<&Notice> {
        self.current.as_ref()
    }

    /// Whether anything stands or waits: the owner keeps stepping while this holds.
    pub fn busy(&self) -> bool {
        self.current.is_some() || !self.queue.is_empty()
    }

    /// The card's rect at rest with its top at `top` ([`PAGE_TOP`], [`PLAYER_TOP`]), under `measure`
    /// (also the pointer stop the dispatcher registers for it). `None` while nothing is visible.
    pub fn rect(&self, measure: &dyn Measure, top: f32) -> Option<Rect> {
        let n = self.current.as_ref().filter(|_| self.visible())?;
        let lay = Layout::of(n, self.focused, measure);
        Some(Rect::new(crate::consts::SCR_W - RIGHT - lay.w, top, lay.w, lay.h))
    }

    /// Draw the card, last, over everything, with its top at `top`.
    pub fn draw(&self, p: Painter, measure: &dyn Measure, top: f32) {
        let Some(n) = self.current.as_ref() else { return };
        let opacity = self.fade.pos.clamp(0.0, 1.0);
        if opacity <= VISIBLE {
            return;
        }
        let lay = Layout::of(n, self.focused, measure);
        let slide = (1.0 - opacity) * SLIDE_PX;
        let r = Rect::new(crate::consts::SCR_W - RIGHT - lay.w + slide, top, lay.w, lay.h);
        let radius = 24.0;
        let p = p.alpha(opacity);
        let boost = theme::GLASS_RIM_LIGHT[3] - theme::GLASS_RIM[3];
        p.rect_rimmed(r, radius, theme::TAB_TRACK_TOP, theme::TAB_TRACK_BOT, theme::GLASS_RIM, boost);
        let mut x = r.x + PAD_X;
        let line1 = r.y + PAD_Y + LINE_H * 0.5;
        if let Some(mark) = n.mark {
            crate::icons::draw(p, mark, Rect::new(x, line1 - MARK * 0.5, MARK, MARK), theme::TEXT_PRIMARY);
            x += MARK + MARK_GAP;
        }
        let sz = theme::size::BODY;
        p.text(lay.text.as_ptr(), x, plx_gfx::text::text_vcenter_y(sz, 0, line1), sz, theme::TEXT_PRIMARY, 0, 0);
        let Some(action) = lay.action.as_ref() else { return };
        let line2 = line1 + LINE_H + if self.focused { (PILL_H - LINE_H) * 0.5 + 6.0 } else { 0.0 };
        let asz = theme::size::LABEL;
        if self.focused {
            // The action as the focused control it now is: the accent pill, OK on its cap.
            let pill = Rect::new(x, line2 - PILL_H * 0.5, lay.action_w + PILL_PAD * 2.0, PILL_H);
            p.rect(pill, PILL_H * 0.5, theme::ACCENT, theme::ACCENT, 1.0);
            p.text(action.as_ptr(), pill.x + PILL_PAD, plx_gfx::text::text_vcenter_y(asz, 1, line2), asz, theme::ACCENT_INK, 0, 1);
            widgets::key_cap_with(
                p,
                pill.x + pill.w + CAP_GAP,
                line2,
                CapFace::Label(c"OK"),
                CapLook { ring: theme::TEXT_SECONDARY, label: theme::TEXT_PRIMARY, fill: None, metrics: CapMetrics::HINT },
                measure,
            );
        } else {
            // How to reach it: the UP cap, then the action's name.
            widgets::key_cap_with(
                p,
                x,
                line2,
                CapFace::Glyph(Icon::ChevronUp),
                CapLook { ring: theme::TEXT_SECONDARY, label: theme::TEXT_PRIMARY, fill: None, metrics: CapMetrics::HINT },
                measure,
            );
            p.text(action.as_ptr(), x + lay.cap_w + CAP_GAP, plx_gfx::text::text_vcenter_y(asz, 1, line2), asz, theme::TEXT_PRIMARY, 0, 1);
        }
    }
}

/// One measured arrangement of a notice.
struct Layout {
    text: CString,
    action: Option<CString>,
    action_w: f32,
    cap_w: f32,
    w: f32,
    h: f32,
}

impl Layout {
    fn of(n: &Notice, focused: bool, measure: &dyn Measure) -> Layout {
        let lead = if n.mark.is_some() { MARK + MARK_GAP } else { 0.0 };
        let room = MAX_W - PAD_X * 2.0 - lead;
        let sz = theme::size::BODY;
        let text = elide(&n.text, room, sz, false, measure);
        let text_w = measure.width_str(&text, sz, false);
        let action = n.action.as_deref().map(|a| CString::new(a).unwrap_or_default());
        let asz = theme::size::LABEL;
        let action_w = action.as_ref().map_or(0.0, |a| measure.width(a, asz, true));
        let cap_w = widgets::key_cap_w_with(CapFace::Glyph(Icon::ChevronUp), CapMetrics::HINT, measure);
        let ok_w = widgets::key_cap_w_with(CapFace::Label(c"OK"), CapMetrics::HINT, measure);
        let line2_w = match (&action, focused) {
            (None, _) => 0.0,
            (Some(_), false) => cap_w + CAP_GAP + action_w,
            (Some(_), true) => action_w + PILL_PAD * 2.0 + CAP_GAP + ok_w,
        };
        let w = (PAD_X * 2.0 + lead + text_w.max(line2_w)).min(MAX_W);
        let lines = if action.is_some() { 2.0 } else { 1.0 };
        let extra = if action.is_some() && focused { PILL_H - LINE_H + 12.0 } else { 0.0 };
        let h = PAD_Y * 2.0 + LINE_H * lines + extra;
        Layout { text: CString::new(text).unwrap_or_default(), action, action_w, cap_w, w, h }
    }
}

/// `s` cut at a character boundary with an ellipsis so it fits `budget` under `measure`.
fn elide(s: &str, budget: f32, sz: i32, bold: bool, measure: &dyn Measure) -> String {
    if measure.width_str(s, sz, bold) <= budget {
        return s.to_owned();
    }
    let mut cut: Vec<char> = s.chars().collect();
    while !cut.is_empty() {
        cut.pop();
        let t: String = cut.iter().collect::<String>().trim_end().to_owned() + "…";
        if measure.width_str(&t, sz, bold) <= budget {
            return t;
        }
    }
    "…".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: u32 = 16;

    /// Step until `f` holds, at most `ms` of frames; returns the time it took.
    fn run_until(n: &mut ActionNotice, ms: u32, f: impl Fn(&ActionNotice) -> bool) -> Option<u32> {
        let mut t = 0;
        while t <= ms {
            if f(n) {
                return Some(t);
            }
            n.step(FRAME);
            t += FRAME;
        }
        None
    }

    #[test]
    fn a_passive_notice_comes_in_stands_its_life_and_leaves_on_its_own() {
        let mut n = ActionNotice::new();
        n.post(Notice::passive(1, "Camille paused"));
        assert!(n.busy() && !n.visible(), "posted, not yet stepped");
        n.step(FRAME);
        assert!(n.visible());
        let gone = run_until(&mut n, LIFE_MS + 3_000, |n| !n.busy()).expect("it times out");
        assert!(gone >= LIFE_MS, "it stood its whole life first ({gone} ms)");
        assert!(gone < LIFE_MS + 2_000, "and left within the fade ({gone} ms)");
    }

    #[test]
    fn an_actionable_notice_stands_longer_and_still_times_out_unanswered() {
        let mut n = ActionNotice::new();
        n.post(Notice::actionable(7, "Camille is watching “Dune” together", "Join"));
        let gone = run_until(&mut n, ACTION_LIFE_MS + 3_000, |n| !n.busy()).expect("it times out");
        assert!(gone >= ACTION_LIFE_MS, "{gone}");
    }

    #[test]
    fn a_standing_notice_is_actionable_until_its_action_is_taken() {
        let mut n = ActionNotice::new();
        n.post(Notice::actionable(7, "Camille is watching “Dune” together", "Join"));
        assert!(!n.actionable(), "not before it is visible");
        n.step(FRAME);
        assert!(n.actionable());
        assert_eq!(n.take_action(), Some(7));
        assert!(!n.actionable(), "taken: the card is leaving");
        assert_eq!(n.take_action(), None, "once");
        assert!(run_until(&mut n, 2_000, |n| !n.busy()).is_some(), "and the card leaves");
    }

    #[test]
    fn a_focused_notice_does_not_expire_but_asks_for_the_focus_back_when_left_alone() {
        let mut n = ActionNotice::new();
        n.post(Notice::actionable(3, "x", "Join"));
        n.step(FRAME);
        n.set_focused(true);
        let asked = run_until(&mut n, FOCUSED_IDLE_MS + 1_000, ActionNotice::wants_release).expect("asks");
        assert!(asked >= FOCUSED_IDLE_MS, "{asked}");
        assert!(n.busy(), "the focused time did not count against its life");
        n.set_focused(false);
        assert!(!n.wants_release());
        assert!(run_until(&mut n, ACTION_LIFE_MS + 3_000, |n| !n.busy()).is_some(), "unfocused, its life runs again");
    }

    #[test]
    fn a_passive_notice_offers_nothing() {
        let mut n = ActionNotice::new();
        n.post(Notice::passive(1, "x"));
        n.step(FRAME);
        assert!(!n.actionable());
        assert_eq!(n.take_action(), None);
    }

    #[test]
    fn a_focused_card_is_taller_for_its_pill() {
        let m = crate::fixture::FixtureMeasure;
        let mut n = ActionNotice::new();
        n.post(Notice::actionable(3, "x", "Join"));
        n.step(FRAME);
        let rest = n.rect(&m, PAGE_TOP).unwrap();
        n.set_focused(true);
        assert!(n.rect(&m, PAGE_TOP).unwrap().h > rest.h);
    }

    #[test]
    fn a_repost_of_the_standing_id_updates_in_place_and_others_wait_their_turn() {
        let mut n = ActionNotice::new();
        n.post(Notice::passive(1, "one"));
        n.step(FRAME);
        n.post(Notice::passive(2, "two"));
        n.post(Notice::passive(1, "one again"));
        assert_eq!(n.current().map(|c| c.text.as_str()), Some("one again"));
        run_until(&mut n, LIFE_MS + 3_000, |n| n.current().is_some_and(|c| c.id == 2)).expect("two follows");
    }

    #[test]
    fn a_sticky_notice_stands_until_withdrawn() {
        let mut n = ActionNotice::new();
        n.post(Notice::passive(9, "Joining “Dune”…").sticky());
        assert!(run_until(&mut n, ACTION_LIFE_MS * 2, |n| !n.busy()).is_none(), "no life of its own");
        n.withdraw(9);
        assert!(run_until(&mut n, 2_000, |n| !n.busy()).is_some());
    }

    #[test]
    fn the_queue_is_bounded_and_drops_passive_notices_before_actionable_ones() {
        let mut n = ActionNotice::new();
        n.post(Notice::passive(100, "standing"));
        n.step(FRAME);
        n.post(Notice::actionable(1, "a", "Join"));
        for id in 2..=4 {
            n.post(Notice::passive(id, "p"));
        }
        n.post(Notice::actionable(5, "b", "Join"));
        assert_eq!(n.queue.len(), MAX_QUEUED);
        assert!(n.queue.iter().any(|q| q.id == 1) && n.queue.iter().any(|q| q.id == 5));
        assert!(!n.queue.iter().any(|q| q.id == 2), "the oldest passive one made room");
    }

    #[test]
    fn a_withdrawn_card_stops_offering_its_action() {
        let mut n = ActionNotice::new();
        n.post(Notice::actionable(3, "x", "Join"));
        n.step(FRAME);
        n.set_focused(true);
        n.withdraw(3);
        assert!(!n.actionable(), "the owner publishes it as no longer actionable, which gives the focus back");
        assert_eq!(n.take_action(), None);
    }

    #[test]
    fn the_card_stands_in_the_top_right_corner_within_the_safe_area() {
        let m = crate::fixture::FixtureMeasure;
        let mut n = ActionNotice::new();
        n.post(Notice::actionable(3, "Camille is watching “A very long title that goes on and on and on” together", "Join"));
        n.step(FRAME);
        let r = n.rect(&m, PLAYER_TOP).unwrap();
        assert!(r.w <= MAX_W + 0.01);
        assert!((r.x + r.w - (crate::consts::SCR_W - RIGHT)).abs() < 0.01, "right-aligned to the safe margin");
        assert!(r.y >= crate::consts::MARGIN_Y);
    }

    /// (Regression, pr-reviewer #5.) On a page the card stands under the page header's whole block:
    /// the header's chips (the Friends page's Share and Join-with-code, the Library's sort and its
    /// direction on the row under the title) lie above the card's top, so it covers none of them.
    #[test]
    fn on_a_page_the_card_clears_the_header_block() {
        use crate::page_header;
        let chip_row = page_header::TOP + page_header::TITLE_H + page_header::TITLE_ROW_GAP;
        let chips = Rect::new(crate::consts::SCR_W - RIGHT - MAX_W, page_header::TOP, MAX_W, chip_row + page_header::BLOCK_H - page_header::TOP);
        let m = crate::fixture::FixtureMeasure;
        let mut n = ActionNotice::new();
        n.post(Notice::actionable(3, "Camille is watching “Dune” together", "Join"));
        n.step(FRAME);
        for focused in [false, true] {
            n.set_focused(focused);
            let r = n.rect(&m, PAGE_TOP).unwrap();
            let i = r.intersect(chips);
            assert!(i.w <= 0.0 || i.h <= 0.0, "the card {r:?} covers the header block {chips:?}");
        }
    }
}
