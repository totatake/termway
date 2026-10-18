use crate::display::{DamageRect, Framebuffer};
use crate::protocol::{KittyStreamEncoder, SixelStreamEncoder};
use crate::terminal::TerminalSize;
use std::io::{self, Write};

use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

struct RenderJob {
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    term_cols: u16,
    term_rows: u16,
    cell_width_px: u16,
    cell_height_px: u16,
    taskbar: Option<String>,
    needs_clear: bool,
    show_hud: bool,
    damage: Option<Vec<DamageRect>>,
}

use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Default)]
pub struct BackendMetrics {
    pub jobs_submitted: AtomicU64,
    pub jobs_rendered: AtomicU64,
    pub partial_frames: AtomicU64,
    pub frames_dropped: AtomicU64,
    pub total_encode_us: AtomicU64,
    pub max_encode_us: AtomicU64,
    pub total_write_us: AtomicU64,
    pub max_write_us: AtomicU64,
    pub total_bytes_written: AtomicU64,
}

struct AsyncRenderState {
    pending_job: Option<RenderJob>,
    spare_buffer: Option<Vec<u8>>,
    shutdown: bool,
}

pub struct TerminalBackend {
    pub term_size: TerminalSize,
    use_sixel: bool,
    pub show_hud: bool,
    pub needs_clear: bool,
    async_state: Arc<(Mutex<AsyncRenderState>, Condvar)>,
    metrics: Arc<BackendMetrics>,
    worker_handle: Option<JoinHandle<()>>,
}

impl TerminalBackend {
    pub fn new(term_size: TerminalSize, use_sixel: bool) -> Self {
        let metrics = Arc::new(BackendMetrics::default());
        let metrics_clone = Arc::clone(&metrics);
        let async_state = Arc::new((
            Mutex::new(AsyncRenderState {
                pending_job: None,
                spare_buffer: None,
                shutdown: false,
            }),
            Condvar::new(),
        ));

        let state_clone = Arc::clone(&async_state);
        let mode_desc = if use_sixel {
            "Sixel"
        } else {
            "Kitty"
        };

        let worker_handle = std::thread::Builder::new()
            .name("termway-async-render".into())
            .spawn(move || {
                let mut stdout = io::stdout();
                let mut sixel_stream = SixelStreamEncoder::new();
                let mut sixel_out = Vec::new();
                let mut kitty_stream = KittyStreamEncoder::new();
                let mut kitty_out = String::new();

                let mut stat_start = std::time::Instant::now();
                let mut stat_frames: u32 = 0;
                let mut stat_bytes: usize = 0;
                let mut fps: f64 = 0.0;
                let mut kb_per_sec: f64 = 0.0;
                let mut last_encode_ms: f64;
                let mut prev_hud_shown = false;

                let (lock, cvar) = &*state_clone;
                loop {
                    let job = {
                        let mut guard = lock.lock().unwrap();
                        while guard.pending_job.is_none() && !guard.shutdown {
                            guard = cvar.wait(guard).unwrap();
                        }
                        if guard.shutdown && guard.pending_job.is_none() {
                            break;
                        }
                        guard.pending_job.take().unwrap()
                    };

                    let encode_start = std::time::Instant::now();
                    let bytes_encoded = if use_sixel {
                        sixel_stream.encode_frame_into(
                            &job.pixels,
                            job.width,
                            job.height,
                            if job.needs_clear { None } else { job.damage.as_deref() },
                            &mut sixel_out,
                        );
                        sixel_out.len()
                    } else {
                        let image_rows = job.term_rows.saturating_sub(1).max(1);
                        let is_partial = kitty_stream.encode_frame_into(
                            &job.pixels,
                            job.width,
                            job.height,
                            job.term_cols,
                            image_rows,
                            job.cell_width_px,
                            job.cell_height_px,
                            if job.needs_clear { None } else { job.damage.as_deref() },
                            job.needs_clear,
                            &mut kitty_out,
                        );
                        if is_partial {
                            metrics_clone.partial_frames.fetch_add(1, Ordering::Relaxed);
                        }
                        kitty_out.len()
                    };

                    let encode_dur = encode_start.elapsed();
                    let encode_us = encode_dur.as_micros() as u64;
                    metrics_clone.jobs_rendered.fetch_add(1, Ordering::Relaxed);
                    metrics_clone.total_encode_us.fetch_add(encode_us, Ordering::Relaxed);
                    metrics_clone.max_encode_us.fetch_max(encode_us, Ordering::Relaxed);
                    metrics_clone.total_bytes_written.fetch_add(bytes_encoded as u64, Ordering::Relaxed);

                    last_encode_ms = encode_dur.as_secs_f64() * 1000.0;
                    stat_frames += 1;
                    stat_bytes += bytes_encoded;

                    let elapsed = stat_start.elapsed();
                    if elapsed >= std::time::Duration::from_millis(500) {
                        let secs = elapsed.as_secs_f64();
                        fps = (stat_frames as f64) / secs;
                        kb_per_sec = (stat_bytes as f64 / 1024.0) / secs;
                        stat_frames = 0;
                        stat_bytes = 0;
                        stat_start = std::time::Instant::now();
                    }

                    let visible_overlay = format!(
                        " [{}] [F12: Shot] [Ctrl-Q: Exit]",
                        mode_desc
                    );
                    let overlay_len = visible_overlay.chars().count();
                    let overlay_row = job.term_rows;
                    let overlay_col = job
                        .term_cols
                        .saturating_sub(overlay_len as u16)
                        .saturating_add(1)
                        .max(1);
                    let overlay_seq = format!(
                        "\x1b[?7l\x1b[{};{}H\x1b[40;37m{}\x1b[0m\x1b[?7h",
                        overlay_row, overlay_col, visible_overlay
                    );

                    let taskbar_str = job.taskbar.as_deref().unwrap_or("");
                    let taskbar_seq = format!("\x1b[{};1H\x1b[2K{}", overlay_row, taskbar_str);

                    let mut clear_seq = String::new();
                    if job.needs_clear {
                        if use_sixel {
                            clear_seq.push_str("\x1b[2J\x1b[H");
                        } else {
                            clear_seq.push_str("\x1b[2J\x1b[H\x1b_Ga=d,d=i,i=2\x1b\\");
                        }
                    } else if prev_hud_shown && !job.show_hud {
                        for r in 2..=5 {
                            clear_seq.push_str(&format!("\x1b[{};1H\x1b[2K", r));
                        }
                    }
                    prev_hud_shown = job.show_hud;

                    let hud_seq = if job.show_hud && job.term_cols >= 20 && job.term_rows >= 6 {
                        let res_str = format!("{}x{}", job.width, job.height);
                        let lines = [
                            format!(" {:>8.1} FPS ", fps),
                            format!(" {:>12} ", res_str),
                            format!(" {:>7.1} KB/s ", kb_per_sec),
                            format!(" {:>9.1} ms ", last_encode_ms),
                        ];
                        let mut seq = String::from("\x1b[?7l");
                        for (idx, line) in lines.iter().enumerate() {
                            let row = 2 + idx as u16;
                            let vis_len = line.chars().count() as u16;
                            let col = job
                                .term_cols
                                .saturating_sub(vis_len)
                                .saturating_add(1)
                                .max(1);
                            seq.push_str(&format!("\x1b[{};{}H\x1b[48;5;236;97m{}\x1b[0m", row, col, line));
                        }
                        seq.push_str("\x1b[?7h");
                        seq
                    } else {
                        String::new()
                    };

                    let write_start = std::time::Instant::now();
                    if use_sixel {
                        let _ = write!(stdout, "\x1b[?2026h{}\x1b[H", clear_seq);
                        let _ = stdout.write_all(&sixel_out);
                        let _ = write!(stdout, "{}{}{}\x1b[?2026l", taskbar_seq, overlay_seq, hud_seq);
                    } else {
                        let _ = write!(stdout, "\x1b[?2026h{}{}{}{}{}\x1b[?2026l", clear_seq, kitty_out, taskbar_seq, overlay_seq, hud_seq);
                    }
                    let _ = stdout.flush();
                    let write_us = write_start.elapsed().as_micros() as u64;
                    metrics_clone.total_write_us.fetch_add(write_us, Ordering::Relaxed);
                    metrics_clone.max_write_us.fetch_max(write_us, Ordering::Relaxed);

                    let mut guard = lock.lock().unwrap();
                    guard.spare_buffer = Some(job.pixels);
                }
            })
            .ok();

        Self {
            term_size,
            use_sixel,
            show_hud: false,
            needs_clear: false,
            async_state,
            metrics,
            worker_handle,
        }
    }

    pub fn toggle_hud(&mut self) {
        self.show_hud = !self.show_hud;
        self.needs_clear = true;
    }

    pub fn set_hud_enabled(&mut self, enabled: bool) {
        if self.show_hud != enabled {
            self.show_hud = enabled;
            self.needs_clear = true;
        }
    }

    pub fn is_hud_enabled(&self) -> bool {
        self.show_hud
    }

    pub fn resize(&mut self, term_size: TerminalSize) {
        self.term_size = term_size;
        self.needs_clear = true;
    }

    pub fn term_size(&self) -> TerminalSize {
        self.term_size
    }

    pub fn is_sixel(&self) -> bool {
        self.use_sixel
    }

    pub fn name(&self) -> &'static str {
        if self.use_sixel {
            "Sixel"
        } else {
            "Kitty"
        }
    }

    pub fn metrics(&self) -> &Arc<BackendMetrics> {
        &self.metrics
    }

    pub fn render_damage(&mut self, fb: &Framebuffer, damage: &[DamageRect]) -> io::Result<()> {
        if damage.is_empty() {
            return Ok(());
        }
        self.render_pixels_with_damage(&fb.pixels, fb.width, fb.height, None, Some(damage.to_vec()))
    }

    pub fn render_pixels(
        &mut self,
        pixels: &[u8],
        width: u32,
        height: u32,
        taskbar: Option<&str>,
    ) -> io::Result<()> {
        self.render_pixels_with_damage(pixels, width, height, taskbar, None)
    }

    pub fn render_pixels_with_damage(
        &mut self,
        pixels: &[u8],
        width: u32,
        height: u32,
        taskbar: Option<&str>,
        damage: Option<Vec<DamageRect>>,
    ) -> io::Result<()> {
        self.metrics.jobs_submitted.fetch_add(1, Ordering::Relaxed);
        let (lock, cvar) = &*self.async_state;
        let mut guard = lock.lock().unwrap();

        let needs_clear = self.needs_clear;
        self.needs_clear = false;

        if let Some(ref mut job) = guard.pending_job {
            // Worker is still busy encoding/flushing previous frame.
            // Drop outdated pending frame and replace with this latest frame (Frame Dropping!).
            self.metrics.frames_dropped.fetch_add(1, Ordering::Relaxed);
            job.pixels.clear();
            job.pixels.extend_from_slice(pixels);
            job.width = width;
            job.height = height;
            job.term_cols = self.term_size.cols;
            job.term_rows = self.term_size.rows;
            job.cell_width_px = self.term_size.cell_width_px;
            job.cell_height_px = self.term_size.cell_height_px;
            job.taskbar = taskbar.map(String::from);
            job.show_hud = self.show_hud;
            if needs_clear {
                job.needs_clear = true;
                job.damage = None;
            } else {
                // Union damage if both frames had damage rects
                match (&mut job.damage, damage) {
                    (Some(ref mut existing), Some(incoming)) => {
                        existing.extend(incoming);
                    }
                    _ => {
                        job.damage = None;
                    }
                }
            }
        } else {
            let mut buf = guard.spare_buffer.take().unwrap_or_else(|| Vec::with_capacity(pixels.len()));
            buf.clear();
            buf.extend_from_slice(pixels);
            guard.pending_job = Some(RenderJob {
                pixels: buf,
                width,
                height,
                term_cols: self.term_size.cols,
                term_rows: self.term_size.rows,
                cell_width_px: self.term_size.cell_width_px,
                cell_height_px: self.term_size.cell_height_px,
                taskbar: taskbar.map(String::from),
                needs_clear,
                show_hud: self.show_hud,
                damage,
            });
        }
        cvar.notify_one();
        Ok(())
    }
}

impl Drop for TerminalBackend {
    fn drop(&mut self) {
        {
            let (lock, cvar) = &*self.async_state;
            let mut guard = lock.lock().unwrap();
            guard.shutdown = true;
            cvar.notify_one();
        }
        if let Some(handle) = self.worker_handle.take() {
            let _ = handle.join();
        }
    }
}

