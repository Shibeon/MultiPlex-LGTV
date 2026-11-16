//! **Watch Together's door into the player**, whichever backend plays: what the session observes
//! ([`facts`]) and what it asks ([`perform`]), through the same shared surfaces the HUD and the
//! remote keys use. Not a machine: like [`super::transport`] it reads the session's readout and the
//! adapter and calls the transport verbs, so it is the IO side of `plx_data::together::player`.
//!
//! - **Observed** from `player::state`, the published position, `viewer_paused_for`,
//!   `seek_pending` and `seen_frame`, the readers the HUD draws from, for the Feed and the URI
//!   backend alike. The URI backend reports no presented frame of its own (`seen_frame` is `None`
//!   there); `plx_data::together::player::observation` then takes its `Playing` status as the
//!   picture and reads its `Buffering` as not ready, which costs a rebuffering URI participant
//!   one re-application of the session when it resumes, never a gesture.
//! - **Performed** through `PlayerAdapter::request_seek` (the backend-agnostic seek the scrub
//!   commit takes: a paused film stays paused, the pump owns that) and `set_transport_paused` (the
//!   one door every Play and Pause press goes through). No playback-rate nudging: the LG pipeline
//!   cannot change rate without a reload, so a drift is left or sought (`together::sync`).
//! - **A request is not a gesture.** Nothing here moves the gesture count the application keeps
//!   (`PlayerFacts::gestures`): a change the session asked for is never sent back to it.

use plx_data::together::player::PlayerFacts;
use plx_data::together::sync::PlayerCmd;

/// What the player says about itself now, `gestures` being the count the application keeps of the
/// person's play, pause and seek requests on this playback.
pub fn facts(ps: &crate::route::PlaybackSession, pa: &super::adapter::PlayerAdapter, gestures: u32) -> PlayerFacts {
    PlayerFacts {
        state: super::state(ps),
        position_ns: super::observed_position_ns(ps),
        viewer_paused: super::transport::viewer_paused_for(ps, pa),
        seek_pending: super::seek_pending(ps).is_some(),
        seen_frame: super::seen_frame(ps),
        gestures,
    }
}

/// Perform what the session asks. `false` when the backend refused it (the sync rule asks again
/// once its settle window has passed; a refusal is never read as the person's gesture).
pub fn perform(ps: &mut crate::route::PlaybackSession, pa: &mut super::adapter::PlayerAdapter, cmd: PlayerCmd) -> bool {
    match cmd {
        PlayerCmd::Seek(t) => {
            let ns = if t.is_finite() { (t.max(0.0) * 1e9) as i64 } else { 0 };
            pa.request_seek(ps, ns)
        }
        PlayerCmd::Play => super::transport::set_transport_paused(ps, pa, false),
        PlayerCmd::Pause => super::transport::set_transport_paused(ps, pa, true),
    }
}
