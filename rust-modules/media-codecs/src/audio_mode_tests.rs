//! The audio mode's DSP graded on synthetic signals: quiet goes up, loud goes down, nothing is ever
//! silenced, the limiter holds its ceiling, and Clear voices lifts the centre and only the centre.
use super::*;

const RATE: u32 = 48_000;

fn sine(freq: f64, amp: f64, frames: usize, channels: usize) -> Vec<i16> {
    (0..frames)
        .flat_map(|i| {
            let v = (amp * (2.0 * std::f64::consts::PI * freq * i as f64 / f64::from(RATE)).sin() * 32_767.0).round() as i16;
            std::iter::repeat(v).take(channels)
        })
        .collect()
}

fn bytes(v: &[i16]) -> Vec<u8> {
    v.iter().flat_map(|s| s.to_le_bytes()).collect()
}

fn samples(b: &[u8]) -> Vec<f64> {
    b.chunks_exact(2).map(|c| f64::from(i16::from_le_bytes([c[0], c[1]])) / 32_768.0).collect()
}

fn rms(v: &[f64]) -> f64 {
    (v.iter().map(|x| x * x).sum::<f64>() / v.len().max(1) as f64).sqrt()
}

fn db(ratio: f64) -> f64 {
    20.0 * ratio.log10()
}

/// Run `mode` over `seconds` of `input` (stereo S16) in 1024-frame buffers; return the output.
fn run(mode: AudioProcessing, input: &[i16], channels: usize) -> Vec<u8> {
    let mut stage = AudioModeStage::new(RATE, channels);
    stage.set_mode(mode);
    let mut out = bytes(input);
    for chunk in out.chunks_mut(1024 * 2 * channels) {
        stage.process_s16le(chunk);
    }
    out
}

/// The level change of the last half second, input to output, in dB.
fn settled_gain_db(input: &[i16], output: &[u8], channels: usize) -> f64 {
    let tail = (RATE as usize / 2) * channels;
    let i = samples(&bytes(&input[input.len() - tail..]));
    let o = samples(&output[output.len() - tail * 2..]);
    db(rms(&o) / rms(&i))
}

#[test]
fn the_approximations_are_within_a_hundredth_of_a_db() {
    for k in -400..=400 {
        let y = k as f32 / 10.0;
        let want = 2f64.powf(f64::from(y));
        let got = f64::from(exp2_approx(y));
        assert!((got / want - 1.0).abs() < 1e-5, "2^{y}: {got} vs {want}");
    }
    for k in 1..2000 {
        let x = (k as f32) * 1e-3;
        let want = f64::from(x).log2();
        assert!((f64::from(log2_approx(x)) - want).abs() < 1e-4, "log2 {x}");
    }
    assert!((f64::from(log2_approx(1e-12)) - (1e-12f64).log2()).abs() < 1e-3);
    for k in 0..=314 {
        let x = f64::from(k) / 100.0;
        let (s, c) = sin_cos_series(x);
        assert!((s - x.sin()).abs() < 1e-9 && (c - x.cos()).abs() < 1e-9, "{x}");
    }
}

#[test]
fn off_is_bit_exact_in_both_widths() {
    let mut stage = AudioModeStage::new(RATE, 2);
    let orig: Vec<u8> = (0..4096u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
    let mut b = orig.clone();
    stage.process_s16le(&mut b);
    assert_eq!(b, orig);
    stage.process_s32le(&mut b);
    assert_eq!(b, orig);
    // a mode turned off again ends bit-exact once its fade is out
    stage.set_mode(AudioProcessing::Night);
    let mut warm = bytes(&sine(440.0, 0.3, 4800, 2));
    stage.process_s16le(&mut warm);
    stage.set_mode(AudioProcessing::Off);
    let mut fade = bytes(&sine(440.0, 0.3, 4800, 2));
    stage.process_s16le(&mut fade);
    assert!(stage.is_bypassed(), "a 20 ms fade is over within 100 ms");
    let mut b = orig.clone();
    stage.process_s16le(&mut b);
    assert_eq!(b, orig);
}

#[test]
fn night_lifts_quiet_dialogue_and_holds_loud_action_down() {
    let quiet = sine(1_000.0, 0.01, RATE as usize * 3, 2); // -40 dBFS peak
    let loud = sine(1_000.0, 0.5, RATE as usize * 3, 2); // -6 dBFS peak
    let up = settled_gain_db(&quiet, &run(AudioProcessing::Night, &quiet, 2), 2);
    let down = settled_gain_db(&loud, &run(AudioProcessing::Night, &loud, 2), 2);
    assert!(up > 6.0, "quiet must come up: {up:.2} dB");
    assert!(down < -3.0, "loud must go down: {down:.2} dB");
    // the range between them shrinks by more than 9 dB (34 dB in, under 25 out)
    assert!(up - down > 9.0, "{up:.2} - {down:.2}");
}

#[test]
fn no_level_is_ever_silenced() {
    for mode in [AudioProcessing::Night, AudioProcessing::Voice] {
        for channels in [1, 2] {
            for amp_db in [-80.0f64, -60.0, -40.0, -20.0, -6.0, 0.0] {
                let amp = 10f64.powf(amp_db / 20.0);
                let input = sine(700.0, amp, RATE as usize, channels);
                let gain = settled_gain_db(&input, &run(mode, &input, channels), channels);
                assert!(gain > -20.0, "{mode:?} {channels}ch at {amp_db} dBFS: {gain:.2} dB");
            }
        }
    }
    // digital silence stays silence, and is not turned into anything
    let silence = vec![0i16; RATE as usize * 2];
    assert!(run(AudioProcessing::Night, &silence, 2).iter().all(|b| *b == 0));
}

#[test]
fn nothing_a_mode_processed_leaves_above_the_ceiling() {
    // a quiet passage brings the make-up up, then a full-scale hit lands on it
    for mode in [AudioProcessing::Night, AudioProcessing::Voice] {
        let mut input = sine(300.0, 0.005, RATE as usize * 2, 2);
        input.extend(sine(300.0, 1.0, RATE as usize, 2));
        input.extend((0..RATE as usize).flat_map(|i| {
            let v = if (i / 40) % 2 == 0 { i16::MAX } else { i16::MIN };
            [v, v]
        }));
        let out = samples(&run(mode, &input, 2));
        let skip = (RATE as usize / 10) * 2; // the 20 ms fade-in is long over
        let peak = out[skip..].iter().fold(0.0f64, |m, v| m.max(v.abs()));
        assert!(peak <= f64::from(CEILING) + 1.0 / 32_768.0, "{mode:?}: peak {peak}");
        assert!(peak > 0.5, "{mode:?}: and the hit is still loud ({peak})");
    }
}

#[test]
fn voice_lifts_the_presence_band_of_a_stereo_stream() {
    let low = sine(200.0, 0.05, RATE as usize * 2, 2);
    let mid = sine(2_500.0, 0.05, RATE as usize * 2, 2);
    let g_low = settled_gain_db(&low, &run(AudioProcessing::Voice, &low, 2), 2);
    let g_mid = settled_gain_db(&mid, &run(AudioProcessing::Voice, &mid, 2), 2);
    assert!(g_mid - g_low > 3.0, "2.5 kHz {g_mid:.2} dB against 200 Hz {g_low:.2} dB");
    // Night has no equalizer: the two bands move together
    let n_low = settled_gain_db(&low, &run(AudioProcessing::Night, &low, 2), 2);
    let n_mid = settled_gain_db(&mid, &run(AudioProcessing::Night, &mid, 2), 2);
    assert!((n_mid - n_low).abs() < 1.0, "{n_mid:.2} {n_low:.2}");
}

#[test]
fn a_lifted_centre_leaves_voice_to_the_limiter_alone() {
    let input = sine(1_000.0, 0.01, RATE as usize * 2, 2);
    let mut stage = AudioModeStage::new(RATE, 2);
    stage.set_mode(AudioProcessing::Voice);
    assert!(stage.wants_centre_lift());
    stage.set_centre_lifted(true);
    let mut out = bytes(&input);
    stage.process_s16le(&mut out);
    let gain = settled_gain_db(&input, &out, 2);
    assert!(gain.abs() < 0.2, "no compression, no EQ at 1 kHz: {gain:.3} dB");
}

#[test]
fn a_change_of_mode_crossfades_without_a_click() {
    let input = sine(500.0, 0.2, RATE as usize * 2, 2);
    let mut stage = AudioModeStage::new(RATE, 2);
    let mut out = bytes(&input);
    let half = out.len() / 2;
    let (a, b) = out.split_at_mut(half);
    stage.set_mode(AudioProcessing::Night);
    stage.process_s16le(a);
    stage.set_mode(AudioProcessing::Voice);
    stage.process_s16le(b);
    let s = samples(&out);
    // the largest step of a 500 Hz sine at 0.2 is 2*pi*500/48000*0.2*gain; allow the +9 dB make-up
    let limit = 2.0 * std::f64::consts::PI * 500.0 / f64::from(RATE) * 0.2 * 3.0 + 0.002;
    let worst = s.chunks_exact(2).map(|f| f[0]).collect::<Vec<_>>().windows(2).fold(0.0f64, |m, w| m.max((w[1] - w[0]).abs()));
    assert!(worst < limit, "a step of {worst} (limit {limit})");
    assert_eq!(stage.report().restarts, 0);
}

#[test]
fn a_shape_the_stage_does_not_handle_passes_untouched() {
    let orig = bytes(&sine(440.0, 0.5, 4800, 6));
    let mut stage = AudioModeStage::new(RATE, 6);
    stage.set_mode(AudioProcessing::Night);
    let mut b = orig.clone();
    stage.process_s16le(&mut b);
    assert_eq!(b, orig);
    let mut stage = AudioModeStage::new(RATE, 2);
    stage.set_mode(AudioProcessing::Night);
    let mut b = orig.clone();
    stage.process(&mut b, 3);
    assert_eq!(b, orig);
}

#[test]
fn full_scale_s32_saturates_instead_of_wrapping() {
    let mut stage = AudioModeStage::new(44_100, 1);
    stage.set_mode(AudioProcessing::Night);
    let mut b: Vec<u8> = (0..44_100).flat_map(|i| if i % 2 == 0 { i32::MAX } else { i32::MIN }.to_le_bytes()).collect();
    stage.process_s32le(&mut b);
    let peak = b.chunks_exact(4).skip(4410).map(|c| f64::from(i32::from_le_bytes([c[0], c[1], c[2], c[3]])).abs()).fold(0.0, f64::max);
    assert!(peak <= f64::from(CEILING) * 2_147_483_648.0 + 256.0, "{peak}");
}

#[test]
fn the_curves_are_monotonic_and_bounded() {
    for curve in [NIGHT, VOICE] {
        let mut last_out = f32::MIN;
        for k in -1000..=0 {
            let level = k as f32 / 10.0;
            let g = curve.gain_db(level);
            assert!((GAIN_MIN_DB..=GAIN_MAX_DB).contains(&g));
            // the output level never falls as the input rises (no inversion of loud and quiet)
            let out = level + g;
            assert!(out >= last_out - 1e-3, "{curve:?} at {level}");
            last_out = out;
        }
    }
}

/// A planar f32 frame of `channels` planes, `value` on the channels in `on`, silence elsewhere.
fn planar_f32(channels: usize, frames: usize, on: &[usize], value: f32) -> Vec<Vec<u8>> {
    (0..channels)
        .map(|c| {
            let v = if on.contains(&c) { value } else { 0.0 };
            (0..frames).flat_map(|_| v.to_ne_bytes()).collect()
        })
        .collect()
}

/// The left output of mixing 5.1 (mask 0x3f: FL FR FC LFE BL BR) with signal on `on` only.
fn left_of_51(on: &[usize], lift: bool) -> (f32, bool) {
    use crate::speed_audio::{downmix_by_mask_lifted, SampleFormat};
    let planes = planar_f32(6, 4, on, 0.5);
    let refs: Vec<&[u8]> = planes.iter().map(|p| p.as_slice()).collect();
    let (mixed, lifted) = downmix_by_mask_lifted(SampleFormat::Flt, true, 0x3f, 6, 4, &refs, lift);
    (mixed[0][0], lifted)
}

#[test]
fn the_centre_lift_raises_the_centre_and_only_the_centre() {
    let (fc, lifted) = left_of_51(&[2], true);
    let (fl, _) = left_of_51(&[0], true);
    let (bl, _) = left_of_51(&[4], true);
    let (lfe, _) = left_of_51(&[3], true);
    assert!(lifted);
    let (fc0, lifted0) = left_of_51(&[2], false);
    let (fl0, _) = left_of_51(&[0], false);
    let (bl0, _) = left_of_51(&[4], false);
    assert!(!lifted0, "no lift unless asked");
    // centre against front: +6 dB further out (+3 on the centre, -3 on the front)
    let rel = db(f64::from(fc / fl)) - db(f64::from(fc0 / fl0));
    assert!((rel - 6.02).abs() < 0.05, "centre relative to front moved {rel:.3} dB");
    // the surround keeps its place relative to the front: both took -3 dB
    let rel = db(f64::from(bl / fl)) - db(f64::from(bl0 / fl0));
    assert!(rel.abs() < 0.01, "surround relative to front moved {rel:.3} dB");
    assert_eq!(lfe, 0.0, "the LFE is still dropped");
    // a full-scale signal on every channel still cannot clip the mix
    use crate::speed_audio::{downmix_by_mask_lifted, SampleFormat};
    let planes = planar_f32(6, 4, &[0, 1, 2, 3, 4, 5], 1.0);
    let refs: Vec<&[u8]> = planes.iter().map(|p| p.as_slice()).collect();
    let (mixed, _) = downmix_by_mask_lifted(SampleFormat::Flt, true, 0x3f, 6, 4, &refs, true);
    assert!(mixed.iter().all(|f| f[0].abs() <= 1.0 + 1e-6 && f[1].abs() <= 1.0 + 1e-6));
    // a source with no centre mixes exactly as it always did, lift or not
    let st = planar_f32(2, 4, &[0, 1], 0.25);
    let refs: Vec<&[u8]> = st.iter().map(|p| p.as_slice()).collect();
    let (a, la) = downmix_by_mask_lifted(SampleFormat::Flt, true, 0x3, 2, 4, &refs, true);
    let b = crate::speed_audio::downmix_by_mask(SampleFormat::Flt, true, 0x3, 2, 4, &refs);
    assert!(!la);
    assert_eq!(a, b);
    // an unnamed 6-channel layout is read as FFmpeg's 5.1 when lifting
    let planes = planar_f32(6, 4, &[2], 0.5);
    let refs: Vec<&[u8]> = planes.iter().map(|p| p.as_slice()).collect();
    let (m, l) = downmix_by_mask_lifted(SampleFormat::Flt, true, 0, 6, 4, &refs, true);
    assert!(l && (m[0][0] - fc).abs() < 1e-6);
}

/// The decode path of a lossy surround track: the converter mixes down (lifting the centre for
/// Voice) and the stage processes the stereo S16 it wrote. A layout change in the middle of the
/// stream (a 5.1 programme followed by a stereo one, as a track switch or a broadcast feeds it)
/// keeps the sound going: every block comes out, of the right length, and none is silent.
#[test]
fn a_surround_to_stereo_change_keeps_playing_through_the_voice_mode() {
    use crate::speed_audio::SampleFormat;
    let mut conv = crate::pcm::PcmConverter::lossy();
    let mut stage = AudioModeStage::new(RATE, 2);
    stage.set_mode(AudioProcessing::Voice);
    let block = 1_536usize;
    let mut pts = 0i64;
    let mut lifted_seen = Vec::new();
    for (n, (channels, mask)) in [(6usize, 0x3fu64); 40].into_iter().chain([(2usize, 0x3u64); 40]).enumerate() {
        let tone: Vec<f32> = (0..block)
            .map(|i| (0.2 * (2.0 * std::f64::consts::PI * 440.0 * (n * block + i) as f64 / 48_000.0).sin()) as f32)
            .collect();
        let planes: Vec<Vec<u8>> = (0..channels)
            .map(|c| tone.iter().flat_map(|v| if c == 3 { 0.0f32 } else { *v }.to_ne_bytes()).collect())
            .collect();
        let refs: Vec<&[u8]> = planes.iter().map(|p| p.as_slice()).collect();
        conv.set_centre_lift(stage.wants_centre_lift());
        let mut out = Vec::new();
        conv.convert(Some(pts), 48_000, SampleFormat::Flt, true, channels, mask, block, &refs, &mut out);
        stage.set_centre_lifted(conv.centre_lifted());
        lifted_seen.push(conv.centre_lifted());
        stage.process_s16le(&mut out);
        assert_eq!(out.len(), block * 4, "block {n}: every frame comes out");
        let level = rms(&samples(&out));
        assert!(level > 0.02, "block {n} ({channels} ch) is not silent: {level}");
        pts += (block as i64) * 1_000_000_000 / 48_000;
    }
    assert!(lifted_seen[..40].iter().all(|l| *l) && lifted_seen[40..].iter().all(|l| !*l),
        "the centre is lifted while there is one, and the stereo half takes the stereo Voice");
    assert_eq!(stage.report().restarts, 0);
}
