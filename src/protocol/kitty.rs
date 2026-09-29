use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;

/// Kitty Graphics Protocol escape sequence generator.
pub struct KittyEncoder;

impl KittyEncoder {
    /// Escape sequence to delete all placed images on screen
    pub fn clear_all_images() -> &'static str {
        "\x1b_Ga=d,d=a\x1b\\"
    }

    /// Encode a full-screen frame buffer to Kitty Graphics Protocol.
    /// Uses image ID and C=1 to suppress cursor movement and prevent screen scrolling.
    /// Prepends cursor home sequence `\x1b[H` so the frame aligns cleanly to the terminal viewport.
    pub fn encode_fullscreen_frame(
        rgba: &[u8],
        width: u32,
        height: u32,
        term_cols: u16,
        term_rows: u16,
        image_id: u32,
    ) -> String {
        let b64_data = BASE64.encode(rgba);
        let mut output = String::with_capacity(b64_data.len() + 256);

        // Move cursor to top-left (1,1) before displaying
        output.push_str("\x1b[H");

        let chunk_size = 4096;
        let total_len = b64_data.len();
        let mut offset = 0;
        let mut first = true;

        while offset < total_len {
            let next_offset = std::cmp::min(offset + chunk_size, total_len);
            let chunk = &b64_data[offset..next_offset];
            let is_last = next_offset >= total_len;
            let m_flag = if is_last { 0 } else { 1 };

            if first {
                output.push_str(&format!(
                    "\x1b_Ga=T,i={},C=1,f=32,s={},v={},c={},r={},m={};",
                    image_id, width, height, term_cols, term_rows, m_flag
                ));
                first = false;
            } else {
                output.push_str(&format!("\x1b_Gm={};", m_flag));
            }

            output.push_str(chunk);
            output.push_str("\x1b\\");

            offset = next_offset;
        }

        output
    }

    /// Encode RGBA pixel data to Kitty Graphics Protocol escape sequence chunks.
    pub fn encode_rgba_image(
        rgba: &[u8],
        width: u32,
        height: u32,
        dst_col: Option<u16>,
        dst_row: Option<u16>,
    ) -> String {
        let b64_data = BASE64.encode(rgba);
        Self::format_chunks(&b64_data, width, height, dst_col, dst_row, None, None)
    }

    /// Encode a sub-rectangle of an RGBA image (dirty/damage region).
    pub fn encode_rgba_subimage(
        full_rgba: &[u8],
        full_width: u32,
        _full_height: u32,
        rect_x: u32,
        rect_y: u32,
        rect_w: u32,
        rect_h: u32,
        dst_col: Option<u16>,
        dst_row: Option<u16>,
    ) -> String {
        let mut sub_rgba = Vec::with_capacity((rect_w * rect_h * 4) as usize);
        let stride = (full_width * 4) as usize;
        let row_bytes = (rect_w * 4) as usize;

        for y in 0..rect_h {
            let src_y = (rect_y + y) as usize;
            let src_start = src_y * stride + (rect_x * 4) as usize;
            let src_end = src_start + row_bytes;
            if src_end <= full_rgba.len() {
                sub_rgba.extend_from_slice(&full_rgba[src_start..src_end]);
            }
        }

        let b64_data = BASE64.encode(&sub_rgba);
        Self::format_chunks(&b64_data, rect_w, rect_h, dst_col, dst_row, None, None)
    }

    fn format_chunks(
        b64_data: &str,
        width: u32,
        height: u32,
        dst_col: Option<u16>,
        dst_row: Option<u16>,
        cell_cols: Option<u16>,
        cell_rows: Option<u16>,
    ) -> String {
        let chunk_size = 4096;
        let total_len = b64_data.len();
        let mut output = String::with_capacity(total_len + 256);

        let mut offset = 0;
        let mut first = true;

        if total_len == 0 {
            return output;
        }

        while offset < total_len {
            let next_offset = std::cmp::min(offset + chunk_size, total_len);
            let chunk = &b64_data[offset..next_offset];
            let is_last = next_offset >= total_len;
            let m_flag = if is_last { 0 } else { 1 };

            if first {
                output.push_str("\x1b_Ga=T,f=32");
                output.push_str(&format!(",s={},v={}", width, height));

                if let Some(c) = cell_cols {
                    output.push_str(&format!(",c={}", c));
                }
                if let Some(r) = cell_rows {
                    output.push_str(&format!(",r={}", r));
                }
                if let (Some(col), Some(row)) = (dst_col, dst_row) {
                    output.push_str(&format!(",X={},Y={}", col, row));
                }
                output.push_str(&format!(",m={};", m_flag));
                first = false;
            } else {
                output.push_str(&format!("\x1b_Gm={};", m_flag));
            }

            output.push_str(chunk);
            output.push_str("\x1b\\");

            offset = next_offset;
        }

        output
    }
}
