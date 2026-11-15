//! Search's document geometry. Both engine placement and rendering use these expressions.
use plx_data::search::Kind;
use plx_ui::cards::RowStyle;
use plx_ui::consts::{CARD_H, CARD_W, MARGIN_X, SCR_H, SCR_W};
use plx_machine::machine::GroupId;
use plx_ui::Rect;

/// `pub`, alongside this module itself (`screens/search/mod.rs`'s `pub mod layout`)
/// so `ui::consts`'s overscan-rects audit can reach it as `crate::search::layout::FIELD`
/// — the deleted legacy Search renderer's own `FIELD`/`CONTENT_TOP` this replaces.
///
/// The MultiPlex web app's search capsule: a pill from the left margin to the screen's middle (the
/// web field ends about half way across), its plate drawn by `page_header::draw_field_plate`. Its
/// top stands where every other page's head does ([`plx_ui::page_header::TOP`], the Library's and My
/// list's title block), so the capsule keeps the same air under the nav bar.
pub const FIELD: Rect = Rect {
    x: MARGIN_X,
    y: plx_ui::page_header::TOP,
    w: SCR_W * 0.5 - MARGIN_X,
    h: 80.0,
};
/// The words' inset inside [`FIELD`] each side: the pill's own end radius, so the first letter
/// starts where the round end meets the straight edge.
pub(super) const FIELD_INSET: f32 = FIELD.h * 0.5;
/// The words' box inside the capsule: [`FIELD`] less [`FIELD_INSET`] each side.
pub(super) const FIELD_TEXT_W: f32 = FIELD.w - 2.0 * FIELD_INSET;
/// The query's size: the web field's words are body copy, not a hero line; TITLE keeps them
/// readable from the couch inside an 80px capsule.
pub(super) const FIELD_TEXT_SIZE: i32 = plx_ui::theme::size::TITLE;
pub(super) const SCOPE_Y: f32 = FIELD.y + FIELD.h + 12.0;
/// See [`FIELD`]'s doc: also reachable as `crate::search::CONTENT_TOP`. Value pinned at
/// 300.0 — the overscan audit and the page's `Head` section height (`Stack` stacks the shelves
/// below it) both depend on it.
pub const CONTENT_TOP: f32 = 300.0;
pub(super) const HEAD_TO_ROW: f32 = 60.0;
pub(super) const KEYBOARD_H: f32 = 324.0;
pub(super) const SCOPE_H: f32 = plx_ui::theme::size::CAPTION as f32 * 1.35;
pub(super) const CLEAR_H: f32 = 60.0;
pub(super) const RECENT_CAP: usize = plx_data::search::recents::CAP;

/// The featured person's section ([`super::Sec::Featured`]): its card and a step of air before
/// the first row's heading.
pub(super) const FEATURE_BLOCK_H: f32 = plx_ui::person_feature::H + plx_ui::theme::space::SM;
/// Where the featured card's plate stands: its headshot on the rows' left margin.
pub(super) const FEATURE_X: f32 = MARGIN_X - plx_ui::person_feature::PAD;
/// The featured card's focus stop in a section whose top is `top`.
pub(super) fn feature(top: f32) -> Rect {
    plx_ui::person_feature::PersonFeature::focus_rect(FEATURE_X, top)
}

pub(super) fn ordinal(kind: Kind) -> u32 {
    match kind {
        Kind::Movie => 0,
        Kind::Show => 1,
        Kind::Episode => 2,
        Kind::Person => 3,
        Kind::Collection => 4,
    }
}
pub(super) fn group(kind: Kind) -> GroupId {
    GroupId(0x5345_4200 + ordinal(kind))
}
static EPISODE: RowStyle = RowStyle { w: 420.0, h: 236.0, ..RowStyle::HOME };
static PERSON: RowStyle = RowStyle { w: 250.0, h: 250.0, circular: true, ..RowStyle::HOME };
static POSTER: RowStyle = RowStyle { w: CARD_W, h: CARD_H, ..RowStyle::HOME };

/// The row style of `kind`'s shelf: the poster shelf's, with the still and the headshot sized to
/// their own aspect.
pub(super) fn style(kind: Kind) -> &'static RowStyle {
    match kind {
        Kind::Episode => &EPISODE,
        Kind::Person => &PERSON,
        _ => &POSTER,
    }
}
/// Whether `kind`'s tiles are POSTERS, and so follow the Under posters setting (their caption
/// built by `registry::captions`, their band sized by `caption_policy::band_h`). The Episode row is a
/// landscape still and the Person row a headshot: neither is a poster, and both keep the legacy
/// block under every setting.
pub(super) fn is_poster(kind: Kind) -> bool {
    matches!(kind, Kind::Movie | Kind::Show | Kind::Collection)
}
/// A shelf block's height under a caption band of `band` px (a shelf's live
/// [`Shelf::under_band`](plx_ui::cards::Shelf::under_band), or [`card_row::under_band`] of a
/// settled 0 / 1).
pub(super) fn block_h(kind: Kind, band: f32) -> f32 {
    HEAD_TO_ROW + style(kind).h + caption_band(band)
}
pub(super) fn caption_band(band: f32) -> f32 {
    plx_ui::consts::SHELF_FOOT + band
}
pub(super) fn recent(slot: usize, scroll: f32) -> Rect {
    Rect::new(
        MARGIN_X,
        CONTENT_TOP + plx_ui::table::HDR_H + slot as f32 * plx_ui::table::ROW_H - scroll,
        820.0,
        plx_ui::table::ROW_H,
    )
}
pub(super) fn clear(terms: usize, scroll: f32, measure: &dyn plx_machine::machine::Measure) -> Rect {
    clear_below(recent(terms.min(RECENT_CAP), scroll).y, measure)
}
/// The Clear control under a recents block whose last row ends at `list_end`.
pub(super) fn clear_below(list_end: f32, measure: &dyn plx_machine::machine::Measure) -> Rect {
    let width = plx_ui::widgets::Button::pill_w_measured(
        plx_platform::i18n::msg::browse_search_clear_c(),
        plx_ui::theme::size::BODY,
        false,
        None,
        measure,
    );
    Rect::new(
        MARGIN_X + plx_ui::table::CONTENT_X,
        list_end + plx_ui::theme::space::MD,
        width,
        CLEAR_H,
    )
}

pub(super) fn empty_band(editing: bool) -> Rect {
    let bottom = if editing { SCR_H - KEYBOARD_H } else { SCR_H };
    Rect::new(0.0, CONTENT_TOP, SCR_W, bottom - CONTENT_TOP)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Measure;
    impl plx_machine::machine::Measure for Measure {
        fn width(&self, _: &std::ffi::CStr, _: i32, _: bool) -> f32 {
            0.0
        }
        fn cap_h(&self, _: i32) -> f32 {
            0.0
        }
        fn line_h(&self, _: i32) -> f32 {
            0.0
        }
    }

    #[test]
    fn clear_geometry_stays_at_the_recents_cap() {
        let measure = Measure;
        let capped = clear(RECENT_CAP, 0.0, &measure);
        let overlong = clear(RECENT_CAP + 1, 0.0, &measure);
        assert_eq!(overlong.y, capped.y);
    }

    fn full() -> f32 {
        plx_ui::cards::under_band(1.0)
    }
    const ALL: [Kind; 5] = plx_data::search::KINDS;

    #[test]
    fn the_reserved_caption_band_holds_the_block_the_shared_component_draws() {
        let drawn = plx_ui::cards::TileLabel::height(true);
        assert!(
            drawn <= caption_band(full()),
            "the label block draws {drawn}px into a band of {}px",
            caption_band(full())
        );
    }

    #[test]
    fn block_heights_are_the_row_pitch_for_posters_and_follow_the_style_otherwise() {
        assert_eq!(block_h(Kind::Movie, full()), plx_ui::consts::ROW_PITCH);
        assert_eq!(block_h(Kind::Show, full()), plx_ui::consts::ROW_PITCH);
        assert_eq!(block_h(Kind::Collection, full()), plx_ui::consts::ROW_PITCH);
        assert_eq!(block_h(Kind::Episode, full()), HEAD_TO_ROW + 236.0 + caption_band(full()));
        assert_eq!(block_h(Kind::Person, full()), HEAD_TO_ROW + 250.0 + caption_band(full()));
        assert_eq!(block_h(Kind::Movie, full()) - block_h(Kind::Episode, full()), CARD_H - 236.0);
    }

    #[test]
    fn the_first_shelfs_whole_row_clears_the_raised_keyboard() {
        let floor = SCR_H - KEYBOARD_H;
        for kind in ALL {
            let bottom = CONTENT_TOP + HEAD_TO_ROW + style(kind).h;
            assert!(bottom <= floor, "{kind:?}: the first row ends at {bottom}, under the keyboard at {floor}");
        }
        assert_eq!(CONTENT_TOP + HEAD_TO_ROW + CARD_H, 735.0);
        assert_eq!(floor, 756.0);
    }

    /// Legacy `ui/search/mod.rs`'s `the_content_line_clears_the_documents_head`: the document's
    /// head is the field plus its scope line, and both are measured off [`FIELD`] — so this is the
    /// one assertion that keeps the three numbers agreeing when any of them moves. Without it
    /// shelf 0's heading would be drawn over the scope line at rest.
    #[test]
    fn the_content_line_clears_the_documents_head() {
        let head_bottom = SCOPE_Y + SCOPE_H;
        assert!(
            CONTENT_TOP > head_bottom,
            "content starts at {CONTENT_TOP} inside a head that ends at {head_bottom}"
        );
    }

    #[test]
    fn a_full_block_finishes_clear_of_the_raised_keyboard() {
        let kbd_top = SCR_H - KEYBOARD_H;
        let bottom = clear(RECENT_CAP, 0.0, &Measure).y + CLEAR_H;
        let clearance = kbd_top - bottom;
        assert!(
            clearance >= plx_ui::theme::space::LG,
            "a full block ends at {bottom} and the keyboard starts at {kbd_top} — {clearance}px"
        );
    }
}
