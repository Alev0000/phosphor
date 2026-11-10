//! The Screen Loop's states between After Effects frames.
//!
//! Frame n of a loop depends on every pass before it, so the loop runs in order. The state after
//! each recent frame is kept (playback and renders step forward one frame at a time), plus one
//! every second as a checkpoint for scrubbing back. A frame further than `RESUME_LIMIT` from any kept
//! state starts a fresh run-up `FRESH_WINDOW` frames earlier, so scrubbing never re-runs more than
//! about a second of passes (only the tunnel's deepest copies remember further back).

use std::collections::BTreeMap;

use phosphor_core::howlaround::HowlLoop;

pub const CHECKPOINT_EVERY: i64 = 30;
pub const RESUME_LIMIT: i64 = 30;
pub const FRESH_WINDOW: i64 = 24;
const RECENT: i64 = 4;
const MEMORY_CAP: usize = 768 << 20;

#[derive(Default)]
pub struct LoopCache {
    key: Option<u64>,
    /// The loop's state after frame k was rendered (start − 1 after a run-up).
    states: BTreeMap<i64, HowlLoop>,
    bytes: usize,
}

/// Where a frame's loop starts from.
pub struct Plan {
    /// Kept state to continue from (the state after that frame), or None for a fresh run-up.
    pub resume: Option<i64>,
    /// First frame to render a pass for.
    pub start: i64,
}

impl LoopCache {
    pub fn plan(&mut self, key: u64, n: i64) -> Plan {
        if self.key != Some(key) {
            self.states.clear();
            self.bytes = 0;
            self.key = Some(key);
        }
        match self.states.range(..n).next_back() {
            Some((&k, _)) if n - k <= RESUME_LIMIT => Plan { resume: Some(k), start: k + 1 },
            _ => Plan { resume: None, start: (n - FRESH_WINDOW).max(0) },
        }
    }

    /// Scene frames a plan needs besides `n` itself.
    pub fn scene_frames(plan: &Plan, n: i64) -> Vec<i64> {
        (plan.start..n).collect()
    }

    pub fn state(&self, k: i64) -> Option<HowlLoop> {
        self.states.get(&k).cloned()
    }

    pub fn store(&mut self, k: i64, n: i64, l: &HowlLoop) {
        let keep = k % CHECKPOINT_EVERY == 0 || k >= n - RECENT || k < 0;
        if !keep || self.states.contains_key(&k) {
            return;
        }
        self.bytes += l.bytes();
        self.states.insert(k, l.clone());

        // Recent states other than checkpoints only matter near the newest frame.
        let stale: Vec<i64> = self
            .states
            .keys()
            .copied()
            .filter(|&f| f % CHECKPOINT_EVERY != 0 && f >= 0 && f < n - 2 * RECENT)
            .collect();
        for f in stale {
            if let Some(s) = self.states.remove(&f) {
                self.bytes -= s.bytes();
            }
        }
        while self.bytes > MEMORY_CAP && self.states.len() > 1 {
            let oldest = *self.states.keys().next().unwrap();
            if let Some(s) = self.states.remove(&oldest) {
                self.bytes -= s.bytes();
            }
        }
    }
}
