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
}
