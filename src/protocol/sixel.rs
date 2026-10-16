use crate::display::DamageRect;

/// Sixel Graphics Protocol encoder.
pub struct SixelEncoder;

impl SixelEncoder {
    /// Encode a full-screen RGBA frame buffer to Sixel format using an RGB332 fixed palette.
    pub fn encode_fullscreen_frame(rgba: &[u8], width: u32, height: u32) -> String {
        let mut out = Vec::with_capacity((width * height) as usize / 2);
        let mut quantized = Vec::new();
        Self::encode_fullscreen_frame_into(rgba, width, height, &mut out, &mut quantized);
        String::from_utf8(out).unwrap_or_default()
    }

    /// Optimized version that reuses preallocated output and quantization buffers (zero-allocation).
    pub fn encode_fullscreen_frame_into(
        rgba: &[u8],
        width: u32,
        height: u32,
        out: &mut Vec<u8>,
        quantized: &mut Vec<u8>,
    ) {
        out.clear();
        let total_pixels = (width * height) as usize;
        if quantized.len() < total_pixels {
            quantized.resize(total_pixels, 0);
        }

        // Sixel intro (DCS 7;0;q) with 1:1 pixel aspect ratio and raster attributes (P2=0 for non-transparent/clear)
        out.extend_from_slice(b"\x1bP7;0;q\"1;1;");
        Self::emit_u32_ascii(out, width);
        out.push(b';');
        Self::emit_u32_ascii(out, height);

        // 1. Quantize the image to RGB332 and collect used colors globally
        let mut used_colors = [false; 256];
        #[cfg(target_arch = "x86_64")]
        {
            if std::is_x86_feature_detected!("avx2") {
                unsafe {
                    Self::quantize_rgb332_avx2(rgba, &mut quantized[..total_pixels], &mut used_colors);
                }
            } else {
                for (i, pixel) in rgba.chunks_exact(4).take(total_pixels).enumerate() {
                    let idx = Self::quantize_pixel_rgb332(pixel[0], pixel[1], pixel[2]);
                    quantized[i] = idx;
                    used_colors[idx as usize] = true;
                }
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            for (i, pixel) in rgba.chunks_exact(4).take(total_pixels).enumerate() {
                let idx = Self::quantize_pixel_rgb332(pixel[0], pixel[1], pixel[2]);
                quantized[i] = idx;
                used_colors[idx as usize] = true;
            }
        }

        // 2. Emit palette definitions ONLY for used colors
        for (i, &is_used) in used_colors.iter().enumerate() {
            if is_used {
                let r = (i >> 5) & 7;
                let g = (i >> 2) & 7;
                let b = i & 3;
                let r_pct = r * 100 / 7;
                let g_pct = g * 100 / 7;
                // Keep the 8 diagonal grayscale entries (r == g && b == r >> 1) strictly neutral
                // instead of skewing toward blue/purple (b * 100 / 3) or olive.
                let b_pct = if r == g && b == (r >> 1) {
                    r_pct
                } else {
                    b * 100 / 3
                };
                out.push(b'#');
                Self::emit_u8_ascii(out, i as u8);
                out.extend_from_slice(b";2;");
                Self::emit_u8_ascii(out, r_pct as u8);
                out.push(b';');
                Self::emit_u8_ascii(out, g_pct as u8);
                out.push(b';');
                Self::emit_u8_ascii(out, b_pct as u8);
            }
        }

        let w = width as usize;
        let h = height as usize;
        let mut band_colors = Vec::with_capacity(32);
        #[cfg(target_arch = "x86_64")]
        let has_avx2 = std::is_x86_feature_detected!("avx2");

        // 3. Process 6-pixel high bands
        for band_y in (0..height).step_by(6) {
            let start_y = band_y as usize;
            let max_y = std::cmp::min(start_y + 6, h);
            let has_next_band = band_y + 6 < height;

            Self::encode_single_band(
                quantized,
                w,
                h,
                start_y,
                max_y,
                has_next_band,
                out,
                &mut band_colors,
                has_avx2,
            );
        }

        // Sixel outro (String Terminator)
        out.extend_from_slice(b"\x1b\\");
    }

    /// Emits the complete 256-color RGB332 palette definitions once using a precomputed buffer.
    pub fn emit_rgb332_full_palette(out: &mut Vec<u8>) {
        static PALETTE_BYTES: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
        let bytes = PALETTE_BYTES.get_or_init(|| {
            let mut buf = Vec::with_capacity(256 * 18);
            for i in 0..=255u8 {
                let r = ((i >> 5) & 7) as u32;
                let g = ((i >> 2) & 7) as u32;
                let b = (i & 3) as u32;
                let r_pct = (r * 100 / 7) as u8;
                let g_pct = (g * 100 / 7) as u8;
                let b_pct = if r == g && b == (r >> 1) {
                    r_pct
                } else {
                    (b * 100 / 3) as u8
                };
                buf.push(b'#');
                Self::emit_u8_ascii(&mut buf, i);
                buf.extend_from_slice(b";2;");
                Self::emit_u8_ascii(&mut buf, r_pct);
                buf.push(b';');
                Self::emit_u8_ascii(&mut buf, g_pct);
                buf.push(b';');
                Self::emit_u8_ascii(&mut buf, b_pct);
            }
            buf
        });
        out.extend_from_slice(bytes);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn encode_single_band(
        quantized: &[u8],
        w: usize,
        h: usize,
        start_y: usize,
        max_y: usize,
        has_next_band: bool,
        out: &mut Vec<u8>,
        band_colors: &mut Vec<u8>,
        has_avx2: bool,
    ) {
        let num_rows = (max_y - start_y) as u8;
        band_colors.clear();
        let mut band_used = [false; 256];
        let mut band_min_x = [w; 256];
        let mut band_max_x = [0; 256];

        for y in start_y..max_y {
            let row = &quantized[y * w..(y + 1) * w];
            for (x, &color) in row.iter().enumerate() {
                let c = color as usize;
                if !band_used[c] {
                    band_used[c] = true;
                    band_colors.push(color);
                    band_min_x[c] = x;
                    band_max_x[c] = x;
                } else {
                    if x < band_min_x[c] { band_min_x[c] = x; }
                    if x > band_max_x[c] { band_max_x[c] = x; }
                }
            }
        }

        if band_colors.is_empty() {
            if has_next_band {
                out.push(b'-');
            }
            return;
        }

        // Fast-path: entire band is a single solid color
        if band_colors.len() == 1 {
            let c = band_colors[0];
            let mask = (1 << num_rows) - 1;
            out.push(b'#');
            Self::emit_u8_ascii(out, c);
            Self::emit_rle(out, mask + 63, w);
            out.push(b'$');
            if has_next_band {
                out.push(b'-');
            }
            return;
        }

        if num_rows == 6 {
            let r0 = &quantized[start_y * w..(start_y + 1) * w];
            let r1 = &quantized[(start_y + 1) * w..(start_y + 2) * w];
            let r2 = &quantized[(start_y + 2) * w..(start_y + 3) * w];
            let r3 = &quantized[(start_y + 3) * w..(start_y + 4) * w];
            let r4 = &quantized[(start_y + 4) * w..(start_y + 5) * w];
            let r5 = &quantized[(start_y + 5) * w..(start_y + 6) * w];

            for &c_u8 in band_colors.iter() {
                let c = c_u8 as usize;
                let start_col = band_min_x[c];
                let end_col = band_max_x[c] + 1;

                out.push(b'#');
                Self::emit_u8_ascii(out, c_u8);

                if start_col > 0 {
                    Self::emit_rle(out, 63, start_col);
                }

                #[cfg(target_arch = "x86_64")]
                if has_avx2 && end_col >= start_col + 32 {
                    unsafe {
                        Self::encode_band_color_avx2(
                            out, r0, r1, r2, r3, r4, r5, c_u8, start_col, end_col,
                        );
                    }
                    out.push(b'$');
                    continue;
                }

                let mut run_char = 0u8;
                let mut run_len = 0;

                for x in start_col..end_col {
                    let mask = ((r0[x] == c_u8) as u8)
                        | (((r1[x] == c_u8) as u8) << 1)
                        | (((r2[x] == c_u8) as u8) << 2)
                        | (((r3[x] == c_u8) as u8) << 3)
                        | (((r4[x] == c_u8) as u8) << 4)
                        | (((r5[x] == c_u8) as u8) << 5);

                    let ch = mask + 63;
                    if ch == run_char {
                        run_len += 1;
                    } else {
                        Self::emit_rle(out, run_char, run_len);
                        run_char = ch;
                        run_len = 1;
                    }
                }
                if run_char != 63 {
                    Self::emit_rle(out, run_char, run_len);
                }
                out.push(b'$');
            }
        } else {
            let r0 = &quantized[start_y * w..(start_y + 1) * w];
            let r1 = if start_y + 1 < h { &quantized[(start_y + 1) * w..(start_y + 2) * w] } else { &[] };
            let r2 = if start_y + 2 < h { &quantized[(start_y + 2) * w..(start_y + 3) * w] } else { &[] };
            let r3 = if start_y + 3 < h { &quantized[(start_y + 3) * w..(start_y + 4) * w] } else { &[] };
            let r4 = if start_y + 4 < h { &quantized[(start_y + 4) * w..(start_y + 5) * w] } else { &[] };
            let r5 = if start_y + 5 < h { &quantized[(start_y + 5) * w..(start_y + 6) * w] } else { &[] };

            for &c_u8 in band_colors.iter() {
                let c = c_u8 as usize;
                let start_col = band_min_x[c];
                let end_col = band_max_x[c] + 1;

                out.push(b'#');
                Self::emit_u8_ascii(out, c_u8);

                if start_col > 0 {
                    Self::emit_rle(out, 63, start_col);
                }

                let mut run_char = 0u8;
                let mut run_len = 0;

                for x in start_col..end_col {
                    let mut mask = 0u8;
                    if r0[x] == c_u8 { mask |= 1; }
                    if !r1.is_empty() && r1[x] == c_u8 { mask |= 2; }
                    if !r2.is_empty() && r2[x] == c_u8 { mask |= 4; }
                    if !r3.is_empty() && r3[x] == c_u8 { mask |= 8; }
                    if !r4.is_empty() && r4[x] == c_u8 { mask |= 16; }
                    if !r5.is_empty() && r5[x] == c_u8 { mask |= 32; }

                    let ch = mask + 63;
                    if ch == run_char {
                        run_len += 1;
                    } else {
                        Self::emit_rle(out, run_char, run_len);
                        run_char = ch;
                        run_len = 1;
                    }
                }
                if run_char != 63 {
                    Self::emit_rle(out, run_char, run_len);
                }
                out.push(b'$');
            }
        }
        if has_next_band {
            out.push(b'-');
        }
    }

    #[inline(always)]
    fn emit_u8_ascii(out: &mut Vec<u8>, v: u8) {
        if v >= 100 {
            out.push(b'0' + (v / 100));
            out.push(b'0' + ((v / 10) % 10));
            out.push(b'0' + (v % 10));
        } else if v >= 10 {
            out.push(b'0' + (v / 10));
            out.push(b'0' + (v % 10));
        } else {
            out.push(b'0' + v);
        }
    }

    #[inline(always)]
    fn emit_u32_ascii(out: &mut Vec<u8>, mut n: u32) {
        if n == 0 {
            out.push(b'0');
            return;
        }
        let mut buf = [0u8; 10];
        let mut i = 10;
        while n > 0 {
            i -= 1;
            buf[i] = b'0' + (n % 10) as u8;
            n /= 10;
        }
        out.extend_from_slice(&buf[i..]);
    }

    #[inline(always)]
    fn emit_rle(out: &mut Vec<u8>, ch: u8, count: usize) {
        if count == 0 {
            return;
        }
        if count <= 3 {
            for _ in 0..count {
                out.push(ch);
            }
        } else {
            out.push(b'!');
            Self::emit_u32_ascii(out, count as u32);
            out.push(ch);
        }
    }

    #[inline(always)]
    fn quantize_pixel_rgb332(r: u8, g: u8, b: u8) -> u8 {
        let max_c = r.max(g).max(b);
        let min_c = r.min(g).min(b);
        if max_c - min_c <= 12 {
            let luma = g & 0xE0;
            luma | (luma >> 3) | (luma >> 6)
        } else {
            (r & 0xE0) | ((g & 0xE0) >> 3) | (b >> 6)
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx2")]
    unsafe fn quantize_rgb332_avx2(
        rgba: &[u8],
        quantized: &mut [u8],
        used_colors: &mut [bool; 256],
    ) {
        use std::arch::x86_64::*;

        let total_pixels = quantized.len().min(rgba.len() / 4);
        let src_ptr = rgba.as_ptr();
        let dst_ptr = quantized.as_mut_ptr();

        let mask_byte = _mm256_set1_epi32(0x0000_00FF);
        let mask_e0 = _mm256_set1_epi32(0x0000_00E0);
        let gray_thresh = _mm256_set1_epi32(13);
        let shuf = _mm256_setr_epi8(
            0, 4, 8, 12, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
            0, 4, 8, 12, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
        );

        let mut i = 0usize;
        while i + 8 <= total_pixels {
            let v = _mm256_loadu_si256(src_ptr.add(i * 4) as *const __m256i);
            let r8 = _mm256_and_si256(v, mask_byte);
            let g8 = _mm256_and_si256(_mm256_srli_epi32(v, 8), mask_byte);
            let b8 = _mm256_and_si256(_mm256_srli_epi32(v, 16), mask_byte);

            let r_top = _mm256_and_si256(r8, mask_e0);
            let g_top = _mm256_and_si256(g8, mask_e0);
            let b_top = _mm256_srli_epi32(b8, 6);
            let color_idx32 = _mm256_or_si256(_mm256_or_si256(r_top, _mm256_srli_epi32(g_top, 3)), b_top);

            let max_c = _mm256_max_epi32(r8, _mm256_max_epi32(g8, b8));
            let min_c = _mm256_min_epi32(r8, _mm256_min_epi32(g8, b8));
            let is_gray = _mm256_cmpgt_epi32(gray_thresh, _mm256_sub_epi32(max_c, min_c));
            let gray_idx32 = _mm256_or_si256(
                _mm256_or_si256(g_top, _mm256_srli_epi32(g_top, 3)),
                _mm256_srli_epi32(g_top, 6),
            );
            let idx32 = _mm256_blendv_epi8(color_idx32, gray_idx32, is_gray);
            let packed = _mm256_shuffle_epi8(idx32, shuf);

            let lo4 = _mm256_cvtsi256_si32(packed) as u32;
            let hi4 = _mm256_extract_epi32::<4>(packed) as u32;
            let combined = (lo4 as u64) | ((hi4 as u64) << 32);
            (dst_ptr.add(i) as *mut u64).write_unaligned(combined);

            used_colors[(lo4 & 0xFF) as usize] = true;
            used_colors[((lo4 >> 8) & 0xFF) as usize] = true;
            used_colors[((lo4 >> 16) & 0xFF) as usize] = true;
            used_colors[(lo4 >> 24) as usize] = true;
            used_colors[(hi4 & 0xFF) as usize] = true;
            used_colors[((hi4 >> 8) & 0xFF) as usize] = true;
            used_colors[((hi4 >> 16) & 0xFF) as usize] = true;
            used_colors[(hi4 >> 24) as usize] = true;

            i += 8;
        }

        while i < total_pixels {
            let p = i * 4;
            let idx = Self::quantize_pixel_rgb332(rgba[p], rgba[p + 1], rgba[p + 2]);
            quantized[i] = idx;
            used_colors[idx as usize] = true;
            i += 1;
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx2")]
    pub unsafe fn quantize_rgb332_avx2_slice(
        rgba: &[u8],
        quantized: &mut [u8],
    ) {
        use std::arch::x86_64::*;

        let total_pixels = quantized.len().min(rgba.len() / 4);
        let src_ptr = rgba.as_ptr();
        let dst_ptr = quantized.as_mut_ptr();

        let mask_byte = _mm256_set1_epi32(0x0000_00FF);
        let mask_e0 = _mm256_set1_epi32(0x0000_00E0);
        let gray_thresh = _mm256_set1_epi32(13);
        let shuf = _mm256_setr_epi8(
            0, 4, 8, 12, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
            0, 4, 8, 12, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
        );

        let mut i = 0usize;
        while i + 8 <= total_pixels {
            let v = _mm256_loadu_si256(src_ptr.add(i * 4) as *const __m256i);
            let r8 = _mm256_and_si256(v, mask_byte);
            let g8 = _mm256_and_si256(_mm256_srli_epi32(v, 8), mask_byte);
            let b8 = _mm256_and_si256(_mm256_srli_epi32(v, 16), mask_byte);

            let r_top = _mm256_and_si256(r8, mask_e0);
            let g_top = _mm256_and_si256(g8, mask_e0);
            let b_top = _mm256_srli_epi32(b8, 6);
            let color_idx32 = _mm256_or_si256(_mm256_or_si256(r_top, _mm256_srli_epi32(g_top, 3)), b_top);

            let max_c = _mm256_max_epi32(r8, _mm256_max_epi32(g8, b8));
            let min_c = _mm256_min_epi32(r8, _mm256_min_epi32(g8, b8));
            let is_gray = _mm256_cmpgt_epi32(gray_thresh, _mm256_sub_epi32(max_c, min_c));
            let gray_idx32 = _mm256_or_si256(
                _mm256_or_si256(g_top, _mm256_srli_epi32(g_top, 3)),
                _mm256_srli_epi32(g_top, 6),
            );
            let idx32 = _mm256_blendv_epi8(color_idx32, gray_idx32, is_gray);
            let packed = _mm256_shuffle_epi8(idx32, shuf);

            let lo4 = _mm256_cvtsi256_si32(packed) as u32;
            let hi4 = _mm256_extract_epi32::<4>(packed) as u32;
            let combined = (lo4 as u64) | ((hi4 as u64) << 32);
            (dst_ptr.add(i) as *mut u64).write_unaligned(combined);

            i += 8;
        }

        while i < total_pixels {
            let p = i * 4;
            let idx = Self::quantize_pixel_rgb332(rgba[p], rgba[p + 1], rgba[p + 2]);
            quantized[i] = idx;
            i += 1;
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx2")]
    #[allow(clippy::too_many_arguments)]
    unsafe fn encode_band_color_avx2(
        out: &mut Vec<u8>,
        r0: &[u8],
        r1: &[u8],
        r2: &[u8],
        r3: &[u8],
        r4: &[u8],
        r5: &[u8],
        c_u8: u8,
        start_col: usize,
        end_col: usize,
    ) {
        use std::arch::x86_64::*;

        let c_vec = _mm256_set1_epi8(c_u8 as i8);
        let mut run_char = 0u8;
        let mut run_len = 0usize;
        let mut x = start_col;

        while x + 32 <= end_col {
            let m0 = _mm256_movemask_epi8(_mm256_cmpeq_epi8(
                _mm256_loadu_si256(r0.as_ptr().add(x) as *const __m256i),
                c_vec,
            )) as u32;
            let m1 = _mm256_movemask_epi8(_mm256_cmpeq_epi8(
                _mm256_loadu_si256(r1.as_ptr().add(x) as *const __m256i),
                c_vec,
            )) as u32;
            let m2 = _mm256_movemask_epi8(_mm256_cmpeq_epi8(
                _mm256_loadu_si256(r2.as_ptr().add(x) as *const __m256i),
                c_vec,
            )) as u32;
            let m3 = _mm256_movemask_epi8(_mm256_cmpeq_epi8(
                _mm256_loadu_si256(r3.as_ptr().add(x) as *const __m256i),
                c_vec,
            )) as u32;
            let m4 = _mm256_movemask_epi8(_mm256_cmpeq_epi8(
                _mm256_loadu_si256(r4.as_ptr().add(x) as *const __m256i),
                c_vec,
            )) as u32;
            let m5 = _mm256_movemask_epi8(_mm256_cmpeq_epi8(
                _mm256_loadu_si256(r5.as_ptr().add(x) as *const __m256i),
                c_vec,
            )) as u32;

            let any = m0 | m1 | m2 | m3 | m4 | m5;
            if any == 0 {
                if run_char == 63 {
                    run_len += 32;
                } else {
                    Self::emit_rle(out, run_char, run_len);
                    run_char = 63;
                    run_len = 32;
                }
            } else if (m0.wrapping_add(1)
                | m1.wrapping_add(1)
                | m2.wrapping_add(1)
                | m3.wrapping_add(1)
                | m4.wrapping_add(1)
                | m5.wrapping_add(1))
                <= 1
            {
                let mask = ((m0 & 1) as u8)
                    | (((m1 & 1) as u8) << 1)
                    | (((m2 & 1) as u8) << 2)
                    | (((m3 & 1) as u8) << 3)
                    | (((m4 & 1) as u8) << 4)
                    | (((m5 & 1) as u8) << 5);
                let ch = mask + 63;
                if ch == run_char {
                    run_len += 32;
                } else {
                    Self::emit_rle(out, run_char, run_len);
                    run_char = ch;
                    run_len = 32;
                }
            } else {
                let mut b0 = m0;
                let mut b1 = m1;
                let mut b2 = m2;
                let mut b3 = m3;
                let mut b4 = m4;
                let mut b5 = m5;
                let mut b_any = any;
                let mut bit_pos = 0usize;

                while bit_pos < 32 {
                    if b_any == 0 {
                        let rem = 32 - bit_pos;
                        if run_char == 63 {
                            run_len += rem;
                        } else {
                            Self::emit_rle(out, run_char, run_len);
                            run_char = 63;
                            run_len = rem;
                        }
                        break;
                    }

                    let tz = b_any.trailing_zeros() as usize;
                    if tz > 0 {
                        if run_char == 63 {
                            run_len += tz;
                        } else {
                            Self::emit_rle(out, run_char, run_len);
                            run_char = 63;
                            run_len = tz;
                        }
                        b0 >>= tz;
                        b1 >>= tz;
                        b2 >>= tz;
                        b3 >>= tz;
                        b4 >>= tz;
                        b5 >>= tz;
                        b_any >>= tz;
                        bit_pos += tz;
                    }

                    let mask = ((b0 & 1) as u8)
                        | (((b1 & 1) as u8) << 1)
                        | (((b2 & 1) as u8) << 2)
                        | (((b3 & 1) as u8) << 3)
                        | (((b4 & 1) as u8) << 4)
                        | (((b5 & 1) as u8) << 5);
                    let ch = mask + 63;
                    if ch == run_char {
                        run_len += 1;
                    } else {
                        Self::emit_rle(out, run_char, run_len);
                        run_char = ch;
                        run_len = 1;
                    }
                    b0 >>= 1;
                    b1 >>= 1;
                    b2 >>= 1;
                    b3 >>= 1;
                    b4 >>= 1;
                    b5 >>= 1;
                    b_any >>= 1;
                    bit_pos += 1;
                }
            }
            x += 32;
        }

        while x < end_col {
            let mask = ((r0[x] == c_u8) as u8)
                | (((r1[x] == c_u8) as u8) << 1)
                | (((r2[x] == c_u8) as u8) << 2)
                | (((r3[x] == c_u8) as u8) << 3)
                | (((r4[x] == c_u8) as u8) << 4)
                | (((r5[x] == c_u8) as u8) << 5);
            let ch = mask + 63;
            if ch == run_char {
                run_len += 1;
            } else {
                Self::emit_rle(out, run_char, run_len);
                run_char = ch;
                run_len = 1;
            }
            x += 1;
        }

        if run_char != 63 {
            Self::emit_rle(out, run_char, run_len);
        }
    }
}

/// Stateful Sixel encoder that caches encoded 6-pixel bands
/// and only re-quantizes & re-encodes bands intersecting dirty regions.
pub struct SixelStreamEncoder {
    width: u32,
    height: u32,
    quantized: Vec<u8>,
    band_cache: Vec<Vec<u8>>,
    band_colors: Vec<u8>,
}

impl Default for SixelStreamEncoder {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            quantized: Vec::new(),
            band_cache: Vec::new(),
            band_colors: Vec::with_capacity(32),
        }
    }
}

impl SixelStreamEncoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn encode_frame_into(
        &mut self,
        rgba: &[u8],
        width: u32,
        height: u32,
        damage: Option<&[DamageRect]>,
        out: &mut Vec<u8>,
    ) {
        let num_bands = height.div_ceil(6) as usize;
        let mut force_all = false;

        if self.width != width || self.height != height {
            self.width = width;
            self.height = height;
            self.quantized.resize((width * height) as usize, 0);
            self.band_cache.clear();
            self.band_cache.resize_with(num_bands, Vec::new);
            force_all = true;
        }

        let mut band_dirty = vec![false; num_bands];
        if force_all || damage.is_none() {
            band_dirty.fill(true);
        } else if let Some(rects) = damage {
            for rect in rects {
                let start_band = (rect.y / 6) as usize;
                let end_band = (((rect.y + rect.height + 5) / 6) as usize).min(num_bands);
                for b in start_band..end_band {
                    band_dirty[b] = true;
                }
            }
        }

        out.clear();
        out.extend_from_slice(b"\x1bP7;0;q\"1;1;");
        SixelEncoder::emit_u32_ascii(out, width);
        out.push(b';');
        SixelEncoder::emit_u32_ascii(out, height);

        // Precomputed RGB332 palette definitions (all 256 colors)
        SixelEncoder::emit_rgb332_full_palette(out);

        #[cfg(target_arch = "x86_64")]
        let has_avx2 = std::is_x86_feature_detected!("avx2");
        #[cfg(not(target_arch = "x86_64"))]
        let has_avx2 = false;

        let w = width as usize;
        let h = height as usize;

        for b in 0..num_bands {
            let band_y = (b * 6) as u32;
            let start_y = band_y as usize;
            let max_y = std::cmp::min(start_y + 6, h);
            let has_next_band = b + 1 < num_bands;

            if band_dirty[b] || self.band_cache[b].is_empty() {
                // Quantize only this band's pixel slice!
                let start_pixel = start_y * w;
                let end_pixel = max_y * w;
                let rgba_slice = &rgba[start_pixel * 4..end_pixel * 4];
                let quant_slice = &mut self.quantized[start_pixel..end_pixel];

                #[cfg(target_arch = "x86_64")]
                if has_avx2 {
                    unsafe {
                        SixelEncoder::quantize_rgb332_avx2_slice(rgba_slice, quant_slice);
                    }
                } else {
                    for (i, pixel) in rgba_slice.chunks_exact(4).enumerate() {
                        quant_slice[i] = SixelEncoder::quantize_pixel_rgb332(pixel[0], pixel[1], pixel[2]);
                    }
                }

                // Encode band into cache
                self.band_cache[b].clear();
                SixelEncoder::encode_single_band(
                    &self.quantized,
                    w,
                    h,
                    start_y,
                    max_y,
                    has_next_band,
                    &mut self.band_cache[b],
                    &mut self.band_colors,
                    has_avx2,
                );
            }

            // Append cached band bytes directly
            out.extend_from_slice(&self.band_cache[b]);
        }

        out.extend_from_slice(b"\x1b\\");
    }
}


