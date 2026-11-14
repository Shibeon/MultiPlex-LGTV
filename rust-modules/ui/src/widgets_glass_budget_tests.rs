//! The top bar's glass at any strip width (the profile chip is a pill of the one glass track) and the dynamic glass refresh cadence.

use super::*;
#[allow(unused_imports)]
use super::test_support::*;

/// MultiPlex's bar, as the application publishes it: Search, the four destinations, the divider,
/// Settings and the chip's slot.
fn multiplex_faces() -> Vec<TabFace> {
    use crate::icons::Icon;
    vec![
        TabFace::Icon(Icon::Search),
        TabFace::IconLabel(Icon::Home, "Home".into()),
        TabFace::IconLabel(Icon::Movie, "Movies".into()),
        TabFace::IconLabel(Icon::Tv, "TV Shows".into()),
        TabFace::IconLabel(Icon::Bookmark, "My list".into()),
        TabFace::Divider,
        TabFace::Icon(Icon::Tune),
        TabFace::Avatar,
    ]
}

/// **The chip is a pill of the track, at its right end — so the band is ONE glass surface.** It
/// used to be a second capsule at the left margin, kept off the track by a name budget, because a
/// second glass surface over the first doubles the material (one blur cache, a second scrim and rim
/// composited over material that already carries both). Inside the track there is nothing to keep
/// apart: the slot is the track's last pill, `TAB_TRACK_PAD` from its right end like every end pill,
/// square, and the avatar stands `CHIP_RING` inside it on every side.
#[test]
fn the_chip_is_the_tracks_last_pill_and_the_avatar_stands_inside_it() {
    let faces = multiplex_faces();
    let widths = tab_metrics_from(&faces, |s| s.to_bytes().len() as f32 * 14.0).1;
    let geometry = tab_geometry(&widths, 0.0);
    let slot = geometry.pill(faces.len() - 1);
    let track = geometry.track;
    assert_eq!((slot.w, slot.h), (TAB_PILL_H, TAB_PILL_H), "the slot is a square pill");
    assert!((track.x + track.w - TAB_TRACK_PAD - (slot.x + slot.w)).abs() < 0.01,
        "the slot ends the track, one inset from its round end: {slot:?} in {track:?}");
    assert!(slot.y - track.y == TAB_TRACK_PAD && track.y + track.h - (slot.y + slot.h) == TAB_TRACK_PAD);
    let avatar = slot.inset(CHIP_RING);
    assert_eq!(avatar.w, CHIP_D);
    assert!(avatar.x > slot.x && avatar.x + avatar.w < slot.x + slot.w, "the focus capsule rings the avatar");
}

/// **The divider is drawn, never a stop.** It is a face so the faces and keys stay one parallel
/// pair, but `tab_members` publishes no member for its key: focus and the pointer pass straight from
/// My list to Settings, and the chip is the bar's last stop.
#[test]
fn the_divider_publishes_no_stop_and_the_chip_is_the_last() {
    let faces = multiplex_faces();
    let widths = tab_metrics_from(&faces, |s| s.to_bytes().len() as f32 * 14.0).1;
    let keys = [TAB_KEY_SEARCH, TAB_KEY_HOME, TAB_KEY_MOVIES, TAB_KEY_SHOWS, TAB_KEY_WATCHLIST,
        TAB_KEY_DIVIDER, TAB_KEY_SETTINGS, TAB_KEY_CHIP];
    let mut members = Vec::new();
    tab_members(&widths, &keys, 1, TopFocus::Chip, 0.0, &mut members);
    let elems: Vec<u32> = members.iter().map(|m| m.elem).collect();
    assert_eq!(elems, [TAB_KEY_SEARCH, TAB_KEY_HOME, TAB_KEY_MOVIES, TAB_KEY_SHOWS, TAB_KEY_WATCHLIST,
        TAB_KEY_SETTINGS, TAB_KEY_CHIP]);
    let watchlist = members[4].drawn;
    let settings = members[5].drawn;
    assert!(settings.x - (watchlist.x + watchlist.w) > 2.0 * 16.0,
        "the divider stands in the gap, so Settings sits further off than a plain gap");
    assert!(members.windows(2).all(|pair| pair[0].drawn.x < pair[1].drawn.x), "left to right");
}

/// A faded glass face (`chip_face`: the hold hint's and the scroll capsule's fade, once the chip's unfurl) fades the whole FACE, and the two rim weights are the half that matters.
///
/// `fs_glass.frag` writes `max(u_tint.a * cov, rimw)`: the rim is deliberately allowed to exceed
/// its own surface's coverage, so that a 1px line survives its antialiased edge. Fade only the
/// tint and the consequence is a bright empty hairline capsule snapping on around the avatar in
/// the first milliseconds of focus — visible, and not something the tint can walk back.
///
/// And at rest it is the track's face to the bit, which is the claim the whole arrangement
/// rests on: one solve, one material, two surfaces.
#[test]
fn a_faded_face_fades_its_rim_with_its_scrim_and_rests_on_the_tracks_own_face() {
    let face = plx_gfx::gfx::GlassFace {
        scrim_top: [0.0, 0.0, 0.0, 0.40],
        scrim_bot: [0.0, 0.0, 0.0, 0.52],
        rim: [1.0, 1.0, 1.0, 0.14],
        rim_lit: [1.0, 1.0, 1.0, 0.28],
        rim_w: 1.0,
    };
    let rest = chip_face(face, 1.0);
    for (a, b) in [
        (rest.scrim_top, face.scrim_top),
        (rest.scrim_bot, face.scrim_bot),
        (rest.rim, face.rim),
        (rest.rim_lit, face.rim_lit),
    ] {
        assert_eq!(a, b, "at rest the chip wears the track's face unchanged");
    }
    let half = chip_face(face, 0.5);
    assert_eq!(half.scrim_top[3], 0.20, "the scrim fades with the unfurl");
    assert_eq!(half.rim[3], 0.07, "…and so does the perimeter");
    assert_eq!(
        half.rim_lit[3], 0.14,
        "…and the lit edge, which the tint alone cannot reach"
    );
    assert_eq!(
        half.rim[..3],
        face.rim[..3],
        "the lamp's COLOUR does not move; its weight does"
    );
    assert_eq!(
        half.rim_w, face.rim_w,
        "one design-system pixel, at every point of the unfurl"
    );
}

/// What [`drawn_strip`] read off one draw of the shared strip.
struct DrawnStrip {
    /// the band published the glass material (the flat capsule publishes none)
    glass: bool,
    /// the pills plus the air between them
    content: f32,
    /// the track the draw stood on ([`tab_track_w`])
    track: f32,
}

/// **DRAW the shared strip for `faces`** (a recording painter, the ground read answered by the
/// last reading) and report the material the draw published on the band, with the widths it stood
/// on. [`tab_glass_on`] takes no width, so asking it says nothing about any strip: the band's
/// published material is what the chip, the account menu's lift and the planner read, and it is
/// the answer that changes if the draw ever decides the material by width again.
fn drawn_strip(generation: u32, faces: &[TabFace]) -> DrawnStrip {
    let _g = serial_for_motion();
    // the draw calls `gfx::sample_ground`, which reads a band image and so needs a walk; with none
    // it answers the last reading and copies nothing
    let labels = TabLabels { generation, faces };
    let mut strip = StripRender::new();
    let mut band = TabBand::new();
    strip.update(labels, 0, TopFocus::Away, DT);
    strip.draw(labels, Painter::recording(), &mut band);
    let (content, track) = with_tab_metrics_for(labels, |_, _, w| (tab_content_w(w), tab_track_w(w)));
    DrawnStrip { glass: band.face().is_some(), content, track }
}

/// **The Belarusian strip keeps the material** (the regression this file exists for since
/// 2026-10-30). "Галоўная", "Фільмы", "Серыялы" and the Watchlist and Search marks measure
/// 132 / 108 / 131 px of bold BODY label on the television, i.e. pills of 168 / 144 / 167 plus two
/// 60 px marks: a 679 px track, 7 px past the 672 the retired `GLASS_TRACK_MAX` allowed, so the
/// whole bar fell to the flat capsule on the owner's television the moment the Watchlist pill was
/// added. (The app's own layout rect for the track reads `620,54,679,76` there; the 676 px a
/// capture shows is the scrim INSIDE the capsule's 1 px rims, not the track.)
///
/// The test DRAWS a strip past that cap ([`drawn_strip`]). The host has no font and measures half
/// an em a byte, so those television widths cannot be reproduced word for word, and the television's
/// width cannot be reproduced at all: a host pill is 14 px a byte plus 36, a mark is 60 and the air
/// is 16, every term even, so no strip the host draws is an odd number of pixels wide and 679 is
/// odd. The strip drawn has the television's shape (three labels, then the Watchlist and Search
/// marks) with 27 bytes of label, which is 686 px: 14 px past the retired cap and 7 px past the
/// television's strip, and the test pins both tracks.
#[test]
fn the_belarusian_five_pill_strip_keeps_the_glass() {
    /// what the retired `GLASS_TRACK_MAX` was at the end
    const RETIRED_GLASS_TRACK_MAX: f32 = 672.0;
    let widths = [168.0, 144.0, 167.0, TAB_ICON_PILL_W, TAB_ICON_PILL_W];
    assert_eq!(tab_track_w(&widths), 679.0, "the strip the owner's television draws");
    let faces = [
        TabFace::Label("Libraries".into()),
        TabFace::Label("Galleries".into()),
        TabFace::Label("Playlists".into()),
        TabFace::Icon(crate::icons::Icon::Bookmark),
        TabFace::Icon(crate::icons::Icon::Search),
    ];
    let drawn = drawn_strip(3, &faces);
    assert_eq!(drawn.track, 686.0, "the host's strip of the television's shape, 27 bytes of label");
    assert!(
        drawn.track > RETIRED_GLASS_TRACK_MAX,
        "the strip drawn ({} px) must be past the cap that dropped the television's",
        drawn.track
    );
    assert!(drawn.glass, "a {} px track dropped to the flat capsule", drawn.track);
}

/// **No strip width takes the material away** — a section table can add libraries and a language
/// can lengthen every label, and the bar must still be glass. This DRAWS the bar ([`drawn_strip`])
/// for the shipped strip and for one far past [`TAB_VIEW_MAX`], which scrolls inside a track that
/// stops growing ([`tab_view_w`]), and asks the band what material the draw published: glass for
/// both. The only refusal left is the `flattabs` experiment.
#[test]
fn any_strip_keeps_the_material_including_one_that_scrolls() {
    let normal = vec![
        TabFace::Label("Home".into()),
        TabFace::Label("Movies".into()),
        TabFace::Label("TV Shows".into()),
        TabFace::Icon(crate::icons::Icon::Bookmark),
        TabFace::Icon(crate::icons::Icon::Search),
    ];
    // two dozen libraries: far more than the viewport holds, whatever the font's advances
    let many: Vec<TabFace> = (0..24)
        .map(|i| TabFace::Label(format!("Library number {i}")))
        .chain([TabFace::Icon(crate::icons::Icon::Bookmark), TabFace::Icon(crate::icons::Icon::Search)])
        .collect();
    let mut widest = 0.0f32;
    for (generation, faces) in [(1, normal), (2, many)] {
        let drawn = drawn_strip(generation, &faces);
        widest = widest.max(drawn.content);
        assert!(
            drawn.glass,
            "a {} px track (content {}) lost the material",
            drawn.track, drawn.content
        );
    }
    assert!(
        widest > TAB_VIEW_MAX,
        "the second strip must outgrow the viewport ({widest} against {TAB_VIEW_MAX}) to mean what it says"
    );
}

/// The shipped default, asserted in ONE place so moving it is a one-line decision with a
/// measurement behind it rather than a cascade through the cadence tests.
#[test]
fn the_shipped_cadence_is_every_changed_present() {
    use crate::frame::backdrop::{decide,Request,Damage,Z,canvas};
    assert!(decide(Request {z:Z::CHROME,rect:canvas(),valid:true}, &[],
        &[Damage {z:Z::PAGE,rect:canvas()}]).refresh);
    assert!(!decide(Request {z:Z::CHROME,rect:canvas(),valid:true}, &[], &[]).refresh);
}

/// **Every glass policy COMPOSITES**, and the axis that let one not to is gone.
///
/// This test used to assert the source-dim arithmetic — `Glass::DYNAMIC.source_rgb(0.5) == 0.75`
/// and so on — for a preset nothing outside this module ever constructed. The mechanism was
/// measured against the item menu over one checker ground and rejected (`e75b5e49`): dimming the
/// page going INTO the backdrop destroys the modulation the frost is layered over, so the panel
/// arrives flat. What is worth pinning now is that no policy can reintroduce it by accident —
/// a material carries no private scheduling policy.
#[test]
fn live_glass_carries_no_private_source_policy() {
    // Scheduling is owned by the layer walk, never an independently phased widget clock.
    assert_eq!(
        std::mem::size_of::<Glass>(),
        0,
        "a second axis on Glass would show up here first"
    );
}

/// **Glass over the blur chain is chrome only; no popover or screen names the blur path.** Every
/// panel's ground goes through `panel_ground`, which draws `popover_glass` (DS6) for a
/// `Ground::Menu` ONLY — glass on the frozen ground, which reads no framebuffer — and stands a
/// `Ground::Sheet` on the latched underlay field (directly, or through `Popover::panel`), as a menu
/// does too with no latched field. The blur chain's users are the top bar's standing track, the profile chip's capsule, the dev tile
/// band and the full-image preview's track/Back chrome — through `widgets.rs` — the standing hold hint's capsule (`hold_hint.rs`: a non-interactive
/// standing note, the track's own material, budgeted by
/// `hold_hint::tests::the_capsules_own_blur_region_fits_the_budget_and_never_meets_the_top_band`),
/// the library's scroll capsule (`scroll_capsule.rs`: the same standing container over the page
/// the chrome tucked away from, budgeted by
/// `scroll_capsule::tests::a_long_names_capsule_stays_inside_the_glass_region_budget`),
/// plus the frame mechanism and the dev load dial.
///
/// A source grep, because the failure it guards is a call that COMPILES: `Glass` and
/// `Painter::backdrop_blur` stay reachable for the chrome, so a panel that reached for them again
/// would draw, look plausible on a still page, and put the ~11% of a frame's GPU the migration
/// removed straight back.
#[test]
fn no_popover_panel_uses_the_blur_path_and_every_one_stands_on_the_field() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    // The blur path's legitimate users: chrome, the frame plan, the dev dial, the API itself, and
    // a fixture that names it in a string.
    const CHROME: &[&str] = &[
        "ui/src/widgets.rs",
        // the standing hold hint: one `Glass::DYNAMIC_BACKDROP.backdrop` call, region-budgeted
        "ui/src/hold_hint.rs",
        // the library's scroll capsule: the nav pill's live glass over the page it tucked away
        // from, one `Glass::DYNAMIC_BACKDROP.backdrop` call, region-budgeted
        "ui/src/scroll_capsule.rs",
        "ui/src/frame/glass.rs",
        "ui/src/frame/backdrop_tests.rs",
        "ui/src/glassload.rs",
        "ui/src/lib.rs",
        "ui/src/fixture.rs",
        "ui/src/widgets_glass_budget_tests.rs",
    ];
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).expect("read src dir") {
            let p = e.expect("dir entry").path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(&root.join("ui/src"), &mut files);
    walk(&root.join("appkit/src"), &mut files);
    walk(&root.join("screens/src"), &mut files);
    let mut offenders = Vec::new();
    for f in &files {
        let rel = f.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
        if CHROME.contains(&rel.as_str()) {
            continue;
        }
        let src = std::fs::read_to_string(f).expect("read source");
        for (n, line) in src.lines().enumerate() {
            let code = line.trim_start();
            if code.starts_with("//") {
                continue;
            }
            if ["Glass::", "GlassState", "backdrop_blur(", ".backdrop("]
                .iter()
                .any(|pat| code.contains(pat))
            {
                offenders.push(format!("{rel}:{}: {code}", n + 1));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "the blur path is chrome-only — a panel's ground is `widgets::panel_ground`:\n{}",
        offenders.join("\n")
    );

    // …and the glass scope is exactly the MENUS (owner decision, wave 2 review): Sort & Filter, the
    // account menu and the item hold menu name `Ground::Menu`; every other panel (alerts, About,
    // Tracks, the person bio, the Also-available sheet, the decision alert) names `Ground::Sheet`.
    // The player's "…" menu stands over the video plane and draws no `panel_ground` at all.
    let ground_of = |rel: &str| {
        let src = std::fs::read_to_string(root.join(rel)).expect("read panel source");
        assert!(
            src.contains("panel_ground(") || src.contains("pop.panel(") || src.contains("popover.panel("),
            "{rel} must draw its ground through `panel_ground` (or `Popover::panel`)"
        );
        let code: String = src.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
        (code.contains("Ground::Menu"), code.contains("Ground::Sheet"))
    };
    for rel in ["screens/src/item_menu.rs", "screens/src/account_menu.rs", "screens/src/library/menu.rs"] {
        assert_eq!(ground_of(rel), (true, false), "{rel} is a MENU: `Ground::Menu` and nothing else");
    }
    for rel in [
        "screens/src/alt_sources.rs",
        "screens/src/about_panel.rs",
        "screens/src/tracks_panel.rs",
        "screens/src/person_bio.rs",
        "ui/src/decision_alert.rs",
    ] {
        assert_eq!(ground_of(rel), (false, true), "{rel} is a SHEET: `Ground::Sheet` and nothing else");
    }
}
