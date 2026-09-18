use std::time::{Duration, Instant};

pub use blair_protocol::RenderStats;

const REPORT_INTERVAL: Duration = Duration::from_secs(5);

/// Accumulates frame timings for one output and periodically reports them.
#[derive(Debug)]
pub struct FrameTimer {
    output: String,
    window_start: Instant,
    frames: u32,
    empty_frames: u32,
    build: Duration,
    render: Duration,
    render_max: Duration,
    last_present: Option<Instant>,
    present_intervals: Duration,
    presents: u32,
    total_frames: u64,
    total_empty_frames: u64,
}

impl FrameTimer {
    pub fn new(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            window_start: Instant::now(),
            frames: 0,
            empty_frames: 0,
            build: Duration::ZERO,
            render: Duration::ZERO,
            render_max: Duration::ZERO,
            last_present: None,
            present_intervals: Duration::ZERO,
            presents: 0,
            total_frames: 0,
            total_empty_frames: 0,
        }
    }

    pub fn record_frame(&mut self, build: Duration, render: Duration, damaged: bool) {
        self.build += build;
        self.render += render;
        self.render_max = self.render_max.max(render);
        if damaged {
            self.frames += 1;
            self.total_frames += 1;
        } else {
            self.empty_frames += 1;
            self.total_empty_frames += 1;
        }
    }

    pub fn record_presented(&mut self, now: Instant) {
        if let Some(last) = self.last_present.replace(now) {
            let interval = now.saturating_duration_since(last);
            if interval < REPORT_INTERVAL {
                self.present_intervals += interval;
                self.presents += 1;
            }
        }
    }

    pub fn reset_presentation(&mut self) {
        self.last_present = None;
    }

    /// Publishes a summary into `stats` once per report interval.
    pub fn maybe_report(&mut self, stats: &mut RenderStats) {
        let elapsed = self.window_start.elapsed();
        if elapsed < REPORT_INTERVAL {
            return;
        }
        let attempts = self.frames + self.empty_frames;
        let average = |total: Duration, count: u32| {
            if count == 0 {
                0.0
            } else {
                total.as_secs_f64() * 1000.0 / f64::from(count)
            }
        };
        *stats = RenderStats {
            frames: self.total_frames,
            empty_frames: self.total_empty_frames,
            fps: f64::from(self.frames) / elapsed.as_secs_f64(),
            build_ms_avg: average(self.build, attempts),
            render_ms_avg: average(self.render, attempts),
            render_ms_max: self.render_max.as_secs_f64() * 1000.0,
            present_interval_ms_avg: average(self.present_intervals, self.presents),
        };
        if attempts > 0 {
            tracing::debug!(
                output = %self.output,
                fps = format_args!("{:.1}", stats.fps),
                frames = self.frames,
                empty = self.empty_frames,
                build_ms = format_args!("{:.2}", stats.build_ms_avg),
                render_ms = format_args!("{:.2}", stats.render_ms_avg),
                render_max_ms = format_args!("{:.2}", stats.render_ms_max),
                present_interval_ms = format_args!("{:.2}", stats.present_interval_ms_avg),
                "frame timings"
            );
        }
        self.window_start = Instant::now();
        self.frames = 0;
        self.empty_frames = 0;
        self.build = Duration::ZERO;
        self.render = Duration::ZERO;
        self.render_max = Duration::ZERO;
        self.present_intervals = Duration::ZERO;
        self.presents = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarizes_a_report_window() {
        let mut timer = FrameTimer::new("test");
        timer.window_start = Instant::now() - REPORT_INTERVAL;
        timer.record_frame(Duration::from_millis(1), Duration::from_millis(4), true);
        timer.record_frame(Duration::from_millis(3), Duration::from_millis(8), true);
        timer.record_frame(Duration::ZERO, Duration::ZERO, false);
        let start = Instant::now();
        timer.record_presented(start);
        timer.record_presented(start + Duration::from_millis(16));
        let mut stats = RenderStats::default();
        timer.maybe_report(&mut stats);
        assert_eq!(stats.frames, 2);
        assert_eq!(stats.empty_frames, 1);
        assert!((stats.render_ms_max - 8.0).abs() < 1e-6);
        assert!((stats.render_ms_avg - 4.0).abs() < 1e-6);
        assert!((stats.present_interval_ms_avg - 16.0).abs() < 1e-6);
        assert!(stats.fps > 0.0);
        assert_eq!(timer.frames, 0);
    }

    #[test]
    fn does_not_report_before_the_interval() {
        let mut timer = FrameTimer::new("test");
        timer.record_frame(Duration::from_millis(1), Duration::from_millis(1), true);
        let mut stats = RenderStats::default();
        timer.maybe_report(&mut stats);
        assert_eq!(stats, RenderStats::default());
        assert_eq!(timer.frames, 1);
    }
}
