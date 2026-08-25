//! Frame timing.
//!
//! Real per-frame wall time, worst case included. An average alone hides
//! the stutter it is measured to catch, so the summary carries p95 and max
//! as well.

use std::time::Duration;

/// Number of recent frames kept for the rolling summary. At 60 Hz this is a
/// two-second window, long enough to be stable and short enough to react.
const WINDOW: usize = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameSummary {
    pub frames: u64,
    pub last_us: u64,
    pub avg_us: u64,
    pub p95_us: u64,
    pub max_us: u64,
    /// Worst frame since the process started, never reset by the window.
    pub worst_ever_us: u64,
}

impl FrameSummary {
    /// Frames per second implied by the rolling average, rounded to one
    /// decimal. Zero while no frame has been recorded.
    pub fn fps(&self) -> f32 {
        if self.avg_us == 0 {
            return 0.0;
        }
        let fps = 1_000_000.0 / self.avg_us as f32;
        (fps * 10.0).round() / 10.0
    }
}

#[derive(Debug, Default)]
pub struct FrameTimer {
    samples: Vec<u64>,
    next: usize,
    frames: u64,
    last_us: u64,
    worst_ever_us: u64,
}

impl FrameTimer {
    pub fn new() -> Self {
        FrameTimer {
            samples: Vec::with_capacity(WINDOW),
            ..Default::default()
        }
    }

    pub fn record(&mut self, frame: Duration) {
        let us = frame.as_micros().min(u64::MAX as u128) as u64;
        self.frames += 1;
        self.last_us = us;
        self.worst_ever_us = self.worst_ever_us.max(us);
        if self.samples.len() < WINDOW {
            self.samples.push(us);
        } else {
            self.samples[self.next] = us;
            self.next = (self.next + 1) % WINDOW;
        }
    }

    pub fn summary(&self) -> FrameSummary {
        if self.samples.is_empty() {
            return FrameSummary {
                frames: self.frames,
                last_us: self.last_us,
                avg_us: 0,
                p95_us: 0,
                max_us: 0,
                worst_ever_us: self.worst_ever_us,
            };
        }
        let sum: u128 = self.samples.iter().map(|&v| v as u128).sum();
        let avg_us = (sum / self.samples.len() as u128) as u64;

        let mut sorted = self.samples.clone();
        sorted.sort_unstable();
        // Nearest-rank p95: the smallest sample at or above 95% of the window.
        let rank = ((sorted.len() as f64) * 0.95).ceil() as usize;
        let index = rank.saturating_sub(1).min(sorted.len() - 1);

        FrameSummary {
            frames: self.frames,
            last_us: self.last_us,
            avg_us,
            p95_us: sorted[index],
            max_us: *sorted.last().expect("non-empty"),
            worst_ever_us: self.worst_ever_us,
        }
    }
}

/// One-off timings taken during startup, shown on screen because "how long
/// until the first frame on a 2000 entry folder" is one of the questions.
///
/// Only `first_frame_ms` is drawn in the header; the rest are printed in the
/// report that the scan produces before the UI starts, and are carried here
/// so a future readout can show them without re-measuring.
#[derive(Debug, Clone, Copy, Default)]
pub struct StartupTimings {
    #[allow(dead_code)]
    pub scan_ms: u128,
    #[allow(dead_code)]
    pub gamelist_ms: u128,
    #[allow(dead_code)]
    pub ui_build_ms: u128,
    pub first_frame_ms: u128,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timer_with(samples: &[u64]) -> FrameTimer {
        let mut timer = FrameTimer::new();
        for &us in samples {
            timer.record(Duration::from_micros(us));
        }
        timer
    }

    #[test]
    fn an_empty_timer_reports_zero_rather_than_dividing_by_nothing() {
        let summary = FrameTimer::new().summary();
        assert_eq!(summary.frames, 0);
        assert_eq!(summary.avg_us, 0);
        assert_eq!(summary.fps(), 0.0);
    }

    #[test]
    fn average_and_fps_match_a_steady_60hz_stream() {
        let summary = timer_with(&[16_666; 30]).summary();
        assert_eq!(summary.avg_us, 16_666);
        assert_eq!(summary.fps(), 60.0);
        assert_eq!(summary.frames, 30);
    }

    #[test]
    fn one_slow_frame_moves_p95_and_max_but_barely_moves_the_average() {
        // This is exactly the case Degauss must not hide: 99 good frames
        // and one 200 ms stall still feels broken.
        let mut samples = vec![16_000u64; 99];
        samples.push(200_000);
        let summary = timer_with(&samples).summary();

        assert!(
            summary.avg_us < 20_000,
            "average stays low: {}",
            summary.avg_us
        );
        assert_eq!(summary.max_us, 200_000, "the stall must be visible in max");
        assert!(summary.p95_us >= 16_000);
        assert_eq!(summary.worst_ever_us, 200_000);
    }

    #[test]
    fn the_window_rolls_but_the_worst_frame_ever_is_kept() {
        let mut timer = timer_with(&[500_000]);
        for _ in 0..WINDOW * 2 {
            timer.record(Duration::from_micros(16_000));
        }
        let summary = timer.summary();

        assert_eq!(summary.max_us, 16_000, "the old stall has left the window");
        assert_eq!(
            summary.worst_ever_us, 500_000,
            "but a run's worst frame must never be forgotten"
        );
        assert_eq!(summary.frames, (WINDOW * 2 + 1) as u64);
    }

    #[test]
    fn p95_uses_nearest_rank_over_the_whole_window() {
        let samples: Vec<u64> = (1..=100).map(|n| n * 1_000).collect();
        let summary = timer_with(&samples).summary();
        assert_eq!(summary.p95_us, 95_000);
    }
}
