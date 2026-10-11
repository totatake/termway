/// RGBA8888 Pixel Framebuffer
#[derive(Debug, Clone)]
pub struct Framebuffer {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>, // RGBA8888 formatted bytes
}

impl Framebuffer {
    pub fn new(width: u32, height: u32) -> Self {
        let size = (width * height * 4) as usize;
        Self {
            width,
            height,
            pixels: vec![0; size],
        }
    }

    /// Copy 32-bit BGRX/ARGB scanlines into this RGBA8888 framebuffer.
    pub fn copy_from_bgrx(&mut self, src_data: &[u8], bytes_per_line: usize, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        let min_len = (h as usize - 1)
            .saturating_mul(bytes_per_line)
            .saturating_add((w as usize) * 4);
        if src_data.len() < min_len {
            return;
        }

        if w != self.width || h != self.height {
            self.width = w;
            self.height = h;
            self.pixels.resize((w * h * 4) as usize, 0);
        }

        #[cfg(target_arch = "x86_64")]
        if std::is_x86_feature_detected!("avx2") {
            unsafe {
                Self::copy_bgrx_to_rgba_avx2(
                    src_data.as_ptr(),
                    bytes_per_line,
                    &mut self.pixels,
                    w as usize,
                    h as usize,
                );
            }
            return;
        }

        for y in 0..h {
            let row_offset = (y as usize) * bytes_per_line;
            let dst_row_start = (y * w * 4) as usize;

            for x in 0..w {
                let src_idx = row_offset + (x as usize) * 4;
                let b = src_data[src_idx];
                let g = src_data[src_idx + 1];
                let r = src_data[src_idx + 2];

                let dst_idx = dst_row_start + (x as usize) * 4;
                self.pixels[dst_idx] = r;
                self.pixels[dst_idx + 1] = g;
                self.pixels[dst_idx + 2] = b;
                self.pixels[dst_idx + 3] = 255u8;
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx2")]
    unsafe fn copy_bgrx_to_rgba_avx2(
        src_data: *const u8,
        bytes_per_line: usize,
        dst_pixels: &mut [u8],
        w: usize,
        h: usize,
    ) {
        use std::arch::x86_64::*;

        let shuf = _mm256_setr_epi8(
            2, 1, 0, -1, 6, 5, 4, -1, 10, 9, 8, -1, 14, 13, 12, -1,
            2, 1, 0, -1, 6, 5, 4, -1, 10, 9, 8, -1, 14, 13, 12, -1,
        );
        let alpha_mask = _mm256_set1_epi32(0xFF00_0000u32 as i32);
        let dst_ptr = dst_pixels.as_mut_ptr();

        for y in 0..h {
            let src_row = src_data.add(y * bytes_per_line);
            let dst_row = dst_ptr.add(y * w * 4);
            let mut x = 0usize;

            while x + 8 <= w {
                let v = _mm256_loadu_si256(src_row.add(x * 4) as *const __m256i);
                let rgba = _mm256_or_si256(_mm256_shuffle_epi8(v, shuf), alpha_mask);
                _mm256_storeu_si256(dst_row.add(x * 4) as *mut __m256i, rgba);
                x += 8;
            }

            let src_row_u32 = src_row as *const u32;
            while x < w {
                let pixel = *src_row_u32.add(x);
                let dst_idx = x * 4;
                *dst_row.add(dst_idx) = ((pixel >> 16) & 0xff) as u8;
                *dst_row.add(dst_idx + 1) = ((pixel >> 8) & 0xff) as u8;
                *dst_row.add(dst_idx + 2) = (pixel & 0xff) as u8;
                *dst_row.add(dst_idx + 3) = 255u8;
                x += 1;
            }
        }
    }
}
