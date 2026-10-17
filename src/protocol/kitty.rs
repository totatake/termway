use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;

/// Kitty Graphics Protocol escape sequence generator.
pub struct KittyEncoder;

use std::fmt::Write;
use miniz_oxide::deflate::core::{
    compress, create_comp_flags_from_zip_params, CompressionStrategy, CompressorOxide, TDEFLFlush,
    TDEFLStatus,
};

use crate::display::DamageRect;

/// High-performance stateful Kitty protocol encoder reusing zlib compression state and buffers.
pub struct KittyStreamEncoder {
    compressor: CompressorOxide,
    compressed_buf: Vec<u8>,
    sub_image_buf: Vec<u8>,
    partial_frames_count: u32,
    has_full_frame: bool,
}

impl Default for KittyStreamEncoder {
    fn default() -> Self {
        let flags = create_comp_flags_from_zip_params(1, 1, CompressionStrategy::Default as i32);
        Self {
            compressor: CompressorOxide::new(flags),
            compressed_buf: Vec::with_capacity(256 * 1024),
            sub_image_buf: Vec::with_capacity(64 * 1024),
            partial_frames_count: 0,
            has_full_frame: false,
        }
    }
}

impl KittyStreamEncoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reset stream state, forcing full-frame transmission on next frame.
    pub fn reset(&mut self) {
        self.partial_frames_count = 0;
        self.has_full_frame = false;
    }

    /// Encodes a frame into Kitty escape sequence using partial rectangular placement
    /// if damage is small enough (<= 50% screen area) or full-frame otherwise.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_frame_into(
        &mut self,
        rgba: &[u8],
        width: u32,
        height: u32,
        term_cols: u16,
        term_rows: u16,
        cell_width_px: u16,
        cell_height_px: u16,
        damage: Option<&[DamageRect]>,
        needs_clear: bool,
        output: &mut String,
    ) {
        const MAX_PARTIAL_FRAMES: u32 = 60;
        let total_area = (width as u64) * (height as u64);

        if needs_clear {
            self.reset();
        }

        // If explicitly no damage (empty damage list) and not forced clear, skip encoding
        if !needs_clear && damage.is_some_and(|d| d.is_empty()) {
            output.clear();
            return;
        }

        let can_partial = !needs_clear
            && self.has_full_frame
            && self.partial_frames_count < MAX_PARTIAL_FRAMES
            && cell_width_px > 0
            && cell_height_px > 0
            && damage.is_some_and(|d| !d.is_empty());

        if can_partial {
            let rects = damage.unwrap();
            let mut min_x = u32::MAX;
            let mut min_y = u32::MAX;
            let mut max_x = 0;
            let mut max_y = 0;

            for r in rects {
                min_x = min_x.min(r.x);
                min_y = min_y.min(r.y);
                max_x = max_x.max(r.x + r.width);
                max_y = max_y.max(r.y + r.height);
            }

            // Snap bounding box outward to character cell boundaries
            let cw = cell_width_px as u32;
            let ch = cell_height_px as u32;
            let start_col = min_x / cw;
            let start_row = min_y / ch;
            let end_col = (max_x + cw - 1) / cw;
            let end_row = (max_y + ch - 1) / ch;

            let snapped_x = (start_col * cw).min(width);
            let snapped_y = (start_row * ch).min(height);
            let snapped_w = ((end_col - start_col) * cw).min(width.saturating_sub(snapped_x));
            let snapped_h = ((end_row - start_row) * ch).min(height.saturating_sub(snapped_y));

            let bbox_area = (snapped_w as u64) * (snapped_h as u64);

            // Only use partial placement if dirty bounding box is <= 50% of the total screen area
            if bbox_area > 0 && bbox_area * 2 <= total_area {
                let col = (start_col as u16 + 1).min(term_cols).max(1);
                let row = (start_row as u16 + 1).min(term_rows).max(1);
                let cols = (((snapped_w + cw - 1) / cw) as u16)
                    .min(term_cols.saturating_sub(col - 1))
                    .max(1);
                let rows = (((snapped_h + ch - 1) / ch) as u16)
                    .min(term_rows.saturating_sub(row - 1))
                    .max(1);

                self.encode_subimage_into(
                    rgba,
                    width,
                    snapped_x,
                    snapped_y,
                    snapped_w,
                    snapped_h,
                    col,
                    row,
                    cols,
                    rows,
                    output,
                );
                self.partial_frames_count += 1;
                return;
            }
        }

        // Full-frame fallback: if previously partial frames were emitted, delete them atomically
        let clear_placements = self.partial_frames_count > 0;
        self.partial_frames_count = 0;
        self.has_full_frame = true;
        self.encode_fullscreen_frame_into_with_cleanup(
            rgba,
            width,
            height,
            term_cols,
            term_rows,
            1,
            clear_placements,
            output,
        );
    }

    /// Compresses and encodes an RGBA sub-rectangle into Kitty protocol chunked escape sequences,
    /// placed at character grid cell (col, row) without moving cursor.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_subimage_into(
        &mut self,
        full_rgba: &[u8],
        full_width: u32,
        rect_x: u32,
        rect_y: u32,
        rect_w: u32,
        rect_h: u32,
        col: u16,
        row: u16,
        cell_cols: u16,
        cell_rows: u16,
        output: &mut String,
    ) {
        let stride = (full_width * 4) as usize;
        let row_bytes = (rect_w * 4) as usize;
        let sub_len = row_bytes * (rect_h as usize);

        self.sub_image_buf.clear();
        if self.sub_image_buf.capacity() < sub_len {
            self.sub_image_buf.reserve(sub_len - self.sub_image_buf.capacity());
        }

        for y in 0..rect_h {
            let src_y = (rect_y + y) as usize;
            let src_start = src_y * stride + (rect_x * 4) as usize;
            let src_end = src_start + row_bytes;
            if src_end <= full_rgba.len() {
                self.sub_image_buf.extend_from_slice(&full_rgba[src_start..src_end]);
            } else if src_start < full_rgba.len() {
                self.sub_image_buf.extend_from_slice(&full_rgba[src_start..]);
                self.sub_image_buf.resize(self.sub_image_buf.len() + (src_end - full_rgba.len()), 0);
            } else {
                self.sub_image_buf.resize(self.sub_image_buf.len() + row_bytes, 0);
            }
        }

        // 1. Zero-allocation RFC 1950 zlib compression (level 1)
        self.compressor.reset();
        self.compressed_buf.clear();
        let min_cap = (self.sub_image_buf.len() / 2).max(1024);
        if self.compressed_buf.capacity() < min_cap {
            self.compressed_buf.reserve(min_cap - self.compressed_buf.capacity());
        }
        self.compressed_buf.resize(self.compressed_buf.capacity(), 0);

        let mut out_pos = 0;
        let mut input = &self.sub_image_buf[..];
        loop {
            let (status, bytes_in, bytes_out) = compress(
                &mut self.compressor,
                input,
                &mut self.compressed_buf[out_pos..],
                TDEFLFlush::Finish,
            );
            out_pos += bytes_out;

            match status {
                TDEFLStatus::Done => {
                    self.compressed_buf.truncate(out_pos);
                    break;
                }
                TDEFLStatus::Okay if bytes_in <= input.len() => {
                    input = &input[bytes_in..];
                    if self.compressed_buf.len().saturating_sub(out_pos) < 4096 {
                        self.compressed_buf.resize(self.compressed_buf.len() * 2, 0);
                    }
                }
                _ => panic!("miniz_oxide compress failed"),
            }
        }

        // 2. Direct streaming Base64 encoding into Kitty chunk packets
        output.clear();
        let est_capacity = (out_pos * 4 / 3) + (out_pos / 3072 + 1) * 64 + 64;
        if output.capacity() < est_capacity {
            output.reserve(est_capacity - output.capacity());
        }

        let _ = write!(output, "\x1b[{};{}H", row, col);

        let chunk_raw_size = 3072;
        let total_raw = self.compressed_buf.len();
        let mut offset = 0;
        let mut first = true;

        while offset < total_raw {
            let next_offset = std::cmp::min(offset + chunk_raw_size, total_raw);
            let raw_chunk = &self.compressed_buf[offset..next_offset];
            let is_last = next_offset >= total_raw;
            let m_flag = if is_last { 0 } else { 1 };

            if first {
                let _ = write!(
                    output,
                    "\x1b_Ga=T,q=2,C=1,f=32,o=z,z=-2147483648,s={},v={},c={},r={},m={};",
                    rect_w, rect_h, cell_cols, cell_rows, m_flag
                );
                first = false;
            } else if m_flag == 1 {
                output.push_str("\x1b_Gm=1;");
            } else {
                output.push_str("\x1b_Gm=0;");
            }

            BASE64.encode_string(raw_chunk, output);
            output.push_str("\x1b\\");

            offset = next_offset;
        }
    }

    /// Compresses and encodes an RGBA full-screen buffer into Kitty protocol chunked escape sequences.
    /// Reuses internal compression state and output capacity with zero heap reallocations.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_fullscreen_frame_into(
        &mut self,
        rgba: &[u8],
        width: u32,
        height: u32,
        term_cols: u16,
        term_rows: u16,
        image_id: u32,
        output: &mut String,
    ) {
        self.encode_fullscreen_frame_into_with_cleanup(
            rgba, width, height, term_cols, term_rows, image_id, false, output,
        );
    }

    /// Fullscreen encoding optionally prepending placement cleanup sequence.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_fullscreen_frame_into_with_cleanup(
        &mut self,
        rgba: &[u8],
        width: u32,
        height: u32,
        term_cols: u16,
        term_rows: u16,
        image_id: u32,
        clear_placements: bool,
        output: &mut String,
    ) {
        // 1. Zero-allocation RFC 1950 zlib compression (level 1)
        self.compressor.reset();
        self.compressed_buf.clear();
        let min_cap = (rgba.len() / 2).max(1024);
        if self.compressed_buf.capacity() < min_cap {
            self.compressed_buf.reserve(min_cap - self.compressed_buf.capacity());
        }
        self.compressed_buf.resize(self.compressed_buf.capacity(), 0);

        let mut out_pos = 0;
        let mut input = rgba;
        loop {
            let (status, bytes_in, bytes_out) = compress(
                &mut self.compressor,
                input,
                &mut self.compressed_buf[out_pos..],
                TDEFLFlush::Finish,
            );
            out_pos += bytes_out;

            match status {
                TDEFLStatus::Done => {
                    self.compressed_buf.truncate(out_pos);
                    break;
                }
                TDEFLStatus::Okay if bytes_in <= input.len() => {
                    input = &input[bytes_in..];
                    if self.compressed_buf.len().saturating_sub(out_pos) < 4096 {
                        self.compressed_buf.resize(self.compressed_buf.len() * 2, 0);
                    }
                }
                _ => panic!("miniz_oxide compress failed"),
            }
        }

        // 2. Direct streaming Base64 encoding into Kitty chunk packets (no intermediate string)
        output.clear();
        let extra = if clear_placements { 16 } else { 0 };
        let est_capacity = (out_pos * 4 / 3) + (out_pos / 3072 + 1) * 64 + 64 + extra;
        if output.capacity() < est_capacity {
            output.reserve(est_capacity - output.capacity());
        }

        if clear_placements {
            output.push_str("\x1b_Ga=d,d=a\x1b\\");
        }
        output.push_str("\x1b[H");

        let chunk_raw_size = 3072;
        let total_raw = self.compressed_buf.len();
        let mut offset = 0;
        let mut first = true;

        while offset < total_raw {
            let next_offset = std::cmp::min(offset + chunk_raw_size, total_raw);
            let raw_chunk = &self.compressed_buf[offset..next_offset];
            let is_last = next_offset >= total_raw;
            let m_flag = if is_last { 0 } else { 1 };

            if first {
                let _ = write!(
                    output,
                    "\x1b_Ga=T,q=2,i={},C=1,f=32,o=z,z=-2147483648,s={},v={},c={},r={},m={};",
                    image_id, width, height, term_cols, term_rows, m_flag
                );
                first = false;
            } else if m_flag == 1 {
                output.push_str("\x1b_Gm=1;");
            } else {
                output.push_str("\x1b_Gm=0;");
            }

            BASE64.encode_string(raw_chunk, output);
            output.push_str("\x1b\\");

            offset = next_offset;
        }
    }
}

thread_local! {
    static THREAD_KITTY_STREAM: std::cell::RefCell<KittyStreamEncoder> = std::cell::RefCell::new(KittyStreamEncoder::new());
}

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
        let mut output = String::new();
        THREAD_KITTY_STREAM.with(|stream| {
            stream.borrow_mut().encode_fullscreen_frame_into(
                rgba, width, height, term_cols, term_rows, image_id, &mut output,
            );
        });
        output
    }

    /// Encode a frame with automatic partial rectangular placement or full-screen fallback.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_frame(
        rgba: &[u8],
        width: u32,
        height: u32,
        term_cols: u16,
        term_rows: u16,
        cell_width_px: u16,
        cell_height_px: u16,
        damage: Option<&[DamageRect]>,
        needs_clear: bool,
    ) -> String {
        let mut output = String::new();
        THREAD_KITTY_STREAM.with(|stream| {
            stream.borrow_mut().encode_frame_into(
                rgba,
                width,
                height,
                term_cols,
                term_rows,
                cell_width_px,
                cell_height_px,
                damage,
                needs_clear,
                &mut output,
            );
        });
        output
    }

    /// Optimized zero-allocation version reusing preallocated base64 and output string buffers.
    /// Uses RFC 1950 zlib compression (`o=z`) at compression level 1 to drastically reduce
    /// terminal PTY bandwidth and escape sequence parsing overhead.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_fullscreen_frame_into(
        rgba: &[u8],
        width: u32,
        height: u32,
        term_cols: u16,
        term_rows: u16,
        image_id: u32,
        _b64_buf: &mut String,
        output: &mut String,
    ) {
        THREAD_KITTY_STREAM.with(|stream| {
            stream.borrow_mut().encode_fullscreen_frame_into(
                rgba, width, height, term_cols, term_rows, image_id, output,
            );
        });
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
    #[allow(clippy::too_many_arguments)]
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
                output.push_str("\x1b_Ga=T,q=2,f=32");
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
