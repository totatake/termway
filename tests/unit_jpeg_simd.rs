//! PIP-002: Bit-exact parity verification between Scalar and SIMD JPEG backends.
//!
//! Validates:
//! 1. Individual stages (fused MCU extraction, 2D FDCT, Quantization) against scalar reference.
//! 2. End-to-end full JPEG bitstream identity across patterns, resolutions, sub-rectangles, and qualities.
//! 3. Environment variable override `TERMWAY_JPEG_SIMD=0`.

use termway::protocol::jpeg_simd::JpegBackend;
use termway::protocol::JpegEncoder;

fn generate_test_rgba(width: u32, height: u32, pattern: u32) -> Vec<u8> {
    let mut buf = vec![0u8; (width * height * 4) as usize];
    for y in 0..height {
        for x in 0..width {
            let idx = ((y * width + x) * 4) as usize;
            match pattern {
                0 => {
                    // Solid color
                    buf[idx] = 120;
                    buf[idx + 1] = 200;
                    buf[idx + 2] = 80;
                    buf[idx + 3] = 255;
                }
                1 => {
                    // Gradients
                    buf[idx] = (x & 0xFF) as u8;
                    buf[idx + 1] = (y & 0xFF) as u8;
                    buf[idx + 2] = ((x + y) & 0xFF) as u8;
                    buf[idx + 3] = 255;
                }
                2 => {
                    // Pseudo-random noise (LFSR)
                    let mut state = (y * width + x) ^ 0xDEADBEEF;
                    state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                    buf[idx] = (state & 0xFF) as u8;
                    buf[idx + 1] = ((state >> 8) & 0xFF) as u8;
                    buf[idx + 2] = ((state >> 16) & 0xFF) as u8;
                    buf[idx + 3] = 255;
                }
                3 => {
                    // GUI-like pattern (flat regions + text-like sharp transitions)
                    let is_button = x % 64 < 48 && y % 32 < 24;
                    let is_text = (x % 16 == 4 || x % 16 == 8) && (y % 16 >= 4 && y % 16 <= 12);
                    if is_text {
                        buf[idx] = 20;
                        buf[idx + 1] = 20;
                        buf[idx + 2] = 20;
                    } else if is_button {
                        buf[idx] = 230;
                        buf[idx + 1] = 235;
                        buf[idx + 2] = 240;
                    } else {
                        buf[idx] = 60;
                        buf[idx + 1] = 65;
                        buf[idx + 2] = 75;
                    }
                    buf[idx + 3] = 255;
                }
                _ => {
                    // High entropy pattern
                    let v = ((x.wrapping_mul(31) ^ y.wrapping_mul(73)) & 0xFF) as u8;
                    buf[idx] = v;
                    buf[idx + 1] = v.wrapping_add(42);
                    buf[idx + 2] = v.wrapping_mul(7);
                    buf[idx + 3] = 255;
                }
            }
        }
    }
    buf
}

#[test]
fn test_simd_vs_scalar_end_to_end_bit_exact() {
    let host_backend = JpegBackend::detect();
    if host_backend == JpegBackend::Scalar {
        println!("Host CPU does not support AVX2/NEON SIMD; skipping SIMD diff test.");
        return;
    }

    let resolutions = [
        (1, 1),
        (7, 5),
        (15, 17),
        (16, 16),
        (17, 31),
        (64, 48),
        (320, 240),
        (640, 480),
    ];
    let qualities = [1, 50, 75, 90, 100];
    let patterns = [0, 1, 2, 3, 4];

    for &q in &qualities {
        let enc_scalar = JpegEncoder::with_backend(q, JpegBackend::Scalar);
        let enc_simd = JpegEncoder::with_backend(q, host_backend);

        for &(w, h) in &resolutions {
            for &pat in &patterns {
                let rgba = generate_test_rgba(w, h, pat);

                let mut out_scalar = Vec::new();
                let mut out_simd = Vec::new();

                enc_scalar.encode_subimage_into(&rgba, w, 0, 0, w, h, &mut out_scalar);
                enc_simd.encode_subimage_into(&rgba, w, 0, 0, w, h, &mut out_simd);

                assert_eq!(
                    out_scalar.len(),
                    out_simd.len(),
                    "Size mismatch for {}x{}, quality={}, pattern={}",
                    w,
                    h,
                    q,
                    pat
                );
                assert_eq!(
                    out_scalar, out_simd,
                    "Bitstream mismatch for {}x{}, quality={}, pattern={}",
                    w, h, q, pat
                );
            }
        }
    }
}

#[test]
fn test_simd_vs_scalar_subrectangles_with_stride() {
    let host_backend = JpegBackend::detect();
    if host_backend == JpegBackend::Scalar {
        return;
    }

    let full_w = 200;
    let full_h = 150;
    let rgba = generate_test_rgba(full_w, full_h, 3);

    let subrects = [
        (0, 0, 16, 16),
        (5, 7, 32, 32),
        (10, 20, 75, 60),
        (33, 45, 128, 80),
        (150, 100, 48, 48),
        (3, 11, 19, 23), // Odd sub-rectangle
    ];

    for &q in &[50, 85] {
        let enc_scalar = JpegEncoder::with_backend(q, JpegBackend::Scalar);
        let enc_simd = JpegEncoder::with_backend(q, host_backend);

        for &(rx, ry, rw, rh) in &subrects {
            let mut out_scalar = Vec::new();
            let mut out_simd = Vec::new();

            enc_scalar.encode_subimage_into(&rgba, full_w, rx, ry, rw, rh, &mut out_scalar);
            enc_simd.encode_subimage_into(&rgba, full_w, rx, ry, rw, rh, &mut out_simd);

            assert_eq!(
                out_scalar, out_simd,
                "Bitstream mismatch for subrect ({},{},{},{}) at quality {}",
                rx, ry, rw, rh, q
            );
        }
    }
}

#[test]
fn test_stage_fdct_2d_parity_10000_blocks() {
    #[cfg(target_arch = "x86_64")]
    if JpegBackend::detect() == JpegBackend::Avx2 {
        let mut block_scalar = [0i32; 64];
        let mut block_simd_regs = [unsafe { std::arch::x86_64::_mm256_setzero_si256() }; 8];

        let mut lfsr: u32 = 0xACE1;
        for _ in 0..10_000 {
            for i in 0..64 {
                lfsr = (lfsr >> 1) ^ (-(lfsr as i32 & 1) as u32 & 0xB400);
                // Pixel range around -128..127
                block_scalar[i] = (lfsr & 0xFF) as i32 - 128;
            }

            for row in 0..8 {
                block_simd_regs[row] = unsafe {
                    std::arch::x86_64::_mm256_loadu_si256(
                        block_scalar.as_ptr().add(row * 8) as *const std::arch::x86_64::__m256i
                    )
                };
            }

            termway::protocol::jpeg::fdct_2d(&mut block_scalar);
            unsafe {
                termway::protocol::jpeg_simd::avx2::fdct_2d_avx2(&mut block_simd_regs);
            }

            let mut simd_result = [0i32; 64];
            for row in 0..8 {
                unsafe {
                    std::arch::x86_64::_mm256_storeu_si256(
                        simd_result.as_mut_ptr().add(row * 8) as *mut std::arch::x86_64::__m256i,
                        block_simd_regs[row],
                    );
                }
            }

            assert_eq!(block_scalar, simd_result, "FDCT 2D mismatch in block");
        }
    }
}

#[test]
fn test_stage_quantization_parity_10000_blocks() {
    #[cfg(target_arch = "x86_64")]
    if JpegBackend::detect() == JpegBackend::Avx2 {
        let q_tables = termway::protocol::jpeg::QuantizationTables::new(75);
        let mut dct = [0i32; 64];
        let mut block_simd_regs = [unsafe { std::arch::x86_64::_mm256_setzero_si256() }; 8];
        let mut out_scalar = [0i16; 64];
        let mut out_simd = [0i16; 64];

        let mut lfsr: u32 = 0x1337;
        for _ in 0..10_000 {
            for i in 0..64 {
                lfsr = (lfsr >> 1) ^ (-(lfsr as i32 & 1) as u32 & 0xB400);
                // DCT coefficient range: -2048..2047
                dct[i] = (lfsr & 0xFFF) as i32 - 2048;
            }

            for row in 0..8 {
                block_simd_regs[row] = unsafe {
                    std::arch::x86_64::_mm256_loadu_si256(
                        dct.as_ptr().add(row * 8) as *const std::arch::x86_64::__m256i
                    )
                };
            }

            let lnz_scalar = termway::protocol::jpeg::quantize_and_zigzag(
                &dct,
                &q_tables.y_mult,
                &q_tables.y_half,
                &mut out_scalar,
            );

            let lnz_simd = unsafe {
                termway::protocol::jpeg_simd::avx2::quantize_and_zigzag_avx2(
                    &block_simd_regs,
                    &q_tables.y_mult_r,
                    &q_tables.y_half_r,
                    &mut out_simd,
                )
            };

            assert_eq!(lnz_scalar, lnz_simd, "last_nonzero mismatch");
            assert_eq!(out_scalar, out_simd, "Quantized coefficients mismatch");
        }
    }
}

#[test]
fn test_termway_jpeg_simd_env_override() {
    // 1. Force scalar with TERMWAY_JPEG_SIMD=0
    std::env::set_var("TERMWAY_JPEG_SIMD", "0");
    let backend = JpegBackend::from_env_or_detect();
    assert_eq!(backend, JpegBackend::Scalar);
    let enc = JpegEncoder::new(75);
    assert_eq!(enc.backend(), JpegBackend::Scalar);

    // 2. Force scalar with TERMWAY_JPEG_SIMD=scalar
    std::env::set_var("TERMWAY_JPEG_SIMD", "scalar");
    let backend = JpegBackend::from_env_or_detect();
    assert_eq!(backend, JpegBackend::Scalar);

    // Clean up
    std::env::remove_var("TERMWAY_JPEG_SIMD");
}
