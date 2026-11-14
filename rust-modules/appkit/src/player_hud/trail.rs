//! **A hugging `facts` run and the controls that trail it** (format 1.2's `facts.w`; `docs/hud-plugins.md`, D15).
//!
//! The MultiPlex head stands its stream pills on the row under the title and the ⓘ disc right after the LAST
//! pill, as the web player does: the pills say what this session's stream is, so how wide they are is only known
//! once it resolved. The compile places the run in its room (`w`) and the controls after it as if the run filled
//! that room ([`ControlRect::trail`]); a frame closes them up behind the chips the run actually keeps, by
//! [`trail_dx`]. The draw (`render`) and the screen's geometry report ([`shift`], the stops and hit rects the
//! FocusEngine and the pointer read) take the SAME shift, from the same reads, so the ring, the hit rect and
//! the pixels never part. Only a head control trails ([`CompiledState::is_head`]): the row, the tabs and a
//! section's actions keep their compiled rect.
//!
//! While a play is pending (`route::play_pending()`: the Up Next hand-off, a version switch) the stream has no
//! facts to show and the run keeps no stream pill; the controls then stay where the last resolved stream put them
//! ([`TrailHold`], on [`super::motion::HudMotion`]) rather than snapping to the run's start and back.
//!
//! The item, the session, the paused state and the measure are arguments, read the way the draw reads them.

use super::geometry::{head_controls, HudGeometry};
use super::sections::{facts_run_in, Shape};
use crate::hud_format::compile::{CompiledState, ControlRect, DrawOp};

/// **How far the trailing controls of `state` move this frame**: the right edge of the hugging run's last chip
/// less the right edge of its room (`<= 0`). `0.0` when the state has no hugging run; the run's own `x` stands
/// for its edge while it keeps no chip (nothing streams yet), so the controls close up to where it starts. While a
/// play is pending, the shift the draw last settled ([`TrailHold`], `row.trail`).
///
/// The run's keyed values (`bold`, `ink`) resolve with the draw's own predicates (`sections::holds_in`, from
/// `paused` and the item), so a package whose `order` run is bold only in some states measures what it draws.
pub fn trail_dx(
    state: &CompiledState,
    meta: plx_data::metadata::MetadataView<'_>,
    ps: &plx_media::route::PlaybackSession,
    paused: bool,
    measure: &dyn plx_machine::machine::Measure,
    row: &super::motion::HudMotion,
) -> f32 {
    let holds = super::sections::holds_in(paused, meta);
    let Some((rect, shape)) = state.ops.iter().find_map(|op| match op {
        DrawOp::Facts { rect, order, badges, stream, genres_max, playback_fact, size, bold, ink, hug: true } => Some((
            *rect,
            Shape { order, badges, stream, genres_max: *genres_max, playback_fact: *playback_fact, size: *size, bold: bold.at(holds), ink: ink.at(holds) },
        )),
        _ => None,
    }) else {
        return 0.0;
    };
    let right = facts_run_in(meta, ps, measure, rect, &shape).map_or(rect.x, |run| run.right(rect.x));
    row.trail.read(right - (rect.x + rect.w), plx_media::route::play_pending())
}

/// **The trail's shift across a pending play**: the draw settles each frame's shift here, the screen's geometry
/// report reads it, and while a play is pending both use the last shift a resolved stream gave instead of the
/// unresolved run's (which keeps no stream pill). A render memo, held on [`super::motion::HudMotion`] beside the
/// scrub preview's; `None` until a stream resolved.
#[derive(Debug, Default)]
pub struct TrailHold {
    last: std::cell::Cell<Option<f32>>,
}

impl TrailHold {
    /// The shift to use for this frame's `dx`: `dx` itself (and remembered) when the stream is resolved, the held
    /// one while a play is `pending`, `dx` when nothing was ever held.
    pub fn settle(&self, dx: f32, pending: bool) -> f32 {
        if !pending {
            self.last.set(Some(dx));
        }
        self.read(dx, pending)
    }

    /// [`TrailHold::settle`]'s answer without remembering anything: the geometry report's read.
    pub fn read(&self, dx: f32, pending: bool) -> f32 {
        match self.last.get() {
            Some(held) if pending => held,
            _ => dx,
        }
    }
}

/// Does `c` move with the hugging run ([`trail_dx`])? A trailing control of the head.
pub fn trails(state: &CompiledState, c: &ControlRect) -> bool {
    c.trail && state.is_head(c)
}

/// **The geometry report of `state` with its trailing head controls moved by `dx`** ([`trail_dx`]): the rects
/// the stops and the pointer read are where the frame draws the controls.
pub fn shift(geo: &mut HudGeometry, state: &CompiledState, dx: f32) {
    if dx == 0.0 {
        return;
    }
    for (i, c) in head_controls(state).take(geo.head.len()).enumerate() {
        if c.trail {
            geo.head[i] = geo.head[i].map(|r| plx_ui::Rect { x: r.x + dx, ..r });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hud_format::Slot;
    use crate::player_hud::motion::HudMotion;
    use plx_data::stores::metadata::{MetadataCmd, MetadataStore};

    // a thread-confined store, so the item installed and the view read are the same owner
    thread_local! {
        static STORE: std::cell::UnsafeCell<MetadataStore> = std::cell::UnsafeCell::new(MetadataStore::default());
    }
    fn store() -> &'static mut MetadataStore {
        STORE.with(|cell| unsafe { &mut *cell.get() })
    }

    /// A playing film named `title` whose one audio track is `codec`/`profile`, `channels` wide.
    fn install(title: &str, codec: &str, profile: &str, channels: i64) {
        plx_data::metadata::set_current_for_test(store().state_mut(), Some(plx_data::metadata::Detail {
            rk: "rk".into(),
            kind: "movie".into(),
            title: title.into(),
            ..Default::default()
        }));
        let audio = vec![plx_data::metadata::Stream { id: 7, codec: codec.into(), profile: profile.into(), channels, default: true, ..Default::default() }];
        assert!(store().run(MetadataCmd::InstallPlaying(Some(plx_data::metadata::PlayingItem {
            sid: plx_plex::plex::ServerId::from_raw(0),
            rk: "rk".into(),
            duration_ms: None,
            show_rk: String::new(),
            kind: "movie".into(),
            audio,
            subs: Vec::new(),
            video_fps: 0.0,
            width: 1920,
            height: 1080,
            bitrate: 0,
            dovi: Default::default(),
            markers: Vec::new(),
            chapters: Vec::new(),
            hdr: false,
            hdr_kind: Default::default(),
        }))));
    }

    fn streaming(acodec: &str) -> plx_media::route::PlaybackSession {
        let mut ps = plx_media::route::PlaybackSession::default();
        plx_media::route::set_url(&mut ps, "http://pms.invalid/library/parts/1/file.mkv");
        plx_media::route::set_stream_acodec(&mut ps, acodec);
        ps
    }

    /// The hugging run of `state`: its room.
    fn room(state: &CompiledState) -> plx_ui::Rect {
        state.ops.iter().find_map(|op| match op { DrawOp::Facts { rect, hug: true, .. } => Some(*rect), _ => None }).expect("the stream row")
    }

    /// **The info disc closes up behind the last pill the row keeps**, with the room's gap before it, for a short
    /// pill and for a long one (a long title changes nothing: the title is a band of its own above the row). With
    /// nothing streaming the run keeps no chip and the disc stands where the run starts. The draw and the
    /// geometry report read this one number, so the ring and the pixels agree.
    #[test]
    fn the_info_disc_trails_the_last_kept_pill_whatever_its_width() {
        let _g = plx_base::testlock::serial();
        let compiled = crate::player_hud::loaded::multiplex_for_test();
        let state = compiled.lookup(0, Slot::Discs, false, false, false).expect("the compact film state");
        let room = room(state);
        let disc = state.control("info").expect("the info disc").rect;
        let gap = disc.x - (room.x + room.w);
        assert!(gap > 0.0, "the disc stands after the room, a gap apart");
        let measure = plx_ui::fixture::FixtureMeasure;

        let idle = plx_media::route::PlaybackSession::default();
        install("A", "aac", "", 2);
        assert_eq!(trail_dx(state, store().view(), &idle, false, &measure, &HudMotion::new()), -room.w, "nothing streams: the disc closes up to the run's start");

        let mut widths = Vec::new();
        for (title, codec, profile, channels) in [
            ("A", "aac", "", 2),
            ("An exceedingly long film title that runs right across the head of the screen and then some", "dca", "ma", 8),
            ("B", "eac3", "dolby digital plus + dolby atmos", 6),
        ] {
            install(title, codec, profile, channels);
            let ps = streaming(codec);
            let dx = trail_dx(state, store().view(), &ps, false, &measure, &HudMotion::new());
            let right = room.x + room.w + dx;
            assert!(dx < 0.0 && right > room.x, "{codec}: the run keeps its pills and ends inside its room ({dx})");
            assert_eq!(disc.x + dx - right, gap, "{codec}: the disc is the room's gap after the last pill");
            let mut geo = state.geometry(crate::hud_format::compile::SkipKind::Intro);
            let before = geo.head.clone();
            shift(&mut geo, state, dx);
            for (i, c) in crate::player_hud::geometry::head_controls(state).enumerate() {
                let moved = before[i].map(|r| r.x + if c.trail { dx } else { 0.0 });
                assert_eq!(geo.head[i].map(|r| r.x), moved, "{codec}: {} in the geometry report", c.id);
            }
            widths.push(right - room.x);
        }
        assert!(widths[1] > widths[0], "a long codec name is a wider row, and the disc follows it: {widths:?}");
        let _ = store().run(MetadataCmd::InstallPlaying(None));
    }

    /// **A pending play keeps the last resolved shift** (the Up Next hand-off): the unresolved run keeps no stream
    /// pill, and without the hold the disc would close up to the run's start for the length of the resolve and
    /// spring back after. Before any stream resolved there is nothing to hold, and once one resolves again its own
    /// shift is the answer.
    #[test]
    fn a_pending_play_holds_the_last_resolved_shift() {
        let hold = TrailHold::default();
        assert_eq!(hold.read(-300.0, true), -300.0, "nothing held yet: the frame's own shift");
        assert_eq!(hold.settle(-120.0, false), -120.0, "a resolved stream's shift is used and held");
        assert_eq!(hold.settle(-300.0, true), -120.0, "pending: the held shift, not the empty run's");
        assert_eq!(hold.read(-300.0, true), -120.0, "the geometry report reads the same");
        assert_eq!(hold.read(-120.0, false), -120.0);
        assert_eq!(hold.settle(-80.0, false), -80.0, "resolved again: the new stream's shift");
        assert_eq!(hold.read(-300.0, true), -80.0);
    }

    /// **The run's keyed values resolve with the draw's predicates**, so a package whose `order` run is bold only
    /// while paused measures the bold run while paused: the shift the screen reports is the one the draw takes.
    #[test]
    fn a_keyed_bold_order_run_is_measured_as_the_frame_draws_it() {
        let _g = plx_base::testlock::serial();
        let compiled = crate::player_hud::loaded::multiplex_for_test();
        let mut state = compiled.lookup(0, Slot::Discs, false, false, false).expect("the compact film state").clone();
        for op in state.ops.iter_mut() {
            if let DrawOp::Facts { hug: true, order, bold, .. } = op {
                *order = vec!["year".into(), "duration".into()];
                let paused = crate::hud_format::parse::CondTerm { negated: false, name: "transport.paused".into() };
                *bold = crate::hud_format::compile::Live { base: false, cases: vec![(paused, true)] };
            }
        }
        // the fixture's widths with bold a quarter wider, so the weight shows in the measure
        struct BoldWider;
        impl plx_machine::machine::Measure for BoldWider {
            fn width(&self, s: &std::ffi::CStr, sz: i32, bold: bool) -> f32 {
                plx_ui::fixture::FixtureMeasure.width(s, sz, bold) * if bold { 1.25 } else { 1.0 }
            }
            fn cap_h(&self, sz: i32) -> f32 {
                plx_ui::fixture::FixtureMeasure.cap_h(sz)
            }
            fn line_h(&self, sz: i32) -> f32 {
                plx_ui::fixture::FixtureMeasure.line_h(sz)
            }
        }
        let measure = BoldWider;
        install("A", "aac", "", 2);
        // after `install`, which sets a bare item: this one has a year and a runtime for the `order` run
        plx_data::metadata::set_current_for_test(store().state_mut(), Some(plx_data::metadata::Detail {
            rk: "rk".into(),
            kind: "movie".into(),
            title: "A".into(),
            year: 2019,
            dur_ms: 5_520_000,
            ..Default::default()
        }));
        let ps = streaming("aac");
        let row = HudMotion::new();
        let playing = trail_dx(&state, store().view(), &ps, false, &measure, &row);
        let paused = trail_dx(&state, store().view(), &ps, true, &measure, &row);
        assert!(paused > playing, "the bold run is wider, so the disc stands further right: {playing} -> {paused}");
        let _ = store().run(MetadataCmd::InstallPlaying(None));
    }
}
