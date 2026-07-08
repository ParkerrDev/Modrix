// SPDX-License-Identifier: GPL-2.0-only
//! Small display formatters shared by the views.

/// Human-readable byte count (`12.4 MB`).
#[expect(
    clippy::cast_precision_loss,
    reason = "display-only: a fraction of a byte lost at petabyte scale is invisible"
)]
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = n as f64;
    let mut unit = 0_usize;
    // Bounded: at most UNITS.len() - 1 steps.
    while value >= 1024.0 && unit < UNITS.len().saturating_sub(1) {
        value /= 1024.0;
        unit = unit.saturating_add(1);
    }
    let suffix = UNITS.get(unit).copied().unwrap_or("B");
    if unit == 0 {
        format!("{n} {suffix}")
    } else {
        format!("{value:.1} {suffix}")
    }
}

/// Completed fraction in `0.0..=1.0` for a progress bar.
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "display-only: progress bars do not need 53-bit precision"
)]
pub fn fraction(done: u64, total: Option<u64>) -> f32 {
    match total {
        Some(t) if t > 0 => ((done as f64 / t as f64).clamp(0.0, 1.0)) as f32,
        _ => 0.0,
    }
}

/// Whole-number percent label (`42%`), or a placeholder when the size is
/// unknown.
pub fn percent(done: u64, total: Option<u64>) -> String {
    match total {
        Some(t) if t > 0 => format!("{:.0}%", f64::from(fraction(done, total)) * 100.0),
        _ => "-".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_byte_magnitudes() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(2048), "2.0 KB");
        assert_eq!(bytes(5 * 1024 * 1024), "5.0 MB");
    }

    #[test]
    fn fraction_handles_unknown_and_zero_totals() {
        assert!(fraction(10, None).abs() < f32::EPSILON);
        assert!(fraction(10, Some(0)).abs() < f32::EPSILON);
        assert!((fraction(50, Some(100)) - 0.5).abs() < f32::EPSILON);
        assert!((fraction(200, Some(100)) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn percent_labels() {
        assert_eq!(percent(50, Some(100)), "50%");
        assert_eq!(percent(1, None), "-");
    }
}
