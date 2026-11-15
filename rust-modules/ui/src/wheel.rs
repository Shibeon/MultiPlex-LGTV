//! **The Magic Remote's wheel over a horizontal row** — the one rule every page's shelves share.
//!
//! The wheel reaches the application as `SDL_MOUSEWHEEL` (the television's SDL2 carries the notch
//! as the plain `Sint32 y` the application's wheel arm reads; `docs/lg-self-checklist.md` records
//! the path as device-proven), one event a notch, + up / - down. On a page it has always been a
//! vertical step. While the pointer rests over a row that scrolls sideways it is the ROW's instead:
//! wheel down shows more on the right, wheel up goes back left, and once the row stands at the end
//! the notch travels toward, the wheel is the page's again, so a reader who keeps turning it is
//! carried on to the next section. A row that fits on screen never takes it.
//!
//! The pieces:
//! - every horizontal row a page draws registers a [`PanZone`] in the frame's hit map
//!   (`DrawFrame::pan_zone`; `cards::Shelf` does it for every shelf on every page), saying where it
//!   is, how far it is scrolled and how far it can go;
//! - [`route`] is the decision, pure over the zone under the pointer and the notch: the row's move,
//!   or the page's;
//! - the application turns a row's move into the pointer glide every row already answers
//!   (`InputKind::Pan`), spread over frames on the shared scroll spring (`consts::K_SCROLL`), so a
//!   notch eases the row along instead of jumping it.
//!
//! It is presentation geometry and a pure function: nothing here reads a device or a clock.

use crate::{Rect, Spring};

/// How many cards one notch carries a row: two posters a notch reads as a step a hand can count,
/// and a 40-card shelf is crossed in a comfortable turn of the wheel.
pub const NOTCH_CARDS: f32 = 2.0;

/// Below this many px of room a row stands at its end: the notch is the page's.
const END_EPS: f32 = 0.5;

/// **One horizontal row as the pointer can scroll it**, registered by the draw that painted it
/// (screen space, the cascade's translate and clip folded in, like a stop).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanZone {
    /// Which row this is, stable while the row stays the same row: its band moves (the caption band
    /// opens as focus enters it, the page scrolls under it) and a glide must still know it.
    pub row: u64,
    /// Where the row's cards are on screen: the band the pointer must be over.
    pub rect: Rect,
    /// The row's horizontal scroll, px.
    pub scroll: f32,
    /// The furthest the row scrolls, px; `0` for a row that fits on screen.
    pub max: f32,
    /// One card's pitch (width and gap), px.
    pub pitch: f32,
}

impl PanZone {
    /// Does the zone cover the point? Half-open, so two rows that touch never both answer.
    pub fn covers(&self, x: f32, y: f32) -> bool {
        self.rect.w > 0.0
            && self.rect.h > 0.0
            && x >= self.rect.x
            && x < self.rect.x + self.rect.w
            && y >= self.rect.y
            && y < self.rect.y + self.rect.h
    }
}

/// Where a wheel notch goes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WheelMove {
    /// The row under the pointer moves by this many px (+ shows more on the right), never past an end.
    Row(f32),
    /// The page's own vertical step (nothing scrolls sideways there, the row fits, or it already
    /// stands at the end the notch travels toward).
    Page,
}

/// **The wheel → scroll decision.** `notch` is the wheel's step (+ up, - down; `0` is no step),
/// `zone` the row under the pointer if any, and `pending` the px of an earlier notch the row has
/// not been moved by yet (the glide still easing it there), so quick notches add up and the end is
/// judged where the row is going, not where it stands this frame.
///
/// Wheel DOWN scrolls the row RIGHT and wheel up left, by [`NOTCH_CARDS`] cards a notch, held to
/// the row's ends: a notch that would pass the end goes as far as the end, and a notch at the end
/// is the page's.
pub fn route(notch: i32, zone: Option<&PanZone>, pending: f32) -> WheelMove {
    let Some(zone) = zone.filter(|z| notch != 0 && z.max > END_EPS && z.pitch.is_finite() && z.pitch > 0.0) else {
        return WheelMove::Page;
    };
    let at = (zone.scroll + if pending.is_finite() { pending } else { 0.0 }).clamp(0.0, zone.max);
    let step = NOTCH_CARDS * zone.pitch * notch.unsigned_abs() as f32;
    let room = if notch < 0 { zone.max - at } else { at };
    if room <= END_EPS {
        return WheelMove::Page;
    }
    let dx = step.min(room);
    WheelMove::Row(if notch < 0 { dx } else { -dx })
}

/// A row's [`PanZone::row`] from whatever names it stably (its page entry and first card, its
/// page and section). Unkeyed, so the number is the same for the same row all run long.
pub fn row_id<T: std::hash::Hash>(of: &T) -> u64 {
    use std::hash::Hasher;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    of.hash(&mut h);
    h.finish()
}

/// The topmost zone under the point, in draw order (the last registered wins, as a stop does).
pub fn zone_at(zones: &[PanZone], x: f32, y: f32) -> Option<&PanZone> {
    zones.iter().rev().find(|z| z.covers(x, y))
}

/// **A notch's move, eased over frames**: the row's share of the wheel on the shared scroll spring
/// ([`crate::consts::K_SCROLL`]), handed out frame by frame as the px the row moves THIS frame, at
/// the point the notch was turned over (the pointer glide's `InputKind::Pan { dx, x, y }`).
///
/// A row's own glide moves it directly ([`crate::card_row::CardRow::glide`]), so a notch delivered
/// whole would jump it; this is the spring between the two. Quick notches add to one target, and
/// [`pending`](Self::pending) is the part not delivered yet, which [`route`] counts toward the end.
/// A notch over a DIFFERENT row ([`PanZone::row`]) drops what the last one had left: the wheel
/// follows the pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WheelGlide {
    /// The whole move the notches asked for since the glide began, px.
    target: f32,
    /// How much of it the spring has delivered (`pos`), and how fast.
    spring: Spring,
    /// Where the notch was turned (the point the row is found by), and which row it was.
    at: Option<(f32, f32, u64)>,
}

impl WheelGlide {
    pub const IDLE: WheelGlide = WheelGlide { target: 0.0, spring: Spring::at(0.0), at: None };

    /// The px of the move not handed out yet, for the row under `(x, y)` (`0` over another row).
    pub fn pending(&self, zone: Option<&PanZone>) -> f32 {
        match (self.at, zone) {
            (Some((_, _, row)), Some(z)) if row == z.row => self.target - self.spring.pos,
            _ => 0.0,
        }
    }

    /// A notch's `dx` ([`WheelMove::Row`]) over `zone`, turned at `(x, y)`.
    pub fn add(&mut self, dx: f32, x: f32, y: f32, zone: &PanZone) {
        if !matches!(self.at, Some((_, _, row)) if row == zone.row) {
            *self = Self::IDLE;
        }
        self.at = Some((x, y, zone.row));
        self.target += dx;
    }

    /// One frame of `dt` seconds: the px the row moves now and where, or `None` when nothing is
    /// under way. The glide ends on the frame the spring lands.
    pub fn step(&mut self, dt: f32) -> Option<(f32, f32, f32)> {
        let (x, y, _) = self.at?;
        let was = self.spring.pos;
        if !self.spring.land(self.target, dt) {
            self.spring.step(self.target, crate::consts::K_SCROLL, dt);
        }
        let dx = self.spring.pos - was;
        if self.spring.pos == self.target && self.spring.vel == 0.0 {
            *self = Self::IDLE;
        }
        (dx != 0.0).then_some((dx, x, y))
    }

    /// Drop what is left (the pointer hid, the D-pad took over, the page went away).
    pub fn stop(&mut self) {
        *self = Self::IDLE;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zone(scroll: f32, max: f32) -> PanZone {
        PanZone { row: 1, rect: Rect::new(0.0, 100.0, 1920.0, 375.0), scroll, max, pitch: 280.0 }
    }

    #[test]
    fn wheel_down_scrolls_the_row_right_and_up_scrolls_it_left() {
        assert_eq!(route(-1, Some(&zone(0.0, 2000.0)), 0.0), WheelMove::Row(560.0));
        assert_eq!(route(1, Some(&zone(1000.0, 2000.0)), 0.0), WheelMove::Row(-560.0));
        assert_eq!(route(-2, Some(&zone(0.0, 2000.0)), 0.0), WheelMove::Row(1120.0), "a double notch is two steps");
    }

    #[test]
    fn a_notch_stops_at_the_end_and_the_next_is_the_pages() {
        assert_eq!(route(-1, Some(&zone(1800.0, 2000.0)), 0.0), WheelMove::Row(200.0), "only as far as the end");
        assert_eq!(route(-1, Some(&zone(2000.0, 2000.0)), 0.0), WheelMove::Page, "at the right end, down is the page's");
        assert_eq!(route(1, Some(&zone(0.0, 2000.0)), 0.0), WheelMove::Page, "at the left end, up is the page's");
        assert_eq!(route(1, Some(&zone(2000.0, 2000.0)), 0.0), WheelMove::Row(-560.0), "at the right end, up still scrolls back");
    }

    #[test]
    fn a_row_that_fits_or_no_row_never_takes_the_wheel() {
        assert_eq!(route(-1, Some(&zone(0.0, 0.0)), 0.0), WheelMove::Page);
        assert_eq!(route(-1, None, 0.0), WheelMove::Page);
        assert_eq!(route(0, Some(&zone(0.0, 2000.0)), 0.0), WheelMove::Page, "no step, no move");
    }

    #[test]
    fn a_notch_still_easing_counts_toward_the_end() {
        assert_eq!(route(-1, Some(&zone(1000.0, 2000.0)), 560.0), WheelMove::Row(440.0));
        assert_eq!(route(-1, Some(&zone(1000.0, 2000.0)), 1000.0), WheelMove::Page, "already on its way to the end");
        assert_eq!(route(-1, Some(&zone(0.0, 2000.0)), f32::NAN), WheelMove::Row(560.0), "a broken pending is no pending");
    }

    #[test]
    fn a_notch_eases_the_row_over_frames_and_delivers_all_of_it() {
        let z = zone(0.0, 2000.0);
        let mut g = WheelGlide::IDLE;
        assert_eq!(g.step(1.0 / 60.0), None, "nothing under way");
        let WheelMove::Row(dx) = route(-1, Some(&z), g.pending(Some(&z))) else { panic!("the row's") };
        g.add(dx, 300.0, 200.0, &z);
        let mut moved = Vec::new();
        while let Some((d, x, y)) = g.step(1.0 / 60.0) {
            assert_eq!((x, y), (300.0, 200.0), "at the point the notch was turned");
            moved.push(d);
            assert!(moved.len() < 600, "the glide lands");
        }
        assert!(moved.len() > 5, "eased over several frames, not one jump: {moved:?}");
        assert!(moved[0] < dx / 2.0, "the first frame is a small part of the notch: {moved:?}");
        assert!(moved.iter().all(|&d| d > 0.0), "never backwards (critically damped): {moved:?}");
        let total: f32 = moved.iter().sum();
        assert!((total - dx).abs() < 0.05, "the whole notch arrives: {total} of {dx}");
        assert_eq!(g, WheelGlide::IDLE, "and the glide is over");
    }

    #[test]
    fn quick_notches_add_up_and_another_row_starts_afresh() {
        let z = zone(0.0, 2000.0);
        let mut g = WheelGlide::IDLE;
        g.add(560.0, 0.0, 200.0, &z);
        g.step(1.0 / 60.0);
        let left = g.pending(Some(&z));
        assert!(left > 0.0 && left < 560.0, "{left}");
        g.add(560.0, 0.0, 200.0, &z);
        assert!((g.pending(Some(&z)) - (left + 560.0)).abs() < 1e-3, "the second notch adds to the first");
        let other = PanZone { row: 2, rect: Rect::new(0.0, 600.0, 1920.0, 375.0), ..z };
        assert_eq!(g.pending(Some(&other)), 0.0, "another row has nothing pending");
        g.add(-560.0, 0.0, 700.0, &other);
        assert_eq!(g.pending(Some(&other)), -560.0, "what the first row had left is dropped");
        g.stop();
        assert_eq!(g.step(1.0 / 60.0), None);
    }

    /// The row's band moves while it glides (its caption band opens as focus enters it, the page
    /// scrolls under it): a second notch over the SAME row adds to what is left, never drops it.
    #[test]
    fn a_row_whose_band_moves_keeps_its_pending_notch() {
        let z = zone(0.0, 2000.0);
        let mut g = WheelGlide::IDLE;
        g.add(560.0, 0.0, 200.0, &z);
        g.step(1.0 / 60.0);
        let left = g.pending(Some(&z));
        assert!(left > 0.0, "{left}");
        let moved = PanZone { rect: Rect::new(0.0, 80.0, 1920.0, 400.0), ..z };
        assert!((g.pending(Some(&moved)) - left).abs() < 1e-3, "the same row, its band moved: still pending");
        g.add(560.0, 0.0, 200.0, &moved);
        assert!((g.pending(Some(&moved)) - (left + 560.0)).abs() < 1e-3, "the second notch adds to the first");
    }

    #[test]
    fn the_topmost_zone_under_the_pointer_answers() {
        let a = PanZone { rect: Rect::new(0.0, 0.0, 1920.0, 400.0), ..zone(0.0, 10.0) };
        let b = PanZone { rect: Rect::new(0.0, 400.0, 1920.0, 400.0), ..zone(5.0, 10.0) };
        let zones = [a, b];
        assert_eq!(zone_at(&zones, 10.0, 399.0), Some(&a));
        assert_eq!(zone_at(&zones, 10.0, 400.0), Some(&b), "touching rows: the edge belongs to one");
        assert_eq!(zone_at(&zones, 10.0, 900.0), None);
    }
}
