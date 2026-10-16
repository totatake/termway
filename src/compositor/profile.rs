use std::sync::atomic::Ordering;
use crate::backend::TerminalBackend;

#[derive(Debug, Default)]
pub struct CompositorMetrics {
    pub render_frames: u64,
    pub render_total_us: u64,
    pub render_max_us: u64,
    pub damage_total_us: u64,
    pub damage_max_us: u64,
    pub clean_skips: u64,
}

impl CompositorMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_render(&mut self, render_us: u64, damage_us: u64) {
        self.render_frames += 1;
        self.render_total_us += render_us;
        self.render_max_us = self.render_max_us.max(render_us);
        self.damage_total_us += damage_us;
        self.damage_max_us = self.damage_max_us.max(damage_us);
    }

    pub fn record_clean_skip(&mut self) {
        self.clean_skips += 1;
    }

    pub fn print_summary(&self, backend: &TerminalBackend, start_time: std::time::Instant) {
        let b_metrics = backend.metrics();
        let jobs_submitted = b_metrics.jobs_submitted.load(Ordering::Relaxed);
        let jobs_rendered = b_metrics.jobs_rendered.load(Ordering::Relaxed);
        let frames_dropped = b_metrics.frames_dropped.load(Ordering::Relaxed);
        let total_encode_us = b_metrics.total_encode_us.load(Ordering::Relaxed);
        let max_encode_us = b_metrics.max_encode_us.load(Ordering::Relaxed);
        let total_write_us = b_metrics.total_write_us.load(Ordering::Relaxed);
        let max_write_us = b_metrics.max_write_us.load(Ordering::Relaxed);
        let total_bytes = b_metrics.total_bytes_written.load(Ordering::Relaxed);

        let elapsed_secs = start_time.elapsed().as_secs_f64();
        let avg_render_ms = if self.render_frames > 0 {
            (self.render_total_us as f64 / self.render_frames as f64) / 1000.0
        } else {
            0.0
        };
        let avg_damage_us = if self.render_frames > 0 {
            self.damage_total_us as f64 / self.render_frames as f64
        } else {
            0.0
        };
        let avg_encode_ms = if jobs_rendered > 0 {
            (total_encode_us as f64 / jobs_rendered as f64) / 1000.0
        } else {
            0.0
        };
        let avg_write_ms = if jobs_rendered > 0 {
            (total_write_us as f64 / jobs_rendered as f64) / 1000.0
        } else {
            0.0
        };
        let kb_per_sec = if elapsed_secs > 0.0 {
            (total_bytes as f64 / 1024.0) / elapsed_secs
        } else {
            0.0
        };

        eprintln!("\n================================== TERMWAY PROFILE REPORT ==================================");
        eprintln!("Session Duration:      {:.2}s", elapsed_secs);
        eprintln!("Compositor Frames:     {} rendered, {} skipped (unchanged clean frames)",
            self.render_frames, self.clean_skips);
        eprintln!("Backend Async Jobs:    {} submitted, {} rendered, {} dropped",
            jobs_submitted, jobs_rendered, frames_dropped);
        eprintln!("Latency Breakdown:");
        eprintln!("  1. Pixman Render:    avg = {:>6.2} ms,  max = {:>6.2} ms", avg_render_ms, self.render_max_us as f64 / 1000.0);
        eprintln!("  2. Damage Tracker:   avg = {:>6.1} μs,  max = {:>6.1} μs", avg_damage_us, self.damage_max_us as f64);
        eprintln!("  3. Protocol Encode:  avg = {:>6.2} ms,  max = {:>6.2} ms ({})", avg_encode_ms, max_encode_us as f64 / 1000.0, backend.name());
        eprintln!("  4. Terminal Write:   avg = {:>6.2} ms,  max = {:>6.2} ms", avg_write_ms, max_write_us as f64 / 1000.0);
        eprintln!("Output Data Bandwidth: total = {:.2} MB, throughput = {:.1} KB/s",
            total_bytes as f64 / (1024.0 * 1024.0), kb_per_sec);
        eprintln!("============================================================================================\n");

        if let Ok(path) = std::env::var("TERMWAY_PROFILE_FILE") {
            let report = format!(
                r#"{{"duration_sec":{:.3},"render_frames":{},"clean_skips":{},"jobs_submitted":{},"jobs_rendered":{},"frames_dropped":{},"avg_render_ms":{:.3},"max_render_ms":{:.3},"avg_damage_us":{:.3},"max_damage_us":{:.3},"avg_encode_ms":{:.3},"max_encode_ms":{:.3},"avg_write_ms":{:.3},"max_write_ms":{:.3},"total_bytes":{},"kb_per_sec":{:.2}}}"#,
                elapsed_secs,
                self.render_frames,
                self.clean_skips,
                jobs_submitted,
                jobs_rendered,
                frames_dropped,
                avg_render_ms,
                self.render_max_us as f64 / 1000.0,
                avg_damage_us,
                self.damage_max_us as f64,
                avg_encode_ms,
                max_encode_us as f64 / 1000.0,
                avg_write_ms,
                max_write_us as f64 / 1000.0,
                total_bytes,
                kb_per_sec,
            );
            let _ = std::fs::write(path, report);
        }
    }
}

