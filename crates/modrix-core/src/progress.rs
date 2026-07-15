// SPDX-License-Identifier: GPL-2.0-only
//! Live progress reporting for long engine operations.
//!
//! A [`Progress`] is a cheap shared sink (atomics + one small mutex) the
//! engine writes into while it recovers, deploys, purges, or installs, and a
//! frontend polls to draw a progress bar and status line. Writers never
//! block on readers; a poisoned message lock degrades to an empty message,
//! never a panic.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// A point-in-time view of the running operation.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProgressSnapshot {
    /// Work units finished.
    pub done: u64,
    /// Total work units (`0` = indeterminate - show activity, not percent).
    pub total: u64,
    /// The latest status line (operation + current item).
    pub message: String,
}

impl ProgressSnapshot {
    /// Completed fraction in `0.0..=1.0`, when the total is known.
    #[must_use]
    pub fn fraction(&self) -> Option<f32> {
        if self.total == 0 {
            return None;
        }
        #[expect(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            reason = "display-only fraction"
        )]
        let f = (self.done as f64 / self.total as f64).clamp(0.0, 1.0) as f32;
        Some(f)
    }
}

/// The shared progress sink. One lives inside every [`crate::Engine`].
#[derive(Debug, Default)]
pub struct Progress {
    active: AtomicBool,
    done: AtomicU64,
    total: AtomicU64,
    message: Mutex<String>,
}

impl Progress {
    /// Start a new operation. `total` of `0` means indeterminate.
    pub fn begin(&self, message: &str, total: u64) {
        self.done.store(0, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
        self.set_message(message);
        self.active.store(true, Ordering::Relaxed);
    }

    /// Replace the status line.
    pub fn set_message(&self, message: &str) {
        if let Ok(mut slot) = self.message.lock() {
            slot.clear();
            slot.push_str(message);
        }
    }

    /// Add finished work units.
    pub fn advance(&self, units: u64) {
        self.done.fetch_add(units, Ordering::Relaxed);
    }

    /// Add finished work units and update the status line.
    pub fn advance_with(&self, units: u64, message: &str) {
        self.advance(units);
        self.set_message(message);
    }

    /// Mark the operation finished; [`Progress::snapshot`] returns `None`.
    pub fn finish(&self) {
        self.active.store(false, Ordering::Relaxed);
    }

    /// The running operation's state, or `None` when idle.
    #[must_use]
    pub fn snapshot(&self) -> Option<ProgressSnapshot> {
        if !self.active.load(Ordering::Relaxed) {
            return None;
        }
        let message = self.message.lock().map(|m| m.clone()).unwrap_or_default();
        Some(ProgressSnapshot {
            done: self.done.load(Ordering::Relaxed),
            total: self.total.load(Ordering::Relaxed),
            message,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_and_fraction() {
        let p = Progress::default();
        assert!(p.snapshot().is_none());
        p.begin("Deploying", 200);
        p.advance(50);
        p.advance_with(50, "Deploying · meshes/x.nif");
        let snap = p.snapshot().expect("active");
        assert_eq!(snap.done, 100);
        assert_eq!(snap.message, "Deploying · meshes/x.nif");
        assert!((snap.fraction().expect("known total") - 0.5).abs() < f32::EPSILON);
        p.finish();
        assert!(p.snapshot().is_none());
    }

    #[test]
    fn indeterminate_total_has_no_fraction() {
        let p = Progress::default();
        p.begin("Extracting archive", 0);
        assert!(p.snapshot().expect("active").fraction().is_none());
    }
}
