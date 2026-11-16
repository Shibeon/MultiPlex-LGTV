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
//! # Focus: the notice never takes it
//!
//! A notice arrives in the corner and leaves on its own ([`LIFE_MS`], [`ACTION_LIFE_MS`]); it is
//! not a focus stop and no key reaches it while it merely stands, so a viewer pressing keys on a
//! page or in the player keeps doing exactly what they were doing (the owner's rule: a notice must
//! not steal focus from playback). An actionable notice says how to reach it on its second line —
//! the UP key cap and the action's name ("[▲] Join") — because that is how it is reached: an UP
//! press that the page's focus model answers with nothing (UP from the top row of a page, from the
//! player's head, or from the bare picture's own top) is an OVERSHOOT, which the dispatcher reports
//! ([`crate::dispatch::Dispatcher::take_overshoot`]) and the application hands to [`ActionNotice::engage`].
//! Engaged, the card shows its action as a focused pill, keeps its place and stops counting its
//! life; OK performs the action ([`Answer::Activated`]), BACK or DOWN gives the keys back
//! ([`Answer::Released`]), and every other key is swallowed while it holds them. An engaged notice
//! nobody answers gives the keys back after [`ENGAGED_IDLE_MS`].
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
//! Sans-IO: values in, a typed [`Answer`] out; the action itself is the application's to perform.

use std::collections::VecDeque;
use std::ffi::CString;

use plx_machine::machine::Measure;

use crate::icons::Icon;
use crate::widgets::{self, CapFace, CapLook, CapMetrics};
use crate::{theme, Painter, Rect, Spring};

/// A passive notice stands this long once in.
pub const LIFE_MS: u32 = 4_000;
/// An actionable notice stands this long once in (unengaged): long enough to read, decide and press.
pub const ACTION_LIFE_MS: u32 = 10_000;
/// An engaged notice nobody answers gives the keys back after this long.
pub const ENGAGED_IDLE_MS: u32 = 12_000;
/// At most this many notices wait behind the one standing.
pub const MAX_QUEUED: usize = 4;
/// The card's widest; a message longer than this elides (it carries server text: names, titles).
pub const MAX_W: f32 = 760.0;
/// The card's right edge from the right of the frame (the safe area's own margin).
pub const RIGHT: f32 = crate::consts::MARGIN_X;
/// The card's top from the top of the frame: under the tab strip on a page, under the clock and the
/// title band of the player's head, so it covers neither.
pub const TOP: f32 = 170.0;
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
/// The action line's pill height and padding when engaged.
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

/// A key while the notice holds the keys ([`ActionNotice::key`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeKey {
    Ok,
    Back,
    Up,
    Down,
    Left,
    Right,
    Other,
}

/// What a key did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    /// The notice does not hold the keys: the key is the page's.
    Ignored,
    /// Held and swallowed.
    Consumed,
    /// The keys are given back (BACK, DOWN): the page keeps its focus as it was.
    Released,
    /// OK on the action: the poster performs notice `id`'s action. The card leaves.
    Activated(u64),
}

/// The notice's state: what stands, what waits, and the two springs. One per application.
pub struct ActionNotice {
    current: Option<Notice>,
    queue: VecDeque<Notice>,
    /// The standing card is leaving (expired, withdrawn, answered).
    leaving: bool,
    engaged: bool,
    /// Time stood fully in, unengaged (ms).
    stood_ms: u32,
    /// Time engaged without an answer (ms).
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
        Self { current: None, queue: VecDeque::new(), leaving: false, engaged: false, stood_ms: 0, idle_ms: 0, fade: Spring::at(0.0) }
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

    /// Take notice `id` away: a standing card leaves (and gives the keys back), a waiting one is forgotten.
    pub fn withdraw(&mut self, id: u64) {
        self.queue.retain(|q| q.id != id);
        if self.current.as_ref().is_some_and(|c| c.id == id) {
            self.leave();
        }
    }

    fn leave(&mut self) {
        self.leaving = true;
        self.engaged = false;
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
                    self.engaged = false;
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
                self.engaged = false;
            }
            return;
        }
        if self.engaged {
            self.idle_ms = self.idle_ms.saturating_add(dt_ms);
            if self.idle_ms >= ENGAGED_IDLE_MS {
                self.engaged = false;
                self.idle_ms = 0;
            }
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

    /// The overshoot (an UP the page answered with nothing) reaches the standing card: an
    /// actionable notice that is not leaving takes the keys. Returns whether it did.
    pub fn engage(&mut self) -> bool {
        let actionable = self.current.as_ref().is_some_and(|c| c.action.is_some());
        if actionable && !self.leaving && self.visible() {
            self.engaged = true;
            self.idle_ms = 0;
        }
        self.engaged
    }

    /// A key, offered to the notice before the page. Only an engaged notice answers anything but
    /// [`Answer::Ignored`].
    pub fn key(&mut self, key: NoticeKey) -> Answer {
        if !self.engaged {
            return Answer::Ignored;
        }
        self.idle_ms = 0;
        match key {
            NoticeKey::Ok => {
                let id = self.current.as_ref().map_or(0, |c| c.id);
                self.leave();
                Answer::Activated(id)
            }
            NoticeKey::Back | NoticeKey::Down => {
                self.engaged = false;
                Answer::Released
            }
            NoticeKey::Up | NoticeKey::Left | NoticeKey::Right | NoticeKey::Other => Answer::Consumed,
        }
    }

    pub fn engaged(&self) -> bool {
        self.engaged
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

    /// The card's rect at rest, under `measure` (also the hit rect a pointer owner registers).
    pub fn rect(&self, measure: &dyn Measure) -> Option<Rect> {
        let n = self.current.as_ref()?;
        let lay = Layout::of(n, self.engaged, measure);
        Some(Rect::new(crate::consts::SCR_W - RIGHT - lay.w, TOP, lay.w, lay.h))
    }

    /// Draw the card, last, over everything.
    pub fn draw(&self, p: Painter, measure: &dyn Measure) {
        let Some(n) = self.current.as_ref() else { return };
        let opacity = self.fade.pos.clamp(0.0, 1.0);
        if opacity <= VISIBLE {
            return;
        }
        let lay = Layout::of(n, self.engaged, measure);
        let slide = (1.0 - opacity) * SLIDE_PX;
        let r = Rect::new(crate::consts::SCR_W - RIGHT - lay.w + slide, TOP, lay.w, lay.h);
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
        let line2 = line1 + LINE_H + if self.engaged { (PILL_H - LINE_H) * 0.5 + 6.0 } else { 0.0 };
        let asz = theme::size::LABEL;
        if self.engaged {
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
    fn of(n: &Notice, engaged: bool, measure: &dyn Measure) -> Layout {
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
        let line2_w = match (&action, engaged) {
            (None, _) => 0.0,
            (Some(_), false) => cap_w + CAP_GAP + action_w,
            (Some(_), true) => action_w + PILL_PAD * 2.0 + CAP_GAP + ok_w,
        };
        let w = (PAD_X * 2.0 + lead + text_w.max(line2_w)).min(MAX_W);
        let lines = if action.is_some() { 2.0 } else { 1.0 };
        let extra = if action.is_some() && engaged { PILL_H - LINE_H + 12.0 } else { 0.0 };
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
    fn a_standing_notice_takes_no_key_until_the_overshoot_engages_it() {
        let mut n = ActionNotice::new();
        n.post(Notice::actionable(7, "Camille is watching “Dune” together", "Join"));
        n.step(FRAME);
        for k in [NoticeKey::Ok, NoticeKey::Back, NoticeKey::Up, NoticeKey::Down] {
            assert_eq!(n.key(k), Answer::Ignored, "{k:?} is the page's while the notice only stands");
        }
        assert!(n.engage());
        assert!(n.engaged());
        assert_eq!(n.key(NoticeKey::Left), Answer::Consumed);
        assert_eq!(n.key(NoticeKey::Ok), Answer::Activated(7));
        assert!(!n.engaged(), "answered: the keys are the page's again");
        assert!(run_until(&mut n, 2_000, |n| !n.busy()).is_some(), "and the card leaves");
    }

    #[test]
    fn back_or_down_gives_the_keys_back_and_the_card_resumes_its_life() {
        for give_back in [NoticeKey::Back, NoticeKey::Down] {
            let mut n = ActionNotice::new();
            n.post(Notice::actionable(3, "x", "Join"));
            n.step(FRAME);
            assert!(n.engage());
            assert_eq!(n.key(give_back), Answer::Released);
            assert!(!n.engaged());
            assert_eq!(n.key(NoticeKey::Ok), Answer::Ignored, "released: OK is the page's again");
            assert!(run_until(&mut n, ACTION_LIFE_MS + 3_000, |n| !n.busy()).is_some());
        }
    }

    #[test]
    fn an_engaged_notice_does_not_expire_but_gives_the_keys_back_when_left_alone() {
        let mut n = ActionNotice::new();
        n.post(Notice::actionable(3, "x", "Join"));
        n.step(FRAME);
        assert!(n.engage());
        let released = run_until(&mut n, ENGAGED_IDLE_MS + 1_000, |n| !n.engaged()).expect("released");
        assert!(released >= ENGAGED_IDLE_MS, "{released}");
        assert!(n.busy(), "the engaged time did not count against its life");
    }

    #[test]
    fn a_passive_notice_cannot_be_engaged() {
        let mut n = ActionNotice::new();
        n.post(Notice::passive(1, "x"));
        n.step(FRAME);
        assert!(!n.engage());
        assert_eq!(n.key(NoticeKey::Ok), Answer::Ignored);
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
    fn withdrawing_the_engaged_card_gives_the_keys_back() {
        let mut n = ActionNotice::new();
        n.post(Notice::actionable(3, "x", "Join"));
        n.step(FRAME);
        n.engage();
        n.withdraw(3);
        assert!(!n.engaged());
        assert_eq!(n.key(NoticeKey::Ok), Answer::Ignored);
    }

    #[test]
    fn the_card_stands_in_the_top_right_corner_within_the_safe_area() {
        let m = crate::fixture::FixtureMeasure;
        let mut n = ActionNotice::new();
        n.post(Notice::actionable(3, "Camille is watching “A very long title that goes on and on and on” together", "Join"));
        n.step(FRAME);
        let r = n.rect(&m).unwrap();
        assert!(r.w <= MAX_W + 0.01);
        assert!((r.x + r.w - (crate::consts::SCR_W - RIGHT)).abs() < 0.01, "right-aligned to the safe margin");
        assert!(r.y >= crate::consts::MARGIN_Y);
    }
}
