use crate::display::Framebuffer;
use x11::xfixes::XFixesGetCursorImage;
use x11::xlib::{Display, XFree};

pub struct HardwareCursor {
    pub width: u32,
    pub height: u32,
    pub xhot: u32,
    pub yhot: u32,
    pub pixels: Vec<u32>, // ARGB32 pixels
}

impl HardwareCursor {
    /// Query the current hardware cursor image from the X11 server
    pub fn query(display: *mut Display) -> Option<Self> {
        if display.is_null() {
            return None;
        }

        unsafe {
            let cursor_ptr = XFixesGetCursorImage(display);
            if cursor_ptr.is_null() {
                return None;
            }

            let img = &*cursor_ptr;
            let width = img.width as u32;
            let height = img.height as u32;
            let len = (width * height) as usize;

            let mut pixels = Vec::with_capacity(len);
            let src_pixels = std::slice::from_raw_parts(img.pixels, len);
            
            for &p in src_pixels {
                pixels.push(p as u32); // Take lower 32 bits which are ARGB
            }

            let cursor = Self {
                width,
                height,
                xhot: img.xhot as u32,
                yhot: img.yhot as u32,
                pixels,
            };

            XFree(cursor_ptr as *mut _);
            Some(cursor)
        }
    }

    /// Alpha-composite the cursor onto the framebuffer at the given absolute mouse coordinates
    pub fn composite(&self, fb: &mut Framebuffer, mouse_x: u32, mouse_y: u32) {
        // Calculate top-left corner of the cursor image
        let start_x = mouse_x.saturating_sub(self.xhot);
        let start_y = mouse_y.saturating_sub(self.yhot);

        for cy in 0..self.height {
            let y = start_y + cy;
            if y >= fb.height {
                continue;
            }

            for cx in 0..self.width {
                let x = start_x + cx;
                if x >= fb.width {
                    continue;
                }

                let cursor_pixel = self.pixels[(cy * self.width + cx) as usize];
                let alpha = (cursor_pixel >> 24) & 0xff;

                if alpha == 0 {
                    continue; // Fully transparent
                }

                let cr = (cursor_pixel >> 16) & 0xff;
                let cg = (cursor_pixel >> 8) & 0xff;
                let cb = cursor_pixel & 0xff;

                let fb_idx = ((y * fb.width + x) * 4) as usize;
                let fb_r = fb.pixels[fb_idx] as u32;
                let fb_g = fb.pixels[fb_idx + 1] as u32;
                let fb_b = fb.pixels[fb_idx + 2] as u32;

                if alpha == 255 {
                    // Fully opaque
                    fb.pixels[fb_idx] = cr as u8;
                    fb.pixels[fb_idx + 1] = cg as u8;
                    fb.pixels[fb_idx + 2] = cb as u8;
                } else {
                    // Standard alpha blending: out = (src * alpha + dst * (255 - alpha)) / 255
                    let inv_alpha = 255 - alpha;
                    fb.pixels[fb_idx] = ((cr * alpha + fb_r * inv_alpha) / 255) as u8;
                    fb.pixels[fb_idx + 1] = ((cg * alpha + fb_g * inv_alpha) / 255) as u8;
                    fb.pixels[fb_idx + 2] = ((cb * alpha + fb_b * inv_alpha) / 255) as u8;
                }
            }
        }
    }
}

/// Query the absolute pointer coordinates (root_x, root_y) from the X server
pub fn query_pointer(display: *mut Display) -> Option<(u32, u32)> {
    if display.is_null() {
        return None;
    }
    unsafe {
        let root = x11::xlib::XDefaultRootWindow(display);
        let mut root_return = 0;
        let mut child_return = 0;
        let mut root_x_return = 0;
        let mut root_y_return = 0;
        let mut win_x_return = 0;
        let mut win_y_return = 0;
        let mut mask_return = 0;

        let res = x11::xlib::XQueryPointer(
            display,
            root,
            &mut root_return,
            &mut child_return,
            &mut root_x_return,
            &mut root_y_return,
            &mut win_x_return,
            &mut win_y_return,
            &mut mask_return,
        );

        if res != 0 {
            Some((root_x_return as u32, root_y_return as u32))
        } else {
            None
        }
    }
}
