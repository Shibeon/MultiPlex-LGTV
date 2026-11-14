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
//! Pure: the item, the session and the measure are arguments, read the way the draw reads them.

use super::geometry::{head_controls, HudGeometry};
use super::sections::{facts_run_in, Shape};
use crate::hud_format::compile::{CompiledState, ControlRect, DrawOp};

/// **How far the trailing controls of `state` move this frame**: the right edge of the hugging run's last chip
/// less the right edge of its room (`<= 0`). `0.0` when the state has no hugging run; the run's own `x` stands
/// for its edge while it keeps no chip (nothing streams yet), so the controls close up to where it starts.
pub fn trail_dx(
    state: &CompiledState,
    meta: plx_data::metadata::MetadataView<'_>,
    ps: &plx_media::route::PlaybackSession,
    measure: &dyn plx_machine::machine::Measure,
) -> f32 {
    let Some((rect, shape)) = state.ops.iter().find_map(|op| match op {
        DrawOp::Facts { rect, order, badges, stream, genres_max, playback_fact, size, bold, ink, hug: true } => Some((
            *rect,
            // a chip's width does not move with the frame's predicates: the inks are colours, and a stream pill
            // is always bold
            Shape { order, badges, stream, genres_max: *genres_max, playback_fact: *playback_fact, size: *size, bold: bold.at(|_| false), ink: ink.at(|_| false) },
        )),
        _ => None,
    }) else {
        return 0.0;
    };
    let right = facts_run_in(meta, ps, measure, rect, &shape).map_or(rect.x, |run| run.right(rect.x));
    right - (rect.x + rect.w)
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
        assert_eq!(trail_dx(state, store().view(), &idle, &measure), -room.w, "nothing streams: the disc closes up to the run's start");

        let mut widths = Vec::new();
        for (title, codec, profile, channels) in [
            ("A", "aac", "", 2),
            ("An exceedingly long film title that runs right across the head of the screen and then some", "dca", "ma", 8),
            ("B", "eac3", "dolby digital plus + dolby atmos", 6),
        ] {
            install(title, codec, profile, channels);
            let ps = streaming(codec);
            let dx = trail_dx(state, store().view(), &ps, &measure);
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
}
