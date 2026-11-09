use std::time::Instant;
use termway::display::DamageTracker;
use termway::protocol::{KittyStreamEncoder, SixelStreamEncoder};

#[derive(Debug, Clone, Copy)]
pub enum Workload {
    Idle,
    CursorBlink,
    PartialScroll,
    FullscreenRedraw,
}

impl Workload {
    pub fn name(&self) -> &'static str {
        match self {
            Workload::Idle => "Idle (0% change)",
            Workload::CursorBlink => "Cursor/Typing (16x32px change)",
            Workload::PartialScroll => "Partial UI / Scroll (30% change)",
            Workload::FullscreenRedraw => "Fullscreen (100% change)",
        }
    }

    pub fn apply(&self, pixels: &mut [u8], width: u32, height: u32, frame_idx: usize) {
        match self {
            Workload::Idle => {
                // No change
            }
            Workload::CursorBlink => {
                // Toggle a 16x32 cursor rectangle at (100, 100)
                let cx = 100u32.min(width.saturating_sub(16));
                let cy = 100u32.min(height.saturating_sub(32));
                let val = if frame_idx % 2 == 0 { 255u8 } else { 40u8 };
                for y in cy..cy + 32 {
                    for x in cx..cx + 16 {
                        let idx = ((y * width + x) * 4) as usize;
                        pixels[idx] = val;
                        pixels[idx + 1] = val;
                        pixels[idx + 2] = val;
                        pixels[idx + 3] = 255;
                    }
                }
            }
            Workload::PartialScroll => {
                // Invalidate middle 30% of lines
                let start_y = height * 35 / 100;
                let end_y = height * 65 / 100;
                for y in start_y..end_y {
                    for x in 0..width {
                        let idx = ((y * width + x) * 4) as usize;
                        pixels[idx] = ((x + frame_idx as u32) % 256) as u8;
                        pixels[idx + 1] = ((y + frame_idx as u32 * 2) % 256) as u8;
                        pixels[idx + 2] = 180;
                        pixels[idx + 3] = 255;
                    }
                }
            }
            Workload::FullscreenRedraw => {
                // Invalidate 100% of the screen
                for y in 0..height {
                    for x in 0..width {
                        let idx = ((y * width + x) * 4) as usize;
                        pixels[idx] = ((x + frame_idx as u32) % 256) as u8;
                        pixels[idx + 1] = ((y + frame_idx as u32) % 256) as u8;
                        pixels[idx + 2] = (((x + y) + frame_idx as u32) % 256) as u8;
                        pixels[idx + 3] = 255;
                    }
                }
            }
        }
    }
}

fn create_base_frame(width: u32, height: u32) -> Vec<u8> {
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    for y in 0..height {
        for x in 0..width {
            let idx = ((y * width + x) * 4) as usize;
            pixels[idx] = (x % 200) as u8;
            pixels[idx + 1] = (y % 200) as u8;
            pixels[idx + 2] = 64;
            pixels[idx + 3] = 255;
        }
    }
    pixels
}

pub struct StageBenchmarkResult {
    pub workload: &'static str,
    pub resolution: (u32, u32),
    pub damage_time_us: f64,
    pub damage_rect_count: usize,
    pub sixel_time_ms: f64,
    pub sixel_bytes: usize,
    pub kitty_time_ms: f64,
    pub kitty_bytes: usize,
}

pub fn run_scenario(
    workload: Workload,
    width: u32,
    height: u32,
    iterations: usize,
) -> StageBenchmarkResult {
    let mut tracker = DamageTracker::new(width, height, 32);
    let base_frame = create_base_frame(width, height);

    // Warmup tracker
    let _ = tracker.track_damage(&base_frame);

    // Streaming encoders and buffers
    let mut sixel_stream = SixelStreamEncoder::new();
    let mut sixel_out = Vec::with_capacity((width * height) as usize / 2);
    let mut kitty_stream = KittyStreamEncoder::new();
    let mut kitty_out = Vec::with_capacity((width * height) as usize / 2);

    // Warmup initial frame
    sixel_stream.encode_frame_into(&base_frame, width, height, None, &mut sixel_out);
    kitty_stream.encode_fullscreen_frame_into(
        &base_frame,
        width,
        height,
        (width / 10) as u16,
        (height / 20) as u16,
        1,
        &mut kitty_out,
    );

    let mut total_damage_us = 0.0;
    let mut total_damage_rects = 0;
    let mut total_sixel_ms = 0.0;
    let mut sixel_bytes = 0;
    let mut total_kitty_ms = 0.0;
    let mut kitty_bytes = 0;

    for i in 1..=iterations {
        let mut frame = base_frame.clone();
        workload.apply(&mut frame, width, height, i);

        // 1. Damage Tracker stage
        let t_d0 = Instant::now();
        let damage = tracker.track_damage(&frame);
        let d_dur = t_d0.elapsed();
        total_damage_us += d_dur.as_secs_f64() * 1_000_000.0;
        total_damage_rects = damage.len();

        // 2. Sixel Encoding stage (utilizing damage rects)
        let t_s0 = Instant::now();
        sixel_stream.encode_frame_into(
            &frame,
            width,
            height,
            Some(&damage),
            &mut sixel_out,
        );
        let s_dur = t_s0.elapsed();
        total_sixel_ms += s_dur.as_secs_f64() * 1_000.0;
        sixel_bytes = sixel_out.len();

        // 3. Kitty Encoding stage (zero-allocation stateful stream with partial placement)
        let t_k0 = Instant::now();
        kitty_stream.encode_frame_into(
            &frame,
            width,
            height,
            (width / 10) as u16,
            (height / 20) as u16,
            10,
            20,
            Some(&damage),
            false,
            &mut kitty_out,
        );
        let k_dur = t_k0.elapsed();
        total_kitty_ms += k_dur.as_secs_f64() * 1_000.0;
        kitty_bytes = kitty_out.len();
    }

    StageBenchmarkResult {
        workload: workload.name(),
        resolution: (width, height),
        damage_time_us: total_damage_us / (iterations as f64),
        damage_rect_count: total_damage_rects,
        sixel_time_ms: total_sixel_ms / (iterations as f64),
        sixel_bytes,
        kitty_time_ms: total_kitty_ms / (iterations as f64),
        kitty_bytes,
    }
}

#[test]
fn benchmark_pipeline_baseline() {
    eprintln!("\n==========================================================================================");
    eprintln!(" TERMWAY PIPELINE STAGE-BY-STAGE BENCHMARK (BASELINE)");
    eprintln!("==========================================================================================");

    let resolutions = [(1280u32, 720u32), (1920u32, 1080u32)];
    let workloads = [
        Workload::Idle,
        Workload::CursorBlink,
        Workload::PartialScroll,
        Workload::FullscreenRedraw,
    ];

    for &(w, h) in &resolutions {
        eprintln!("\n[RESOLUTION: {}x{} ({:.2} MP)]", w, h, (w * h) as f64 / 1_000_000.0);
        eprintln!(
            "{:<32} | {:>10} | {:>12} | {:>12} | {:>12} | {:>12}",
            "Workload", "Damage(μs)", "Sixel(ms)", "Sixel(KB)", "Kitty(ms)", "Kitty(KB)"
        );
        eprintln!("{:-<32}-+-{:-<10}-+-{:-<12}-+-{:-<12}-+-{:-<12}-+-{:-<12}", "", "", "", "", "", "");

        let iters = if (w * h) > 1_500_000 { 5 } else { 10 };
        for &wl in &workloads {
            let res = run_scenario(wl, w, h, iters);
            eprintln!(
                "{:<32} | {:>8.1} μs | {:>9.2} ms | {:>9.1} KB | {:>9.2} ms | {:>9.1} KB",
                res.workload,
                res.damage_time_us,
                res.sixel_time_ms,
                res.sixel_bytes as f64 / 1024.0,
                res.kitty_time_ms,
                res.kitty_bytes as f64 / 1024.0,
            );
        }
    }
    eprintln!("==========================================================================================\n");
}

#[test]
fn benchmark_jpeg_1080p_scalar_vs_simd() {
    use termway::protocol::jpeg_simd::JpegBackend;
    use termway::protocol::JpegEncoder;

    let host_backend = JpegBackend::detect();
    if host_backend == JpegBackend::Scalar {
        eprintln!("Host CPU does not support AVX2/NEON SIMD; skipping comparison benchmark.");
        return;
    }

    let w = 1920u32;
    let h = 1080u32;
    let rgba = create_base_frame(w, h);

    let enc_scalar = JpegEncoder::with_backend(75, JpegBackend::Scalar);
    let enc_simd = JpegEncoder::with_backend(75, host_backend);

    let mut out_scalar = Vec::with_capacity(512 * 1024);
    let mut out_simd = Vec::with_capacity(512 * 1024);

    // Warm-up
    enc_scalar.encode_subimage_into(&rgba, w, 0, 0, w, h, &mut out_scalar);
    enc_simd.encode_subimage_into(&rgba, w, 0, 0, w, h, &mut out_simd);

    let iters = 5;

    let t0 = Instant::now();
    for _ in 0..iters {
        enc_scalar.encode_subimage_into(&rgba, w, 0, 0, w, h, &mut out_scalar);
    }
    let elapsed_scalar_ms = t0.elapsed().as_secs_f64() * 1000.0 / iters as f64;

    let t1 = Instant::now();
    for _ in 0..iters {
        enc_simd.encode_subimage_into(&rgba, w, 0, 0, w, h, &mut out_simd);
    }
    let elapsed_simd_ms = t1.elapsed().as_secs_f64() * 1000.0 / iters as f64;

    let speedup = elapsed_scalar_ms / elapsed_simd_ms;
    eprintln!("\n==========================================================================================");
    eprintln!(" JPEG 1080p (1920x1080) BENCHMARK: SCALAR vs {:?}", host_backend);
    eprintln!("==========================================================================================");
    eprintln!(" Scalar time: {:>7.2} ms (size: {} KB)", elapsed_scalar_ms, out_scalar.len() / 1024);
    eprintln!(" SIMD time:   {:>7.2} ms (size: {} KB)", elapsed_simd_ms, out_simd.len() / 1024);
    eprintln!(" Speedup:     {:>7.2}x", speedup);
    eprintln!("==========================================================================================\n");

    assert_eq!(out_scalar, out_simd, "Scalar and SIMD 1080p output must be identical");
}

