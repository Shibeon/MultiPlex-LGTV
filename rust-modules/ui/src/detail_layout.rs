//! Shared detail hero geometry, also used by the contrast and safe-area audits.
//!
//! **The Detail hero is two columns** (MultiPlex web's detail page): the item's poster card on the
//! left ([`POSTER_X`], [`POSTER_TOP`], [`POSTER_W`] × [`POSTER_H`]) and a text column to its right
//! ([`TEXT_X`]) that grows DOWN from the title's foot ([`TITLE_FOOT`]): on an episode's or a season's
//! page the subtitle (its own title, under its show's logo), the meta line (date, runtime,
//! content rating), the ratings row (the providers' scores, then the viewer's five stars), the
//! action row (the wide Play/Resume capsule and the round More disc), the synopsis, the two-column
//! credit table and the technical pills. [`hero_chain`] is that ladder; absent rows leave no gap.
use super::{consts::{MARGIN_X, MARGIN_Y}, theme, widgets};
use super::hstack::Item;
use super::vstack::{Anchor, VStack};
#[cfg(any(test, feature = "test-support"))]
use super::{consts::SCR_W, Rect};
#[cfg(test)]
use super::consts::SCR_H;

/// The reading column's width: the synopsis wraps here, and the credit values elide here.
pub const HERO_TEXT_W: f32 = theme::hero::TEXT_W;
/// The title foot the CATALOG page (`screens::catalog`) stands its single-column hero on. The
/// Detail hero's own two-column ladder is anchored at [`TITLE_FOOT`] instead.
pub const TITLE_BOTTOM: f32 = 566.0;

/// The poster card's left edge: the safe margin, where every page's content starts.
pub const POSTER_X: f32 = MARGIN_X;
/// The poster card's top edge, in page coordinates at rest.
pub const POSTER_TOP: f32 = 240.0;
/// The poster card's width, and its height at the poster's own 2:3.
pub const POSTER_W: f32 = 326.0;
pub const POSTER_H: f32 = POSTER_W * 1.5;
/// The resolution the poster card asks the server for: the card's own size, rounded up a little
/// so the shared card's focus-free draw never upscales.
pub const POSTER_RES: (std::os::raw::c_int, std::os::raw::c_int) = (360, 540);
/// The text column's left edge: the poster's right edge and the page's widest rung of air.
pub const TEXT_X: f32 = POSTER_X + POSTER_W + theme::space::XL;
/// The title band's foot: a one-line typed title's line box, one small rung below the poster's top,
/// so its cap band starts level with the card beside it. A taller logo or a second title line
/// grows upward from here (`hero_logo`'s bottom anchor), and nothing under it moves.
pub const TITLE_FOOT: f32 = POSTER_TOP + theme::hero::TITLE_LEAD + theme::space::SM;

/// One credit row's line box in the credit table (label · rule · value).
pub const CREDIT_LEAD: f32 = theme::space::LG;
/// The air on either side of the credit table's vertical rule.
pub const CREDIT_RULE_GAP: f32 = theme::space::MD;
/// The rule's own width.
pub const CREDIT_RULE_W: f32 = 2.0;
/// The air between two technical pills.
pub const TECH_GAP: f32 = theme::space::SM;

/// The facts row's ink, and the second time this trade has been made: at `TEXT_TERTIARY`
/// (L=0.317) the row needed α≈0.79 of scrim for 4.5:1 — an essentially black hero corner — so
/// `widgets_hero_scrim_tests` carried it at a lowered 2.6 floor and wrote the ink step down as
/// deferred. One ink step costs nothing on screen and takes the row to the ordinary 3.0 floor.
pub const FACTS_INK: [f32; 4] = theme::TEXT_SECONDARY;
/// A credit row's VALUE ink (the names, the genres): secondary, like the facts.
pub const CREDIT_INK: [f32; 4] = theme::TEXT_SECONDARY;
/// A credit row's uppercase LABEL ink: tertiary, the About labels' weight.
pub const CREDIT_LABEL_INK: [f32; 4] = theme::TEXT_TERTIARY;
pub const COMPACT_TITLE_BOT: f32 = MARGIN_Y + theme::logo::COMPACT_H_MAX;
pub const TOP_MARGIN: f32 = COMPACT_TITLE_BOT + 26.0;


/// Top-left anchor for the hero logo while a trailer plays in the background — independent of
/// scroll, unlike the pinned [`COMPACT_TITLE_BOT`] title (which answers "how far down have I
/// scrolled", not "is a trailer playing"). Sits inside the safe-area margins.
pub const PREVIEW_LOGO_X: f32 = MARGIN_X;
pub const PREVIEW_LOGO_Y: f32 = MARGIN_Y;
/// Keeps a wide wordmark from reaching toward the center of the screen once it has shrunk.
pub const PREVIEW_LOGO_MAX_W: f32 = 480.0;

/// Where each row of the text column stands, in page coordinates at rest. A row the item does not
/// have is `None` and takes no room.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeroChain {
    /// The subtitle line: an episode's or a season's own title under its show's logo
    /// ([`HeroRows::subtitle`]).
    pub subtitle_y: Option<f32>,
    /// The meta line (date · runtime · content rating, and how it plays).
    pub meta_y: f32,
    /// The ratings row: the providers' groups and the viewer's stars.
    pub ratings_y: Option<f32>,
    /// The action row: Play/Resume and the More disc.
    pub btn_y: f32,
    pub syn_y: Option<f32>,
    /// The credit table's first row.
    pub credits_y: Option<f32>,
    /// The technical pills' row.
    pub tech_y: Option<f32>,
    /// The hero's foot: the lower of the text column's last row and the poster card.
    pub foot: f32,
}

/// What the text column holds, measured: each row that is present, by the height it draws.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HeroRows {
    /// The title band names the item's show and the page's own title stands on its own line under
    /// it ([`theme::hero::SUBTITLE_H`]), the facts a small rung below.
    pub subtitle: bool,
    pub ratings: bool,
    /// The synopsis' measured height; `None` (or nothing to say) takes no room.
    pub synopsis_h: Option<f32>,
    /// How many credit rows the table has (0: no table).
    pub credits: usize,
    pub tech: bool,
}

/// Detail's ladder, anchored at the title's foot and growing down: the identity group (meta,
/// ratings) tight under the title, block air around the action row and the prose, medium air
/// between the reference rows under it.
pub fn hero_chain(rows: HeroRows) -> HeroChain {
    let cd = widgets::StatusOverlay::CTRL_H;
    let items = [
        // the first row's air is the anchor's: a stack ignores `before` on its first item
        if rows.subtitle { Item::new(theme::hero::SUBTITLE_H) } else { Item::absent() },
        Item::new(theme::hero::FACTS_H).before(theme::space::XS),
        if rows.ratings { Item::new(widgets::RATING_MARK_D).before(theme::space::MD) } else { Item::absent() },
        Item::new(cd).before(theme::space::LG),
        rows.synopsis_h.filter(|h| *h > 0.0).map_or(Item::absent(), |h| Item::new(h).before(theme::space::LG)),
        if rows.credits > 0 {
            Item::new(CREDIT_LEAD * rows.credits as f32).before(theme::space::MD)
        } else {
            Item::absent()
        },
        if rows.tech { Item::new(widgets::BADGE_H).before(theme::space::MD) } else { Item::absent() },
    ];
    let stack = VStack::new(&items, 0.0, Anchor::Top(TITLE_FOOT + theme::space::SM));
    let y = |i| stack.span(i).map(|(y, _)| y);
    let text_foot = stack.placed().last().map_or(TITLE_FOOT, |s| s.y + s.h);
    HeroChain {
        subtitle_y: y(0),
        meta_y: y(1).unwrap_or(TITLE_FOOT),
        ratings_y: y(2),
        btn_y: y(3).unwrap_or(TITLE_FOOT),
        syn_y: y(4),
        credits_y: y(5),
        tech_y: y(6),
        foot: text_foot.max(POSTER_TOP + POSTER_H),
    }
}

pub fn base_scrim_a(y: f32, hero_vis: f32) -> f32 {
    super::landing_hero::base_scrim_a(y, hero_vis)
}

#[cfg(any(test, feature = "test-support"))]
pub fn overscan_rects(out: &mut Vec<(&'static str, Rect)>) {
    let tall = theme::logo::COMPACT_H_MAX;
    out.push(("detail pinned compact title (tallest logo)",
        Rect::new(MARGIN_X, COMPACT_TITLE_BOT - tall, SCR_W - 2.0 * MARGIN_X, tall)));
    out.push(("detail hero poster", Rect::new(POSTER_X, POSTER_TOP, POSTER_W, POSTER_H)));
    out.push(("detail hero text column", Rect::new(TEXT_X, TITLE_FOOT - 200.0, HERO_TEXT_W, 200.0)));
    out.push(("detail below-hero flow, at rest", Rect::new(MARGIN_X, TOP_MARGIN, SCR_W - 2.0 * MARGIN_X, 100.0)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_column_stacks_down_from_the_title_foot() {
        let full = hero_chain(HeroRows { ratings: true, synopsis_h: Some(72.0), credits: 2, tech: true, ..HeroRows::default() });
        assert_eq!(full.subtitle_y, None, "no subtitle: the facts lead the column");
        assert_eq!(full.meta_y, TITLE_FOOT + theme::space::SM);
        let ratings = full.ratings_y.unwrap();
        assert_eq!(ratings, full.meta_y + theme::hero::FACTS_H + theme::space::MD);
        assert_eq!(full.btn_y, ratings + widgets::RATING_MARK_D + theme::space::LG);
        let syn = full.syn_y.unwrap();
        assert_eq!(syn, full.btn_y + widgets::StatusOverlay::CTRL_H + theme::space::LG);
        let credits = full.credits_y.unwrap();
        assert_eq!(credits, syn + 72.0 + theme::space::MD);
        let tech = full.tech_y.unwrap();
        assert_eq!(tech, credits + 2.0 * CREDIT_LEAD + theme::space::MD);
        assert_eq!(full.foot, tech + widgets::BADGE_H);
        assert!(full.foot < SCR_H, "the whole hero stands on the first screen");
    }

    /// An episode's (or a season's) own title stands on its own line under the show's logo, the
    /// facts one small rung below it, and everything under them moves down by exactly that line.
    #[test]
    fn the_subtitle_line_stands_between_the_title_band_and_the_facts() {
        let rows = HeroRows { ratings: true, synopsis_h: Some(72.0), credits: 2, tech: true, ..HeroRows::default() };
        let plain = hero_chain(rows);
        let sub = hero_chain(HeroRows { subtitle: true, ..rows });
        assert_eq!(sub.subtitle_y, Some(TITLE_FOOT + theme::space::SM));
        assert_eq!(sub.meta_y, TITLE_FOOT + theme::space::SM + theme::hero::SUBTITLE_H + theme::space::XS);
        let shift = sub.meta_y - plain.meta_y;
        assert_eq!(shift, theme::hero::SUBTITLE_H + theme::space::XS);
        assert_eq!(sub.btn_y, plain.btn_y + shift);
        assert_eq!(sub.tech_y.unwrap(), plain.tech_y.unwrap() + shift);
        assert!(theme::hero::SUBTITLE_SIZE < theme::size::DISPLAY, "smaller than any hero title");
        assert!(theme::hero::SUBTITLE_SIZE > theme::hero::IDENTITY_SIZE, "larger than the facts");
    }

    #[test]
    fn an_absent_row_leaves_no_gap_and_the_poster_holds_the_foot() {
        let bare = hero_chain(HeroRows::default());
        assert_eq!(bare.ratings_y, None);
        assert_eq!(bare.btn_y, bare.meta_y + theme::hero::FACTS_H + theme::space::LG);
        assert_eq!((bare.syn_y, bare.credits_y, bare.tech_y), (None, None, None));
        assert_eq!(bare.foot, POSTER_TOP + POSTER_H, "a short column ends where the poster does");
        let empty_blurb = hero_chain(HeroRows { synopsis_h: Some(0.0), ..HeroRows::default() });
        assert_eq!(empty_blurb, bare, "an empty synopsis is no row at all");
    }

    #[test]
    fn the_columns_sit_inside_the_safe_area() {
        assert_eq!(POSTER_X, MARGIN_X);
        assert!(TEXT_X > POSTER_X + POSTER_W);
        assert!(TEXT_X + HERO_TEXT_W <= SCR_W - MARGIN_X);
        assert!(TITLE_FOOT > POSTER_TOP && TITLE_FOOT < POSTER_TOP + POSTER_H);
        assert!(POSTER_TOP > TOP_MARGIN, "the compact title's band is clear of the poster");
    }

    #[test]
    fn base_scrim_uses_the_shared_proposal_foot_and_clamps_inputs() {
        assert_eq!(base_scrim_a(0.0, 1.0), 0.0);
        assert_eq!(base_scrim_a(SCR_H, 1.0), theme::hero::SCRIM_FOOT_A);
        assert_eq!(base_scrim_a(SCR_H + 100.0, 2.0), theme::hero::SCRIM_FOOT_A);
        assert_eq!(base_scrim_a(SCR_H, -1.0), 0.0);
    }

    #[test]
    fn overscan_rects_name_the_detail_regions() {
        let mut got = Vec::new();
        overscan_rects(&mut got);
        let names: Vec<&str> = got.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, [
            "detail pinned compact title (tallest logo)",
            "detail hero poster",
            "detail hero text column",
            "detail below-hero flow, at rest",
        ]);
        let poster = got[1].1;
        assert_eq!((poster.x, poster.y, poster.w, poster.h), (POSTER_X, POSTER_TOP, POSTER_W, POSTER_H));
    }
}
