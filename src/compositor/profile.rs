use std::sync::atomic::Ordering;
use crate::backend::TerminalBackend;

/// Fixed-size log-histogram for computing p95/p99 percentiles without runtime heap allocations.
/// Buckets represent latency ranges in microseconds:
/// - 0..32: 0..32ms (1ms buckets)
/// - 32..48: 32..64ms (2ms buckets)
/// - 48..56: 64..128ms (8ms buckets)
/// - 56..63: 128..512ms (64ms buckets)
/// - 63: >= 512ms
#[derive(Debug, Clone, Copy)]
pub struct LatencyHistogram {
    buckets: [u64; 64],
    count: u64,
}

impl Default for LatencyHistogram {
    fn default() -> Self {
        Self {
            buckets: [0; 64],
            count: 0,
        }
    }
}

impl LatencyHistogram {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub fn record(&mut self, us: u64) {
        let ms = us / 1000;
        let idx = if ms < 32 {
            ms as usize
        } else if ms < 64 {
            32 + ((ms - 32) / 2) as usize
        } else if ms < 128 {
            48 + ((ms - 64) / 8) as usize
        } else if ms < 512 {
            56 + ((ms - 128) / 64) as usize
        } else {
            63
        };
        self.buckets[idx] += 1;
        self.count += 1;
    }

    pub fn percentile(&self, p: f64) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        let target = ((self.count as f64) * p / 100.0).ceil() as u64;
        let mut accum = 0;
        for (i, &b) in self.buckets.iter().enumerate() {
            accum += b;
            if accum >= target {
                return if i < 32 {
                    i as f64 + 0.5
                } else if i < 48 {
                    32.0 + (i - 32) as f64 * 2.0 + 1.0
                } else if i < 56 {
                    64.0 + (i - 48) as f64 * 8.0 + 4.0
                } else if i < 63 {
                    128.0 + (i - 56) as f64 * 64.0 + 32.0
                } else {
                    512.0
                };
            }
        }
        512.0
    }
}

#[derive(Debug, Default)]
pub struct CompositorMetrics {
    pub render_frames: u64,
    pub render_total_us: u64,
    pub render_max_us: u64,
    pub render_hist: LatencyHistogram,
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
        self.render_hist.record(render_us);
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
        let partial_frames = b_metrics.partial_frames.load(Ordering::Relaxed);
        let frames_dropped = b_metrics.frames_dropped.load(Ordering::Relaxed);
        let total_encode_us = b_metrics.total_encode_us.load(Ordering::Relaxed);
        let max_encode_us = b_metrics.max_encode_us.load(Ordering::Relaxed);
        let total_write_us = b_metrics.total_write_us.load(Ordering::Relaxed);
        let max_write_us = b_metrics.max_write_us.load(Ordering::Relaxed);
        let total_bytes = b_metrics.total_bytes_written.load(Ordering::Relaxed);

        let p95_encode_ms = b_metrics.percentile_encode(95.0);
        let p99_encode_ms = b_metrics.percentile_encode(99.0);
        let p95_write_ms = b_metrics.percentile_write(95.0);
        let p99_write_ms = b_metrics.percentile_write(99.0);
        let p95_render_ms = self.render_hist.percentile(95.0);
        let p99_render_ms = self.render_hist.percentile(99.0);

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
        eprintln!("Backend Async Jobs:    {} submitted, {} rendered ({} partial), {} dropped",
            jobs_submitted, jobs_rendered, partial_frames, frames_dropped);
        eprintln!("Latency Breakdown (avg / p95 / p99 / max):");
        eprintln!("  1. Pixman Render:    avg = {:>6.2} ms,  p95 = {:>6.2} ms,  p99 = {:>6.2} ms,  max = {:>6.2} ms",
            avg_render_ms, p95_render_ms, p99_render_ms, self.render_max_us as f64 / 1000.0);
        eprintln!("  2. Damage Tracker:   avg = {:>6.1} μs,                     max = {:>6.1} μs",
            avg_damage_us, self.damage_max_us as f64);
        eprintln!("  3. Protocol Encode:  avg = {:>6.2} ms,  p95 = {:>6.2} ms,  p99 = {:>6.2} ms,  max = {:>6.2} ms ({})",
            avg_encode_ms, p95_encode_ms, p99_encode_ms, max_encode_us as f64 / 1000.0, backend.name());
        eprintln!("  4. Terminal Write:   avg = {:>6.2} ms,  p95 = {:>6.2} ms,  p99 = {:>6.2} ms,  max = {:>6.2} ms",
            avg_write_ms, p95_write_ms, p99_write_ms, max_write_us as f64 / 1000.0);
        eprintln!("Output Data Bandwidth: total = {:.2} MB, throughput = {:.1} KB/s",
            total_bytes as f64 / (1024.0 * 1024.0), kb_per_sec);
        eprintln!("============================================================================================\n");

        if let Ok(path) = std::env::var("TERMWAY_PROFILE_FILE") {
            let trace_file_entry = if let Some(t_path) = crate::trace::trace_file_path() {
                format!(r#","trace_file":{:?}"#, t_path)
            } else if let Ok(t_path) = std::env::var("TERMWAY_TRACE_FILE") {
                format!(r#","trace_file":{:?}"#, t_path)
            } else if crate::trace::enabled() {
                String::from(r#","trace_file":"/tmp/termway_trace.json""#)
            } else {
                String::new()
            };

            let report = format!(
                r#"{{"duration_sec":{:.3},"render_frames":{},"clean_skips":{},"jobs_submitted":{},"jobs_rendered":{},"partial_frames":{},"frames_dropped":{},"avg_render_ms":{:.3},"p95_render_ms":{:.3},"p99_render_ms":{:.3},"max_render_ms":{:.3},"avg_damage_us":{:.3},"max_damage_us":{:.3},"avg_encode_ms":{:.3},"p95_encode_ms":{:.3},"p99_encode_ms":{:.3},"max_encode_ms":{:.3},"avg_write_ms":{:.3},"p95_write_ms":{:.3},"p99_write_ms":{:.3},"max_write_ms":{:.3},"total_bytes":{},"kb_per_sec":{:.2}{}}}"#,
                elapsed_secs,
                self.render_frames,
                self.clean_skips,
                jobs_submitted,
                jobs_rendered,
                partial_frames,
                frames_dropped,
                avg_render_ms,
                p95_render_ms,
                p99_render_ms,
                self.render_max_us as f64 / 1000.0,
                avg_damage_us,
                self.damage_max_us as f64,
                avg_encode_ms,
                p95_encode_ms,
                p99_encode_ms,
                max_encode_us as f64 / 1000.0,
                avg_write_ms,
                p95_write_ms,
                p99_write_ms,
                max_write_us as f64 / 1000.0,
                total_bytes,
                kb_per_sec,
                trace_file_entry,
            );
            let _ = std::fs::write(path, report);
        }

        // If trace is enabled, dump trace events
        if crate::trace::enabled() {
            let trace_path = crate::trace::trace_file_path()
                .or_else(|| std::env::var("TERMWAY_TRACE_FILE").ok())
                .unwrap_or_else(|| "/tmp/termway_trace.json".to_string());
            let _ = crate::trace::writer::dump_to_file(&trace_path);
        }
    }
}

