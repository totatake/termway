use crate::backend::vectored::writev_all;
use crate::display::DamageRect;
use crate::protocol::{KittyStreamEncoder, SixelStreamEncoder};
use crate::terminal::TerminalSize;
use std::io::{self, IoSlice, Write};
use std::os::unix::io::RawFd;

use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

struct RenderJob {
    frame_id: u32,
    enqueue_ts_us: u64,
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

#[derive(Debug)]
pub struct BackendMetrics {
    pub jobs_submitted: AtomicU64,
    pub jobs_rendered: AtomicU64,
    pub partial_frames: AtomicU64,
    pub frames_dropped: AtomicU64,
    pub total_encode_us: AtomicU64,
    pub max_encode_us: AtomicU64,
    pub encode_buckets: [AtomicU64; 64],
    pub total_write_us: AtomicU64,
    pub max_write_us: AtomicU64,
    pub write_buckets: [AtomicU64; 64],
    pub total_bytes_written: AtomicU64,

    // Double-buffering pipeline metrics (M3)
    pub frames_written: AtomicU64,
    pub total_encode_stall_us: AtomicU64,
    pub max_encode_stall_us: AtomicU64,
    pub total_write_idle_us: AtomicU64,
}

impl Default for BackendMetrics {
    fn default() -> Self {
        const INIT_BUCKET: AtomicU64 = AtomicU64::new(0);
        Self {
            jobs_submitted: AtomicU64::new(0),
            jobs_rendered: AtomicU64::new(0),
            partial_frames: AtomicU64::new(0),
            frames_dropped: AtomicU64::new(0),
            total_encode_us: AtomicU64::new(0),
            max_encode_us: AtomicU64::new(0),
            encode_buckets: [INIT_BUCKET; 64],
            total_write_us: AtomicU64::new(0),
            max_write_us: AtomicU64::new(0),
            write_buckets: [INIT_BUCKET; 64],
            total_bytes_written: AtomicU64::new(0),

            frames_written: AtomicU64::new(0),
            total_encode_stall_us: AtomicU64::new(0),
            max_encode_stall_us: AtomicU64::new(0),
            total_write_idle_us: AtomicU64::new(0),
        }
    }
}

impl BackendMetrics {
    #[inline]
    fn bucket_index(us: u64) -> usize {
        let ms = us / 1000;
        if ms < 32 {
            ms as usize
        } else if ms < 64 {
            32 + ((ms - 32) / 2) as usize
        } else if ms < 128 {
            48 + ((ms - 64) / 8) as usize
        } else if ms < 512 {
            56 + ((ms - 128) / 64) as usize
        } else {
            63
        }
    }

    pub fn record_encode_us(&self, us: u64) {
        let idx = Self::bucket_index(us);
        self.encode_buckets[idx].fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_write_us(&self, us: u64) {
        let idx = Self::bucket_index(us);
        self.write_buckets[idx].fetch_add(1, Ordering::Relaxed);
    }

    pub fn percentile_encode(&self, p: f64) -> f64 {
        Self::calc_percentile(&self.encode_buckets, self.jobs_rendered.load(Ordering::Relaxed), p)
    }

    pub fn percentile_write(&self, p: f64) -> f64 {
        let count = self.frames_written.load(Ordering::Relaxed);
        let total = if count > 0 { count } else { self.jobs_rendered.load(Ordering::Relaxed) };
        Self::calc_percentile(&self.write_buckets, total, p)
    }

    fn calc_percentile(buckets: &[AtomicU64; 64], total_count: u64, p: f64) -> f64 {
        if total_count == 0 {
            return 0.0;
        }
        let target = ((total_count as f64) * p / 100.0).ceil() as u64;
        let mut accum = 0;
        for (i, b) in buckets.iter().enumerate() {
            accum += b.load(Ordering::Relaxed);
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

/// Buffer slot for encoded frame handoff between Encode and Write threads.
#[derive(Debug)]
pub struct EncodedFrame {
    pub frame_id: u32,
    pub prefix: Vec<u8>,
    pub payload: Vec<u8>,
    pub suffix: Vec<u8>,
    pub is_partial: bool,
    pub damage_count: u32,
}

impl EncodedFrame {
    pub fn new() -> Self {
        Self {
            frame_id: 0,
            prefix: Vec::with_capacity(512),
            payload: Vec::with_capacity(256 * 1024),
            suffix: Vec::with_capacity(1024),
            is_partial: false,
            damage_count: 0,
        }
    }

    pub fn io_slices<'a>(&'a self) -> [IoSlice<'a>; 3] {
        [
            IoSlice::new(&self.prefix),
            IoSlice::new(&self.payload),
            IoSlice::new(&self.suffix),
        ]
    }

    pub fn total_bytes(&self) -> usize {
        self.prefix.len() + self.payload.len() + self.suffix.len()
    }
}

struct WriteHandoff {
    ready: Option<Box<EncodedFrame>>,
    free: Vec<Box<EncodedFrame>>,
    shutdown: bool,
}

struct AsyncRenderState {
    pending_job: Option<RenderJob>,
    spare_buffer: Option<Vec<u8>>,
    shutdown: bool,
}

pub struct TerminalBackend {
    pub term_size: TerminalSize,
    use_sixel: bool,
    use_jpeg: bool,
    pub show_hud: bool,
    pub needs_clear: bool,
    async_state: Arc<(Mutex<AsyncRenderState>, Condvar)>,
    write_state: Option<Arc<(Mutex<WriteHandoff>, Condvar)>>,
    metrics: Arc<BackendMetrics>,
    encode_handle: Option<JoinHandle<()>>,
    write_handle: Option<JoinHandle<()>>,
    output_fd: RawFd,
    mock_encode_delay_ms: Arc<AtomicU64>,
    mock_write_delay_ms: Arc<AtomicU64>,
}

impl TerminalBackend {
    pub fn new(term_size: TerminalSize, use_sixel: bool, use_jpeg: bool, jpeg_quality: u8) -> Self {
        Self::with_fd_internal(term_size, use_sixel, use_jpeg, jpeg_quality, libc::STDOUT_FILENO)
    }

    #[doc(hidden)]
    pub fn with_output_fd(
        term_size: TerminalSize,
        use_sixel: bool,
        use_jpeg: bool,
        jpeg_quality: u8,
        fd: RawFd,
    ) -> Self {
        Self::with_fd_internal(term_size, use_sixel, use_jpeg, jpeg_quality, fd)
    }

    #[doc(hidden)]
    pub fn set_artificial_delays(&self, encode_ms: u64, write_ms: u64) {
        self.mock_encode_delay_ms.store(encode_ms, Ordering::Relaxed);
        self.mock_write_delay_ms.store(write_ms, Ordering::Relaxed);
    }

    fn with_fd_internal(
        term_size: TerminalSize,
        use_sixel: bool,
        use_jpeg: bool,
        jpeg_quality: u8,
        output_fd: RawFd,
    ) -> Self {
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

        let mock_encode_delay_ms = Arc::new(AtomicU64::new(0));
        let mock_write_delay_ms = Arc::new(AtomicU64::new(0));

        let mode_desc = if use_sixel {
            "Sixel"
        } else if use_jpeg {
            "Kitty-JPEG"
        } else {
            "Kitty"
        };

        let is_serial = match std::env::var("TERMWAY_PIPELINE") {
            Ok(v) => v.trim().eq_ignore_ascii_case("serial"),
            Err(_) => false,
        };

        if is_serial {
            // Serial fallback mode (M1 single-thread behavior)
            let state_clone = Arc::clone(&async_state);
            let mock_enc_delay = Arc::clone(&mock_encode_delay_ms);
            let mock_wr_delay = Arc::clone(&mock_write_delay_ms);

            let worker_handle = std::thread::Builder::new()
                .name("termway-async-render".into())
                .spawn(move || {
                    let stdout = io::stdout();
                    let mut sixel_stream = SixelStreamEncoder::new();
                    let mut sixel_out = Vec::new();
                    let mut kitty_stream = KittyStreamEncoder::new();
                    kitty_stream.set_use_jpeg(use_jpeg);
                    kitty_stream.set_jpeg_quality(jpeg_quality);
                    let mut kitty_out = Vec::new();

                    let mut prefix_buf = Vec::with_capacity(512);
                    let mut suffix_buf = Vec::with_capacity(1024);

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

                        let delay = mock_enc_delay.load(Ordering::Relaxed);
                        if delay > 0 {
                            std::thread::sleep(std::time::Duration::from_millis(delay));
                        }

                        let encode_start = std::time::Instant::now();
                        let enc_start_us = if crate::trace::enabled() { crate::trace::now_us() } else { 0 };
                        let mut is_partial_frame = false;
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
                                is_partial_frame = true;
                            }
                            kitty_out.len()
                        };

                        let encode_dur = encode_start.elapsed();
                        let encode_us = encode_dur.as_micros() as u64;
                        metrics_clone.jobs_rendered.fetch_add(1, Ordering::Relaxed);
                        metrics_clone.total_encode_us.fetch_add(encode_us, Ordering::Relaxed);
                        metrics_clone.max_encode_us.fetch_max(encode_us, Ordering::Relaxed);
                        metrics_clone.record_encode_us(encode_us);
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

                        let visible_overlay = crate::compositor::Taskbar::format_overlay_text(mode_desc);
                        let overlay_len = crate::compositor::Taskbar::overlay_width(mode_desc);
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
                            for r in 2..=6 {
                                clear_seq.push_str(&format!("\x1b[{};1H\x1b[2K", r));
                            }
                        }
                        prev_hud_shown = job.show_hud;

                        let hud_seq = if job.show_hud && job.term_cols >= 20 && job.term_rows >= 6 {
                            let res_str = format!("{}x{}", job.width, job.height);
                            let mut lines = vec![
                                format!(" {:>8.1} FPS ", fps),
                                format!(" {:>12} ", res_str),
                                format!(" {:>7.1} KB/s ", kb_per_sec),
                                format!(" {:>9.1} ms ", last_encode_ms),
                            ];
                            if crate::trace::enabled() {
                                let ovf = crate::trace::ring::total_overflow();
                                lines.push(format!(" Ovf:{:>7} ", ovf));
                            }
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

                        let assemble_start = std::time::Instant::now();
                        let assemble_start_us = if crate::trace::enabled() { crate::trace::now_us() } else { 0 };

                        prefix_buf.clear();
                        prefix_buf.extend_from_slice(b"\x1b[?2026h");
                        prefix_buf.extend_from_slice(clear_seq.as_bytes());
                        if use_sixel {
                            prefix_buf.extend_from_slice(b"\x1b[H");
                        }

                        suffix_buf.clear();
                        suffix_buf.extend_from_slice(taskbar_seq.as_bytes());
                        suffix_buf.extend_from_slice(overlay_seq.as_bytes());
                        suffix_buf.extend_from_slice(hud_seq.as_bytes());
                        suffix_buf.extend_from_slice(b"\x1b[?2026l");

                        let assemble_us = assemble_start.elapsed().as_micros() as u64;

                        let delay = mock_wr_delay.load(Ordering::Relaxed);
                        if delay > 0 {
                            std::thread::sleep(std::time::Duration::from_millis(delay));
                        }

                        let mut tiocoutq: libc::c_int = 0;
                        if crate::trace::enabled() {
                            unsafe {
                                let _ = libc::ioctl(output_fd, libc::TIOCOUTQ, &mut tiocoutq);
                            }
                        }

                        let payload_bytes = if use_sixel { &sixel_out[..] } else { &kitty_out[..] };
                        let total_frame_len = prefix_buf.len() + payload_bytes.len() + suffix_buf.len();

                        let write_start = std::time::Instant::now();
                        let write_start_us = if crate::trace::enabled() { crate::trace::now_us() } else { 0 };
                        let mut syscall_stats = crate::backend::vectored::WritevStats::default();
                        {
                            let mut out_guard = stdout.lock();
                            let _ = out_guard.flush();
                            let mut slices = [
                                IoSlice::new(&prefix_buf),
                                IoSlice::new(payload_bytes),
                                IoSlice::new(&suffix_buf),
                            ];
                            if let Ok(stats) = writev_all(output_fd, &mut slices) {
                                syscall_stats = stats;
                            }
                        }
                        let write_us = write_start.elapsed().as_micros() as u64;
                        metrics_clone.frames_written.fetch_add(1, Ordering::Relaxed);
                        metrics_clone.total_write_us.fetch_add(write_us, Ordering::Relaxed);
                        metrics_clone.max_write_us.fetch_max(write_us, Ordering::Relaxed);
                        metrics_clone.record_write_us(write_us);

                        if crate::trace::enabled() {
                            crate::trace::slice(
                                crate::trace::ThreadRole::AsyncRender,
                                crate::trace::EventId::BackendEncode,
                                enc_start_us,
                                encode_us as u32,
                                job.frame_id,
                                bytes_encoded as u32,
                                job.damage.as_ref().map(|d| d.len() as u32).unwrap_or(0),
                                is_partial_frame as u16,
                            );
                            crate::trace::slice(
                                crate::trace::ThreadRole::AsyncRender,
                                crate::trace::EventId::FramePrepare,
                                assemble_start_us,
                                assemble_us as u32,
                                job.frame_id,
                                (prefix_buf.len() + suffix_buf.len()) as u32,
                                payload_bytes.len() as u32,
                                job.show_hud as u16,
                            );
                            let iovcnt = 3u32;
                            let syscalls = (syscall_stats.syscalls & 0x7F) << 8;
                            crate::trace::slice(
                                crate::trace::ThreadRole::AsyncRender,
                                crate::trace::EventId::BackendWrite,
                                write_start_us,
                                write_us as u32,
                                job.frame_id,
                                total_frame_len as u32,
                                tiocoutq.max(0) as u32,
                                (iovcnt | syscalls) as u16,
                            );
                            if job.frame_id > 0 {
                                crate::trace::flow_end(
                                    crate::trace::ThreadRole::AsyncRender,
                                    crate::trace::EventId::FrameComplete,
                                    write_start_us + write_us,
                                    job.frame_id,
                                );
                            }
                        }

                        let mut guard = lock.lock().unwrap();
                        guard.spare_buffer = Some(job.pixels);
                    }
                })
                .ok();

            return Self {
                term_size,
                use_sixel,
                use_jpeg,
                show_hud: false,
                needs_clear: false,
                async_state,
                write_state: None,
                metrics,
                encode_handle: worker_handle,
                write_handle: None,
                output_fd,
                mock_encode_delay_ms,
                mock_write_delay_ms,
            };
        }

        // Double-buffering Pipeline mode (M3)
        // Two pre-allocated buffer slots cycling between Encode and Write threads
        let write_state = Arc::new((
            Mutex::new(WriteHandoff {
                ready: None,
                free: vec![Box::new(EncodedFrame::new()), Box::new(EncodedFrame::new())],
                shutdown: false,
            }),
            Condvar::new(),
        ));

        let write_state_encode = Arc::clone(&write_state);
        let write_state_worker = Arc::clone(&write_state);
        let state_clone = Arc::clone(&async_state);
        let metrics_encode = Arc::clone(&metrics);
        let metrics_write = Arc::clone(&metrics);
        let mock_enc_delay = Arc::clone(&mock_encode_delay_ms);
        let mock_wr_delay = Arc::clone(&mock_write_delay_ms);

        let encode_handle = std::thread::Builder::new()
            .name("termway-async-render".into())
            .spawn(move || {
                let mut sixel_stream = SixelStreamEncoder::new();
                let mut sixel_out = Vec::new();
                let mut kitty_stream = KittyStreamEncoder::new();
                kitty_stream.set_use_jpeg(use_jpeg);
                kitty_stream.set_jpeg_quality(jpeg_quality);
                let mut kitty_out = Vec::new();

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

                    // Acquire a free buffer slot (ping-pong double-buffering)
                    let slot_wait_start = std::time::Instant::now();
                    let slot_wait_start_us = if crate::trace::enabled() { crate::trace::now_us() } else { 0 };
                    let mut slot = {
                        let (w_lock, w_cvar) = &*write_state_encode;
                        let mut w_guard = w_lock.lock().unwrap();
                        while w_guard.free.is_empty() && !w_guard.shutdown {
                            w_guard = w_cvar.wait(w_guard).unwrap();
                        }
                        if w_guard.shutdown && w_guard.free.is_empty() {
                            break;
                        }
                        w_guard.free.pop().unwrap()
                    };
                    let slot_stall_us = slot_wait_start.elapsed().as_micros() as u64;
                    if slot_stall_us > 100 {
                        metrics_encode.total_encode_stall_us.fetch_add(slot_stall_us, Ordering::Relaxed);
                        metrics_encode.max_encode_stall_us.fetch_max(slot_stall_us, Ordering::Relaxed);
                        if crate::trace::enabled() {
                            crate::trace::slice(
                                crate::trace::ThreadRole::AsyncRender,
                                crate::trace::EventId::EncodeStall,
                                slot_wait_start_us,
                                slot_stall_us as u32,
                                job.frame_id,
                                0,
                                0,
                                0,
                            );
                        }
                    }

                    let delay = mock_enc_delay.load(Ordering::Relaxed);
                    if delay > 0 {
                        std::thread::sleep(std::time::Duration::from_millis(delay));
                    }

                    let encode_start = std::time::Instant::now();
                    let enc_start_us = if crate::trace::enabled() { crate::trace::now_us() } else { 0 };
                    let mut is_partial_frame = false;
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
                            metrics_encode.partial_frames.fetch_add(1, Ordering::Relaxed);
                            is_partial_frame = true;
                        }
                        kitty_out.len()
                    };

                    let encode_dur = encode_start.elapsed();
                    let encode_us = encode_dur.as_micros() as u64;
                    metrics_encode.jobs_rendered.fetch_add(1, Ordering::Relaxed);
                    metrics_encode.total_encode_us.fetch_add(encode_us, Ordering::Relaxed);
                    metrics_encode.max_encode_us.fetch_max(encode_us, Ordering::Relaxed);
                    metrics_encode.record_encode_us(encode_us);
                    metrics_encode.total_bytes_written.fetch_add(bytes_encoded as u64, Ordering::Relaxed);

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

                    let visible_overlay = crate::compositor::Taskbar::format_overlay_text(mode_desc);
                    let overlay_len = crate::compositor::Taskbar::overlay_width(mode_desc);
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
                        for r in 2..=6 {
                            clear_seq.push_str(&format!("\x1b[{};1H\x1b[2K", r));
                        }
                    }
                    prev_hud_shown = job.show_hud;

                    let hud_seq = if job.show_hud && job.term_cols >= 20 && job.term_rows >= 6 {
                        let res_str = format!("{}x{}", job.width, job.height);
                        let mut lines = vec![
                            format!(" {:>8.1} FPS ", fps),
                            format!(" {:>12} ", res_str),
                            format!(" {:>7.1} KB/s ", kb_per_sec),
                            format!(" {:>9.1} ms ", last_encode_ms),
                        ];
                        if crate::trace::enabled() {
                            let ovf = crate::trace::ring::total_overflow();
                            lines.push(format!(" Ovf:{:>7} ", ovf));
                        }
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

                    let assemble_start = std::time::Instant::now();
                    let assemble_start_us = if crate::trace::enabled() { crate::trace::now_us() } else { 0 };

                    slot.frame_id = job.frame_id;
                    slot.is_partial = is_partial_frame;
                    slot.damage_count = job.damage.as_ref().map(|d| d.len() as u32).unwrap_or(0);

                    slot.prefix.clear();
                    slot.prefix.extend_from_slice(b"\x1b[?2026h");
                    slot.prefix.extend_from_slice(clear_seq.as_bytes());
                    if use_sixel {
                        slot.prefix.extend_from_slice(b"\x1b[H");
                    }

                    slot.suffix.clear();
                    slot.suffix.extend_from_slice(taskbar_seq.as_bytes());
                    slot.suffix.extend_from_slice(overlay_seq.as_bytes());
                    slot.suffix.extend_from_slice(hud_seq.as_bytes());
                    slot.suffix.extend_from_slice(b"\x1b[?2026l");

                    // Zero-copy swap payload buffer with encoder output
                    if use_sixel {
                        std::mem::swap(&mut slot.payload, &mut sixel_out);
                    } else {
                        std::mem::swap(&mut slot.payload, &mut kitty_out);
                    }

                    let assemble_us = assemble_start.elapsed().as_micros() as u64;

                    if crate::trace::enabled() {
                        crate::trace::slice(
                            crate::trace::ThreadRole::AsyncRender,
                            crate::trace::EventId::BackendEncode,
                            enc_start_us,
                            encode_us as u32,
                            job.frame_id,
                            bytes_encoded as u32,
                            job.damage.as_ref().map(|d| d.len() as u32).unwrap_or(0),
                            is_partial_frame as u16,
                        );
                        crate::trace::slice(
                            crate::trace::ThreadRole::AsyncRender,
                            crate::trace::EventId::FramePrepare,
                            assemble_start_us,
                            assemble_us as u32,
                            job.frame_id,
                            (slot.prefix.len() + slot.suffix.len()) as u32,
                            slot.payload.len() as u32,
                            job.show_hud as u16,
                        );
                    }

                    // Return pixel buffer to main thread for recycling
                    {
                        let mut guard = lock.lock().unwrap();
                        guard.spare_buffer = Some(job.pixels);
                    }

                    // Rendezvous handoff to Write thread: wait until ready is empty (INV-1)
                    let r_wait_start = std::time::Instant::now();
                    let r_wait_start_us = if crate::trace::enabled() { crate::trace::now_us() } else { 0 };
                    {
                        let (w_lock, w_cvar) = &*write_state_encode;
                        let mut w_guard = w_lock.lock().unwrap();
                        while w_guard.ready.is_some() && !w_guard.shutdown {
                            w_guard = w_cvar.wait(w_guard).unwrap();
                        }
                        let r_stall_us = r_wait_start.elapsed().as_micros() as u64;
                        if r_stall_us > 100 {
                            metrics_encode.total_encode_stall_us.fetch_add(r_stall_us, Ordering::Relaxed);
                            metrics_encode.max_encode_stall_us.fetch_max(r_stall_us, Ordering::Relaxed);
                            if crate::trace::enabled() {
                                crate::trace::slice(
                                    crate::trace::ThreadRole::AsyncRender,
                                    crate::trace::EventId::EncodeStall,
                                    r_wait_start_us,
                                    r_stall_us as u32,
                                    slot.frame_id,
                                    0,
                                    0,
                                    0,
                                );
                            }
                        }
                        w_guard.ready = Some(slot);
                        w_cvar.notify_all();
                    }
                }

                // Signal Write thread on shutdown
                {
                    let (w_lock, w_cvar) = &*write_state_encode;
                    let mut w_guard = w_lock.lock().unwrap();
                    w_guard.shutdown = true;
                    w_cvar.notify_all();
                }
            })
            .ok();

        let write_handle = std::thread::Builder::new()
            .name("termway-async-write".into())
            .spawn(move || {
                let stdout = io::stdout();
                let (w_lock, w_cvar) = &*write_state_worker;

                loop {
                    let idle_start = std::time::Instant::now();
                    let idle_start_us = if crate::trace::enabled() { crate::trace::now_us() } else { 0 };
                    let slot = {
                        let mut w_guard = w_lock.lock().unwrap();
                        while w_guard.ready.is_none() && !w_guard.shutdown {
                            w_guard = w_cvar.wait(w_guard).unwrap();
                        }
                        if w_guard.ready.is_none() && w_guard.shutdown {
                            break;
                        }
                        let s = w_guard.ready.take().unwrap();
                        w_cvar.notify_all(); // Notify encode thread that ready is now None
                        s
                    };

                    let idle_us = idle_start.elapsed().as_micros() as u64;
                    if idle_us > 100 {
                        metrics_write.total_write_idle_us.fetch_add(idle_us, Ordering::Relaxed);
                        if crate::trace::enabled() {
                            crate::trace::slice(
                                crate::trace::ThreadRole::AsyncWrite,
                                crate::trace::EventId::WriteIdle,
                                idle_start_us,
                                idle_us as u32,
                                slot.frame_id,
                                0,
                                0,
                                0,
                            );
                        }
                    }

                    let delay = mock_wr_delay.load(Ordering::Relaxed);
                    if delay > 0 {
                        std::thread::sleep(std::time::Duration::from_millis(delay));
                    }

                    let mut tiocoutq: libc::c_int = 0;
                    if crate::trace::enabled() {
                        unsafe {
                            let _ = libc::ioctl(output_fd, libc::TIOCOUTQ, &mut tiocoutq);
                        }
                    }

                    let write_start = std::time::Instant::now();
                    let write_start_us = if crate::trace::enabled() { crate::trace::now_us() } else { 0 };
                    let total_bytes = slot.total_bytes();
                    let mut syscall_stats = crate::backend::vectored::WritevStats::default();

                    {
                        let mut out_guard = stdout.lock();
                        let _ = out_guard.flush();
                        let mut slices = slot.io_slices();
                        if let Ok(stats) = writev_all(output_fd, &mut slices) {
                            syscall_stats = stats;
                        }
                    }

                    let write_us = write_start.elapsed().as_micros() as u64;
                    metrics_write.frames_written.fetch_add(1, Ordering::Relaxed);
                    metrics_write.total_write_us.fetch_add(write_us, Ordering::Relaxed);
                    metrics_write.max_write_us.fetch_max(write_us, Ordering::Relaxed);
                    metrics_write.record_write_us(write_us);

                    if crate::trace::enabled() {
                        let iovcnt = 3u32;
                        let syscalls = (syscall_stats.syscalls & 0x7F) << 8;
                        crate::trace::slice(
                            crate::trace::ThreadRole::AsyncWrite,
                            crate::trace::EventId::BackendWrite,
                            write_start_us,
                            write_us as u32,
                            slot.frame_id,
                            total_bytes as u32,
                            tiocoutq.max(0) as u32,
                            (iovcnt | syscalls) as u16,
                        );
                        if slot.frame_id > 0 {
                            crate::trace::flow_end(
                                crate::trace::ThreadRole::AsyncWrite,
                                crate::trace::EventId::FrameComplete,
                                write_start_us + write_us,
                                slot.frame_id,
                            );
                        }
                    }

                    // Return slot to free pool
                    {
                        let mut w_guard = w_lock.lock().unwrap();
                        w_guard.free.push(slot);
                        w_cvar.notify_all(); // Notify encode thread that a free slot is available!
                    }
                }
            })
            .ok();

        Self {
            term_size,
            use_sixel,
            use_jpeg,
            show_hud: false,
            needs_clear: false,
            async_state,
            write_state: Some(write_state),
            metrics,
            encode_handle,
            write_handle,
            output_fd,
            mock_encode_delay_ms,
            mock_write_delay_ms,
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

    pub fn output_fd(&self) -> RawFd {
        self.output_fd
    }

    pub fn is_sixel(&self) -> bool {
        self.use_sixel
    }

    pub fn is_jpeg(&self) -> bool {
        self.use_jpeg
    }

    pub fn name(&self) -> &'static str {
        if self.use_sixel {
            "Sixel"
        } else if self.use_jpeg {
            "Kitty-JPEG"
        } else {
            "Kitty"
        }
    }

    pub fn metrics(&self) -> &Arc<BackendMetrics> {
        &self.metrics
    }

    pub fn render_pixels_with_damage(
        &mut self,
        pixels: &[u8],
        width: u32,
        height: u32,
        taskbar: Option<&str>,
        damage: Option<Vec<DamageRect>>,
    ) -> io::Result<()> {
        let frame_id = if crate::trace::enabled() { crate::trace::frame::alloc_frame_id() } else { 0 };
        self.render_pixels_with_damage_and_id(pixels, width, height, taskbar, damage, frame_id)
    }

    pub fn render_pixels_with_damage_and_id(
        &mut self,
        pixels: &[u8],
        width: u32,
        height: u32,
        taskbar: Option<&str>,
        damage: Option<Vec<DamageRect>>,
        frame_id: u32,
    ) -> io::Result<()> {
        self.metrics.jobs_submitted.fetch_add(1, Ordering::Relaxed);
        let now_us = if crate::trace::enabled() { crate::trace::now_us() } else { 0 };
        let (lock, cvar) = &*self.async_state;
        let mut guard = lock.lock().unwrap();

        let needs_clear = self.needs_clear;
        self.needs_clear = false;

        if let Some(ref mut job) = guard.pending_job {
            // Worker is still busy encoding/flushing previous frame.
            // Drop outdated pending frame and replace with this latest frame (Frame Dropping!).
            self.metrics.frames_dropped.fetch_add(1, Ordering::Relaxed);
            if crate::trace::enabled() {
                let pending_age_us = now_us.saturating_sub(job.enqueue_ts_us) as u32;
                crate::trace::instant(
                    crate::trace::ThreadRole::Main,
                    crate::trace::EventId::BackendDrop,
                    now_us,
                    job.frame_id,
                    frame_id,
                    pending_age_us,
                    0,
                );
            }
            job.frame_id = frame_id;
            job.enqueue_ts_us = now_us;
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
                frame_id,
                enqueue_ts_us: now_us,
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
        // 1. Signal Encode thread to shutdown
        {
            let (lock, cvar) = &*self.async_state;
            let mut guard = lock.lock().unwrap();
            guard.shutdown = true;
            cvar.notify_one();
        }
        // 2. Join Encode thread (it drains pending_job and notifies Write thread)
        if let Some(handle) = self.encode_handle.take() {
            let _ = handle.join();
        }
        // 3. For pipeline mode: signal write_state and join Write thread
        if let Some(ref write_state) = self.write_state {
            let (w_lock, w_cvar) = &**write_state;
            let mut w_guard = w_lock.lock().unwrap();
            w_guard.shutdown = true;
            w_cvar.notify_all();
        }
        if let Some(handle) = self.write_handle.take() {
            let _ = handle.join();
        }
    }
}

