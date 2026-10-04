// Minimal 8x8 monochrome bitmap font (ASCII 32..=126) for dependency-free text rendering
pub const FONT_WIDTH: usize = 8;
pub const FONT_HEIGHT: usize = 8;

// Basic 8x8 font table for ASCII characters
include!("font_data.rs");

pub fn render_char(data: &mut [u8], stride: usize, target_w: i32, target_h: i32, x: i32, y: i32, c: char, color: [u8; 4]) {
    let code = c as usize;
    if code < 32 || code > 126 {
        return;
    }
    let glyph = &FONT_8X8[code - 32];
    for row in 0..8 {
        let py = y + row as i32;
        if py < 0 || py >= target_h { continue; }
        let bits = glyph[row];
        for col in 0..8 {
            if (bits & (1 << (7 - col))) != 0 {
                let px = x + col as i32;
                if px >= 0 && px < target_w {
                    let off = py as usize * stride + px as usize * 4;
                    data[off..off + 4].copy_from_slice(&color);
                }
            }
        }
    }
}

pub fn render_text(data: &mut [u8], stride: usize, target_w: i32, target_h: i32, start_x: i32, start_y: i32, text: &str, color: [u8; 4], max_chars: usize) {
    let mut cur_x = start_x;
    for (i, c) in text.chars().enumerate() {
        if i >= max_chars {
            break;
        }
        render_char(data, stride, target_w, target_h, cur_x, start_y, c, color);
        cur_x += 8;
    }
}
