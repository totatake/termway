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
        for (i, pixel) in rgba.chunks_exact(4).enumerate() {
            let idx = (pixel[0] & 0xE0) | ((pixel[1] & 0xE0) >> 3) | (pixel[2] >> 6);
            quantized[i] = idx;
            used_colors[idx as usize] = true;
        }

        // 2. Emit palette definitions ONLY for used colors
        for (i, &is_used) in used_colors.iter().enumerate() {
            if is_used {
                let r = (i >> 5) & 7;
                let g = (i >> 2) & 7;
                let b = i & 3;
                let r_pct = r * 100 / 7;
                let g_pct = g * 100 / 7;
                let b_pct = b * 100 / 3;
                out.push(b'#');
                Self::emit_u32_ascii(out, i as u32);
                out.extend_from_slice(b";2;");
                Self::emit_u32_ascii(out, r_pct as u32);
                out.push(b';');
                Self::emit_u32_ascii(out, g_pct as u32);
                out.push(b';');
                Self::emit_u32_ascii(out, b_pct as u32);
            }
        }

        let w = width as usize;
        let h = height as usize;
        let mut band_colors = Vec::with_capacity(32);

        // 3. Process 6-pixel high bands
        for band_y in (0..height).step_by(6) {
            let start_y = band_y as usize;
            let max_y = std::cmp::min(start_y + 6, h);
            let num_rows = (max_y - start_y) as u8;

            // Find distinct colors used in THIS band
            band_colors.clear();
            let mut band_used = [false; 256];
            for y in start_y..max_y {
                let row = &quantized[y * w..(y + 1) * w];
                for &color in row {
                    if !band_used[color as usize] {
                        band_used[color as usize] = true;
                        band_colors.push(color);
                    }
                }
            }

            if band_colors.is_empty() {
                if band_y + 6 < height {
                    out.push(b'-');
                }
                continue;
            }

            // Fast-path: entire band is a single solid color
            if band_colors.len() == 1 {
                let c = band_colors[0];
                let mask = (1 << num_rows) - 1;
                out.push(b'#');
                Self::emit_u32_ascii(out, c as u32);
                Self::emit_rle(out, mask + 63, w);
                out.push(b'$');
                if band_y + 6 < height {
                    out.push(b'-');
                }
                continue;
            }

            // Row slices for this 6-pixel band
            let r0 = &quantized[start_y * w..(start_y + 1) * w];
            let r1 = if start_y + 1 < h { &quantized[(start_y + 1) * w..(start_y + 2) * w] } else { &[] };
            let r2 = if start_y + 2 < h { &quantized[(start_y + 2) * w..(start_y + 3) * w] } else { &[] };
            let r3 = if start_y + 3 < h { &quantized[(start_y + 3) * w..(start_y + 4) * w] } else { &[] };
            let r4 = if start_y + 4 < h { &quantized[(start_y + 4) * w..(start_y + 5) * w] } else { &[] };
            let r5 = if start_y + 5 < h { &quantized[(start_y + 5) * w..(start_y + 6) * w] } else { &[] };

            // Encode each color present in the band
            for &c_u8 in &band_colors {
                out.push(b'#');
                Self::emit_u32_ascii(out, c_u8 as u32);

                let mut run_char = 0u8;
                let mut run_len = 0;

                for x in 0..w {
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
                Self::emit_rle(out, run_char, run_len);
                out.push(b'$'); // Carriage return to start of band
            }
            if band_y + 6 < height {
                out.push(b'-'); // Line feed to next band (skip for last band to prevent terminal scrolling)
            }
        }

        // Sixel outro (String Terminator)
        out.extend_from_slice(b"\x1b\\");
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
}

