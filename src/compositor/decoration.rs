use pixman::Image;
use super::bitmap_font::BitmapCanvas;
use super::window::{ManagedWindow, BORDER_WIDTH, BUTTON_SIZE, TITLEBAR_HEIGHT};

pub struct WindowTheme {
    pub active_titlebar_bg: [u8; 4],   // RGBA (Openbox/Fluxbox dark steel blue)
    pub inactive_titlebar_bg: [u8; 4], // RGBA (dim dark gray)
    pub active_border: [u8; 4],
    pub inactive_border: [u8; 4],
    pub title_text_color: [u8; 4],
    pub inactive_text_color: [u8; 4],
    pub close_button_bg: [u8; 4],
    pub close_icon_color: [u8; 4],
    pub button_bg: [u8; 4],
    pub button_icon_color: [u8; 4],
    pub resize_grip_color: [u8; 4],
}

impl Default for WindowTheme {
    fn default() -> Self {
        Self {
            active_titlebar_bg: [38, 50, 72, 255],     // Slate dark blue
            inactive_titlebar_bg: [28, 30, 36, 255],   // Dark charcoal
            active_border: [70, 95, 130, 255],         // Crisp blue-gray border
            inactive_border: [45, 48, 55, 255],        // Subtle dark border
            title_text_color: [230, 235, 245, 255],    // Crisp bright white-blue
            inactive_text_color: [140, 145, 155, 255], // Muted text
            close_button_bg: [190, 55, 55, 255],       // Openbox/Fluxbox red button
            close_icon_color: [255, 255, 255, 255],
            button_bg: [52, 65, 88, 255],              // Dark slate button bg
            button_icon_color: [230, 235, 245, 255],   // Bright icon
            resize_grip_color: [110, 125, 145, 255],
        }
    }
}

pub struct DecorationRenderer {
    theme: WindowTheme,
}

impl Default for DecorationRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl DecorationRenderer {
    pub fn new() -> Self {
        Self {
            theme: WindowTheme::default(),
        }
    }

    pub fn draw_decorations(
        &self,
        target_image: &mut Image<'static, 'static>,
        target_w: u32,
        target_h: u32,
        window: &ManagedWindow,
    ) {
        if !window.has_ssd() {
            return;
        }

        let stride = target_image.stride();
        let data = unsafe {
            let ptr = target_image.data() as *mut u8;
            std::slice::from_raw_parts_mut(ptr, (target_h as usize) * stride)
        };

        let mut canvas = DecorationCanvas::new(data, stride, target_w as i32, target_h as i32);

        let frame = window.frame_geometry();
        let titlebar = window.titlebar_geometry();
        let active = window.active;

        let border_color = if active {
            self.theme.active_border
        } else {
            self.theme.inactive_border
        };

        let titlebar_color = if active {
            self.theme.active_titlebar_bg
        } else {
            self.theme.inactive_titlebar_bg
        };

        // 1. Draw border outline
        canvas.draw_rect_outline(
            frame.loc.x,
            frame.loc.y,
            frame.size.w,
            frame.size.h,
            border_color,
        );

        // 2. Draw titlebar background
        canvas.fill_rect(
            titlebar.loc.x + BORDER_WIDTH,
            titlebar.loc.y + BORDER_WIDTH,
            titlebar.size.w - BORDER_WIDTH * 2,
            titlebar.size.h - BORDER_WIDTH,
            titlebar_color,
        );

        // 3. Draw close button [X]
        let close_btn = window.close_button_geometry();
        canvas.fill_rect(
            close_btn.loc.x,
            close_btn.loc.y,
            close_btn.size.w,
            close_btn.size.h,
            self.theme.close_button_bg,
        );

        // Draw 'X' inside close button
        canvas.draw_cross(
            close_btn.loc.x + 3,
            close_btn.loc.y + 3,
            close_btn.size.w - 6,
            self.theme.close_icon_color,
        );

        // 4. Draw Maximize / Restore button [□]
        let max_btn = window.maximize_button_geometry();
        canvas.fill_rect(
            max_btn.loc.x,
            max_btn.loc.y,
            max_btn.size.w,
            max_btn.size.h,
            self.theme.button_bg,
        );
        if window.is_maximized {
            // Restore icon (two overlapping boxes)
            canvas.draw_rect_outline(
                max_btn.loc.x + 5,
                max_btn.loc.y + 3,
                max_btn.size.w - 8,
                max_btn.size.h - 8,
                self.theme.button_icon_color,
            );
            canvas.draw_rect_outline(
                max_btn.loc.x + 3,
                max_btn.loc.y + 5,
                max_btn.size.w - 8,
                max_btn.size.h - 8,
                self.theme.button_icon_color,
            );
        } else {
            // Single square outline
            canvas.draw_rect_outline(
                max_btn.loc.x + 3,
                max_btn.loc.y + 3,
                max_btn.size.w - 6,
                max_btn.size.h - 6,
                self.theme.button_icon_color,
            );
        }

        // 5. Draw Minimize button [-]
        let min_btn = window.minimize_button_geometry();
        canvas.fill_rect(
            min_btn.loc.x,
            min_btn.loc.y,
            min_btn.size.w,
            min_btn.size.h,
            self.theme.button_bg,
        );
        canvas.fill_rect(
            min_btn.loc.x + 3,
            min_btn.loc.y + min_btn.size.h - 5,
            min_btn.size.w - 6,
            2,
            self.theme.button_icon_color,
        );

        // 6. Draw resize grip handle at bottom-right corner (only if not maximized)
        if !window.is_maximized {
            let grip = window.resize_handle_geometry();
            canvas.draw_resize_grip(
                grip.loc.x,
                grip.loc.y,
                grip.size.w,
                self.theme.resize_grip_color,
            );
        }

        // 7. Draw Title text with 8x8 font
        let text_color = if active {
            self.theme.title_text_color
        } else {
            self.theme.inactive_text_color
        };
        let max_chars = ((titlebar.size.w - (BUTTON_SIZE + 4) * 3 - 20).max(0) / 8) as usize;
        canvas.inner.render_text(
            titlebar.loc.x + 8,
            titlebar.loc.y + (TITLEBAR_HEIGHT - 8) / 2,
            &window.title,
            text_color,
            max_chars,
        );
    }
}

/// Helper canvas for drawing window decoration shapes.
struct DecorationCanvas<'a> {
    inner: BitmapCanvas<'a>,
}

impl<'a> DecorationCanvas<'a> {
    fn new(data: &'a mut [u8], stride: usize, width: i32, height: i32) -> Self {
        Self {
            inner: BitmapCanvas::new(data, stride, width, height),
        }
    }

    #[inline]
    fn set_pixel(&mut self, x: i32, y: i32, color: [u8; 4]) {
        let offset = y as usize * self.inner.stride + x as usize * 4;
        self.inner.data[offset] = color[0];
        self.inner.data[offset + 1] = color[1];
        self.inner.data[offset + 2] = color[2];
        self.inner.data[offset + 3] = color[3];
    }

    fn fill_rect(&mut self, rx: i32, ry: i32, rw: i32, rh: i32, color: [u8; 4]) {
        let x0 = rx.max(0);
        let y0 = ry.max(0);
        let x1 = (rx + rw).min(self.inner.width);
        let y1 = (ry + rh).min(self.inner.height);

        for y in y0..y1 {
            let row_offset = y as usize * self.inner.stride;
            for x in x0..x1 {
                let off = row_offset + (x as usize * 4);
                self.inner.data[off..off + 4].copy_from_slice(&color);
            }
        }
    }

    fn draw_rect_outline(&mut self, rx: i32, ry: i32, rw: i32, rh: i32, color: [u8; 4]) {
        let w = self.inner.width;
        let h = self.inner.height;
        // Top and bottom horizontal lines
        for x in rx..(rx + rw) {
            if x >= 0 && x < w {
                if ry >= 0 && ry < h {
                    self.set_pixel(x, ry, color);
                }
                if (ry + rh - 1) >= 0 && (ry + rh - 1) < h {
                    self.set_pixel(x, ry + rh - 1, color);
                }
            }
        }
        // Left and right vertical lines
        for y in ry..(ry + rh) {
            if y >= 0 && y < h {
                if rx >= 0 && rx < w {
                    self.set_pixel(rx, y, color);
                }
                if (rx + rw - 1) >= 0 && (rx + rw - 1) < w {
                    self.set_pixel(rx + rw - 1, y, color);
                }
            }
        }
    }

    fn draw_cross(&mut self, x: i32, y: i32, size: i32, color: [u8; 4]) {
        let w = self.inner.width;
        let h = self.inner.height;
        for i in 0..size {
            let p1x = x + i;
            let p1y = y + i;
            let p2x = x + size - 1 - i;
            let p2y = y + i;
            if p1x >= 0 && p1x < w && p1y >= 0 && p1y < h {
                self.set_pixel(p1x, p1y, color);
            }
            if p2x >= 0 && p2x < w && p2y >= 0 && p2y < h {
                self.set_pixel(p2x, p2y, color);
            }
        }
    }

    fn draw_resize_grip(&mut self, x: i32, y: i32, size: i32, color: [u8; 4]) {
        let w = self.inner.width;
        let h = self.inner.height;
        for d in [3, 6, 9] {
            for i in 0..d {
                let px = x + size - d + i;
                let py = y + size - 1 - i;
                if px >= 0 && px < w && py >= 0 && py < h {
                    self.set_pixel(px, py, color);
                }
            }
        }
    }
}
