//! Queue transport policy and existing-queue window refresh for the episode controls.
use super::{cur_rk, cur_sid, pq_id, pq_item_id, with_queue, PlaybackSession};
use plx_plex::plex::{queue_index_of, QueueRow, ServerId};
use std::sync::Arc;
use std::time::Duration;

/// The longest the window re-read waits for the outgoing episode's `stopped` report to land
/// (see [`QueueContinuation::refreshed`]): a stop on a slow server must not hold a start hostage.
const STOP_WAIT: Duration = Duration::from_millis(1500);

/// An owned continuation of the server's existing queue, captured before request_play retires
/// the outgoing state. Shared across resolve snapshots without copying a show's rows per frame.
#[derive(Clone)]
pub struct QueueContinuation {
    id: String,
    rows: Arc<Vec<QueueRow>>,
    selected: usize,
    /// How many `state=stopped` reports had been announced when the continuation was captured,
    /// which is after the outgoing episode's own (the handoff stops it before it requests the
    /// next): the window re-read waits for these to land, so the rows it reads carry the position
    /// that stop reported. `0` for a continuation built from rows alone.
    stop: u64,
    /// The queue is a shuffle's: every window of it, the re-reads included, keeps the specials out
    /// ([`numbered_episodes`]).
    numbered_only: bool,
    /// The rows are the answer of the POST that just created the queue (a Shuffle's draw): nothing
    /// has played since, so [`Self::refreshed`] has nothing newer to read and does not re-read it.
    fresh: bool,
}

/// **A shuffle leaves the specials out**: when `rows` hold an episode of a numbered season, the
/// season-0 episodes (Plex's "Specials") are dropped from them. A window of specials alone is kept
/// whole, so a show that has nothing else (or the Specials season's own Shuffle) still plays. Rows that are not episodes are never touched.
///
/// The app cannot tell from the documentation whether a show-keyed PMS queue holds the specials
/// (`docs/pms-api.md`, "A shuffled queue"), so it does not lean on either answer: this is applied to
/// every window of a shuffled queue the app holds, and is a no-op on a queue that never had any.
pub(super) fn numbered_episodes(mut rows: Vec<QueueRow>) -> Vec<QueueRow> {
    let special = |row: &QueueRow| row.kind == "episode" && row.season == 0;
    if rows.iter().any(|row| row.kind == "episode" && row.season > 0) {
        rows.retain(|row| !special(row));
    }
    rows
}

impl QueueContinuation {
    /// Where `target` sits in queue `id`'s `rows`, if it is exactly that queue item.
    fn select(id: &str, rows: &[QueueRow], target: &QueueRow) -> Option<usize> {
        if id.is_empty() || target.item_id == 0 {
            return None;
        }
        let selected = queue_index_of(rows, target.item_id, target.sid, &target.rk)?;
        let row = &rows[selected];
        if row.item_id != target.item_id || row.sid != target.sid || row.rk != target.rk {
            return None;
        }
        Some(selected)
    }

    pub(super) fn from_queue(id: String, rows: Vec<QueueRow>, target: &QueueRow) -> Option<Self> {
        let selected = Self::select(&id, &rows, target)?;
        Some(Self { id, rows: Arc::new(rows), selected, stop: 0, numbered_only: false, fresh: false })
    }

    /// The continuation a SHUFFLED play starts on, and the row it starts: the queue the server
    /// drew (`id`, its window `rows`), the specials left out ([`numbered_episodes`]), on the row the
    /// server SELECTED, or, when that row was a special, on the first numbered episode after it in
    /// the draw (before it when none follows). `None` for a queue that is empty or has no id.
    pub(super) fn shuffle_start(id: i64, selected_item_id: i64, rows: Vec<QueueRow>) -> Option<(Self, QueueRow)> {
        if id <= 0 {
            return None;
        }
        let at = rows.iter().position(|row| row.item_id == selected_item_id).unwrap_or(0);
        let kept = numbered_episodes(rows.clone());
        let survives = |row: &QueueRow| kept.iter().any(|k| k.item_id == row.item_id && k.rk == row.rk);
        let target = rows.iter().skip(at).chain(rows.iter().take(at).rev()).find(|row| survives(row))?.clone();
        let queue = Self::from_queue(id.to_string(), kept, &target)?;
        Some((Self { numbered_only: true, fresh: true, ..queue }, target))
    }

    /// This continuation, its window re-read set to wait for the stops announced up to `stop`.
    #[cfg(test)]
    pub(super) fn awaiting(self, stop: u64) -> Self {
        Self { stop, ..self }
    }

    /// The stop count the window re-read waits for.
    #[cfg(test)]
    pub(super) fn stop(&self) -> u64 {
        self.stop
    }

    /// The live queue is lent, not owned, so its rows are copied only once `target` is found.
    pub(super) fn capture(ps: &PlaybackSession, target: &QueueRow) -> Option<Self> {
        with_queue(ps, |rows| {
            let id = pq_id(ps);
            let selected = Self::select(&id, rows, target)?;
            Some(Self {
                id,
                rows: Arc::new(rows.to_vec()),
                selected,
                stop: super::decision::timeline_stops_announced(),
                numbered_only: super::decision::request_shuffled(ps),
                fresh: false,
            })
        })
    }

    /// The live queue continued on the row NOW playing, when that row is `rk` on `sid`: what a
    /// retry of a shuffled playback resolves against instead of POSTing a new queue
    /// (`decision::request_play_with_queue`).
    pub(super) fn current(ps: &PlaybackSession, sid: ServerId, rk: &str) -> Option<Self> {
        let row = with_queue(ps, |rows| current_index(ps, rows).map(|i| rows[i].clone()))?;
        if row.sid != sid || row.rk != rk {
            return None;
        }
        Self::capture(ps, &row)
    }

    pub(super) fn matches(&self, sid: ServerId, rk: &str) -> bool {
        self.rows[self.selected].sid == sid && self.rows[self.selected].rk == rk
    }

    fn with_window(&self, rows: Vec<QueueRow>) -> Self {
        let rows = if self.numbered_only { numbered_episodes(rows) } else { rows };
        Self::from_queue(self.id.clone(), rows, &self.rows[self.selected])
            .map(|window| Self { stop: self.stop, numbered_only: self.numbered_only, ..window })
            .unwrap_or_else(|| self.clone())
    }

    /// This queue re-read around its selected item.
    ///
    /// **After the outgoing episode's `stopped` report has landed**: the rows carry each episode's
    /// `viewOffset`, and a handoff (Previous, Next) re-reads them while that report is still on its
    /// way, so a row read before it lands holds the position from before the viewer left. Stepping
    /// back to the episode just left would then resume it from that stale point, and its next
    /// report would write that over the real one. The wait is the shared stop fence
    /// (`decision::TIMELINE_STOP_FENCE`), bounded by [`STOP_WAIT`].
    ///
    /// A queue a Shuffle has just drawn ([`Self::shuffle_start`]) is returned as it is: the POST's
    /// answer is already the window around its selected row, and no stop can have changed it.
    pub(super) fn refreshed(&self, client: &plx_plex::plex::Client) -> Self {
        self.refreshed_within(client, STOP_WAIT)
    }

    pub(super) fn refreshed_within(&self, client: &plx_plex::plex::Client, bound: Duration) -> Self {
        if self.fresh {
            return self.clone();
        }
        super::decision::wait_for_timeline_stop(self.stop, bound);
        client
            .play_queue_window(&self.id, self.rows[self.selected].item_id)
            .map(|rows| self.with_window(rows))
            .unwrap_or_else(|| self.clone())
    }

    pub(super) fn resolve(&self) -> super::decision::QueueInfo {
        super::decision::QueueInfo {
            id: self.id.clone(),
            item_id: self.rows[self.selected].item_id.to_string(),
            up_next: self
                .rows
                .get(self.selected + 1)
                .and_then(super::plan::up_next_of),
            rows: self.rows.as_ref().clone(),
            machine_id: String::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueStep {
    Previous,
    Next,
}

/// The row now playing among `rows`, by the queue's identity rule (item id, rating key as the
/// fallback).
fn current_index(ps: &PlaybackSession, rows: &[QueueRow]) -> Option<usize> {
    queue_index_of(rows, pq_item_id(ps).parse().unwrap_or(0), cur_sid(ps), &cur_rk(ps))
}

fn neighbor(rows: &[QueueRow], current: usize, step: QueueStep) -> Option<&QueueRow> {
    let index = match step {
        QueueStep::Previous => current.checked_sub(1)?,
        QueueStep::Next => current.checked_add(1)?,
    };
    rows.get(index)
}

/// Clone before teardown/request_play can retire the outgoing session's queue.
pub fn queue_neighbor(ps: &PlaybackSession, step: QueueStep) -> Option<QueueRow> {
    with_queue(ps, |rows| neighbor(rows, current_index(ps, rows)?, step).cloned())
}

/// Per-frame availability without copying either item's strings into the HUD.
pub fn queue_neighbors(ps: &PlaybackSession) -> (bool, bool) {
    with_queue(ps, |rows| {
        let Some(index) = current_index(ps, rows) else {
            return (false, false);
        };
        (index > 0, index + 1 < rows.len())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`neighbor`] of the row `item_id`/`rk` names, located as [`current_index`] locates it.
    fn neighbor_of<'a>(rows: &'a [QueueRow], item_id: i64, sid: ServerId, rk: &str, step: QueueStep)
        -> Option<&'a QueueRow> {
        neighbor(rows, queue_index_of(rows, item_id, sid, rk)?, step)
    }

    #[test]
    fn queue_navigation_queue_handoff_retains_the_predecessor_and_original_queue_ids() {
        let sid = ServerId::from_raw(1);
        let rows: Vec<_> = [11, 12]
            .into_iter()
            .map(|item_id| QueueRow {
                item_id,
                sid,
                rk: item_id.to_string(),
                kind: "episode".into(),
                ..Default::default()
            })
            .collect();
        // A fresh PlayQueue POST for item 12 can return only [12]. The continuation must carry
        // [11, 12] instead, or Previous on the newly started item has nowhere to go.
        let handoff = QueueContinuation::from_queue("queue-7".into(), rows.clone(), &rows[1]).unwrap();
        let landed = handoff.resolve();
        assert_eq!(landed.id, "queue-7");
        assert_eq!(landed.item_id, "12");
        assert_eq!(landed.rows.len(), 2);
        assert_eq!(
            neighbor_of(&landed.rows, 12, sid, "12", QueueStep::Previous)
                .unwrap()
                .rk,
            "11"
        );
        assert!(handoff.matches(sid, "12"));
        assert!(!handoff.matches(ServerId::from_raw(2), "12"));
        assert!(!handoff.matches(sid, "11"));
        let back = QueueContinuation::from_queue(landed.id, landed.rows.clone(), &landed.rows[0])
            .unwrap()
            .resolve();
        assert_eq!(back.item_id, "11");
        assert_eq!(back.up_next.unwrap().rk, "12");
    }

    #[test]
    fn queue_navigation_queue_window_moves_forward_without_losing_the_selected_item() {
        let sid = ServerId::from_raw(1);
        let rows: Vec<_> = [11, 12, 13]
            .into_iter()
            .map(|item_id| QueueRow {
                item_id,
                sid,
                rk: item_id.to_string(),
                kind: "episode".into(),
                ..Default::default()
            })
            .collect();
        let handoff =
            QueueContinuation::from_queue("queue-7".into(), rows[..2].to_vec(), &rows[1]).unwrap();
        let landed = handoff.with_window(rows.clone()).resolve();
        assert_eq!(landed.rows.len(), 3);
        assert_eq!(landed.item_id, "12");
        assert_eq!(landed.up_next.unwrap().rk, "13");
        // Empty/expired/wrong-item windows cannot discard the known predecessor or selection.
        assert_eq!(handoff.with_window(Vec::new()).resolve().rows.len(), 2);
        assert_eq!(handoff.with_window(rows[2..].to_vec()).resolve().item_id, "12");
    }

    #[test]
    fn queue_navigation_neighbors_use_queue_identity_and_never_wrap() {
        let sid = ServerId::from_raw(1);
        let rows: Vec<_> = [11, 12, 13]
            .into_iter()
            .map(|item_id| QueueRow {
                item_id,
                sid,
                rk: "repeated-rating-key".into(),
                ..Default::default()
            })
            .collect();
        assert_eq!(
            neighbor_of(&rows, 12, sid, "repeated-rating-key", QueueStep::Previous)
                .unwrap()
                .item_id,
            11
        );
        assert_eq!(
            neighbor_of(&rows, 12, sid, "repeated-rating-key", QueueStep::Next)
                .unwrap()
                .item_id,
            13
        );
        assert!(neighbor_of(&rows, 11, sid, "repeated-rating-key", QueueStep::Previous).is_none());
        assert!(neighbor_of(&rows, 13, sid, "repeated-rating-key", QueueStep::Next).is_none());
        assert!(neighbor_of(&rows, 0, sid, "absent", QueueStep::Next).is_none());
    }

    fn episode(item_id: i64, season: i64) -> QueueRow {
        QueueRow {
            item_id,
            sid: ServerId::from_raw(1),
            rk: item_id.to_string(),
            kind: "episode".into(),
            season,
            ..Default::default()
        }
    }

    /// **Every window of a shuffled queue leaves the specials out**, the re-reads included: the
    /// server's window around the playing row may hold season-0 rows, and Up Next must not land on
    /// one. An ordinary queue's window is kept as the server sent it.
    #[test]
    fn queue_navigation_a_shuffled_queues_window_reread_drops_the_specials() {
        let (queue, row) = QueueContinuation::shuffle_start(9, 2, vec![episode(2, 1), episode(3, 2)]).unwrap();
        assert_eq!(row.item_id, 2);
        let reread = queue.with_window(vec![episode(2, 1), episode(7, 0), episode(3, 2)]).resolve();
        let ids: Vec<i64> = reread.rows.iter().map(|row| row.item_id).collect();
        assert_eq!(ids, [2, 3]);
        assert_eq!(reread.up_next.map(|next| next.rk), Some("3".to_owned()));
        let ordinary = QueueContinuation::from_queue("9".into(), vec![episode(2, 1)], &episode(2, 1)).unwrap();
        let reread = ordinary.with_window(vec![episode(2, 1), episode(7, 0)]).resolve();
        assert_eq!(reread.rows.len(), 2, "an ordinary queue keeps what the server sent");
    }

    #[test]
    fn queue_navigation_numbered_episodes_keeps_a_queue_of_specials_and_non_episodes() {
        let specials = numbered_episodes(vec![episode(1, 0), episode(2, 0)]);
        assert_eq!(specials.len(), 2);
        let movie = QueueRow { item_id: 5, kind: "movie".into(), ..Default::default() };
        let mixed = numbered_episodes(vec![movie, episode(1, 0), episode(2, 4)]);
        let ids: Vec<i64> = mixed.iter().map(|row| row.item_id).collect();
        assert_eq!(ids, [5, 2]);
    }
}
