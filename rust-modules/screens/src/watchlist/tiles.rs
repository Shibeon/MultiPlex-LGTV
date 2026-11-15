//! A Watchlist tile's marks and washes (boards TileStates and Watchlist).
//!
//! One bare mark in the watched tick's slot over the shared corner veil, and it is also the card's
//! answer to "is it in my own libraries": a ring with a play triangle for a released title one of
//! the user's servers holds (until it is watched, when the poster's own watched tick takes the slot
//! back), a ring with a bar for one no server holds, a ring with a question mark for one no server
//! has answered for yet ("to check"), a struck-through server for one whose server is offline, and
//! a clock for an upcoming title, matched or not. A released title that no server holds also wears
//! a heavier wash (55% at rest, 25% when focused). The veil, the slot, the size and the ink are the
//! watched tick's own (`widgets::corner_mark`, `theme::TILE_MARK_VEIL`); only the glyph differs,
//! so a tile never carries two marks (`ui/CLAUDE.md`, card marks).
use plx_data::watchlist::model::TileState;
use plx_ui::cards::Tile;
use plx_ui::icons::Icon;
use plx_ui::theme;
use plx_ui::widgets;
use plx_ui::Painter;

/// The no-server wash at rest: `rgba(0,0,0,.55)` over the whole tile (Watchlist, TileStates).
pub const NO_SERVER_WASH: f32 = 0.55;
/// The same wash on the focused tile: `rgba(0,0,0,.25)`, so the title being looked at reads whole
/// enough while its neighbours stay quiet.
pub const NO_SERVER_WASH_FOCUSED: f32 = 0.25;

/// The corner glyph of a state, `None` when the slot is left to the poster's watched tick: a
/// held, released title that is already `watched`.
pub fn mark(state: TileState, watched: bool) -> Option<Icon> {
    match state {
        // Q6 (default, awaiting the owner): matched AND upcoming keeps the clock and stays playable.
        TileState::ComingSoon | TileState::ComingSoonMatched => Some(Icon::Clock),
        TileState::ServerOffline => Some(Icon::ServerSlash),
        TileState::NotOnAnyServer => Some(Icon::CircleSlash),
        TileState::Unknown => Some(Icon::CircleQuestion),
        TileState::Available if watched => None,
        TileState::Available => Some(Icon::CirclePlay),
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

/// Whether the state's mark replaces the poster's own watched tick (a tile has ONE corner mark).
/// Only a held, released title leaves the slot to the tick.
pub fn owns_corner(state: TileState) -> bool {
    state != TileState::Available
}

/// Paint a tile's state over its drawn poster (`CardSource::overlay`): the wash, then the mark.
/// `tile` is where the card was drawn, press and pop included, so both ride the focus pop;
/// `watched` is whether the poster wears its watched tick.
pub fn paint(p: Painter, state: TileState, watched: bool, tile: &Tile) {
    let w = wash(state, tile.focused);
    if w > 0.0 {
        p.rrect(tile.rect, tile.radius, tile.radius, theme::scrim_black(w));
    }
    if let Some(icon) = mark(state, watched) {
        widgets::corner_mark(p, tile.rect, tile.radius, icon);
    }
}
