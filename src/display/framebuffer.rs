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

    /// Read image data from an X11 XImage structure into this framebuffer.
    /// Converts standard 24/32-bit BGRX or ARGB to RGBA8888.
    ///
    /// # Safety
    /// `ximage` must be a valid non-null pointer to an XImage.
    pub unsafe fn copy_from_ximage(&mut self, ximage: *mut x11::xlib::XImage) {
        if ximage.is_null() {
            return;
        }

        let img = &*ximage;
        let w = img.width as u32;
        let h = img.height as u32;

        if w != self.width || h != self.height {
            self.width = w;
            self.height = h;
            self.pixels.resize((w * h * 4) as usize, 0);
        }

        let src_data = img.data as *const u8;
        let bytes_per_line = img.bytes_per_line as usize;
        let bits_per_pixel = img.bits_per_pixel;

        if bits_per_pixel == 32 {
            #[cfg(target_arch = "x86_64")]
            if std::is_x86_feature_detected!("avx2") {
                Self::copy_bgrx_to_rgba_avx2(
                    src_data,
                    bytes_per_line,
                    &mut self.pixels,
                    w as usize,
                    h as usize,
                );
                return;
            }

            for y in 0..h {
                let src_row = src_data.add((y as usize) * bytes_per_line) as *const u32;
                let dst_row_start = (y * w * 4) as usize;

                for x in 0..w {
                    let pixel = *src_row.add(x as usize);
                    // Extract colors (usually 0x00RRGGBB or 0x00BBGGRR depending on visual)
                    // Xvfb standard 24/32bpp default is TrueColor with red_mask=0xff0000, green_mask=0x00ff00, blue_mask=0x0000ff
                    let r = ((pixel >> 16) & 0xff) as u8;
                    let g = ((pixel >> 8) & 0xff) as u8;
                    let b = (pixel & 0xff) as u8;
                    let a = 255u8;

                    let dst_idx = dst_row_start + (x as usize) * 4;
                    self.pixels[dst_idx] = r;
                    self.pixels[dst_idx + 1] = g;
                    self.pixels[dst_idx + 2] = b;
                    self.pixels[dst_idx + 3] = a;
                }
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
