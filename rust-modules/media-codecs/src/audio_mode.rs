//! **The viewer's audio mode, applied to the PCM the app decodes** (`AudioProcessing`: Off, Night,
//! Voice). Pure Rust: the bundled FFmpeg is built `--disable-avfilter --disable-swresample`
//! (`ci/build-ffmpeg.sh`), so `acompressor`/`pan`/`equalizer` are not there to call, and adding
//! libavfilter would be a new shipped library. The desktop client's mpv chain is the model.
//!
//! * **Night** (`NIGHT`): a feed-forward compressor on the linked (loudest-channel) mean-square
//!   level, threshold -30 dB, ratio 3:1, soft knee, +9 dB of make-up, so a quiet line is lifted and an
//!   explosion is held down; then the limiter.
//! * **Voice** on a stereo or mono stream (`VOICE`): a gentler compressor (-26 dB, 2:1, +5 dB) and a
//!   peaking presence lift of +4 dB at 2.5 kHz (Q 0.9, RBJ biquad); then the limiter. On a
//!   multichannel source the dialogue lives in the centre, and the app sees it only while it mixes
//!   the source down (`speed_audio::CentreLift`: centre +3 dB, every other channel -3 dB, faded in and
//!   out over 20 ms, mixing the same channels as the plain downmix);
//!   the stage is then told so ([`AudioModeStage::set_centre_lifted`]) and runs the limiter alone,
//!   as the desktop's `pan` chain does.
//! * **The limiter** has no lookahead and an instant attack: the gain that brings a sample's
//!   loudest channel down to the ceiling (-1 dBFS) is applied to that very sample, then released
//!   over 60 ms. So no sample a mode processed leaves above the ceiling, whatever the make-up did,
//!   and the stage adds no latency (nothing to correct in the stamps).
//!
//! **A mode never mutes and never breaks playback.** The compressor's gain is bounded to
//! [-18, +12] dB and its make-up fades out below -55 dB (so the noise floor of a silence is not
//! pumped up), the limiter can only pull a sample to the ceiling, a change of mode crossfades
//! (20 ms out, then 20 ms in) rather than switching between samples, and the centre lift ramps its
//! gains over 20 ms of its own in the downmix (a test bounds the step between adjacent frames of a
//! steady 5.1 source to 1/900 of the whole change; that is a ramp, not a proof that no program is
//! audibly affected), a sample whose arithmetic is not finite is replaced by the
//! untouched input and the state restarted, and a shape the stage does not handle (more than two
//! channels, a width other than 16 or 32 bits) passes untouched. `Off` with the crossfade finished
//! returns before it reads a byte: the samples are bit-exact.
//!
//! **No allocation per buffer, and no libm**: the level and the gain go through `log2_approx` and
//! `exp2_approx` (bit tricks and short series, accurate to well under 0.01 dB), the biquad's
//! coefficients are designed once with series `sin`/`cos` (`ci/allow/libm.txt` keeps transcendentals
//! to render code). Gain is computed once per 16 frames and ramped linearly across them.
//!
//! **Cost, an estimate (not measured on the television):** per stereo frame two sample
//! conversions, two biquads (5 multiply-adds each), the gain ramp, the limiter's compare and release,
//! two saturating writes: about 45 floating-point operations; plus one `log2`/`exp2` pair and the
//! curve every 16 frames (about 4 a frame). At 48 kHz that is about 2.4 Mflop/s, on the order of 1%
//! of one Cortex-A9-class core. A real figure needs the set and `ARM_PROFILE=release`.
use plx_playback_contract::feed::AudioProcessing;

/// Frames per gain decision (the control rate is `rate / BLOCK`, 3 kHz at 48 kHz).
const BLOCK: usize = 16;
/// The limiter's ceiling: -1 dBFS.
pub const CEILING: f32 = 0.891_250_94;
/// The crossfade between processed and untouched sound on a change of mode, each way.
const FADE_S: f32 = 0.020;
/// The compressor's gain never leaves this range, in dB.
const GAIN_MIN_DB: f32 = -18.0;
const GAIN_MAX_DB: f32 = 12.0;
/// Below `FLOOR_LO_DB` (mean-square level) no make-up is applied, above `FLOOR_HI_DB` all of it.
const FLOOR_LO_DB: f32 = -70.0;
const FLOOR_HI_DB: f32 = -55.0;
/// The presence lift of Voice on a stereo or mono stream.
const PRESENCE_HZ: f64 = 2_500.0;
const PRESENCE_DB: f64 = 4.0;
const PRESENCE_Q: f64 = 0.9;
/// `10 * log10(2)`: mean square in log2 to dB.
const DB_PER_OCTAVE_POWER: f32 = 3.010_3;
/// `20 * log10(2)`: amplitude dB to log2.
const DB_PER_OCTAVE_AMPLITUDE: f32 = 6.020_6;

/// A static gain curve: threshold and knee in dB of mean-square level, `slope` = 1 - 1/ratio.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Curve {
    threshold_db: f32,
    slope: f32,
    knee_db: f32,
    makeup_db: f32,
}

const NIGHT: Curve = Curve { threshold_db: -30.0, slope: 2.0 / 3.0, knee_db: 6.0, makeup_db: 9.0 };
const VOICE: Curve = Curve { threshold_db: -26.0, slope: 0.5, knee_db: 6.0, makeup_db: 5.0 };

impl Curve {
    /// The gain, in dB, for a mean-square level `level_db`: the soft-knee reduction above the
    /// threshold, plus the make-up faded in over the floor, clamped to the stage's range.
    fn gain_db(&self, level_db: f32) -> f32 {
        let over = level_db - self.threshold_db;
        let half = self.knee_db * 0.5;
        let reduction = if over <= -half {
            0.0
        } else if over >= half {
            self.slope * over
        } else {
            let x = over + half;
            self.slope * x * x / (2.0 * self.knee_db)
        };
        let floor = ((level_db - FLOOR_LO_DB) / (FLOOR_HI_DB - FLOOR_LO_DB)).clamp(0.0, 1.0);
        (self.makeup_db * floor - reduction).clamp(GAIN_MIN_DB, GAIN_MAX_DB)
    }
}

/// A one-pole smoothing coefficient for a time constant of `time_s` at `rate_hz` updates a second:
/// `x / (1 + x)` with `x = 1 / (time * rate)`, the first terms of `1 - e^(-x)`.
fn one_pole(time_s: f32, rate_hz: f32) -> f32 {
    let x = 1.0 / (time_s * rate_hz).max(1e-6);
    x / (1.0 + x)
}

/// `log2(x)` for a positive finite `x`, to about 1e-5: the exponent from the bits, the mantissa
/// `m` in [1, 2) through `ln m = 2 atanh((m - 1) / (m + 1))` to the seventh power.
pub(crate) fn log2_approx(x: f32) -> f32 {
    let x = x.max(f32::MIN_POSITIVE);
    let bits = x.to_bits();
    let exponent = ((bits >> 23) & 0xff) as i32 - 127;
    let m = f32::from_bits((bits & 0x007f_ffff) | 0x3f80_0000);
    let t = (m - 1.0) / (m + 1.0);
    let t2 = t * t;
    let ln_m = 2.0 * t * (1.0 + t2 * (1.0 / 3.0 + t2 * (1.0 / 5.0 + t2 * (1.0 / 7.0))));
    exponent as f32 + ln_m * std::f32::consts::LOG2_E
}

/// `2^y`, to about 2e-6 relative, for `y` in about [-126, 127] (clamped there): the integer part
/// into the exponent bits, the fraction through the series of `e^(f ln 2)` to the seventh power.
pub(crate) fn exp2_approx(y: f32) -> f32 {
    let y = y.clamp(-126.0, 127.0);
    let n = y.floor();
    let f = (y - n) * std::f32::consts::LN_2;
    let mut term = 1.0f32;
    let mut sum = 1.0f32;
    for k in 1..=7 {
        term *= f / k as f32;
        sum += term;
    }
    sum * f32::from_bits(((n as i32 + 127) as u32) << 23)
}

/// `(sin x, cos x)` for `x` in [0, pi], by series to the 21st power (error below 1e-12 there).
fn sin_cos_series(x: f64) -> (f64, f64) {
    let x2 = x * x;
    let (mut s, mut c) = (x, 1.0);
    let (mut ts, mut tc) = (x, 1.0);
    for k in 1..=10 {
        let k = f64::from(k);
        ts *= -x2 / ((2.0 * k) * (2.0 * k + 1.0));
        tc *= -x2 / ((2.0 * k - 1.0) * (2.0 * k));
        s += ts;
        c += tc;
    }
    (s, c)
}

/// A normalized biquad (`a0` = 1), transposed direct form II.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl Biquad {
    /// RBJ's peaking equalizer: `gain_db` at `freq` with quality `q`, at `rate`. `None` when the
    /// frequency is not well inside the band (an 8 kHz stream has no room for a 2.5 kHz bell).
    fn peaking(rate: u32, freq: f64, gain_db: f64, q: f64) -> Option<Biquad> {
        let rate = f64::from(rate);
        if rate <= 0.0 || freq >= 0.45 * rate {
            return None;
        }
        let w0 = 2.0 * std::f64::consts::PI * freq / rate;
        let (sin_w, cos_w) = sin_cos_series(w0);
        // A = 10^(dB / 40) = 2^(dB / 40 * log2 10)
        let a = f64::from(exp2_approx((gain_db / 40.0 * std::f64::consts::LOG2_10) as f32));
        let alpha = sin_w / (2.0 * q);
        let a0 = 1.0 + alpha / a;
        Some(Biquad {
            b0: ((1.0 + alpha * a) / a0) as f32,
            b1: (-2.0 * cos_w / a0) as f32,
            b2: ((1.0 - alpha * a) / a0) as f32,
            a1: (-2.0 * cos_w / a0) as f32,
            a2: ((1.0 - alpha / a) / a0) as f32,
        })
    }

    #[inline]
    fn run(&self, z: &mut [f32; 2], x: f32) -> f32 {
        let y = self.b0 * x + z[0];
        z[0] = self.b1 * x - self.a1 * y + z[1];
        z[1] = self.b2 * x - self.a2 * y;
        y
    }
}

/// What a stage has done to a stream, for the one log line a stream writes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AudioModeReport {
    /// Frames that went through a mode (wet above zero).
    pub frames: u64,
    /// Samples the limiter pulled down to the ceiling.
    pub limited: u64,
    /// Times a non-finite result was replaced by the untouched input and the state restarted.
    pub restarts: u64,
}

/// **One stream's audio-mode processing**, in place on interleaved little-endian PCM of one or two
/// channels, 16 or 32 bits ([`Self::process_s16le`], [`Self::process_s32le`]). Owned by the one
/// thread that decodes the stream; built once per decoder, never per buffer.
pub struct AudioModeStage {
    rate: u32,
    channels: usize,
    /// The mode last asked for, and the one whose parameters run now (they differ during a crossfade).
    wanted: AudioProcessing,
    active: AudioProcessing,
    /// The upstream mix-down lifted a centre channel: Voice then runs the limiter alone.
    centre_lifted: bool,
    /// 0 = the untouched sound, 1 = fully processed; moves by `fade_step` a frame.
    wet: f32,
    fade_step: f32,
    /// Mean-square envelope, smoothed gain in dB, and the linear gain ramp of the current block.
    env: f32,
    gain_db: f32,
    gain: f32,
    gain_step: f32,
    block_pos: usize,
    block_sum: f32,
    limiter: f32,
    eq: Option<Biquad>,
    eq_state: [[f32; 2]; 2],
    env_attack: f32,
    env_release: f32,
    gain_attack: f32,
    gain_release: f32,
    limiter_release: f32,
    report: AudioModeReport,
}

impl AudioModeStage {
    /// A stage for `channels` channels at `rate` Hz, `Off`.
    pub fn new(rate: u32, channels: usize) -> Self {
        let rate_f = rate.max(1) as f32;
        let block_rate = rate_f / BLOCK as f32;
        AudioModeStage {
            rate,
            channels,
            wanted: AudioProcessing::Off,
            active: AudioProcessing::Off,
            centre_lifted: false,
            wet: 0.0,
            fade_step: 1.0 / (FADE_S * rate_f).max(1.0),
            env: 0.0,
            gain_db: 0.0,
            gain: 1.0,
            gain_step: 0.0,
            block_pos: 0,
            block_sum: 0.0,
            limiter: 1.0,
            eq: None,
            eq_state: [[0.0; 2]; 2],
            env_attack: one_pole(0.020, block_rate),
            env_release: one_pole(0.150, block_rate),
            gain_attack: one_pole(0.015, block_rate),
            gain_release: one_pole(0.250, block_rate),
            limiter_release: one_pole(0.060, rate_f),
            report: AudioModeReport::default(),
        }
    }

    /// The mode for every frame from now on; a change crossfades through the untouched sound.
    pub fn set_mode(&mut self, mode: AudioProcessing) {
        self.wanted = mode;
    }

    /// The mode last asked for.
    pub fn mode(&self) -> AudioProcessing {
        self.wanted
    }

    /// Whether the PCM handed in was mixed down with its centre lifted (`speed_audio::CentreLift`
    /// found a centre). Voice then compresses nothing and only limits.
    pub fn set_centre_lifted(&mut self, lifted: bool) {
        self.centre_lifted = lifted;
    }

    /// Whether the upstream mix-down should lift the centre: Voice is asked for.
    pub fn wants_centre_lift(&self) -> bool {
        self.wanted == AudioProcessing::Voice
    }

    pub fn report(&self) -> AudioModeReport {
        self.report
    }

    /// Nothing to do: `Off`, with no crossfade left to finish. The samples stay bit-exact.
    pub fn is_bypassed(&self) -> bool {
        self.wanted == AudioProcessing::Off && self.active == AudioProcessing::Off && self.wet <= 0.0
    }

    /// A seek: the sound at the new position is not the old one, so the level history goes; the
    /// mode and its crossfade position stay.
    pub fn reset(&mut self) {
        self.restart_state();
    }

    fn restart_state(&mut self) {
        self.env = 0.0;
        self.gain_db = 0.0;
        self.gain = 1.0;
        self.gain_step = 0.0;
        self.block_pos = 0;
        self.block_sum = 0.0;
        self.limiter = 1.0;
        self.eq_state = [[0.0; 2]; 2];
    }

    /// Take on `self.wanted`'s parameters (called with the sound fully dry).
    fn activate_wanted(&mut self) {
        self.active = self.wanted;
        self.restart_state();
        self.eq = match self.active {
            AudioProcessing::Voice => Biquad::peaking(self.rate, PRESENCE_HZ, PRESENCE_DB, PRESENCE_Q),
            AudioProcessing::Night | AudioProcessing::Off => None,
        };
    }

    /// The curve the active mode compresses with; `None` when it only limits.
    fn curve(&self) -> Option<Curve> {
        match self.active {
            AudioProcessing::Night => Some(NIGHT),
            AudioProcessing::Voice if !self.centre_lifted => Some(VOICE),
            AudioProcessing::Voice | AudioProcessing::Off => None,
        }
    }

    /// Process interleaved S16 LE in place.
    pub fn process_s16le(&mut self, bytes: &mut [u8]) {
        self.process(bytes, 2);
    }

    /// Process interleaved S32 LE in place.
    pub fn process_s32le(&mut self, bytes: &mut [u8]) {
        self.process(bytes, 4);
    }

    /// Process interleaved little-endian samples of `width` bytes in place.
    pub fn process(&mut self, bytes: &mut [u8], width: usize) {
        if self.is_bypassed() || !(1..=2).contains(&self.channels) || !(width == 2 || width == 4) {
            return;
        }
        let frame_bytes = width * self.channels;
        for frame in bytes.chunks_exact_mut(frame_bytes) {
            let mut x = [0.0f32; 2];
            for (c, s) in frame.chunks_exact(width).enumerate() {
                x[c] = read_sample(s);
            }
            if let Some(y) = self.frame(x) {
                for (c, s) in frame.chunks_exact_mut(width).enumerate() {
                    write_sample(s, y[c]);
                }
            }
        }
    }

    /// One frame: `None` leaves the input untouched (bypassed, or a non-finite result).
    #[inline]
    fn frame(&mut self, x: [f32; 2]) -> Option<[f32; 2]> {
        // the crossfade: out while the mode is changing, in once it has changed
        if self.wanted != self.active {
            if self.wet <= 0.0 {
                self.activate_wanted();
            } else {
                self.wet = (self.wet - self.fade_step).max(0.0);
            }
        } else if self.active != AudioProcessing::Off && self.wet < 1.0 {
            self.wet = (self.wet + self.fade_step).min(1.0);
        }
        if self.active == AudioProcessing::Off && self.wet <= 0.0 {
            return None;
        }
        let ch = self.channels;
        // the detector: the loudest channel's square, before any gain
        let sq = x[..ch].iter().fold(0.0f32, |m, v| m.max(v * v));
        self.block_sum += sq;
        self.gain += self.gain_step;
        let mut y = [0.0f32; 2];
        for c in 0..ch {
            let mut v = x[c] * self.gain;
            // the presence lift is the stereo half of Voice: a lifted centre already did that work
            if let (Some(eq), false) = (self.eq, self.centre_lifted) {
                v = eq.run(&mut self.eq_state[c], v);
            }
            y[c] = v;
        }
        // the limiter: instant attack, so this sample's loudest channel is at the ceiling at most
        let peak = y[..ch].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let released = self.limiter + (1.0 - self.limiter) * self.limiter_release;
        self.limiter = if peak * released > CEILING { CEILING / peak } else { released };
        if peak * released > CEILING {
            self.report.limited += ch as u64;
        }
        let mut out = [0.0f32; 2];
        for c in 0..ch {
            let wet = y[c] * self.limiter;
            out[c] = x[c] + self.wet * (wet - x[c]);
        }
        self.block_pos += 1;
        if self.block_pos == BLOCK {
            self.end_block();
        }
        if !out[..ch].iter().all(|v| v.is_finite()) || !self.gain.is_finite() || !self.limiter.is_finite() {
            self.report.restarts += 1;
            self.restart_state();
            return None;
        }
        self.report.frames += 1;
        Some(out)
    }

    /// The gain decision for the next block, from the level of the one just ended.
    fn end_block(&mut self) {
        let mean = self.block_sum / BLOCK as f32;
        self.block_sum = 0.0;
        self.block_pos = 0;
        let coeff = if mean > self.env { self.env_attack } else { self.env_release };
        self.env += (mean - self.env) * coeff;
        let target_db = match self.curve() {
            Some(curve) => curve.gain_db(DB_PER_OCTAVE_POWER * log2_approx(self.env + 1e-12)),
            None => 0.0,
        };
        let coeff = if target_db < self.gain_db { self.gain_attack } else { self.gain_release };
        self.gain_db += (target_db - self.gain_db) * coeff;
        let target = exp2_approx(self.gain_db / DB_PER_OCTAVE_AMPLITUDE);
        self.gain_step = (target - self.gain) / BLOCK as f32;
    }
}

/// A little-endian 16- or 32-bit sample as a fraction of full scale.
#[inline]
fn read_sample(s: &[u8]) -> f32 {
    if s.len() == 2 {
        f32::from(i16::from_le_bytes([s[0], s[1]])) / 32_768.0
    } else {
        (f64::from(i32::from_le_bytes([s[0], s[1], s[2], s[3]])) / 2_147_483_648.0) as f32
    }
}

/// Write a fraction of full scale back as the sample's width, rounded and saturated.
#[inline]
fn write_sample(s: &mut [u8], v: f32) {
    if s.len() == 2 {
        let q = (v * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16;
        s.copy_from_slice(&q.to_le_bytes());
    } else {
        let q = (f64::from(v) * 2_147_483_648.0).round().clamp(-2_147_483_648.0, 2_147_483_647.0) as i32;
        s.copy_from_slice(&q.to_le_bytes());
    }
}

#[cfg(test)]
#[path = "audio_mode_tests.rs"]
mod tests;
