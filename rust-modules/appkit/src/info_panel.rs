//! The Info section's data: what the playing item says about itself (`info_content`, the meta line's
//! chips, the live playback fact) and the action labels. The card is drawn by the HUD's Info section
//! (`player_hud::sections`); the screen's keys and focus are the player screen's, over the compiled
//! state's buttons. The retired hand-written card's geometry (`card_geometry`, `button_rect`) stays under
//! `cfg(test)`, the reference the compiled card is pinned to.
use crate::chip_row::{Chip, ChipRow};
use plx_data::metadata;
#[cfg(test)]
use plx_ui::consts::{SCR_H, SCR_W};
use plx_ui::theme;
#[cfg(test)]
use plx_ui::Rect;

/// The action column's length: `actions` always returns two labels, only their TEXT depends on
/// `is_episode`. [`button_rect`] sizes the column from this constant instead of building the array.
#[cfg(test)]
const N_ACTIONS: usize = 2;

/// The ratingKey of the page the playing item belongs to: the show for an episode, the movie
/// itself otherwise. What the Info card's second action opens, and what the `go_to_detail`
/// command opens and is enabled by. `None` when nothing playing has one.
pub fn detail_rk<'a>(meta: metadata::MetadataView<'a>) -> Option<&'a str> {
    meta.now_playing()
        .map(|n| n.detail_rk.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| meta.current().map(|d| d.rk.as_str()))
        .filter(|s| !s.is_empty())
}

/// whether the playing item is an episode (→ "Go to Show") rather than a movie ("Go to Movie").
/// A trailer session does not install a playing-leaf descriptor, so with `now_playing` absent
/// the loaded page is the parent — a show's trailer still says Go to Show.
fn is_episode(meta: metadata::MetadataView<'_>) -> bool {
    meta.now_playing()
        .map(|n| n.is_episode)
        .unwrap_or_else(|| meta.current().is_some_and(|d| d.is_show))
}

/// the action-button labels for the playing item. An ARRAY, not a `Vec`: the count is fixed at
/// [`N_ACTIONS`], so a caller that only needs the count uses that constant instead and this is called
/// only where the actual label TEXT is wanted.
pub(crate) fn actions(meta: metadata::MetadataView<'_>) -> [&'static str; 2] {
    [
        plx_platform::i18n::msg::widgets_info_from_beginning(),
        if is_episode(meta) {
            plx_platform::i18n::msg::widgets_info_go_show()
        } else {
            plx_platform::i18n::msg::widgets_info_go_movie()
        },
    ]
}

/// The card's own rect and its side padding — **pure**, since every input is a fixed layout
/// constant (`SCR_W`/`SCR_H` alone). Shared by the compiled card (the still/title/synopsis layout) and
/// [`button_rect`] (the action column), so the two formulas can never drift apart.
#[cfg(test)]
pub(crate) fn card_geometry() -> (Rect, f32) {
    let cx = 80.0f32;
    let cw = SCR_W - 160.0;
    let ch = 236.0f32;
    let cyt = SCR_H - 176.0 - ch; // sit just above the Info/Chapters tabs (tabs at SCR_H-128)
    (Rect::new(cx, cyt, cw, ch), 28.0f32)
}

/// The `i`-th action button's rect, widened for the longest translated action label. The compiled
/// state's buttons stand at it (`compile_tests`), so a stop built from it lands on the pixel the button
/// is drawn at.
#[cfg(test)]
pub(crate) fn button_rect(i: usize, measure: &dyn plx_machine::machine::Measure) -> Rect {
    let (card, pad) = card_geometry();
    let labels = [
        plx_platform::i18n::msg::widgets_info_from_beginning_c(),
        plx_platform::i18n::msg::widgets_info_go_show_c(),
        plx_platform::i18n::msg::widgets_info_go_movie_c(),
    ];
    let bw = labels.iter().map(|label| {
        plx_ui::widgets::Button::pill_w_measured(label, theme::size::BODY, true, None, measure)
    }).fold(352.0f32, f32::max);
    let bh = 70.0f32;
    let bx = card.x + card.w - pad - bw;
    let n = N_ACTIONS;
    let total_bh = n as f32 * bh + n.saturating_sub(1) as f32 * 16.0;
    let by0 = card.y + (card.h - total_bh) * 0.5;
    Rect::new(bx, by0 + i as f32 * (bh + 16.0), bw, bh)
}

/// **What the Info section says about the playing item**, resolved once: `now_playing` describes the
/// episode (show title + SxEy + its still) or the movie, the loaded `Detail` backs the genres, and the
/// capability badges come from the PLAYING item's own tracks (`current()` is the show, with episode 1's
/// streams, during a show-page episode play, or another item entirely). The ONE reading of those
/// fields: the HUD renderer's Info section arms draw from it.
pub(crate) struct InfoContent<'a> {
    /// the playing leaf's own name: the episode's, else the movie's (the show-title + SxEy treatment
    /// lives on the transport HUD under the playbar, not here)
    pub title: String,
    pub summary: String,
    pub year: i64,
    pub dur_ms: i64,
    pub rating: String,
    /// the 16:9 still (episode) or landscape art (movie)
    pub thumb: String,
    pub genres: &'a [String],
    pub audio: &'a [metadata::Stream],
    pub subs: &'a [metadata::Stream],
    /// the item the session is playing, for the stream pills' picture facts (HDR, Dolby Vision)
    pub playing: Option<&'a metadata::PlayingItem>,
}

/// The playing item's [`InfoContent`], or `None` when nothing playing has a name to show.
pub(crate) fn info_content<'a>(meta: metadata::MetadataView<'a>) -> Option<InfoContent<'a>> {
    let np = meta.now_playing();
    let d = meta.current();
    if np.is_none() && d.is_none() {
        return None;
    }
    let is_ep = np.map(|n| n.is_episode).unwrap_or(false);
    let big_title = np.map(|n| n.title.clone()).or_else(|| d.map(|x| x.title.clone())).unwrap_or_default();
    let ep_name = np.map(|n| n.ep_title.clone()).unwrap_or_default();
    let summary = np
        .map(|n| n.summary.clone())
        .filter(|s| !s.is_empty())
        .or_else(|| d.map(|x| x.summary.clone()))
        .unwrap_or_default();
    let thumb = np
        .map(|n| n.thumb.clone())
        .filter(|s| !s.is_empty())
        .or_else(|| d.map(|x| if !x.art.is_empty() { x.art.clone() } else { x.thumb.clone() }))
        .unwrap_or_default();
    let (audio, subs): (&[metadata::Stream], &[metadata::Stream]) = match meta.playing() {
        Some(t) => (&t.audio, &t.subs),
        None => (d.map(|x| x.audio.as_slice()).unwrap_or(&[]), d.map(|x| x.subs.as_slice()).unwrap_or(&[])),
    };
    Some(InfoContent {
        title: if is_ep { ep_name } else { big_title },
        summary,
        year: np.map(|n| n.year).or_else(|| d.map(|x| x.year)).unwrap_or(0),
        dur_ms: np.map(|n| n.dur_ms).or_else(|| d.map(|x| x.dur_ms)).unwrap_or(0),
        rating: np.map(|n| n.rating.clone()).filter(|s| !s.is_empty()).or_else(|| d.map(|x| x.rating.clone())).unwrap_or_default(),
        thumb,
        genres: d.map(|x| x.genres.as_slice()).unwrap_or(&[]),
        audio,
        subs,
        playing: meta.playing(),
    })
}

/// A package's `facts` cell as the chip builder reads it (`DrawOp::Facts`): which of the item's own
/// facts, in what order, how many genres, whether the live playback fact follows, which badges, in
/// what priority order, and the inks.
pub(crate) struct FactsShape<'a> {
    /// `genres`, `year`, `duration`, in the order they run
    pub order: &'a [&'a str],
    /// `content_rating`, `audio_format`, `cc`, `sdh`, `ad`, in drop-priority order
    pub badges: &'a [&'a str],
    /// `resolution`, `dynamic_range`, `stream_mode`, `audio_codec`: the stream pills ([`StreamOutput::pill`]), after the badges
    pub stream: &'a [&'a str],
    pub genres_max: usize,
    pub playback_fact: bool,
    pub ink: [f32; 4],
    pub bold: i32,
    /// The declared plain-facts rung; badges retain their shared badge treatment.
    pub size: i32,
}

/// **The metadata line and the capability badges, built and measured** but not yet fit or drawn: the
/// item's own facts as ONE text run (parts a dot apart exactly as the Detail page's identity line
/// spaces them, `widgets::DottedRun`), the live playback fact in its own quieter run (what the server
/// is actually sending, [`playback_now`]: REGULAR weight and tertiary ink, because it is a fact about
/// this session and not a property of the item; no chip and no capsule, a conversion that worked is
/// not a warning), then the badges. Every candidate's width is known before any of them touches the
/// screen, so the caller's `row.fit(tw)` decides what fits ([`ChipRow`]: tail-first drop, issue #26:
/// the chips used to draw straight under the action column).
///
/// The row's actual chip COUNT, not "there might be something to say", is what the caller's layout
/// must reserve a line for: a single candidate wider than the room fits down to nothing.
pub(crate) fn info_chips(
    c: &InfoContent<'_>,
    shape: &FactsShape<'_>,
    ps: &plx_media::route::PlaybackSession,
    measure: &dyn plx_machine::machine::Measure,
) -> ChipRow {
    // What the server is actually sending, straight off the resolved route. `has_url`, not
    // `!url().is_empty()`: the card draws at ~60/s, and `url()` clones the longest string in the app.
    let now_fact = shape.playback_fact.then(|| {
        playback_now(
            plx_media::route::has_url(ps) && !plx_media::route::play_pending(),
            plx_media::route::is_transcoding(ps),
            plx_media::route::is_remux(ps),
            &plx_media::route::stream_vcodec(ps),
        )
    });
    let now_fact = now_fact.flatten();
    let output = if shape.stream.is_empty() { None } else { stream_output(ps, c.playing) };
    let mut chips = ChipRow::new();
    let mut meta = Vec::new();
    for part in shape.order {
        match *part {
            "genres" => meta.extend(c.genres.iter().take(shape.genres_max).cloned()),
            "year" if c.year > 0 => meta.push(c.year.to_string()),
            "content_rating" if !c.rating.is_empty() => meta.push(c.rating.clone()),
            "duration" if c.dur_ms > 0 => meta.push(plx_ui::fmt::duration(c.dur_ms)),
            _ => {}
        }
    }
    let has_meta = !meta.is_empty();
    if has_meta {
        chips.push(Chip::run_at(&meta, false, shape.size, shape.bold, shape.ink, measure));
    }
    // It continues the meta run, so it leads with a dot when there is one. Whether it is needed depends
    // only on whether the meta line EXISTS, not on whether it ends up FITTING: a chip only draws once
    // every higher-priority chip already fit.
    if let Some(fact) = now_fact.as_ref() {
        chips.push(Chip::run_at(std::slice::from_ref(fact), has_meta, shape.size, 0, theme::TEXT_TERTIARY, measure));
    }
    for badge in shape.badges {
        match *badge {
            "content_rating" if !c.rating.is_empty() => chips.push(Chip::badge(c.rating.clone(), measure)),
            "audio_format" => {
                if let Some(tag) = c.audio.first().and_then(|s| audio_badge(&s.codec)) {
                    chips.push(Chip::badge(tag, measure));
                }
            }
            "cc" if !c.subs.is_empty() => chips.push(Chip::badge(plx_platform::i18n::msg::widgets_badge_cc().to_string(), measure)),
            "sdh" if c.subs.iter().any(|s| s.sdh) => chips.push(Chip::badge(plx_platform::i18n::msg::widgets_badge_sdh().to_string(), measure)),
            "ad" if c.audio.iter().any(|s| s.ad) => chips.push(Chip::badge(plx_platform::i18n::msg::widgets_badge_ad().to_string(), measure)),
            _ => {}
        }
    }
    for name in shape.stream {
        if let Some(pill) = output.as_ref().and_then(|o| o.pill(name)) {
            chips.push(Chip::stream(pill, measure));
        }
    }
    chips
}

/// **What the playing stream is**, read off the resolved route and the playing item: the inputs of
/// [`StreamOutput::derive`], the one place the stream pills and the playback details panel come from. `None`
/// while nothing resolved is streaming (a pending play still describes the previous item, see
/// [`playback_now`]).
pub(crate) fn stream_input(ps: &plx_media::route::PlaybackSession, item: Option<&metadata::PlayingItem>) -> Option<StreamInput> {
    use plx_media::route;
    if !route::has_url(ps) || route::play_pending() {
        return None;
    }
    let (transcoding, remux) = (route::is_transcoding(ps), route::is_remux(ps));
    let src_range = item.and_then(|i| SourceRange::of(i.hdr_kind, i.dovi.present, i.hdr));
    let sid = route::cur_audio_sid(ps);
    let track = item.and_then(|i| i.audio.iter().find(|s| s.id == sid && sid != 0).or_else(|| i.audio.iter().find(|s| s.default)).or_else(|| i.audio.first()));
    let src_audio = track.map(|t| AudioFacts { codec: t.codec.clone(), channels: t.channels, profile: t.profile.clone(), title: t.title.clone(), atmos: t.has_atmos() });
    let acodec = route::stream_acodec(ps);
    let declared = plx_platform::devcaps::audio_codec_id(&acodec);
    let out_audio = match route::stream_pcm(ps) {
        Some(Ok(pcm)) => AudioOut::App { channels: i64::from(pcm.channels) },
        _ if acodec.is_empty() => AudioOut::Kept,
        // the stream declares another codec than the track it carries: the server converted the sound
        _ if transcoding && track.is_some_and(|t| plx_platform::devcaps::audio_codec_id(&t.codec) != declared) => {
            AudioOut::Server { codec: acodec, channels: route::stream_achannels(ps) }
        }
        _ => AudioOut::Kept,
    };
    Some(StreamInput {
        src_raster: item.map_or((0, 0), |i| (i.width, i.height)),
        src_range,
        out_raster: route::stream_raster(ps),
        video_kept: !transcoding || remux,
        src_audio,
        out_audio,
    })
}

/// The dynamic range the source file carries (`PlayingItem::hdr_kind`), named as the web player names it
/// (`packages/core/src/playback.ts`, `hdrKind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SourceRange {
    DolbyVision,
    Hdr10,
    Hlg,
    /// HDR whose format the record does not say: an item built from a recording made before
    /// `hdr_kind` existed
    Hdr,
}

impl SourceRange {
    /// The source's range from the item's [`metadata::HdrKind`], its Dolby Vision record and its HDR flag;
    /// `None` for SDR.
    fn of(kind: metadata::HdrKind, dovi: bool, hdr: bool) -> Option<SourceRange> {
        use metadata::HdrKind as K;
        match kind {
            K::DolbyVision => Some(SourceRange::DolbyVision),
            K::Hdr10 => Some(SourceRange::Hdr10),
            K::Hlg => Some(SourceRange::Hlg),
            K::Sdr if dovi => Some(SourceRange::DolbyVision),
            K::Sdr if hdr => Some(SourceRange::Hdr),
            K::Sdr => None,
        }
    }

    /// The pill's label, the web player's: a format name, never translated.
    fn label(self) -> &'static str {
        match self {
            SourceRange::DolbyVision => "Dolby Vision",
            SourceRange::Hdr10 => "HDR10",
            SourceRange::Hlg => "HLG",
            SourceRange::Hdr => "HDR",
        }
    }
}

/// The audio track the file carries, as PMS describes it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AudioFacts {
    pub codec: String,
    pub channels: i64,
    pub profile: String,
    pub title: String,
    /// the track carries Atmos ([`metadata::Stream::has_atmos`]: PMS says so in the profile)
    pub atmos: bool,
}

/// What reaches the TV's sound path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AudioOut {
    /// the track as the file carries it: the TV decodes it or passes it to a receiver, which the app cannot see
    Kept,
    /// the server converted it to `codec`, `channels` wide (0: unstated)
    Server { codec: String, channels: i64 },
    /// the app decodes it and feeds PCM, `channels` wide
    App { channels: i64 },
}

/// The inputs of [`StreamOutput::derive`]: plain values, so every arm is host-testable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StreamInput {
    /// the source's stored frame size (0: unknown)
    pub src_raster: (i64, i64),
    pub src_range: Option<SourceRange>,
    /// the picture the stream carries (`route::stream_raster`)
    pub out_raster: (i64, i64),
    /// the stream keeps the source's video (a direct play, a remux that copies it); false while the server
    /// re-encodes the picture
    pub video_kept: bool,
    pub src_audio: Option<AudioFacts>,
    pub out_audio: AudioOut,
}

/// One stream pill: its label, its look, and whether it is `unsure` (deduced, not verifiable on this TV: the
/// web player's dashed pill with a small "?").
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Pill {
    pub label: String,
    pub kind: crate::chip_row::StreamPill,
    pub unsure: bool,
}

/// **What the stream is, said the web player's way** (`apps/web/src/playerui.tsx`, `PlayerOutput`): the pills
/// of the row beside the ⓘ disc and the "source → output" lines and notes of its panel. Built by
/// [`StreamOutput::derive`] from [`StreamInput`] alone.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct StreamOutput {
    /// the picture the stream carries (`720p`), the one solid pill
    pub resolution: Option<Pill>,
    /// the dynamic range the TV is shown: the source's HDR kind when the video is kept, else `SDR`
    pub range: Option<Pill>,
    /// "Transcoding" while the server re-encodes the picture, else nothing: a direct play or a remux is not a
    /// fact worth a pill (the web player's changelog, "only transcoding is shown")
    pub mode: Option<Pill>,
    /// the sound, codec and channels in one pill ("Dolby Digital 5.1")
    pub audio: Option<Pill>,
    /// the panel's lines, each the value after "Source:" / "Output:"; `—` when nothing is known
    pub video_source: String,
    pub video_output: String,
    pub audio_source: String,
    pub audio_output: String,
    /// the panel's notes: every conversion explained once, then what the "?" means when a pill is unsure;
    /// each capitalised with a final period
    pub notes: Vec<String>,
}

/// The em dash the panel shows for an unknown value (the web player's `'—'`).
const UNKNOWN: &str = "\u{2014}";

impl StreamOutput {
    /// Derive the pills, the panel's lines and its notes from `input`.
    pub(crate) fn derive(input: &StreamInput) -> StreamOutput {
        use crate::chip_row::StreamPill as K;
        use plx_platform::i18n::msg;
        let pill = |label: String, kind: K, unsure: bool| Pill { label, kind, unsure };
        let join = |parts: &[Option<&str>]| parts.iter().flatten().copied().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ");
        let mut hints: Vec<String> = Vec::new();

        // the picture
        let src_res = plx_ui::fmt::resolution("", input.src_raster.0, input.src_raster.1);
        let out_res = plx_ui::fmt::resolution("", input.out_raster.0, input.out_raster.1);
        let range = match (input.src_range, input.video_kept) {
            (Some(r), true) => Some(pill(r.label().to_owned(), K::Hdr, false)),
            (Some(r), false) => {
                hints.push(msg::player_stream_to_sdr(r.label()));
                Some(pill("SDR".to_owned(), K::Sdr, false))
            }
            (None, _) if out_res.is_some() => Some(pill("SDR".to_owned(), K::Sdr, false)),
            (None, _) => None,
        };
        let mode = (!input.video_kept).then(|| {
            hints.push(msg::player_stream_transcode_note().to_owned());
            pill(msg::player_stream_transcoding().to_owned(), K::Mode, false)
        });
        let src_range_label = input.src_range.map(SourceRange::label).or(src_res.as_ref().map(|_| "SDR"));
        let video_source = join(&[src_res.as_deref(), src_range_label]);
        let video_output = join(&[out_res.as_deref(), range.as_ref().map(|r| r.label.as_str())]);

        // the sound
        let src_audio = input.src_audio.as_ref().map(audio_label).filter(|s| !s.is_empty());
        let audio = match &input.out_audio {
            AudioOut::Server { codec, channels } => {
                hints.push(msg::player_stream_server_audio().to_owned());
                let label = join(&[Some(&metadata::friendly_codec(codec)), channels_label(*channels).as_deref()]);
                (!label.is_empty()).then(|| pill(label, K::Audio, false))
            }
            AudioOut::App { channels } => {
                hints.push(msg::player_stream_app_audio().to_owned());
                Some(pill(join(&[Some("PCM"), channels_label(*channels).as_deref()]), K::Audio, false))
            }
            AudioOut::Kept => src_audio.clone().map(|label| {
                hints.push(msg::player_stream_tv_audio().to_owned());
                pill(label, K::Audio, true)
            }),
        };
        let audio_output = audio.as_ref().map(|a| a.label.clone());

        let unsure = [&range, &mode, &audio].into_iter().flatten().any(|p| p.unsure);
        if unsure {
            hints.push(msg::player_stream_unsure_note().to_owned());
        }
        let mut notes: Vec<String> = Vec::new();
        for hint in hints {
            let note = sentence(&hint);
            if !notes.contains(&note) {
                notes.push(note);
            }
        }
        let or_unknown = |s: String| if s.is_empty() { UNKNOWN.to_owned() } else { s };
        StreamOutput {
            resolution: out_res.map(|r| pill(r, K::Resolution, false)),
            range,
            mode,
            audio,
            video_source: or_unknown(video_source),
            video_output: or_unknown(video_output),
            audio_source: or_unknown(src_audio.unwrap_or_default()),
            audio_output: or_unknown(audio_output.unwrap_or_default()),
            notes,
        }
    }

    /// The pill a `facts.stream` name shows, `None` for a fact the stream does not state.
    pub(crate) fn pill(&self, name: &str) -> Option<&Pill> {
        match name {
            "resolution" => self.resolution.as_ref(),
            "dynamic_range" => self.range.as_ref(),
            "stream_mode" => self.mode.as_ref(),
            "audio_codec" => self.audio.as_ref(),
            _ => None,
        }
    }

    /// The panel's notes, one to a line (`TextView` breaks at a newline).
    pub(crate) fn notes_text(&self) -> String {
        self.notes.join("\n")
    }
}

/// A note as the panel prints it: capitalised, with a final period (the web player's
/// `n.charAt(0).toUpperCase() + n.slice(1) + '.'`).
fn sentence(hint: &str) -> String {
    let mut chars = hint.chars();
    let Some(first) = chars.next() else { return String::new() };
    format!("{}{}.", first.to_uppercase(), chars.as_str())
}

/// A track's codec and channels as one label ("Dolby Digital 5.1"): the app's codec names
/// ([`metadata::friendly_codec`]), with the web player's refinements of a DTS family and of Atmos, which only
/// the track's profile and title state.
fn audio_label(a: &AudioFacts) -> String {
    let codec = a.codec.to_ascii_lowercase();
    let title = a.title.to_ascii_lowercase();
    let profile = a.profile.to_ascii_lowercase();
    let mut name = metadata::friendly_codec(codec.trim_end_matches("-ma"));
    if codec == "dca" || codec == "dts" {
        if title.contains("dts:x") || title.contains("dts-x") {
            name = "DTS:X".to_owned();
        } else if profile.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w == "ma") || title.contains("dts-hd ma") {
            name = "DTS-HD MA".to_owned();
        } else if profile.contains("hra") || title.contains("dts-hd") {
            name = "DTS-HD".to_owned();
        }
    }
    // Atmos travels in a TrueHD or Dolby Digital Plus track: PMS names it in the profile, a title may too
    if a.atmos || title.contains("atmos") {
        name = "Dolby Atmos".to_owned();
    }
    match channels_label(a.channels) {
        Some(ch) if !name.is_empty() => format!("{name} {ch}"),
        _ => name,
    }
}

/// A channel count as the web player names it: 7.1, 5.1, Stereo, Mono, else "N ch"; `None` when unstated.
fn channels_label(n: i64) -> Option<String> {
    use plx_platform::i18n::msg;
    match n {
        n if n >= 8 => Some("7.1".to_owned()),
        n if n >= 6 => Some("5.1".to_owned()),
        2 => Some(msg::widgets_tracks_stereo().to_owned()),
        1 => Some(msg::widgets_tracks_mono().to_owned()),
        n if n > 0 => Some(msg::widgets_tracks_channels(n)),
        _ => None,
    }
}

/// The stream output of `ps` playing `item`, or `None` while nothing resolved is streaming.
pub(crate) fn stream_output(ps: &plx_media::route::PlaybackSession, item: Option<&metadata::PlayingItem>) -> Option<StreamOutput> {
    stream_input(ps, item).map(|i| StreamOutput::derive(&i))
}

// ---- helpers ----

/// A premium audio format worth badging on the meta line, named by the ONE codec map
/// ([`metadata::friendly_codec`]); everyday codecs (AAC/MP3/…) get no badge.
fn audio_badge(codec: &str) -> Option<String> {
    matches!(
        codec.to_ascii_lowercase().as_str(),
        "truehd" | "eac3" | "ec-3" | "ac3" | "dts" | "dca"
    )
    .then(|| metadata::friendly_codec(codec))
}

/// A VIDEO codec's display name. The sibling of [`metadata::friendly_codec`], which is the one
/// AUDIO/subtitle map and spells h264 as "H264" — the dotted form is the one everybody else's
/// player uses for the video lane, and this is the only place the app names a video codec to a
/// user. An unknown codec is upper-cased rather than dropped: "Converting · AV1" is still the
/// truth on a set that cannot decode AV1 (the dev set, and every set whose codec table lists no AV1
/// decoder), and inventing a friendly name for a codec we have never seen would not be.
pub fn video_codec_name(codec: &str) -> String {
    match codec.to_ascii_lowercase().as_str() {
        "h264" | "avc" | "avc1" => "H.264".to_string(),
        "hevc" | "h265" | "hvc1" | "hev1" => "HEVC".to_string(),
        other => other.to_uppercase(),
    }
}

/// The live playback fact for the meta line: **what the server is actually sending**, read off the
/// running stream and never predicted.
///
/// That distinction is the whole design of this line. Everything else on the row is a property of
/// the ITEM (genre, year, runtime), knowable before Play; this is a property of THIS SESSION, and
/// the only honest source for it is `route`'s own record of what was resolved — `stream_vcodec` is
/// the codec the `/decision` OUTPUT named, not the one the profile asked for. Predicting it from
/// the source file is how a client ends up telling the user it is direct playing a file the server
/// quietly re-encoded.
///
/// Three answers, because the server has three behaviours and only one of them touches the pixels:
/// a direct play (we pull the file), a container-only REMUX (the video is always copied and the
/// audio is copied when the profile admits it, converted otherwise — Plex's own "Direct Stream";
/// the mock has no case for it because its model has only "direct play" and "converts", but
/// calling a remux a conversion would state that the server re-encoded the pixels when it did
/// not), and a real re-encode, which names the codec it is producing. Hardware vs software is
/// deliberately NOT stated: that is the server's runtime choice and it reaches the client only in
/// the live transcode session, so naming it would be a guess.
///
/// **No warning belongs on this line.** The tone-mapping case in particular lives on the DETAIL
/// page, before Play: this card is behind a keypress, so a warning here is one the user finds only
/// after watching a washed-out picture. That is `Player Screen.dc.html`'s own standing comment
/// ("Warning before Play is the DETAIL page's job"), and it contradicts a bullet in the **design
/// project's** `CLAUDE.md` — the Pass-rules file in the Claude Design project, NOT this repo's root
/// `CLAUDE.md`, which says nothing about tone mapping — that lists this card beside the detail
/// facts row. The mock is the later artifact and its reasoning matches the owner's standing rule
/// that a warning exists to be seen BEFORE Play, so the code follows it; the two documents still
/// need reconciling by the owner, and that is a docs edit, not a behaviour change.
///
/// PURE, so every arm is host-testable. `streaming` is "there is a resolved stream to describe".
/// The caller composes it from `route::has_url()` **and** `!route::play_pending()`, and the second
/// half is load-bearing rather than belt-and-braces: `request_play` does not clear `URL`,
/// `TSESSION` or `CUR_REMUX` (only a real stop does, in `engine::teardown`), so on an item→item
/// switch — Up Next, or Play from a detail page while a session is live — every one of those three
/// still describes the PREVIOUS item for the whole resolve. Without the pending test this line
/// would state the last session's fact as this one's.
pub fn playback_now(
    streaming: bool,
    transcoding: bool,
    remux: bool,
    vcodec: &str,
) -> Option<String> {
    if !streaming {
        return None;
    }
    if !transcoding {
        return Some(plx_platform::i18n::msg::widgets_playback_direct_play().to_string());
    }
    if remux {
        return Some(plx_platform::i18n::msg::widgets_playback_direct_stream().to_string());
    }
    let name = video_codec_name(vcodec);
    // a re-encode whose output codec we somehow do not know still converted — say that much
    Some(if name.is_empty() {
        plx_platform::i18n::msg::widgets_playback_converting().to_string()
    } else {
        plx_platform::i18n::msg::widgets_playback_converting_codec(&name)
    })
}

#[cfg(test)]
mod tests {
    use super::playback_now;

    /// The meta line's live fact, arm by arm. It is the one thing on that row the app could get
    /// wrong by GUESSING, so each arm pins a different way of guessing:
    ///   * a direct play says so and names no codec — there is no conversion to describe;
    ///   * a container remux is NOT a conversion (the video is copied, and the audio too when the
    ///     profile admits it), and calling it one would state that the server touched the pixels
    ///     when it did not — `route::is_remux`'s own doc
    ///     is that these are different facts;
    ///   * a re-encode names the codec the server is actually PRODUCING (`route::stream_vcodec` is
    ///     the `/decision` OUTPUT), spelled the way a viewer reads it, and HEVC output is reachable
    ///     only on a Pass server — which this function neither knows nor asks, because it reports
    ///     what happened rather than what was allowed;
    ///   * and with no stream resolved there is no fact at all: during the resolve window nothing
    ///     has been sent yet, and stating a fact about it would be exactly the prediction the whole
    ///     line refuses to make.
    #[test]
    fn the_meta_lines_playback_fact_describes_the_stream_that_is_running() {
        assert_eq!(
            playback_now(true, false, false, "hevc").as_deref(),
            Some("Direct Play")
        );
        // remux: `vcodec` is the SOURCE codec copied through, and it is still not a conversion
        assert_eq!(
            playback_now(true, true, true, "hevc").as_deref(),
            Some("Direct Stream")
        );
        assert_eq!(
            playback_now(true, true, false, "h264").as_deref(),
            Some("Converting \u{b7} H.264")
        );
        assert_eq!(
            playback_now(true, true, false, "hevc").as_deref(),
            Some("Converting \u{b7} HEVC")
        );
        // a codec map we have never met is upper-cased, not dropped and not renamed
        assert_eq!(
            playback_now(true, true, false, "av1").as_deref(),
            Some("Converting \u{b7} AV1")
        );
        // a re-encode whose output codec never landed still converted
        assert_eq!(
            playback_now(true, true, false, "").as_deref(),
            Some("Converting")
        );
        // nothing resolved yet → no fact, on every combination of the flags
        for (t, r) in [(false, false), (true, false), (true, true)] {
            assert!(
                playback_now(false, t, r, "h264").is_none(),
                "no stream, no fact"
            );
        }
    }
}

#[cfg(test)]
mod column_tests {
    use super::*;

    /// Two buttons stack vertically, sharing an x and a width, in the order the card places them —
    /// "From Beginning" above "Go to Show"/"Go to Movie".
    #[test]
    fn the_two_buttons_share_a_column_and_stack_in_draw_order() {
        let measure = plx_ui::fixture::FixtureMeasure;
        let (a, b) = (button_rect(0, &measure), button_rect(1, &measure));
        assert_eq!(a.x, b.x);
        assert_eq!(a.w, b.w);
        assert!(a.y < b.y, "From Beginning sits above the second action");
    }
}

#[cfg(test)]
mod detail_tests {
    use super::*;

    // TEST ONLY: a thread-confined store, so `set_current_for_test`/`apply` and the `view()`
    // this test's `detail_rk`/`is_episode` calls read from are the SAME owner, not two disconnected
    // `MetadataStore::default()`s (same pattern as `screens::detail::tests`'s `TEST_METADATA`).
    thread_local! {
        static TEST_METADATA: std::cell::UnsafeCell<plx_data::stores::metadata::MetadataStore> =
            std::cell::UnsafeCell::new(plx_data::stores::metadata::MetadataStore::default());
    }

    fn test_store() -> &'static mut plx_data::stores::metadata::MetadataStore {
        TEST_METADATA.with(|cell| unsafe { &mut *cell.get() })
    }

    #[test]
    fn go_to_after_a_trailer_opens_the_loaded_parent() {
        let _g = plx_base::testlock::serial();
        test_store().run(plx_data::stores::metadata::MetadataCmd::SetNowPlaying(None));
        plx_data::metadata::set_current_for_test(test_store().state_mut(), Some(plx_data::metadata::Detail {
            rk: "parent-movie".into(),
            kind: "movie".into(),
            ..Default::default()
        }));
        assert_eq!(detail_rk(test_store().view()), Some("parent-movie"));
        assert!(!is_episode(test_store().view()), "a movie parent labels Go to Movie");

        plx_data::metadata::set_current_for_test(test_store().state_mut(), Some(plx_data::metadata::Detail {
            rk: "parent-show".into(),
            kind: "show".into(),
            is_show: true,
            ..Default::default()
        }));
        assert_eq!(detail_rk(test_store().view()), Some("parent-show"));
        assert!(is_episode(test_store().view()), "a show parent labels Go to Show");

        plx_data::metadata::set_current_for_test(test_store().state_mut(), Some(plx_data::metadata::Detail {
            sid: plx_plex::plex::ServerId::UNSET,
            rk: "parent-show".into(),
            kind: "show".into(),
            is_show: true,
            title: "Show".into(),
            extras: vec![plx_data::metadata::Extra {
                rk: "9".into(),
                title: "Official Trailer".into(),
                dur_ms: 120_000,
                part: "/p".into(),
                ..Default::default()
            }],
            ..Default::default()
        }));
        let trailer = plx_data::metadata::trailer_now_playing(test_store().state(), plx_plex::plex::ServerId::UNSET, "9");
        test_store().run(plx_data::stores::metadata::MetadataCmd::SetNowPlaying(trailer));
        assert_eq!(detail_rk(test_store().view()), Some("parent-show"));
        assert!(is_episode(test_store().view()), "an installed show-trailer card still says Go to Show");
        assert_eq!(
            test_store().view().now_playing().map(|n| n.dur_ms),
            Some(120_000),
            "the extra's duration, not the show's"
        );

        test_store().run(plx_data::stores::metadata::MetadataCmd::SetNowPlaying(None));
        plx_data::metadata::set_current_for_test(test_store().state_mut(), None);
    }
}

/// **The stream pills and the playback details panel, said the web player's way** (`StreamOutput::derive`),
/// on plain inputs. Every expected string is built from the same catalogue the code reads (`msg::*`,
/// `metadata::friendly_codec`), so the tests hold in whatever language the test process resolves.
#[cfg(test)]
mod stream_output_tests {
    use super::{metadata, AudioFacts, AudioOut, SourceRange, StreamInput, StreamOutput, UNKNOWN};
    use crate::chip_row::StreamPill as K;
    use plx_platform::i18n::msg;

    fn input() -> StreamInput {
        StreamInput {
            src_raster: (1920, 1080),
            src_range: None,
            out_raster: (1920, 1080),
            video_kept: true,
            src_audio: Some(AudioFacts { codec: "ac3".into(), channels: 6, ..AudioFacts::default() }),
            out_audio: AudioOut::Kept,
        }
    }

    fn note(s: &str) -> String {
        super::sentence(s)
    }

    /// The row the MultiPlex HUD draws: resolution, range, audio. Never a mode pill, whatever the stream does.
    fn row(o: &StreamOutput) -> Vec<(String, K, bool)> {
        ["resolution", "dynamic_range", "audio_codec"].iter().filter_map(|n| o.pill(n)).map(|p| (p.label.clone(), p.kind, p.unsure)).collect()
    }

    #[test]
    fn a_direct_play_shows_resolution_sdr_and_the_unsure_audio_and_no_transcode_note() {
        let o = StreamOutput::derive(&input());
        let dd = format!("{} 5.1", metadata::friendly_codec("ac3"));
        assert_eq!(row(&o), [("1080p".to_owned(), K::Resolution, false), ("SDR".to_owned(), K::Sdr, false), (dd.clone(), K::Audio, true)]);
        assert!(o.mode.is_none(), "a direct play has no mode pill");
        assert_eq!((o.video_source.as_str(), o.video_output.as_str()), ("1080p SDR", "1080p SDR"));
        assert_eq!((o.audio_source.as_str(), o.audio_output.as_str()), (dd.as_str(), dd.as_str()));
        assert_eq!(o.notes, [note(msg::player_stream_tv_audio()), note(msg::player_stream_unsure_note())]);
        assert!(!o.notes.contains(&note(msg::player_stream_transcode_note())));
    }

    #[test]
    fn a_transcode_explains_itself_in_the_panel_and_draws_no_mode_pill_in_the_row() {
        let o = StreamOutput::derive(&StreamInput { out_raster: (1280, 720), video_kept: false, ..input() });
        assert!(row(&o).iter().all(|(_, kind, _)| *kind != K::Mode), "no mode pill in the row: {:?}", row(&o));
        assert_eq!(row(&o)[0].0, "720p");
        assert_eq!(o.notes[0], note(msg::player_stream_transcode_note()));
        assert_eq!((o.video_source.as_str(), o.video_output.as_str()), ("1080p SDR", "720p SDR"));
        // a remux keeps the video: no transcode note
        let remux = StreamOutput::derive(&StreamInput { video_kept: true, ..input() });
        assert!(!remux.notes.contains(&note(msg::player_stream_transcode_note())));
    }

    #[test]
    fn hdr_is_kept_on_a_direct_play_and_converted_to_sdr_with_a_note_on_a_transcode() {
        let kept = StreamOutput::derive(&StreamInput { src_range: Some(SourceRange::DolbyVision), ..input() });
        assert_eq!(kept.range.as_ref().map(|p| (p.label.as_str(), p.kind)), Some(("Dolby Vision", K::Hdr)));
        assert_eq!(kept.video_output, "1080p Dolby Vision");
        let tone = StreamOutput::derive(&StreamInput { src_range: Some(SourceRange::Hdr10), video_kept: false, ..input() });
        assert_eq!(tone.range.as_ref().map(|p| (p.label.as_str(), p.kind)), Some(("SDR", K::Sdr)));
        assert_eq!(tone.video_source, "1080p HDR10");
        assert!(tone.notes.contains(&note(&msg::player_stream_to_sdr("HDR10"))), "{:?}", tone.notes);
    }

    /// The range is named the web player's way (`hdrKind`): HDR10 for PQ, HLG, Dolby Vision, and plain
    /// HDR only for an item whose record predates the format.
    #[test]
    fn the_range_names_hdr10_hlg_and_dolby_vision() {
        use metadata::HdrKind as H;
        let label = |kind, dovi, hdr| SourceRange::of(kind, dovi, hdr).map(SourceRange::label);
        assert_eq!(label(H::Hdr10, false, true), Some("HDR10"));
        assert_eq!(label(H::Hlg, false, true), Some("HLG"));
        assert_eq!(label(H::DolbyVision, true, true), Some("Dolby Vision"));
        assert_eq!(label(H::Sdr, true, true), Some("Dolby Vision"), "a Dolby Vision record names it");
        assert_eq!(label(H::Sdr, false, true), Some("HDR"), "a record from before the format");
        assert_eq!(label(H::Sdr, false, false), None);
        assert_eq!(H::of("smpte2084", false), H::Hdr10);
        assert_eq!(H::of("arib-std-b67", false), H::Hlg);
        assert_eq!(H::of("smpte2084", true), H::DolbyVision);
        assert_eq!(H::of("bt709", false), H::Sdr);
        let kept = StreamOutput::derive(&StreamInput { src_range: Some(SourceRange::Hlg), ..input() });
        assert_eq!(kept.range.as_ref().map(|p| p.label.as_str()), Some("HLG"));
    }

    #[test]
    fn server_and_app_audio_are_sure_and_say_who_converted_the_sound() {
        let server = StreamOutput::derive(&StreamInput { out_audio: AudioOut::Server { codec: "aac".into(), channels: 2 }, ..input() });
        let aac = format!("{} {}", metadata::friendly_codec("aac"), msg::widgets_tracks_stereo());
        assert_eq!(server.audio.as_ref().map(|p| (p.label.as_str(), p.unsure)), Some((aac.as_str(), false)));
        assert_eq!(server.notes, [note(msg::player_stream_server_audio())], "nothing unsure, no unsure note");
        let app = StreamOutput::derive(&StreamInput { out_audio: AudioOut::App { channels: 8 }, ..input() });
        assert_eq!(app.audio_output, "PCM 7.1");
        assert_eq!(app.notes, [note(msg::player_stream_app_audio())]);
    }

    #[test]
    fn premium_audio_takes_the_web_players_names() {
        let label = |codec: &str, profile: &str, title: &str, channels| {
            super::audio_label(&AudioFacts { codec: codec.into(), channels, profile: profile.into(), title: title.into(), atmos: false })
        };
        assert_eq!(label("dca", "ma", "", 8), "DTS-HD MA 7.1");
        assert_eq!(label("dca", "", "English DTS:X", 8), "DTS:X 7.1");
        assert_eq!(label("dca", "hra", "", 6), "DTS-HD 5.1");
        assert_eq!(label("truehd", "", "TrueHD Atmos 7.1", 8), "Dolby Atmos 7.1");
        let profiled = AudioFacts { codec: "eac3".into(), channels: 6, profile: "dolby digital plus + dolby atmos".into(), atmos: true, ..AudioFacts::default() };
        assert_eq!(super::audio_label(&profiled), "Dolby Atmos 5.1", "PMS names Atmos in the profile");
        assert_eq!(label("eac3", "", "", 0), metadata::friendly_codec("eac3"), "no channels, no number");
    }

    #[test]
    fn nothing_known_reads_as_a_dash_and_draws_no_pill() {
        let o = StreamOutput::derive(&StreamInput { src_raster: (0, 0), out_raster: (0, 0), src_audio: None, ..input() });
        assert!(row(&o).is_empty());
        for v in [&o.video_source, &o.video_output, &o.audio_source, &o.audio_output] {
            assert_eq!(v, UNKNOWN);
        }
        assert!(o.notes.is_empty());
    }

    #[test]
    fn notes_are_sentences_and_one_to_a_line() {
        assert_eq!(super::sentence("élan vital"), "Élan vital.");
        let o = StreamOutput::derive(&input());
        assert_eq!(o.notes_text().lines().count(), o.notes.len());
    }
}
