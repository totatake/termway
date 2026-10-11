// Minimal 8x8 monochrome bitmap font (ASCII 32..=126) for dependency-free text rendering
pub const FONT_WIDTH: usize = 8;
pub const FONT_HEIGHT: usize = 8;

// Basic 8x8 font table for ASCII characters
include!("font_data.rs");

/// Canvas buffer abstraction for font and decoration rasterization.
pub struct BitmapCanvas<'a> {
    pub data: &'a mut [u8],
    pub stride: usize,
    pub width: i32,
    pub height: i32,
}

impl<'a> BitmapCanvas<'a> {
    pub fn new(data: &'a mut [u8], stride: usize, width: i32, height: i32) -> Self {
        Self { data, stride, width, height }
    }

    pub fn render_char(&mut self, x: i32, y: i32, c: char, color: [u8; 4]) {
        let code = c as usize;
        if !(32..=126).contains(&code) {
            return;
        }
        let glyph = &FONT_8X8[code - 32];
        for (row, &bits) in glyph.iter().enumerate() {
            let py = y + row as i32;
            if py < 0 || py >= self.height {
                continue;
            }
            for col in 0..8 {
                if (bits & (1 << (7 - col))) != 0 {
                    let px = x + col;
                    if px >= 0 && px < self.width {
                        let off = py as usize * self.stride + px as usize * 4;
                        self.data[off..off + 4].copy_from_slice(&color);
                    }
                }
            }
        }
    }

    pub fn fill_rect(&mut self, rx: i32, ry: i32, rw: i32, rh: i32, color: [u8; 4]) {
        let x0 = rx.max(0);
        let y0 = ry.max(0);
        let x1 = (rx + rw).min(self.width);
        let y1 = (ry + rh).min(self.height);

        for y in y0..y1 {
            let row_offset = y as usize * self.stride;
            for x in x0..x1 {
                let off = row_offset + (x as usize * 4);
                self.data[off..off + 4].copy_from_slice(&color);
            }
        }
    }

    pub fn draw_rect_outline(&mut self, rx: i32, ry: i32, rw: i32, rh: i32, color: [u8; 4]) {
        let w = self.width;
        let h = self.height;
        for x in rx..(rx + rw) {
            if x >= 0 && x < w {
                if ry >= 0 && ry < h {
                    let off = ry as usize * self.stride + x as usize * 4;
                    self.data[off..off + 4].copy_from_slice(&color);
                }
                if (ry + rh - 1) >= 0 && (ry + rh - 1) < h {
                    let off = (ry + rh - 1) as usize * self.stride + x as usize * 4;
                    self.data[off..off + 4].copy_from_slice(&color);
                }
            }
        }
        for y in ry..(ry + rh) {
            if y >= 0 && y < h {
                if rx >= 0 && rx < w {
                    let off = y as usize * self.stride + rx as usize * 4;
                    self.data[off..off + 4].copy_from_slice(&color);
                }
                if (rx + rw - 1) >= 0 && (rx + rw - 1) < w {
                    let off = y as usize * self.stride + (rx + rw - 1) as usize * 4;
                    self.data[off..off + 4].copy_from_slice(&color);
                }
            }
        }
    }

    pub fn render_char_scaled(&mut self, x: i32, y: i32, c: char, color: [u8; 4], scale: i32) {
        let code = c as usize;
        if !(32..=126).contains(&code) {
            return;
        }
        let glyph = &FONT_8X8[code - 32];
        for (row, &bits) in glyph.iter().enumerate() {
            for sy in 0..scale {
                let py = y + (row as i32 * scale) + sy;
                if py < 0 || py >= self.height {
                    continue;
                }
                let row_offset = py as usize * self.stride;
                for col in 0..8 {
                    if (bits & (1 << (7 - col))) != 0 {
                        for sx in 0..scale {
                            let px = x + (col * scale) + sx;
                            if px >= 0 && px < self.width {
                                let off = row_offset + px as usize * 4;
                                self.data[off..off + 4].copy_from_slice(&color);
                            }
                        }
                    }
                }
            }
        }
    }

    pub fn render_text(&mut self, start_x: i32, start_y: i32, text: &str, color: [u8; 4], max_chars: usize) {
        self.render_text_scaled(start_x, start_y, text, color, max_chars, 1);
    }

    pub fn render_text_scaled(&mut self, start_x: i32, start_y: i32, text: &str, color: [u8; 4], max_chars: usize, scale: i32) {
        self.render_text_scaled_spaced(start_x, start_y, text, color, max_chars, scale, 0);
    }

    pub fn render_text_scaled_spaced(
        &mut self,
        start_x: i32,
        start_y: i32,
        text: &str,
        color: [u8; 4],
        max_chars: usize,
        scale: i32,
        extra_spacing: i32,
    ) {
        let mut cur_x = start_x;
        let advance = (8 * scale) + extra_spacing;
        for (i, c) in text.chars().enumerate() {
            if i >= max_chars {
                break;
            }
            self.render_char_scaled(cur_x, start_y, c, color, scale);
            cur_x += advance;
        }
    }

    /// Renders scaled text with a smooth 2D gradient from top-left (`color_tl`) to bottom-right (`color_br`).
    pub fn render_text_gradient_scaled(
        &mut self,
        start_x: i32,
        start_y: i32,
        text: &str,
        color_tl: [u8; 4],
        color_br: [u8; 4],
        max_chars: usize,
        scale: i32,
        extra_spacing: i32,
    ) {
        let chars: Vec<char> = text.chars().take(max_chars).collect();
        if chars.is_empty() || scale <= 0 {
            return;
        }
        let advance = (8 * scale) + extra_spacing;
        let total_w = ((chars.len() as i32 * advance) - extra_spacing).max(1);
        let total_h = (8 * scale).max(1);

        let mut cur_x = start_x;
        for c in chars {
            let code = c as usize;
            if (32..=126).contains(&code) {
                let glyph = &FONT_8X8[code - 32];
                for (row, &bits) in glyph.iter().enumerate() {
                    for sy in 0..scale {
                        let local_y = (row as i32 * scale) + sy;
                        let py = start_y + local_y;
                        if py < 0 || py >= self.height {
                            continue;
                        }
                        let row_offset = py as usize * self.stride;
                        for col in 0..8 {
                            if (bits & (1 << (7 - col))) != 0 {
                                for sx in 0..scale {
                                    let px = cur_x + (col * scale) + sx;
                                    if px >= 0 && px < self.width {
                                        let local_x = px - start_x;
                                        // Blend factor t in 0..=256 combining horizontal (65%) and vertical (35%) gradient
                                        let tx = (local_x * 256 / total_w).clamp(0, 256);
                                        let ty = (local_y * 256 / total_h).clamp(0, 256);
                                        let t = (tx * 166 + ty * 90) / 256;
                                        let r = ((color_tl[0] as i32 * (256 - t) + color_br[0] as i32 * t) / 256) as u8;
                                        let g = ((color_tl[1] as i32 * (256 - t) + color_br[1] as i32 * t) / 256) as u8;
                                        let b = ((color_tl[2] as i32 * (256 - t) + color_br[2] as i32 * t) / 256) as u8;
                                        let off = row_offset + px as usize * 4;
                                        self.data[off..off + 4].copy_from_slice(&[r, g, b, 255]);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            cur_x += advance;
        }
    }
}



