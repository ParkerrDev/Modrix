// SPDX-License-Identifier: GPL-2.0-only
//! Segment allocation: hands idle connections contiguous runs of incomplete
//! pieces to fetch with `Range` requests.
//!
//! A work-queue model (rather than a static N-way carve) so it resumes cleanly
//! even if the connection count changed between runs: a worker simply asks for
//! the next incomplete-and-unassigned run. Runs are length-capped so several
//! workers get work concurrently and an interrupted run costs little re-download.

/// Never hand a single connection more than this many pieces at once, so a crash
/// re-downloads at most this much per in-flight run.
const MAX_RUN_PIECES: usize = 64;

/// Tracks which pieces are done and which are currently assigned to a worker.
#[derive(Debug)]
pub(crate) struct SegmentMap {
    done: Vec<bool>,
    assigned: Vec<bool>,
    run_cap: usize,
}

impl SegmentMap {
    /// Build from a resumed/fresh `done` vector and the connection count (which
    /// tunes how finely work is handed out).
    pub(crate) fn new(done: Vec<bool>, connections: u8) -> Self {
        let count = done.len();
        let conns = usize::from(connections.max(1));
        // Enough runs for ~2 per connection, clamped to keep re-download bounded.
        let run_cap = count
            .div_ceil(conns.saturating_mul(2))
            .clamp(1, MAX_RUN_PIECES);
        Self {
            assigned: vec![false; count],
            done,
            run_cap,
        }
    }

    /// Whether every piece is complete.
    pub(crate) fn is_complete(&self) -> bool {
        self.done.iter().all(|&d| d)
    }

    /// Count of completed pieces.
    pub(crate) fn done_count(&self) -> usize {
        self.done.iter().filter(|&&d| d).count()
    }

    /// A snapshot of the done vector (for the control file).
    pub(crate) fn snapshot(&self) -> Vec<bool> {
        self.done.clone()
    }

    /// Claim the next contiguous run of incomplete, unassigned pieces (length
    /// capped), marking them assigned. Returns the `[start, end)` piece range.
    pub(crate) fn next_run(&mut self) -> Option<(usize, usize)> {
        let start = (0..self.done.len()).find(|&i| self.is_free(i))?;
        let mut end = start;
        while end < self.done.len() && self.is_free(end) && end.saturating_sub(start) < self.run_cap
        {
            if let Some(slot) = self.assigned.get_mut(end) {
                *slot = true;
            }
            end = end.saturating_add(1);
        }
        Some((start, end))
    }

    /// Mark a piece complete (and no longer assigned).
    pub(crate) fn mark_done(&mut self, piece: usize) {
        if let Some(slot) = self.done.get_mut(piece) {
            *slot = true;
        }
    }

    /// Un-assign the still-incomplete pieces of a failed run so a retry can claim
    /// them again.
    pub(crate) fn release(&mut self, start: usize, end: usize) {
        for i in start..end {
            let done = self.done.get(i).copied().unwrap_or(true);
            if !done && let Some(slot) = self.assigned.get_mut(i) {
                *slot = false;
            }
        }
    }

    fn is_free(&self, i: usize) -> bool {
        self.done.get(i) == Some(&false) && self.assigned.get(i) == Some(&false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hands_out_all_pieces_exactly_once() {
        let mut map = SegmentMap::new(vec![false; 100], 4);
        let mut covered = [false; 100];
        while let Some((s, e)) = map.next_run() {
            #[expect(clippy::needless_range_loop, reason = "index needed for mark_done")]
            for i in s..e {
                assert!(!covered[i], "piece {i} handed out twice");
                covered[i] = true;
                map.mark_done(i);
            }
        }
        assert!(covered.iter().all(|&c| c));
        assert!(map.is_complete());
    }

    #[test]
    fn runs_are_capped_for_parallelism() {
        let mut map = SegmentMap::new(vec![false; 1000], 8);
        let (s, e) = map.next_run().unwrap();
        assert!(e.saturating_sub(s) <= 64, "run should be capped");
        // A second worker gets a different, non-overlapping run immediately.
        let (s2, e2) = map.next_run().unwrap();
        assert!(s2 >= e, "second run must not overlap the first");
        assert!(e2 > s2);
    }

    #[test]
    fn skips_already_done_pieces_on_resume() {
        // Pieces 0..50 already done → first run starts at 50.
        let mut done = vec![false; 100];
        for d in done.iter_mut().take(50) {
            *d = true;
        }
        let mut map = SegmentMap::new(done, 4);
        let (s, _e) = map.next_run().unwrap();
        assert_eq!(s, 50);
    }

    #[test]
    fn released_run_is_reclaimable() {
        let mut map = SegmentMap::new(vec![false; 10], 1);
        let (s, e) = map.next_run().unwrap();
        map.release(s, e);
        // Same pieces are handed out again.
        let (s2, _e2) = map.next_run().unwrap();
        assert_eq!(s2, s);
    }
}
