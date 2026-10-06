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

    pub fn render_text(&mut self, start_x: i32, start_y: i32, text: &str, color: [u8; 4], max_chars: usize) {
        let mut cur_x = start_x;
        for (i, c) in text.chars().enumerate() {
            if i >= max_chars {
                break;
            }
            self.render_char(cur_x, start_y, c, color);
            cur_x += 8;
        }
    }
}

