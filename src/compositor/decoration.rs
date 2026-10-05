use pixman::Image;
use super::bitmap_font::render_text;
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
            resize_grip_color: [110, 125, 145, 255],
        }
    }
}

pub struct DecorationRenderer {
    theme: WindowTheme,
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

        let stride = target_image.stride() as usize;
        let data = unsafe {
            let ptr = target_image.data() as *mut u8;
            std::slice::from_raw_parts_mut(ptr, (target_h as usize) * stride)
        };

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
        Self::draw_rect_outline(
            data,
            target_w as i32,
            target_h as i32,
            stride,
            frame.loc.x,
            frame.loc.y,
            frame.size.w,
            frame.size.h,
            border_color,
        );

        // 2. Draw titlebar background
        Self::fill_rect(
            data,
            target_w as i32,
            target_h as i32,
            stride,
            titlebar.loc.x + BORDER_WIDTH,
            titlebar.loc.y + BORDER_WIDTH,
            titlebar.size.w - BORDER_WIDTH * 2,
            titlebar.size.h - BORDER_WIDTH,
            titlebar_color,
        );

        // 3. Draw close button [X]
        let close_btn = window.close_button_geometry();
        Self::fill_rect(
            data,
            target_w as i32,
            target_h as i32,
            stride,
            close_btn.loc.x,
            close_btn.loc.y,
            close_btn.size.w,
            close_btn.size.h,
            self.theme.close_button_bg,
        );

        // Draw 'X' inside close button
        Self::draw_cross(
            data,
            target_w as i32,
            target_h as i32,
            stride,
            close_btn.loc.x + 3,
            close_btn.loc.y + 3,
            close_btn.size.w - 6,
            self.theme.close_icon_color,
        );

        // 4. Draw resize grip handle at bottom-right corner
        let grip = window.resize_handle_geometry();
        Self::draw_resize_grip(
            data,
            target_w as i32,
            target_h as i32,
            stride,
            grip.loc.x,
            grip.loc.y,
            grip.size.w,
            self.theme.resize_grip_color,
        );

        // 5. Draw Title text with 8x8 font
        let text_color = if active {
            self.theme.title_text_color
        } else {
            self.theme.inactive_text_color
        };
        let max_chars = ((titlebar.size.w - BUTTON_SIZE - 20).max(0) / 8) as usize;
        render_text(
            data,
            stride,
            target_w as i32,
            target_h as i32,
            titlebar.loc.x + 8,
            titlebar.loc.y + (TITLEBAR_HEIGHT - 8) / 2,
            &window.title,
            text_color,
            max_chars,
        );
    }

    #[inline]
    fn set_pixel(data: &mut [u8], stride: usize, x: i32, y: i32, color: [u8; 4]) {
        let offset = y as usize * stride + x as usize * 4;
        data[offset] = color[0];
        data[offset + 1] = color[1];
        data[offset + 2] = color[2];
        data[offset + 3] = color[3];
    }

    fn fill_rect(
        data: &mut [u8],
        target_w: i32,
        target_h: i32,
        stride: usize,
        rx: i32,
        ry: i32,
        rw: i32,
        rh: i32,
        color: [u8; 4],
    ) {
        let x0 = rx.max(0);
        let y0 = ry.max(0);
        let x1 = (rx + rw).min(target_w);
        let y1 = (ry + rh).min(target_h);

        for y in y0..y1 {
            let row_offset = y as usize * stride;
            for x in x0..x1 {
                let off = row_offset + (x as usize * 4);
                data[off..off + 4].copy_from_slice(&color);
            }
        }
    }

    fn draw_rect_outline(
        data: &mut [u8],
        target_w: i32,
        target_h: i32,
        stride: usize,
        rx: i32,
        ry: i32,
        rw: i32,
        rh: i32,
        color: [u8; 4],
    ) {
        // Top and bottom horizontal lines
        for x in rx..(rx + rw) {
            if x >= 0 && x < target_w {
                if ry >= 0 && ry < target_h {
                    Self::set_pixel(data, stride, x, ry, color);
                }
                if (ry + rh - 1) >= 0 && (ry + rh - 1) < target_h {
                    Self::set_pixel(data, stride, x, ry + rh - 1, color);
                }
            }
        }
        // Left and right vertical lines
        for y in ry..(ry + rh) {
            if y >= 0 && y < target_h {
                if rx >= 0 && rx < target_w {
                    Self::set_pixel(data, stride, rx, y, color);
                }
                if (rx + rw - 1) >= 0 && (rx + rw - 1) < target_w {
                    Self::set_pixel(data, stride, rx + rw - 1, y, color);
                }
            }
        }
    }

    fn draw_cross(
        data: &mut [u8],
        target_w: i32,
        target_h: i32,
        stride: usize,
        x: i32,
        y: i32,
        size: i32,
        color: [u8; 4],
    ) {
        for i in 0..size {
            let p1x = x + i;
            let p1y = y + i;
            let p2x = x + size - 1 - i;
            let p2y = y + i;
            if p1x >= 0 && p1x < target_w && p1y >= 0 && p1y < target_h {
                Self::set_pixel(data, stride, p1x, p1y, color);
            }
            if p2x >= 0 && p2x < target_w && p2y >= 0 && p2y < target_h {
                Self::set_pixel(data, stride, p2x, p2y, color);
            }
        }
    }

    fn draw_resize_grip(
        data: &mut [u8],
        target_w: i32,
        target_h: i32,
        stride: usize,
        x: i32,
        y: i32,
        size: i32,
        color: [u8; 4],
    ) {
        for d in [3, 6, 9] {
            for i in 0..d {
                let px = x + size - d + i;
                let py = y + size - 1 - i;
                if px >= 0 && px < target_w && py >= 0 && py < target_h {
                    Self::set_pixel(data, stride, px, py, color);
                }
            }
        }
    }
}
