use std::io::Write;

/// Sixel Graphics Protocol encoder.
pub struct SixelEncoder;

impl SixelEncoder {
    /// Encode a full-screen RGBA frame buffer to Sixel format using an RGB332 fixed palette.
    pub fn encode_fullscreen_frame(rgba: &[u8], width: u32, height: u32) -> String {
        let mut out = Vec::with_capacity((width * height) as usize / 2);
        
        // Sixel intro (DCS 7;1;q) with 1:1 pixel aspect ratio and raster attributes
        let _ = write!(&mut out, "\x1bP7;1;q\"1;1;{};{}", width, height);

        // 1. Quantize the image to RGB332 and collect used colors globally
        let mut used_colors = [false; 256];
        let mut quantized = vec![0u8; (width * height) as usize];
        
        for (i, pixel) in rgba.chunks_exact(4).enumerate() {
            let r = pixel[0] >> 5; // 3 bits (0-7)
            let g = pixel[1] >> 5; // 3 bits (0-7)
            let b = pixel[2] >> 6; // 2 bits (0-3)
            let idx = (r << 5) | (g << 2) | b;
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
                let _ = write!(&mut out, "#{};2;{};{};{}", i, r_pct, g_pct, b_pct);
            }
        }

        // 3. Process 6-pixel high bands
        for band_y in (0..height).step_by(6) {
            let max_y = std::cmp::min(band_y + 6, height);
            
            // Find colors used in THIS band
            let mut band_used = [false; 256];
            for y in band_y..max_y {
                let row_start = (y * width) as usize;
                for x in 0..width as usize {
                    band_used[quantized[row_start + x] as usize] = true;
                }
            }

            // Encode each color present in the band
            for (c, &is_used) in band_used.iter().enumerate() {
                if !is_used {
                    continue;
                }

                let _ = write!(&mut out, "#{}", c);

                let mut run_char = 0u8;
                let mut run_len = 0;

                for x in 0..width as usize {
                    let mut mask = 0;
                    for row in 0..6 {
                        let y = band_y + row;
                        if y >= height { break; }
                        let idx = (y * width) as usize + x;
                        if quantized[idx] as usize == c {
                            mask |= 1 << row;
                        }
                    }
                    let ch = mask + 63;

                    if ch == run_char {
                        run_len += 1;
                    } else {
                        Self::emit_rle(&mut out, run_char, run_len);
                        run_char = ch;
                        run_len = 1;
                    }
                }
                Self::emit_rle(&mut out, run_char, run_len);
                out.push(b'$'); // Carriage return to start of band
            }
            if band_y + 6 < height {
                out.push(b'-'); // Line feed to next band (skip for last band to prevent terminal scrolling)
            }
        }

        // Sixel outro (String Terminator)
        out.extend_from_slice(b"\x1b\\");

        String::from_utf8(out).unwrap_or_default()
    }

    #[inline]
    fn emit_rle(out: &mut Vec<u8>, ch: u8, count: usize) {
        if count == 0 {
            return;
        }
        if count <= 3 {
            for _ in 0..count {
                out.push(ch);
            }
        } else {
            let _ = write!(out, "!{}{}", count, ch as char);
        }
    }
}
