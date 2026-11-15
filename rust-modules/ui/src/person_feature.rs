//! The featured person: one person drawn as a wide card — a round headshot, the name, the
//! departments they work in and one line about their work — for a page that puts a person ahead of
//! its rows (the Search screen, when the query names someone). The MultiPlex web app's search draws
//! the same block above its results.
//!
//! A shared component (`ui/src/CLAUDE.md`, rule 4): the headshot is [`widgets::card`]'s person art,
//! the departments a [`widgets::dotted_run`], and the focused card wears the table row's accent
//! plate ([`crate::table::PILL_RAD`]), the selection treatment the recent searches already use on
//! the same page, so nothing here is a new visual primitive. Focus travel is the caller's: this is
//! one focus stop, placed by [`PersonFeature::focus_rect`].
use crate::label::{Label, VAlign};
use crate::widgets::{self, Art};
use crate::{theme, Painter, Rect};
use std::ffi::CStr;

/// The headshot's diameter.
pub const PHOTO_D: f32 = 168.0;
/// The plate's inset around the headshot and the words.
pub const PAD: f32 = theme::space::SM;
/// The block's height: the headshot and the plate's inset each side.
pub const H: f32 = PHOTO_D + 2.0 * PAD;
/// The block's width: room for a long name and its departments, never the whole row.
pub const W: f32 = 1000.0;
/// The gap from the headshot to the words.
const GAP: f32 = theme::space::LG;
/// The name's, the departments' and the note's sizes: a page heading's, then body copy.
const NAME_SIZE: i32 = theme::size::TITLE;
const JOBS_SIZE: i32 = theme::size::BODY;
const NOTE_SIZE: i32 = theme::size::CAPTION;
/// The person art's resolution, the person page portrait's request.
const ART_RES: (std::os::raw::c_int, std::os::raw::c_int) = (300, 300);

/// One featured person, borrowed from the caller for one draw.
pub struct PersonFeature<'a> {
    /// The server whose photo transcoder draws `thumb` (`Art::Person`'s `sid`).
    pub sid: u16,
    pub thumb: &'a str,
    pub name: &'a CStr,
    /// The departments, already localized, in order ("Actor", "Director").
    pub jobs: &'a [&'a str],
    /// One line about their work ("12 titles on MultiPlex"); empty draws none.
    pub note: &'a CStr,
}

/// Where the parts of a block at `at` sit: the pure half of [`PersonFeature::draw`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    pub plate: Rect,
    pub photo: Rect,
    /// The left edge of the words.
    pub text_x: f32,
    /// The cap tops of the name, the departments and the note.
    pub name_y: f32,
    pub jobs_y: f32,
    pub note_y: f32,
}

/// The block's parts for a block whose top-left is (`x`, `y`).
pub fn geometry(x: f32, y: f32) -> Geometry {
    let plate = Rect::new(x, y, W, H);
    let photo = Rect::new(x + PAD, y + PAD, PHOTO_D, PHOTO_D);
    // The three lines stand centred on the headshot, one SM apart (their cap heights approximated by
    // their sizes: the words are short and a pixel either way is not visible on a 168 px photo).
    let lines = NAME_SIZE as f32 + JOBS_SIZE as f32 + NOTE_SIZE as f32 + 2.0 * theme::space::SM;
    let name_y = photo.y + (PHOTO_D - lines) * 0.5;
    let jobs_y = name_y + NAME_SIZE as f32 + theme::space::SM;
    let note_y = jobs_y + JOBS_SIZE as f32 + theme::space::SM;
    Geometry { plate, photo, text_x: photo.x + PHOTO_D + GAP, name_y, jobs_y, note_y }
}

impl PersonFeature<'_> {
    /// The focus stop of a block whose top-left is (`x`, `y`): its plate.
    pub fn focus_rect(x: f32, y: f32) -> Rect {
        geometry(x, y).plate
    }

    /// Draw the block with its top-left at (`x`, `y`). `focused` lays the accent plate under it and
    /// inks the words for it.
    pub fn draw(&self, p: Painter, x: f32, y: f32, focused: bool) {
        let g = geometry(x, y);
        if focused {
            p.rrect(g.plate, crate::table::PILL_RAD, crate::table::PILL_RAD, theme::ACCENT);
        }
        widgets::card(p, g.photo, Art::Person { sid: self.sid, key: self.thumb, res: ART_RES },
            PHOTO_D * 0.5, false, 1.0, 0.0);
        let (ink, quiet) = if focused { (theme::ACCENT_INK, theme::ACCENT_INK) }
            else { (theme::TEXT_PRIMARY, theme::TEXT_SECONDARY) };
        Label::new(self.name.as_ptr(), NAME_SIZE, ink).bold().v(VAlign::CapTop)
            .draw(p, Rect::new(g.text_x, g.name_y, 0.0, 0.0));
        if !self.jobs.is_empty() {
            widgets::dotted_run(p, self.jobs, g.text_x, g.jobs_y, JOBS_SIZE, quiet, theme::space::XS);
        }
        if !self.note.is_empty() {
            Label::new(self.note.as_ptr(), NOTE_SIZE, quiet).v(VAlign::CapTop)
                .draw(p, Rect::new(g.text_x, g.note_y, 0.0, 0.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_stand_beside_the_headshot_inside_the_plate() {
        let g = geometry(80.0, 300.0);
        assert_eq!(g.plate, Rect::new(80.0, 300.0, W, H));
        assert_eq!(g.photo, Rect::new(80.0 + PAD, 300.0 + PAD, PHOTO_D, PHOTO_D));
        assert!(g.text_x > g.photo.x + g.photo.w);
        assert!(g.name_y >= g.photo.y && g.note_y + NOTE_SIZE as f32 <= g.photo.y + g.photo.h,
            "the three lines sit within the headshot's height");
        assert!(g.name_y < g.jobs_y && g.jobs_y < g.note_y);
        assert_eq!(PersonFeature::focus_rect(80.0, 300.0), g.plate);
    }
}
