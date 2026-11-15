//! The rule that keeps everyone at the same place: last writer wins in Lamport order. A port of the
//! web client's `packages/core/src/togethersync.ts` (same thresholds, `newer`, `hostYields`, the
//! adopt/follow/ignore tick rule), as a value the session machine owns.
//!
//! It touches neither the network nor the player: observations of the local player come in as
//! [`PlayerObs`], what the player should do leaves as [`PlayerCmd`], and time is the `dt` handed to
//! [`TogetherSync::advance`]. Two differences from the web, both forced by the LG player:
//!
//! - **No playback-rate nudging.** The web closes a small drift by changing the rate a few
//!   percent; the LG pipeline cannot change rate without a reload. A drift under the soft
//!   threshold is left as it is, a larger one is a seek.
//! - **Keyframe slack.** An LG seek lands on a keyframe, up to one GOP BEFORE the target. Seeking
//!   again would land at the same keyframe, forever. So the first settled observation after a seek
//!   this rule commanded measures the landing error, and when it is a plausible keyframe distance
//!   ([`KEYFRAME_SLACK`] behind, [`KEYFRAME_AHEAD`] ahead) it is remembered and accepted: the
//!   player is held to be where the session is, offset by that error, until the next seek or
//!   gesture. A landing outside that range is not a keyframe and is never accepted as slack: the
//!   seek is commanded again, exactly [`RESEEK_LIMIT`] more times, after which the rule stops
//!   seeking for that reference ([`TogetherSync::landing_failed`]) and keeps the last good slack,
//!   so no input makes it seek in a loop. The next reference starts afresh.

use super::wire::{IntentIn, SyncAction, TickIn};

/// Gap (s) corrected by a seek after a command.
pub const HARD: f64 = 0.8;
/// Gap (s) corrected by a seek when following the host.
pub const SOFT: f64 = 2.0;
/// Gap (s) from the expected position past which a local change is a seek gesture.
pub const JUMP: f64 = 1.6;
/// After a correction, player changes are not the person's gestures (µs).
pub const SETTLE_US: u64 = 1_500_000;
/// How far (s) before its target an LG seek may land on a keyframe and still be accepted.
pub const KEYFRAME_SLACK: f64 = 10.0;
/// How far (s) after its target a landing is still taken as on target.
pub const KEYFRAME_AHEAD: f64 = 0.5;
/// Re-seeks after a landing too far off to be a keyframe, before the rule stops seeking for that
/// reference (the error is never accepted as slack).
pub const RESEEK_LIMIT: u32 = 2;
/// The largest clock a JavaScript peer holds exactly (`Number.MAX_SAFE_INTEGER`).
pub const MAX_SAFE_CLOCK: u64 = (1 << 53) - 1;

/// A command's stamp: Lamport clock and author.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stamp {
    pub c: u64,
    pub by: String,
}

/// True when `a` is strictly newer than `b`: a larger clock, or the same clock and a larger author.
pub fn newer(a: &Stamp, b: &Stamp) -> bool {
    a.c > b.c || (a.c == b.c && a.by > b.by)
}

/// Two hosts after a cut: `a` yields to `b` when `b` took the hand more recently (`hc`), then by
/// the larger identifier.
pub fn host_yields(a_hc: u64, a_id: &str, b_hc: u64, b_id: &str) -> bool {
    b_hc > a_hc || (b_hc == a_hc && b_id > a_id)
}

/// The session's reference: where it is, learnt at `at_us` of this rule's clock.
#[derive(Clone, Debug, PartialEq)]
pub struct SyncRef {
    pub time: f64,
    pub playing: bool,
    pub at_us: u64,
    pub stamp: Stamp,
}

/// One observation of the local player.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerObs {
    /// Position (s).
    pub time: f64,
    pub paused: bool,
    /// The video is loaded and really advancing (before that nothing is watched or announced).
    pub ready: bool,
    /// A seek is in progress: the position is not a landing yet.
    pub seeking: bool,
}

/// What the local player is asked to do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PlayerCmd {
    Seek(f64),
    Play,
    Pause,
}

/// What a tick did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TickVerdict {
    /// Newer than ours (a missed or lost command): adopted and applied.
    Adopt,
    /// The host's, at our stamp: re-anchors the clock.
    Follow,
    Ignore,
}

/// A local command to send (the caller adds identity and item).
#[derive(Clone, Debug, PartialEq)]
pub struct IntentOut {
    pub action: SyncAction,
    pub time: f64,
    pub playing: bool,
    pub c: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Last {
    time: f64,
    paused: bool,
    at_us: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Seen {
    obs: PlayerObs,
    at_us: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TogetherSync {
    me: String,
    sref: Option<SyncRef>,
    clock: u64,
    now_us: u64,
    seen: Option<Seen>,
    last: Last,
    applying_until_us: u64,
    was_ready: bool,
    /// A seek this rule commanded whose landing has not been observed.
    landing: bool,
    /// Accepted landing error (s, positive = behind the session).
    slack: f64,
    /// Re-seeks made for the current reference after implausible landings.
    reseeks: u32,
    /// The current reference's seek kept missing: no more seeks until the next reference.
    gave_up: bool,
}

impl TogetherSync {
    pub fn new(me: &str) -> TogetherSync {
        TogetherSync {
            me: me.to_string(),
            sref: None,
            clock: 0,
            now_us: 0,
            seen: None,
            last: Last { time: 0.0, paused: true, at_us: 0 },
            applying_until_us: 0,
            was_ready: false,
            landing: false,
            slack: 0.0,
            reseeks: 0,
            gave_up: false,
        }
    }

    /// The only clock this rule sees.
    pub fn advance(&mut self, dt_us: u64) {
        self.now_us = self.now_us.saturating_add(dt_us);
    }

    pub fn now_us(&self) -> u64 {
        self.now_us
    }

    /// The largest clock seen or issued.
    pub fn clock(&self) -> u64 {
        self.clock
    }

    pub fn reference(&self) -> Option<&SyncRef> {
        self.sref.as_ref()
    }

    pub fn attached(&self) -> bool {
        self.seen.is_some()
    }

    /// The last commanded seek kept missing (see [`RESEEK_LIMIT`]).
    pub fn landing_failed(&self) -> bool {
        self.gave_up
    }

    /// The accepted keyframe landing error (s).
    pub fn slack(&self) -> f64 {
        self.slack
    }

    pub fn attach(&mut self, obs: PlayerObs) {
        self.seen = Some(Seen { obs, at_us: self.now_us });
        self.last = Last { time: obs.time, paused: obs.paused, at_us: self.now_us };
        self.was_ready = false;
        self.landing = false;
        self.slack = 0.0;
        self.reseeks = 0;
        self.gave_up = false;
    }

    pub fn detach(&mut self) {
        self.seen = None;
        self.landing = false;
    }

    /// Session left: no reference.
    pub fn reset(&mut self) {
        self.sref = None;
        self.clock = 0;
    }

    /// The stamp of the followed reference, carried in ticks.
    pub fn stamp(&self) -> Stamp {
        self.sref.as_ref().map(|r| r.stamp.clone()).unwrap_or_default()
    }

    fn elapsed_s(&self, since_us: u64) -> f64 {
        self.now_us.saturating_sub(since_us) as f64 / 1e6
    }

    /// The session's position now, or `None` without a reference.
    pub fn position(&self) -> Option<f64> {
        self.sref.as_ref().map(|r| r.time + if r.playing { self.elapsed_s(r.at_us) } else { 0.0 })
    }

    /// The local player's position now, extrapolated from its last observation.
    pub fn player_time(&self) -> Option<f64> {
        self.seen.map(|s| s.obs.time + if s.obs.ready && !s.obs.paused { self.elapsed_s(s.at_us) } else { 0.0 })
    }

    pub fn player_paused(&self) -> Option<bool> {
        self.seen.map(|s| s.obs.paused)
    }

    /// A reference learnt another way (the host's state handed with a grant).
    pub fn adopt(&mut self, time: f64, playing: bool, stamp: Stamp) {
        let stamp = Stamp { c: safe(stamp.c), by: stamp.by };
        self.clock = self.clock.max(stamp.c);
        self.set_reference(SyncRef { time: finite(time), playing, at_us: self.now_us, stamp });
    }

    /// A new reference: a seek for it gets its own [`RESEEK_LIMIT`] retries.
    fn set_reference(&mut self, r: SyncRef) {
        self.sref = Some(r);
        self.reseeks = 0;
        self.gave_up = false;
    }

    /// Another participant's command. True when it becomes the reference (and is applied).
    pub fn on_intent(&mut self, m: &IntentIn, cmds: &mut Vec<PlayerCmd>) -> bool {
        let s = match m.c {
            Some(c) => Stamp { c: safe(c), by: m.id.clone() },
            None => Stamp { c: tick_clock(self.clock), by: m.id.clone() },
        };
        self.clock = self.clock.max(s.c);
        if self.sref.as_ref().is_some_and(|r| !newer(&s, &r.stamp)) {
            return false;
        }
        self.set_reference(SyncRef { time: finite(m.time), playing: m.playing, at_us: self.now_us, stamp: s });
        self.apply(true, cmds);
        true
    }

    /// Another participant's tick.
    pub fn on_tick(&mut self, m: &TickIn, i_am_host: bool, cmds: &mut Vec<PlayerCmd>) -> TickVerdict {
        let s = m.stamp.as_ref().map(|(c, by)| Stamp { c: safe(*c), by: by.clone() });
        if let Some(s) = &s {
            self.clock = self.clock.max(s.c);
        }
        if !m.live {
            return TickVerdict::Ignore;
        }
        let (time, playing, at_us) = (finite(m.time), m.playing, self.now_us);
        let Some(current) = self.sref.as_ref().map(|r| r.stamp.clone()) else {
            if s.is_none() && !m.host {
                return TickVerdict::Ignore;
            }
            self.set_reference(SyncRef { time, playing, at_us, stamp: s.unwrap_or_default() });
            self.apply(true, cmds);
            return TickVerdict::Adopt;
        };
        if let Some(s) = s.as_ref().filter(|s| newer(s, &current)) {
            self.set_reference(SyncRef { time, playing, at_us, stamp: s.clone() });
            self.apply(true, cmds);
            return TickVerdict::Adopt;
        }
        let same = s.as_ref().is_none_or(|s| *s == current);
        if m.host && !i_am_host && same {
            self.sref = Some(SyncRef { time, playing, at_us, stamp: current });
            return TickVerdict::Follow;
        }
        TickVerdict::Ignore
    }

    /// Watches the local player (the web polls every 500 ms): the command to send when the person
    /// just paused, resumed or sought; follows the session otherwise. `online`: the message will
    /// really leave (otherwise the gesture stays local and the session wins on return).
    pub fn poll(&mut self, obs: PlayerObs, host: bool, online: bool, cmds: &mut Vec<PlayerCmd>) -> Option<IntentOut> {
        let now = self.now_us;
        self.seen = Some(Seen { obs, at_us: now });
        let (time, paused) = (obs.time, obs.paused);
        if !obs.ready || obs.seeking {
            if !obs.ready {
                self.was_ready = false;
            }
            self.last = Last { time, paused, at_us: now };
            return None;
        }
        if self.landing && now >= self.applying_until_us {
            // The first settled observation after a seek this rule asked for: how far off it landed.
            self.landing = false;
            self.last = Last { time, paused, at_us: now };
            if let Some(expected) = self.position() {
                let err = expected - time;
                if (-KEYFRAME_AHEAD..=KEYFRAME_SLACK).contains(&err) {
                    // A keyframe away: hold the player there.
                    self.slack = err.max(0.0);
                    self.reseeks = 0;
                } else if self.reseeks < RESEEK_LIMIT {
                    // Too far off to be a keyframe: seek again (the last good slack still applies).
                    self.reseeks += 1;
                    self.apply(true, cmds);
                } else {
                    // Still missing: stop seeking for this reference, keep the last good slack.
                    self.gave_up = true;
                }
            }
            return None;
        }
        if !self.was_ready {
            // The video starts: not a gesture. Join the session if there is one.
            self.was_ready = true;
            self.last = Last { time, paused, at_us: now };
            if !host && self.sref.is_some() {
                self.apply(true, cmds);
            }
            return None;
        }
        let expected = self.last.time + if self.last.paused { 0.0 } else { self.elapsed_s(self.last.at_us) };
        let mut out = None;
        if now > self.applying_until_us {
            let jumped = (time - expected).abs() > JUMP;
            if paused != self.last.paused || jumped {
                if online {
                    self.clock = tick_clock(self.clock);
                    let c = self.clock;
                    self.set_reference(SyncRef { time, playing: !paused, at_us: now, stamp: Stamp { c, by: self.me.clone() } });
                    self.slack = 0.0;
                    let action = if jumped { SyncAction::Seek } else if paused { SyncAction::Pause } else { SyncAction::Play };
                    out = Some(IntentOut { action, time, playing: !paused, c });
                }
            } else if !host && online {
                self.apply(false, cmds);
            }
        }
        self.last = Last { time, paused, at_us: now };
        out
    }

    /// Brings the local player to the reference. `hard`: an explicit command.
    pub fn apply(&mut self, hard: bool, cmds: &mut Vec<PlayerCmd>) {
        let Some(seen) = self.seen.filter(|s| s.obs.ready) else { return };
        let Some(r) = self.sref.clone() else { return };
        let now = self.now_us;
        let expected = r.time + if r.playing { self.elapsed_s(r.at_us) } else { 0.0 };
        let here = self.player_time().unwrap_or(seen.obs.time);
        let diff = here + self.slack - expected;
        let paused = seen.obs.paused;
        let limit = if hard { HARD } else { SOFT };
        if r.playing == paused || (diff.abs() > limit && !self.gave_up) {
            self.applying_until_us = now + SETTLE_US;
        }
        let mut obs = seen.obs;
        let seek = diff.abs() > limit && !self.gave_up;
        if seek {
            cmds.push(PlayerCmd::Seek(expected));
            obs.time = expected;
            self.landing = true;
        } else {
            obs.time = here;
        }
        if r.playing && paused {
            cmds.push(PlayerCmd::Play);
            obs.paused = false;
        } else if !r.playing && !paused {
            cmds.push(PlayerCmd::Pause);
            obs.paused = true;
        }
        // Until the player reports again, it is where it was just told to be: the seek target, or
        // where it already is (the session's position less the accepted slack). Recording the
        // session's position instead would read the slack back as a jump, a seek gesture.
        self.seen = Some(Seen { obs, at_us: now });
        self.last = Last { time: obs.time, paused: !r.playing, at_us: now };
    }
}

/// A peer's clock as a JavaScript number holds it exactly.
fn safe(c: u64) -> u64 {
    c.min(MAX_SAFE_CLOCK)
}

/// The next clock value, never past [`MAX_SAFE_CLOCK`].
pub(crate) fn tick_clock(c: u64) -> u64 {
    c.saturating_add(1).min(MAX_SAFE_CLOCK)
}

fn finite(x: f64) -> f64 {
    if x.is_finite() { x } else { 0.0 }
}

#[cfg(test)]
mod tests {
    //! Ported from the web's `packages/core/test/togethersync.test.ts`, plus the LG keyframe cases.
    use super::*;

    /// A stand-in player: advances with the clock while playing; volume and subtitles are its own.
    /// `gop` is how far before a commanded target a seek lands (0: exact, the web's player).
    #[derive(Clone, Debug)]
    struct FakePlayer {
        pos: f64,
        paused: bool,
        since_us: u64,
        volume: f64,
        subtitle: &'static str,
        seeks: Vec<f64>,
        gop: f64,
    }

    impl FakePlayer {
        fn new() -> Self {
            FakePlayer { pos: 0.0, paused: true, since_us: 0, volume: 0.8, subtitle: "fr", seeks: Vec::new(), gop: 0.0 }
        }
        fn time(&self, now: u64) -> f64 {
            self.pos + if self.paused { 0.0 } else { (now - self.since_us) as f64 / 1e6 }
        }
        fn play(&mut self, now: u64) {
            if self.paused {
                self.since_us = now;
                self.paused = false;
            }
        }
        fn pause(&mut self, now: u64) {
            if !self.paused {
                self.pos = self.time(now);
                self.paused = true;
            }
        }
        fn seek(&mut self, t: f64, now: u64) {
            self.seeks.push(t);
            self.pos = (t - self.gop).max(0.0);
            self.since_us = now;
        }
    }

    struct Member {
        id: &'static str,
        host: bool,
        sync: TogetherSync,
        player: FakePlayer,
        now: u64,
    }

    impl Member {
        fn obs(&self) -> PlayerObs {
            PlayerObs { time: self.player.time(self.now), paused: self.player.paused, ready: true, seeking: false }
        }
        fn run(&mut self, cmds: Vec<PlayerCmd>) {
            for c in cmds {
                match c {
                    PlayerCmd::Seek(t) => self.player.seek(t, self.now),
                    PlayerCmd::Play => self.player.play(self.now),
                    PlayerCmd::Pause => self.player.pause(self.now),
                }
            }
        }
        fn poll(&mut self, online: bool) -> Option<IntentOut> {
            let mut cmds = Vec::new();
            let out = self.sync.poll(self.obs(), self.host, online, &mut cmds);
            self.run(cmds);
            out
        }
        fn on_intent(&mut self, m: &IntentIn) -> bool {
            let mut cmds = Vec::new();
            let r = self.sync.on_intent(m, &mut cmds);
            self.run(cmds);
            r
        }
        fn on_tick(&mut self, m: &TickIn, host: bool) -> TickVerdict {
            let mut cmds = Vec::new();
            let r = self.sync.on_tick(m, host, &mut cmds);
            self.run(cmds);
            r
        }
        fn tick(&self) -> TickIn {
            let s = self.sync.stamp();
            TickIn {
                id: self.id.into(),
                name: self.id.into(),
                thumb: None,
                host: self.host,
                live: true,
                time: self.player.time(self.now),
                playing: !self.player.paused,
                item: None,
                stamp: Some((s.c, s.by)),
                vis: None,
                hc: 0,
                want: false,
            }
        }
        fn intent_of(&self, out: IntentOut) -> IntentIn {
            IntentIn { id: self.id.into(), name: self.id.into(), action: Some(out.action), time: out.time, playing: out.playing, item: None, c: Some(out.c) }
        }
        /// A local gesture, read by the next poll.
        fn gesture(&mut self, act: impl FnOnce(&mut FakePlayer, u64)) -> IntentIn {
            self.at(100);
            let now = self.now;
            act(&mut self.player, now);
            let out = self.poll(true).expect("a gesture is a command");
            self.intent_of(out)
        }
        fn at(&mut self, ms: u64) {
            self.now += ms * 1000;
            self.sync.advance(ms * 1000);
        }
    }

    fn member(id: &'static str, host: bool, playing: bool, start_ms: u64) -> Member {
        let mut sync = TogetherSync::new(id);
        sync.advance(start_ms * 1000);
        let mut m = Member { id, host, sync, player: FakePlayer::new(), now: start_ms * 1000 };
        if playing {
            m.player.play(m.now);
        }
        let obs = m.obs();
        m.sync.attach(obs);
        m.poll(true); // ready: first reading
        m
    }

    fn all(ms: &mut [&mut Member], dt: u64) {
        for m in ms.iter_mut() {
            m.at(dt);
        }
    }

    #[test]
    fn stamps_order_by_clock_then_author_never_newer_than_itself() {
        let s = |c, by: &str| Stamp { c, by: by.into() };
        assert!(newer(&s(2, "a"), &s(1, "z")));
        assert!(newer(&s(2, "b"), &s(2, "a")));
        assert!(!newer(&s(2, "a"), &s(2, "a")));
    }

    #[test]
    fn simultaneous_commands_converge_whatever_the_arrival_order() {
        let t0 = 1_000_000;
        let mut h = member("h", true, false, t0);
        let mut a = member("a", false, false, t0);
        let mut b = member("b", false, false, t0);
        // A pauses at 100 s, B seeks to 300 s, neither having seen the other.
        let ia = a.gesture(|p, now| {
            p.seek(100.0, now);
            p.pause(now);
        });
        let ib = b.gesture(|p, now| p.seek(300.0, now));
        h.at(100);
        assert_eq!(ia.c, ib.c);
        h.on_intent(&ib);
        h.on_intent(&ia);
        a.on_intent(&ib);
        b.on_intent(&ia);
        for m in [&h, &a, &b] {
            assert_eq!(m.sync.stamp(), Stamp { c: ib.c.unwrap(), by: "b".into() });
            assert!((m.player.time(m.now) - 300.0).abs() < 0.9, "{}", m.id);
        }
        // B never went back: the losing command, arriving after, is ignored.
        assert_eq!(b.player.seeks, [300.0]);
    }

    #[test]
    fn a_command_that_saw_the_previous_one_wins_everywhere() {
        let t0 = 2_000_000;
        let mut h = member("h", true, true, t0);
        let mut z = member("z", false, true, t0);
        let mut a = member("a", false, true, t0);
        let iz = z.gesture(|p, now| p.pause(now));
        all(&mut [&mut h, &mut a], 100);
        a.on_intent(&iz);
        h.on_intent(&iz);
        all(&mut [&mut h, &mut z, &mut a], 2000);
        let ia = a.gesture(|p, now| p.play(now));
        all(&mut [&mut h, &mut z], 100);
        assert!(ia.c > iz.c);
        h.on_intent(&ia);
        z.on_intent(&ia);
        for m in [&h, &z, &a] {
            assert_eq!(m.sync.stamp(), Stamp { c: ia.c.unwrap(), by: "a".into() });
            assert!(!m.player.paused);
        }
    }

    #[test]
    fn a_stale_host_tick_is_ignored_no_going_back() {
        let t0 = 3_000_000;
        let mut h = member("h", true, true, t0);
        let mut a = member("a", false, true, t0);
        let stale = h.tick();
        let ia = a.gesture(|p, now| p.seek(500.0, now));
        assert_eq!(a.on_tick(&stale, false), TickVerdict::Ignore);
        assert!((a.player.time(a.now) - 500.0).abs() < 0.2);
        h.at(100);
        h.on_intent(&ia);
        all(&mut [&mut h, &mut a], 1000);
        assert_eq!(a.on_tick(&h.tick(), false), TickVerdict::Follow);
    }

    #[test]
    fn an_older_clients_commands_are_ordered_by_arrival() {
        let mut a = member("a", false, false, 4_000_000);
        let old = |id: &str, action, time| IntentIn { id: id.into(), name: id.into(), action: Some(action), time, playing: false, item: None, c: None };
        assert!(a.on_intent(&old("old", SyncAction::Pause, 10.0)));
        assert!(a.on_intent(&old("old2", SyncAction::Seek, 20.0)));
        assert!((a.player.time(a.now) - 20.0).abs() < 0.1);
    }

    #[test]
    fn a_command_missed_during_a_cut_is_caught_up_by_the_first_tick() {
        let t0 = 5_000_000;
        let mut h = member("h", true, true, t0);
        let mut a = member("a", false, true, t0);
        let mut c = member("c", false, true, t0);
        let ia = a.gesture(|p, now| {
            p.seek(42.0, now);
            p.pause(now);
        });
        h.at(100);
        h.on_intent(&ia);
        all(&mut [&mut h, &mut a], 4000);
        c.at(4100);
        assert_eq!(c.on_tick(&h.tick(), false), TickVerdict::Adopt);
        assert!(c.player.paused);
        assert!((c.player.time(c.now) - 42.0).abs() < 0.5);
        assert_eq!(c.sync.stamp(), h.sync.stamp());
    }

    #[test]
    fn a_command_lost_on_the_way_is_resent_by_its_authors_tick() {
        let t0 = 6_000_000;
        let mut h = member("h", true, true, t0);
        let mut a = member("a", false, true, t0);
        a.gesture(|p, now| p.pause(now));
        h.at(3100);
        a.at(3000);
        assert_eq!(h.on_tick(&a.tick(), true), TickVerdict::Adopt);
        assert!(h.player.paused);
    }

    #[test]
    fn an_offline_gesture_is_neither_sent_nor_stamped_and_the_session_wins_back() {
        let t0 = 7_000_000;
        let mut h = member("h", true, true, t0);
        let mut a = member("a", false, true, t0);
        let ta = a.tick();
        h.on_tick(&ta, true);
        let th = h.tick();
        a.on_tick(&th, false);
        a.at(500);
        assert_eq!(a.poll(false), None);
        let before = a.sync.stamp();
        a.at(500);
        let now = a.now;
        a.player.pause(now);
        assert_eq!(a.poll(false), None);
        assert_eq!(a.sync.stamp(), before);
        h.at(4000);
        a.at(3000);
        let th = h.tick();
        a.on_tick(&th, false);
        a.at(500);
        h.at(500);
        a.poll(true);
        assert!(!a.player.paused);
        assert!((a.player.time(a.now) - h.player.time(h.now)).abs() < 2.1);
    }

    #[test]
    fn a_newcomer_without_a_reference_takes_the_first_host_tick() {
        let t0 = 8_000_000;
        let mut h = member("h", true, false, t0);
        h.player.seek(1200.0, h.now);
        h.player.play(h.now);
        let mut n = TogetherSync::new("n");
        n.advance(t0 * 1000);
        assert_eq!(n.on_tick(&h.tick(), false, &mut Vec::new()), TickVerdict::Adopt);
        n.advance(2_000_000);
        assert!((n.position().unwrap() - 1202.0).abs() < 0.5);
    }

    #[test]
    fn volume_and_subtitles_never_leave() {
        let mut a = member("a", false, true, 9_000_000);
        for v in [0.1, 0.0, 1.0] {
            a.at(500);
            a.player.volume = v;
            a.player.subtitle = if v > 0.0 { "en" } else { "off" };
            assert_eq!(a.poll(true), None);
        }
    }

    #[test]
    fn a_received_command_touches_neither_volume_nor_subtitles_and_carries_only_position_and_play() {
        let t0 = 10_000_000;
        let mut a = member("a", false, false, t0);
        let mut b = member("b", false, false, t0);
        b.player.volume = 0.3;
        let ia = a.gesture(|p, now| {
            p.seek(77.0, now);
            p.play(now);
        });
        b.at(100);
        let out = Message::Intent(Intent { id: ia.id.clone(), name: ia.name.clone(), action: ia.action.unwrap(), time: ia.time, playing: ia.playing, c: ia.c.unwrap(), item: None });
        let json: serde_json::Value = serde_json::from_str(&out.to_json()).unwrap();
        let mut keys: Vec<&String> = json.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(keys, ["action", "c", "id", "name", "playing", "t", "time"]);
        b.on_intent(&ia);
        assert_eq!(b.player.volume, 0.3);
        assert_eq!(b.player.subtitle, "fr");
        assert!((b.player.time(b.now) - 77.0).abs() < 0.5);
    }

    use super::super::wire::{Intent, Message};

    /// LG: a seek lands one GOP early. The landing is measured once and accepted; following the
    /// host never seeks again for it, and no gesture is invented from the early landing.
    #[test]
    fn an_early_keyframe_landing_is_accepted_not_sought_again() {
        let t0 = 11_000_000;
        let mut h = member("h", true, true, t0);
        let mut a = member("a", false, true, t0);
        a.player.gop = 3.0;
        let ih = h.gesture(|p, now| p.seek(600.0, now));
        a.at(100);
        assert!(a.on_intent(&ih));
        assert_eq!(a.player.seeks.len(), 1);
        // The settle window passes; the first settled observation measures the landing (~3 s).
        for _ in 0..20 {
            all(&mut [&mut h, &mut a], 500);
            assert_eq!(a.poll(true), None, "the early landing is not a gesture");
            let th = h.tick();
            a.on_tick(&th, false);
        }
        assert!((a.sync.slack() - 3.0).abs() < 0.1, "{}", a.sync.slack());
        assert_eq!(a.player.seeks.len(), 1, "no seek loop: {:?}", a.player.seeks);
        // A new command seeks again, and its landing is measured afresh.
        let ih = h.gesture(|p, now| p.seek(100.0, now));
        a.at(100);
        a.on_intent(&ih);
        assert_eq!(a.player.seeks.len(), 2);
    }

    /// A landing too far off to be a keyframe (here 30 s early) is never taken as slack: the commanded
    /// seek is retried exactly RESEEK_LIMIT times, then the rule stops seeking for that reference and
    /// keeps the last good slack. A new command starts afresh.
    #[test]
    fn an_implausible_landing_is_retried_exactly_reseek_limit_times_then_left() {
        let t0 = 12_000_000;
        let mut h = member("h", true, true, t0);
        let mut a = member("a", false, true, t0);
        a.player.gop = 30.0;
        let ih = h.gesture(|p, now| p.seek(900.0, now));
        a.at(100);
        a.on_intent(&ih);
        for _ in 0..60 {
            all(&mut [&mut h, &mut a], 500);
            assert_eq!(a.poll(true), None, "a missed landing is not a gesture");
            let th = h.tick();
            a.on_tick(&th, false);
        }
        assert_eq!(a.player.seeks.len(), 1 + RESEEK_LIMIT as usize, "{:?}", a.player.seeks);
        assert_eq!(a.sync.slack(), 0.0, "an implausible error is never accepted as slack");
        assert!(a.sync.landing_failed());
        let ih = h.gesture(|p, now| p.seek(100.0, now));
        a.at(100);
        a.on_intent(&ih);
        assert_eq!(a.player.seeks.len(), 2 + RESEEK_LIMIT as usize);
        assert!(!a.sync.landing_failed());
    }

    /// Once a keyframe slack is accepted, the player sits `slack` behind the session on purpose.
    /// Nothing that re-applies the session without seeking may then read that offset as a jump
    /// (a seek gesture that would drag everyone back): a not-ready blip, an adopted tick, an
    /// intent within HARD.
    #[test]
    fn an_accepted_slack_is_never_read_back_as_a_gesture() {
        let t0 = 13_000_000;
        let mut h = member("h", true, true, t0);
        let mut a = member("a", false, true, t0);
        a.player.gop = 3.0;
        let ih = h.gesture(|p, now| p.seek(600.0, now));
        a.at(100);
        a.on_intent(&ih);
        let settle = |h: &mut Member, a: &mut Member, n: usize| {
            for _ in 0..n {
                all(&mut [&mut *h, &mut *a], 500);
                assert_eq!(a.poll(true), None, "no gesture from the slack");
            }
        };
        settle(&mut h, &mut a, 8);
        assert!((a.sync.slack() - 3.0).abs() < 0.1, "{}", a.sync.slack());
        let seeks = a.player.seeks.len();
        // A not-ready blip (the pipeline rebuffers), then ready again.
        let mut cmds = Vec::new();
        let mut o = a.obs();
        o.ready = false;
        a.sync.poll(o, false, true, &mut cmds);
        a.at(500);
        h.at(500);
        assert_eq!(a.poll(true), None);
        settle(&mut h, &mut a, 4);
        // A tick with a newer stamp at the session's position: adopted, within HARD.
        let mut th = h.tick();
        let (c, _) = th.stamp.clone().unwrap();
        th.stamp = Some((c + 1, "h".into()));
        a.at(10);
        assert_eq!(a.on_tick(&th, false), TickVerdict::Adopt);
        settle(&mut h, &mut a, 4);
        // An intent at the session's position (a play while playing): within HARD.
        let at = h.player.time(h.now);
        let i = IntentIn { id: "h".into(), name: "h".into(), action: Some(SyncAction::Play), time: at, playing: true, item: None, c: Some(c + 2) };
        a.at(10);
        assert!(a.on_intent(&i));
        settle(&mut h, &mut a, 4);
        assert_eq!(a.player.seeks.len(), seeks, "none of these seeks: {:?}", a.player.seeks);
    }

    /// A peer's clock is a JavaScript number: one past 2^53 - 1 is not exact there, so it is read
    /// clamped, and counting from it never overflows.
    #[test]
    fn a_huge_clock_is_clamped_and_never_overflows() {
        let t0 = 14_000_000;
        let mut a = member("a", false, true, t0);
        let i = IntentIn { id: "z".into(), name: "z".into(), action: Some(SyncAction::Play), time: 5.0, playing: true, item: None, c: Some(u64::MAX) };
        a.at(10);
        assert!(a.on_intent(&i));
        assert_eq!(a.sync.clock(), MAX_SAFE_CLOCK);
        let old = IntentIn { c: None, ..i.clone() };
        a.at(10);
        a.on_intent(&old);
        assert_eq!(a.sync.clock(), MAX_SAFE_CLOCK);
        for _ in 0..4 {
            a.at(500);
            a.poll(true);
        }
        let out = a.gesture(|p, now| p.seek(50.0, now));
        assert_eq!(out.c, Some(MAX_SAFE_CLOCK));
    }

    #[test]
    fn host_yields_to_the_more_recent_claim_then_the_larger_id() {
        assert!(host_yields(0, "a-original", 7, "b-relayed"));
        assert!(!host_yields(7, "b-relayed", 0, "a-original"));
        assert!(host_yields(3, "x", 3, "y") != host_yields(3, "y", 3, "x"));
    }
}
