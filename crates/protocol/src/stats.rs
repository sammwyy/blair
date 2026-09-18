/// Frame timings of the most recent reporting interval.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RenderStats {
    /// Frames submitted to the display since startup.
    pub frames: u64,
    /// Repaints that produced no damage and were therefore not submitted.
    pub empty_frames: u64,
    pub fps: f64,
    /// Average time spent collecting render elements.
    pub build_ms_avg: f64,
    pub render_ms_avg: f64,
    pub render_ms_max: f64,
    /// Average time between presented frames.
    pub present_interval_ms_avg: f64,
}
