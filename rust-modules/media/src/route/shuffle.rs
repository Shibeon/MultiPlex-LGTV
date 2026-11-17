//! **Shuffle**: play a show or a season in an order the server draws at random.
//!
//! A Shuffle is an ordinary play through the common request door, whose request starts as a
//! `ShuffleState::Draw` of the show or season ([`request_play_shuffle`]): it is superseded,
//! cancelled, held behind a preview and retried exactly like every other play, because it IS one.
//! Its one difference is on the resolve worker, where the episode is not known until the server has
//! drawn the order:
//!
//! 1. [`draw`] POSTs the container's queue with `shuffle=1`
//!    ([`plx_plex::plex::Client::create_shuffled_play_queue`]), leaves the specials out
//!    ([`super::queue_navigation::numbered_episodes`]) and picks the row it starts on; the resolve
//!    then plans THAT row, continuing the drawn queue instead of POSTing a new one, so Up Next,
//!    Previous and Next walk the draw.
//! 2. On the main thread, the landing ([`land`]) makes the request the drawn episode's (a
//!    `ShuffleState::Walk`, which a retry or a quality change continues) and installs the Info
//!    card's descriptor for it.
use super::decision::{land_drawn_request, queue_machine_id, PlaybackSession};
use super::plan::ResolveEnv;
use super::queue_navigation::QueueContinuation;
use plx_plex::plex::{QueueRow, ServerId};

/// MAIN THREAD, NON-BLOCKING. Shuffle the show or season `container_rk` on `sid`: the request a
/// page's Shuffle makes, through the same door as a Play (`request_play`'s contract). `title` is
/// the HUD title until the drawn episode lands.
pub fn request_play_shuffle(
    ps: &mut PlaybackSession,
    meta: &mut plx_data::stores::metadata::MetadataStore,
    caps: std::sync::Arc<plx_platform::devcaps::Caps>,
    sid: ServerId,
    container_rk: &str,
    title: &str,
) -> bool {
    super::decision::request_shuffle_draw(ps, meta, caps, sid, container_rk, title, "")
}

/// The queue and the row a draw's answer gives, or `None` for an answer with nothing to play.
/// `answer` is the queue's `(id, selected item id, window)`. Its own function so the choice is
/// graded without a server.
fn start_of(answer: Option<(i64, i64, Vec<QueueRow>)>) -> Option<(QueueContinuation, QueueRow)> {
    let (id, selected_item_id, rows) = answer?;
    QueueContinuation::shuffle_start(id, selected_item_id, rows)
}

/// The drawn start, installed on the resolve's environment: the row is resolved against the
/// drawn queue (and keyed off its show), never against a new one.
fn install(env: &mut ResolveEnv, (queue, row): (QueueContinuation, QueueRow)) -> QueueRow {
    env.queue_continuation = Some(queue);
    env.queue_show_rk = row.show_rk.clone();
    row
}

/// **RESOLVE WORKER.** Draw `container_rk`'s shuffled queue on `env.sid` and install it on `env`;
/// the row the resolve plays, or `None` when there is nothing to play (no client, no machine id,
/// a refused or empty queue). The machine id is found by the queue's own three rungs
/// (`decision::queue_machine_id`), the session cached one being `env.machine_id`.
pub(super) fn draw(env: &mut ResolveEnv, container_rk: &str) -> Option<QueueRow> {
    let client = plx_plex::plex::client_for(env.sid)?;
    let (machine_id, _) = queue_machine_id(client, &env.machine_id);
    if machine_id.is_empty() {
        crate::player::log("shuffle: no machineIdentifier (skip)");
        return None;
    }
    let session = super::plan::new_sess(container_rk);
    let start = start_of(client.create_shuffled_play_queue(&machine_id, container_rk, &session)
        .map(|queue| (queue.id, queue.selected_item_id, queue.items)));
    crate::player::log(&format!(
        "shuffle: container={container_rk} {}",
        start.as_ref().map_or_else(|| "no queue".to_owned(),
            |(_, row)| format!("first S{}E{} rk={}", row.season, row.index, row.rk))
    ));
    start.map(|start| install(env, start))
}

/// The Info card's descriptor of a drawn episode, from its queue row: the page under the player is
/// the SHOW's (or the season's), so `current()` cannot describe the leaf, as for an episode played
/// off a show page. The row carries no summary, year or rating; those stay empty.
fn now_playing_of(row: &QueueRow) -> plx_data::metadata::NowPlaying {
    let episode = row.kind == "episode";
    plx_data::metadata::NowPlaying {
        is_episode: episode,
        is_real_episode: episode,
        is_movie: false,
        title: if row.show_title.is_empty() { row.title.clone() } else { row.show_title.clone() },
        ep_title: row.title.clone(),
        season: row.season,
        index: row.index,
        summary: String::new(),
        year: 0,
        dur_ms: row.dur_ms,
        rating: String::new(),
        thumb: row.thumb.clone(),
        detail_rk: row.show_rk.clone(),
    }
}

/// **MAIN THREAD, the landing of a draw** (`decision::pump_play`, before the plan is installed):
/// the request becomes the drawn episode's, the Info card describes it, and the start position is
/// the episode's own resume point unless the play already carries one. Returns that position.
pub(super) fn land(
    ps: &mut PlaybackSession,
    meta: &mut plx_data::stores::metadata::MetadataStore,
    row: &QueueRow,
    resume_ns: i64,
) -> i64 {
    land_drawn_request(ps, row);
    meta.run(plx_data::stores::metadata::MetadataCmd::SetNowPlaying(Some(now_playing_of(row))));
    if resume_ns > 0 {
        resume_ns
    } else {
        plx_data::metadata::resume_ns(row.resume_ms, row.dur_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn episode(item_id: i64, rk: &str, season: i64) -> QueueRow {
        QueueRow {
            item_id,
            sid: ServerId::from_raw(1),
            rk: rk.into(),
            kind: "episode".into(),
            season,
            index: 1,
            ..Default::default()
        }
    }

    fn answer(selected_item_id: i64, items: Vec<QueueRow>) -> Option<(i64, i64, Vec<QueueRow>)> {
        Some((9, selected_item_id, items))
    }

    fn rks(queue: &QueueContinuation) -> Vec<String> {
        queue.resolve().rows.iter().map(|row| row.rk.clone()).collect()
    }

    /// The draw starts on the row the server selected, and the queue the playback continues is the
    /// server's queue, in the server's order: Up Next is the row after it.
    #[test]
    fn a_shuffle_starts_on_the_selected_row_of_the_drawn_queue() {
        let (queue, row) = start_of(answer(3, vec![episode(3, "c", 2), episode(1, "a", 1), episode(2, "b", 1)])).unwrap();
        assert_eq!(row.rk, "c");
        assert_eq!(rks(&queue), ["c", "a", "b"]);
        let landed = queue.resolve();
        assert_eq!((landed.id.as_str(), landed.item_id.as_str()), ("9", "3"));
        assert_eq!(landed.up_next.map(|next| next.rk), Some("a".to_owned()));
    }

    /// **No specials in a show's shuffle**: season-0 rows leave the queue, and a draw the server
    /// opened on a special starts on the next numbered episode instead.
    #[test]
    fn a_shows_shuffle_leaves_the_specials_out() {
        let (queue, row) = start_of(answer(5, vec![
            episode(5, "sp1", 0), episode(1, "a", 1), episode(6, "sp2", 0), episode(2, "b", 3),
        ])).unwrap();
        assert_eq!(row.rk, "a");
        assert_eq!(rks(&queue), ["a", "b"]);
        // a selected special at the END of the window starts on the episode before it
        let (_, row) = start_of(answer(5, vec![episode(1, "a", 1), episode(5, "sp1", 0)])).unwrap();
        assert_eq!(row.rk, "a");
    }

    /// A show (or the Specials season) that holds nothing but specials still shuffles them.
    #[test]
    fn a_queue_of_specials_alone_is_kept() {
        let (queue, row) = start_of(answer(6, vec![episode(5, "sp1", 0), episode(6, "sp2", 0)])).unwrap();
        assert_eq!(row.rk, "sp2");
        assert_eq!(rks(&queue), ["sp1", "sp2"]);
    }

    /// Nothing to play is no start: a failed POST, an empty queue, a queue without an id.
    #[test]
    fn an_answer_with_nothing_to_play_is_no_start() {
        assert!(start_of(None).is_none());
        assert!(start_of(answer(1, Vec::new())).is_none());
        assert!(start_of(Some((0, 1, vec![episode(1, "a", 1)]))).is_none());
    }

    /// The drawn start is what the resolve then plans: the row is matched by the drawn queue (so
    /// `build_stream` continues it rather than POSTing), keyed off the row's show.
    #[test]
    fn the_drawn_row_resolves_against_the_drawn_queue() {
        let mut env = super::super::decision::shuffle_test_env();
        let mut row = episode(3, "c", 2);
        row.show_rk = "50".into();
        let start = start_of(answer(3, vec![row, episode(1, "a", 1)])).unwrap();
        let played = install(&mut env, start);
        assert_eq!(played.rk, "c");
        assert_eq!(env.queue_show_rk, "50");
        assert!(env.queue_continuation.as_ref().is_some_and(|queue| queue.matches(played.sid, "c")));
    }
}
