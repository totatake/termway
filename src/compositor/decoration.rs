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
        scale: f64,
    ) {
        if !window.has_ssd() {
            return;
        }

        let stride = target_image.stride();
        let data = unsafe {
            let ptr = target_image.data() as *mut u8;
            std::slice::from_raw_parts_mut(ptr, (target_h as usize) * stride)
        };

        let mut canvas = DecorationCanvas::new(data, stride, target_w as i32, target_h as i32, scale);

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
            canvas.s(titlebar.loc.x + 8),
            canvas.s(titlebar.loc.y + (TITLEBAR_HEIGHT - 8) / 2),
            &window.title,
            text_color,
            max_chars,
        );
    }

    /// Renders a borderless, futuristic startup splash screen featuring a large gradient logo
    /// and Linux kernel / systemd style boot telemetry logs below.
    pub fn draw_splash(
        &self,
        target_image: &mut Image<'static, 'static>,
        screen_w: u32,
        screen_h: u32,
        elapsed_millis: u128,
        info: &str,
        command_name: &str,
    ) {
        if screen_w < 240 || screen_h < 160 {
            return;
        }

        let stride = target_image.stride();
        let data_ptr = unsafe { target_image.data() };
        let slice = unsafe {
            std::slice::from_raw_parts_mut(data_ptr as *mut u8, (screen_h as usize) * stride)
        };

        let mut canvas = DecorationCanvas::new(slice, stride, screen_w as i32, screen_h as i32, 1.0);

        // 1. Choose optimal large scale for the hero "TERMWAY" logo (5x on standard/large screens, 4x or 3x on compact)
        let logo_scale = if screen_w >= 560 && screen_h >= 280 {
            5
        } else if screen_w >= 420 {
            4
        } else {
            3
        };

        let prompt_str = ">_";
        let brand_str = "TERMWAY";
        let char_px = 8 * logo_scale;
        let letter_spacing = logo_scale; // 5px between letters at 5x scale
        let gap_px = 4 * logo_scale;     // 20px gap between ">_" and "TERMWAY"

        let prompt_w = (prompt_str.len() as i32 * (char_px + letter_spacing)) - letter_spacing;
        let brand_w = (brand_str.len() as i32 * (char_px + letter_spacing)) - letter_spacing;
        let total_logo_w = prompt_w + gap_px + brand_w;
        let logo_h = char_px;

        // Total vertical block height: logo + subtitle + horizon scanner + 4 kernel log lines
        let block_h = logo_h + 18 + 22 + 78;
        let start_y = ((screen_h as i32 - block_h) / 2).max(16);
        let logo_x = ((screen_w as i32 - total_logo_w) / 2).max(12);

        // 2. Render Hero Prompt ">_" with deep blue-to-cyan gradient and subtle depth shadow
        canvas.inner.render_text_scaled_spaced(
            logo_x + 3,
            start_y + 3,
            prompt_str,
            [6, 10, 18, 255],
            4,
            logo_scale,
            letter_spacing,
        );
        canvas.inner.render_text_gradient_scaled(
            logo_x,
            start_y,
            prompt_str,
            [70, 230, 255, 255],  // Top-left bright cyan
            [35, 130, 240, 255],  // Bottom-right royal blue
            4,
            logo_scale,
            letter_spacing,
        );

        // 3. Render Large "TERMWAY" Hero Logo with futuristic blue theme gradient
        let brand_x = logo_x + prompt_w + gap_px;
        // Soft ambient glow / drop shadow
        canvas.inner.render_text_scaled_spaced(
            brand_x + 3,
            start_y + 3,
            brand_str,
            [6, 10, 18, 255],
            16,
            logo_scale,
            letter_spacing,
        );
        canvas.inner.render_text_gradient_scaled(
            brand_x,
            start_y,
            brand_str,
            [185, 240, 255, 255], // Top-left ice cyan-white highlight
            [45, 120, 250, 255],  // Bottom-right deep electric cobalt blue
            16,
            logo_scale,
            letter_spacing,
        );

        // 4. Subtitle below logo
        let sub_y = start_y + logo_h + 14;
        let sub = concat!("v", env!("CARGO_PKG_VERSION"), "  //  ZERO-COPY TERMINAL GUI COMPOSITOR");
        let sub_w = (sub.len() as i32) * 8;
        let sub_x = ((screen_w as i32 - sub_w) / 2).max(12);
        let max_screen_chars = ((screen_w as i32 - 24).max(0) / 8) as usize;
        canvas.inner.render_text(
            sub_x,
            sub_y,
            sub,
            [115, 155, 205, 255],
            max_screen_chars,
        );

        // 5. Sleek Horizon Gradient Divider & Animated Scanner Beam (no boxy frame)
        let bar_y = sub_y + 20;
        let bar_w = 420.min(screen_w as i32 - 48).max(120);
        let bar_x = (screen_w as i32 - bar_w) / 2;

        // Draw subtle horizontal gradient track (fades out at left/right edges)
        let half_w = (bar_w / 2).max(1);
        for dx in 0..bar_w {
            let dist = (dx - half_w).abs();
            let intensity = (256 - (dist * 220 / half_w)).clamp(36, 256);
            let r = (28 * intensity / 256) as u8;
            let g = (52 * intensity / 256) as u8;
            let b = (92 * intensity / 256) as u8;
            canvas.set_pixel(bar_x + dx, bar_y, [r, g, b, 255]);
            canvas.set_pixel(bar_x + dx, bar_y + 1, [r, g, b, 255]);
        }

        // Animated glowing blue-cyan scanner pulse along the horizon line
        let pulse_w = (bar_w / 3).max(32);
        let total_span = (bar_w + pulse_w) as u128;
        let pulse_phase = ((elapsed_millis / 5) % total_span) as i32 - pulse_w;
        let active_x0 = (bar_x + pulse_phase).max(bar_x);
        let active_x1 = (bar_x + pulse_phase + pulse_w).min(bar_x + bar_w);
        if active_x1 > active_x0 {
            let span = (active_x1 - active_x0).max(1);
            let center = active_x0 + span / 2;
            let half_span = (span / 2).max(1);
            for px in active_x0..active_x1 {
                let d = (px - center).abs();
                let w_factor = (256 - (d * 200 / half_span)).clamp(56, 256);
                let r = (95 * w_factor / 256) as u8;
                let g = (195 * w_factor / 256) as u8;
                let b = (255 * w_factor / 256) as u8;
                canvas.set_pixel(px, bar_y, [r, g, b, 255]);
                canvas.set_pixel(px, bar_y + 1, [r, g, b, 255]);
            }
        }

        // 6. Linux Kernel / Systemd style Boot Log Stream (direct on background, no box border)
        let log_w = 400.min(screen_w as i32 - 32);
        let log_x = ((screen_w as i32 - log_w) / 2).max(16);
        let log_y = bar_y + 18;
        let log_max_chars = ((screen_w as i32 - log_x - 12).max(0) / 8) as usize;

        let ok_color = [75, 225, 150, 255];
        let ts_color = [90, 120, 160, 255];
        let text_dim = [150, 170, 198, 255];
        let text_bright = [205, 228, 252, 255];

        // Kernel Line 1
        canvas.inner.render_text(log_x, log_y, "[  OK  ]", ok_color, 8);
        canvas.inner.render_text(
            log_x + 72,
            log_y,
            "Reached target Smithay Wayland Core & Seat.",
            text_dim,
            log_max_chars.saturating_sub(9),
        );

        // Kernel Line 2
        let pipe_line = if info.is_empty() {
            "Graphics Pipeline Active"
        } else {
            info
        };
        canvas.inner.render_text(log_x, log_y + 18, "[  OK  ]", ok_color, 8);
        canvas.inner.render_text(
            log_x + 72,
            log_y + 18,
            pipe_line,
            text_bright,
            log_max_chars.saturating_sub(9),
        );

        // Kernel Line 3
        canvas.inner.render_text(log_x, log_y + 36, "[  OK  ]", ok_color, 8);
        canvas.inner.render_text(
            log_x + 72,
            log_y + 36,
            "Started XWayland X11 & D-Bus Session Bridge.",
            text_dim,
            log_max_chars.saturating_sub(9),
        );

        // Kernel Line 4 (Active process launch with spinner & blinking cursor)
        let spinner_frames = ["|", "/", "-", "\\"];
        let spinner = spinner_frames[((elapsed_millis / 100) % 4) as usize];
        let blink_on = (elapsed_millis / 250) % 2 == 0;
        let cursor_str = if blink_on { "_" } else { " " };
        let cmd_display = if command_name.is_empty() { "gui-client" } else { command_name };

        let status_tag = format!("[  {}   ]", spinner);
        canvas.inner.render_text(log_x, log_y + 54, &status_tag, [90, 195, 255, 255], 8);
        let exec_msg = format!("Spawning client: {} {}", cmd_display, cursor_str);
        canvas.inner.render_text(
            log_x + 72,
            log_y + 54,
            &exec_msg,
            [115, 205, 255, 255],
            log_max_chars.saturating_sub(9),
        );
        let _ = ts_color;
    }
}

/// Helper canvas for drawing window decoration shapes.
struct DecorationCanvas<'a> {
    inner: BitmapCanvas<'a>,
    scale: f64,
}

impl<'a> DecorationCanvas<'a> {
    fn new(data: &'a mut [u8], stride: usize, width: i32, height: i32, scale: f64) -> Self {
        Self {
            inner: BitmapCanvas::new(data, stride, width, height),
            scale: if scale <= 0.0 { 1.0 } else { scale },
        }
    }

    #[inline]
    fn s(&self, val: i32) -> i32 {
        if (self.scale - 1.0).abs() < 1e-5 {
            val
        } else {
            (val as f64 * self.scale).round() as i32
        }
    }

    #[inline]
    fn set_pixel(&mut self, x: i32, y: i32, color: [u8; 4]) {
        if x < 0 || y < 0 || x >= self.inner.width || y >= self.inner.height {
            return;
        }
        let offset = y as usize * self.inner.stride + x as usize * 4;
        self.inner.data[offset] = color[0];
        self.inner.data[offset + 1] = color[1];
        self.inner.data[offset + 2] = color[2];
        self.inner.data[offset + 3] = color[3];
    }

    fn fill_rect(&mut self, rx: i32, ry: i32, rw: i32, rh: i32, color: [u8; 4]) {
        let (rx, ry, rw, rh) = (self.s(rx), self.s(ry), self.s(rw), self.s(rh));
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
        let (rx, ry, rw, rh) = (self.s(rx), self.s(ry), self.s(rw), self.s(rh));
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
        let (x, y, size) = (self.s(x), self.s(y), self.s(size));
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
        let (x, y, size) = (self.s(x), self.s(y), self.s(size));
        let w = self.inner.width;
        let h = self.inner.height;
        for d in [3, 6, 9] {
            let scaled_d = self.s(d);
            for i in 0..scaled_d {
                let px = x + size - scaled_d + i;
                let py = y + size - 1 - i;
                if px >= 0 && px < w && py >= 0 && py < h {
                    self.set_pixel(px, py, color);
                }
            }
        }
    }
}
