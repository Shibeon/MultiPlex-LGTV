//! A Watchlist tile's marks and washes (boards TileStates and Watchlist).
//!
//! One bare mark in the watched tick's slot over the shared corner veil: a clock for an upcoming
//! title, a struck-through server for one whose server is offline, a ring with a bar for one no server holds. A released title that no server
//! holds wears a ring-and-bar over a heavier wash (55% at rest, 25% when focused). The veil, the
//! slot, the size and the ink are the watched tick's own (`widgets::corner_mark`,
//! `theme::TILE_MARK_VEIL`); only the glyph differs.
//!
//! **Every card also says, in words, whether the user's own libraries hold it**: a small chip on the
//! poster's top-left corner, the shared fine-print capsule over artwork (`widgets::tile_chip`),
//! reading "Available", "Not available" or "To check" ([`avail_label`]). Words rather than a glyph,
//! so the answer is a label any reader of the screen can say aloud, and so it never borrows the
//! corner mark's slot or the watched tick's meaning (`ui/CLAUDE.md`, card marks).
use plx_data::watchlist::model::{Availability, TileState};
use plx_platform::i18n::msg;
use plx_ui::cards::Tile;
use plx_ui::icons::Icon;
use plx_ui::theme;
use plx_ui::widgets;
use plx_machine::machine::Measure;
use plx_ui::{Painter, Rect};

/// The no-server wash at rest: `rgba(0,0,0,.55)` over the whole tile (Watchlist, TileStates).
pub const NO_SERVER_WASH: f32 = 0.55;
/// The same wash on the focused tile: `rgba(0,0,0,.25)`, so the title being looked at reads whole
/// enough while its neighbours stay quiet.
pub const NO_SERVER_WASH_FOCUSED: f32 = 0.25;

/// The corner glyph of a state, `None` when it wears none.
pub fn mark(state: TileState) -> Option<Icon> {
    match state {
        // Q6 (default, awaiting the owner): matched AND upcoming keeps the clock and stays playable.
        TileState::ComingSoon | TileState::ComingSoonMatched => Some(Icon::Clock),
        TileState::ServerOffline => Some(Icon::ServerSlash),
        TileState::NotOnAnyServer => Some(Icon::CircleSlash),
        TileState::Available | TileState::Unknown => None,
    }
}

/// The wash weight over the tile, `0` for a state with none.
pub fn wash(state: TileState, focused: bool) -> f32 {
    match (state, focused) {
        (TileState::NotOnAnyServer, false) => NO_SERVER_WASH,
        (TileState::NotOnAnyServer, true) => NO_SERVER_WASH_FOCUSED,
        _ => 0.0,
    }
}

/// Whether the state replaces the poster's own watched tick (a tile has ONE corner mark).
pub fn owns_corner(state: TileState) -> bool {
    mark(state).is_some()
}

/// The words of an availability chip: the card's accessible label for what the libraries hold.
pub fn avail_label(avail: Availability) -> &'static str {
    match avail {
        Availability::Available => msg::watchlist_avail_available(),
        Availability::Unavailable => msg::watchlist_avail_unavailable(),
        Availability::Unknown => msg::watchlist_avail_unknown(),
    }
}

/// Where the availability chip stands: its left edge and its top, `space::XS` in from the poster's
/// top-left corner (the corner mark owns the top-right one).
pub fn avail_anchor(tile: Rect) -> (f32, f32) {
    let inset = theme::space::XS;
    (tile.x + inset, tile.y + inset)
}

/// Paint a tile's state over its drawn poster (`CardSource::overlay`): the wash, then the mark,
/// then the availability chip. `tile` is where the card was drawn, press and pop included, so all
/// three ride the focus pop.
pub fn paint(p: Painter, state: TileState, avail: Availability, tile: &Tile, measure: &dyn Measure) {
    let w = wash(state, tile.focused);
    if w > 0.0 {
        p.rrect(tile.rect, tile.radius, tile.radius, theme::scrim_black(w));
    }
    if let Some(icon) = mark(state) {
        widgets::corner_mark(p, tile.rect, tile.radius, icon);
    }
    let (x, y) = avail_anchor(tile.rect);
    widgets::tile_chip(p, x, y, avail_label(avail), measure);
}
