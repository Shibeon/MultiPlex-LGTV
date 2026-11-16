//! **Watch Together against the real Feed pump** (`hostsim`): the session's rule
//! (`plx_data::together::sync`) driving the native-seek rig's engine through the player's together
//! door ([`crate::player::together`]) and reading it back through the readers the HUD uses
//! (`facts` → `observation`). The rig's clock is the Starfish clock callback, advanced by hand
//! while the transport plays; everything between the command and the observation is production
//! code: the seek door, the pump's native seek, the keyframe landing and the transport door.
use std::sync::atomic::Ordering::Relaxed;

use plx_data::together::player::observation;
use plx_data::together::sync::{IntentOut, PlayerCmd, PlayerObs, TogetherSync, KEYFRAME_SLACK};
use plx_data::together::wire::{IntentIn, SyncAction};

use super::native_seek_pump_tests::Rig;
use crate::player::together::{facts, perform};
use plx_play_feed::shared::FEED_SHARED;

const STEP_MS: u32 = 500;
const START_NS: i64 = 60_000_000_000;

/// The rig, the person's rule beside it, and the content clock the rig's sink reports.
struct Member {
    rig: Rig,
    sync: TogetherSync,
    clock_ns: i64,
    presented: i64,
    /// Every command the rule gave, in order.
    log: Vec<PlayerCmd>,
    /// Every intent the rule sent.
    sent: Vec<IntentOut>,
}

impl Member {
    /// A participant's player playing at 1:00 with a picture up, attached and past its start.
    fn playing() -> Member {
        let rig = Rig::new(None);
        let mut m = Member { rig, sync: TogetherSync::new("moi"), clock_ns: START_NS, presented: 1, log: Vec::new(), sent: Vec::new() };
        m.present();
        for _ in 0..3 {
            m.rig.frame();
        }
        let first = m.obs();
        assert!(first.ready && !first.seeking, "{first:?}");
        m.sync.attach(first);
        m.period();
        m
    }

    fn obs(&self) -> PlayerObs {
        observation(&facts(&self.rig.ps, &self.rig.pa)).expect("a playback to observe")
    }

    /// The sink presents a frame at the clock.
    fn present(&mut self) {
        let epoch = self.rig.eng().native_epoch;
        plx_play_feed::sf_event::sf_on_event(epoch, 0, self.clock_ns, std::ptr::null());
        self.presented += 1;
        FEED_SHARED.frames.store(self.presented as i32, Relaxed);
    }

    /// One observation period (the store samples the player every 500 ms): the pump runs its
    /// frames, the clock moves while the transport plays and no seek is in flight, the rule polls,
    /// and what it asks is performed through the together door.
    fn period(&mut self) -> Option<IntentOut> {
        let f = facts(&self.rig.ps, &self.rig.pa);
        if !f.viewer_paused && !f.seek_pending && f.state == plx_playback_contract::readout::PlaybackState::Playing {
            self.clock_ns += i64::from(STEP_MS) * 1_000_000;
            self.present();
        }
        self.rig.now += STEP_MS - 16 * 4;
        for _ in 0..4 {
            self.rig.frame();
        }
        self.sync.advance(u64::from(STEP_MS) * 1_000);
        let mut cmds = Vec::new();
        let out = self.sync.poll(self.obs(), false, true, &mut cmds);
        self.perform(cmds);
        if let Some(o) = &out {
            self.sent.push(o.clone());
        }
        out
    }

    fn perform(&mut self, cmds: Vec<PlayerCmd>) {
        for cmd in cmds {
            let _ = perform(&mut self.rig.ps, &mut self.rig.pa, cmd);
            self.log.push(cmd);
        }
    }

    /// The host's command reaches this member.
    fn intent(&mut self, action: SyncAction, time: f64, playing: bool, c: u64) {
        let m = IntentIn { id: "hote".into(), name: "Hôte".into(), action: Some(action), time, playing, item: None, c: Some(c) };
        let mut cmds = Vec::new();
        assert!(self.sync.on_intent(&m, &mut cmds), "the host's command is the newer one");
        self.perform(cmds);
    }

    fn seeks(&self) -> usize {
        self.log.iter().filter(|c| matches!(c, PlayerCmd::Seek(_))).count()
    }

    fn position_s(&self) -> f64 {
        self.obs().time
    }
}

/// The host seeks to 35:18; the LG pipeline lands on the keyframe 2 s before, and the session's
/// clock has moved on while it primed. The rule measures that landing once, holds the player
/// there (inside `KEYFRAME_SLACK`), and seeks no more: no rate nudge, no re-seek, and nothing the
/// session asked is sent back to it.
#[test]
fn a_session_seek_lands_on_the_keyframe_and_is_held_there_without_a_second_seek() {
    let _serial = plx_base::testlock::serial();
    let mut m = Member::playing();
    let target = 2118.0;
    m.intent(SyncAction::Seek, target, true, 5);
    assert_eq!(m.log, [PlayerCmd::Seek(target)], "one seek, through the together door");
    assert!(m.obs().seeking, "the seek is in flight: not a landing yet");
    // The pump arms the native seek; the demux finds the keyframe 2 s before the target.
    for _ in 0..3 {
        m.period();
    }
    assert!(m.rig.eng().flushed, "the pump armed the native seek");
    let keyframe_ns = 2_116_000_000_000;
    m.rig.land_keyframe_with_key(keyframe_ns, 80, 1);
    for _ in 0..4 {
        m.period();
    }
    // The first frame after the seek: the clock reports the keyframe.
    m.clock_ns = keyframe_ns;
    m.present();
    for _ in 0..20 {
        m.period();
    }
    assert_eq!(m.seeks(), 1, "held on the keyframe: {:?}", m.log);
    assert!(!m.sync.landing_failed());
    let slack = m.sync.slack();
    assert!(slack > 2.0 && slack <= KEYFRAME_SLACK, "the landing is held as slack: {slack}");
    assert!((m.position_s() - (keyframe_ns as f64 / 1e9 + 10.0)).abs() < 0.6, "the player plays on from the keyframe: {}", m.position_s());
    assert!(m.sent.is_empty(), "the session's own seek is not sent back: {:?}", m.sent);
    assert_eq!(crate::route::transport_gestures(&m.rig.ps), 0, "a session's request is not a gesture");
}

/// The host pauses: the participant's player pauses through the one transport door, and the change
/// is not sent back. The person's own Pause (counted by the application's transport door) is sent,
/// once; a pause nobody counted (the lifecycle's own) is not the person's: the rule puts the
/// session's Play back instead of sending it. (The rig's engine is not graded on accepting that
/// Play: its native resume needs a Load this rig does not run.)
#[test]
fn a_session_pause_is_not_echoed_and_only_the_persons_own_pause_is_sent() {
    let _serial = plx_base::testlock::serial();
    {
        let mut m = Member::playing();
        FEED_SHARED.reset_hls_clock_for_test();
        let here = m.position_s();
        m.intent(SyncAction::Pause, here, false, 5);
        assert_eq!(m.log, [PlayerCmd::Pause]);
        for _ in 0..8 {
            m.period();
        }
        assert!(facts(&m.rig.ps, &m.rig.pa).viewer_paused, "the player paused");
        assert!(m.sent.is_empty(), "the session's pause is not sent back: {:?}", m.sent);
        assert_eq!(m.log, [PlayerCmd::Pause], "and nothing else was asked");
    }
    {
        // The person pauses (the application's transport door counts it): sent, once.
        let mut m = Member::playing();
        FEED_SHARED.reset_hls_clock_for_test();
        crate::route::note_gesture(&mut m.rig.ps);
        assert!(crate::player::transport::set_transport_paused(&mut m.rig.ps, &mut m.rig.pa, true));
        for _ in 0..8 {
            m.period();
        }
        assert_eq!(m.sent.len(), 1, "{:?}", m.sent);
        assert_eq!((m.sent[0].action, m.sent[0].playing), (SyncAction::Pause, false));
        assert!(m.log.is_empty(), "the person's pause stands: {:?}", m.log);
    }
    {
        // A pause nobody counted: corrected back to the session, never sent.
        let mut m = Member::playing();
        m.intent(SyncAction::Play, m.position_s(), true, 5);
        m.log.clear();
        for _ in 0..4 {
            m.period();
        }
        FEED_SHARED.reset_hls_clock_for_test();
        assert!(crate::player::transport::set_transport_paused(&mut m.rig.ps, &mut m.rig.pa, true));
        for _ in 0..8 {
            m.period();
        }
        assert!(m.sent.is_empty(), "an uncounted change is not the person's: {:?}", m.sent);
        assert_eq!(m.log.first(), Some(&PlayerCmd::Play), "it is put back to the session: {:?}", m.log);
    }
}

/// A claim's presentation hold is up (this member picked a track and its PMS half flies) and owes
/// the stream a Play back at the landing. The session's Pause, performed through the one transport
/// door, is a transport press like the viewer's own (`claim_hold::note_user_transport`): it forgets
/// that restore, so the landing leaves this member paused with the session instead of playing on.
#[test]
fn a_session_pause_during_a_claim_hold_stands_past_the_landing() {
    let _serial = plx_base::testlock::serial();
    let mut m = Member::playing();
    FEED_SHARED.reset_hls_clock_for_test();
    crate::player::claim_hold::hold_for_test(9, true);
    let paused = perform(&mut m.rig.ps, &mut m.rig.pa, PlayerCmd::Pause);
    let restore = crate::player::claim_hold::take(9);
    crate::player::claim_hold::clear();
    assert!(paused, "the session's Pause was taken");
    assert!(!restore, "the landing owes no Play back over the session's Pause");
}
