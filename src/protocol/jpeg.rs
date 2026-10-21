//! High-performance, zero-allocation Baseline JPEG (JFIF YCbCr 4:2:0) encoder in pure Rust.
//!
//! Features:
//! - Standard Baseline DCT (SOF0) compatible with all JPEG decoders (libjpeg, stb_image, WebKit, Noctty).
//! - Sub-rectangle direct stride reading: encodes arbitrary `(rect_x, rect_y, rect_w, rect_h)`
//!   directly from raw RGBA buffers without intermediate full-frame allocation or memory copying.
//! - Fixed-point integer FDCT (Loeffler/LLM) with zero floating-point overhead.
//! - In-place MCU streaming: each 16x16 macroblock fits entirely in L1 cache (384 bytes).
//! - Byte-stuffing BitWriter with zero dynamic heap allocation into pre-allocated `Vec<u8>`.

// Standard Annex K Luminance Quantization Table (Zig-Zag order)
const STD_LUMA_QTABLE: [u8; 64] = [
    16, 11, 12, 14, 12, 10, 16, 14,
    13, 14, 18, 17, 16, 19, 24, 40,
    26, 24, 22, 22, 24, 49, 35, 37,
    29, 40, 58, 51, 61, 60, 57, 51,
    56, 55, 64, 72, 92, 78, 64, 68,
    87, 69, 55, 56, 80, 109, 81, 87,
    95, 98, 103, 104, 103, 62, 77, 113,
    121, 112, 100, 120, 92, 101, 103, 99,
];

// Standard Annex K Chrominance Quantization Table (Zig-Zag order)
const STD_CHROMA_QTABLE: [u8; 64] = [
    17, 18, 24, 47, 99, 99, 99, 99,
    18, 21, 26, 66, 99, 99, 99, 99,
    24, 26, 56, 99, 99, 99, 99, 99,
    47, 66, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99,
];

// Standard Zig-Zag reordering table
#[rustfmt::skip]
const ZIGZAG: [usize; 64] = [
     0,  1,  8, 16,  9,  2,  3, 10,
    17, 24, 32, 25, 18, 11,  4,  5,
    12, 19, 26, 33, 40, 48, 41, 34,
    27, 20, 13,  6,  7, 14, 21, 28,
    35, 42, 49, 56, 57, 50, 43, 36,
    29, 22, 15, 23, 30, 37, 44, 51,
    58, 59, 52, 45, 38, 31, 39, 46,
    53, 60, 61, 54, 47, 55, 62, 63,
];

// Standard Huffman bits and values (Annex K.3)
const LUMA_DC_BITS: [u8; 16] = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
const LUMA_DC_VALS: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];

const CHROMA_DC_BITS: [u8; 16] = [0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0];
const CHROMA_DC_VALS: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];

#[rustfmt::skip]
const LUMA_AC_BITS: [u8; 16] = [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 125];
#[rustfmt::skip]
const LUMA_AC_VALS: [u8; 162] = [
    0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12,
    0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07,
    0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xa1, 0x08,
    0x23, 0x42, 0xb1, 0xc1, 0x15, 0x52, 0xd1, 0xf0,
    0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0a, 0x16,
    0x17, 0x18, 0x19, 0x1a, 0x25, 0x26, 0x27, 0x28,
    0x29, 0x2a, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39,
    0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49,
    0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59,
    0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69,
    0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79,
    0x7a, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89,
    0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98,
    0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7,
    0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6,
    0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5,
    0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4,
    0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe1, 0xe2,
    0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea,
    0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8,
    0xf9, 0xfa,
];

#[rustfmt::skip]
const CHROMA_AC_BITS: [u8; 16] = [0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 119];
#[rustfmt::skip]
const CHROMA_AC_VALS: [u8; 162] = [
    0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21,
    0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71,
    0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91,
    0xa1, 0xb1, 0xc1, 0x09, 0x23, 0x33, 0x52, 0xf0,
    0x15, 0x62, 0x72, 0xd1, 0x0a, 0x16, 0x24, 0x34,
    0xe1, 0x25, 0xf1, 0x17, 0x18, 0x19, 0x1a, 0x26,
    0x27, 0x28, 0x29, 0x2a, 0x35, 0x36, 0x37, 0x38,
    0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48,
    0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58,
    0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68,
    0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78,
    0x79, 0x7a, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87,
    0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96,
    0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5,
    0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4,
    0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3,
    0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2,
    0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda,
    0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9,
    0xea, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8,
    0xf9, 0xfa,
];

/// Precomputed Huffman lookup table mapping symbol -> (code, length in bits).
#[derive(Clone, Copy)]
pub struct HuffmanLut {
    codes: [u16; 256],
    lengths: [u8; 256],
}

impl HuffmanLut {
    const fn new(bits: &[u8; 16], vals: &[u8]) -> Self {
        let mut codes = [0u16; 256];
        let mut lengths = [0u8; 256];

        let mut code = 0u16;
        let mut val_idx = 0;

        let mut len = 1usize;
        while len <= 16 {
            let count = bits[len - 1] as usize;
            let mut i = 0;
            while i < count {
                let symbol = vals[val_idx] as usize;
                codes[symbol] = code;
                lengths[symbol] = len as u8;
                code += 1;
                val_idx += 1;
                i += 1;
            }
            code <<= 1;
            len += 1;
        }

        Self { codes, lengths }
    }

    #[inline(always)]
    pub fn get(&self, symbol: u8) -> (u16, u8) {
        let s = symbol as usize;
        (self.codes[s], self.lengths[s])
    }
}

// Precomputed static Huffman tables
static LUMA_DC_LUT: HuffmanLut = HuffmanLut::new(&LUMA_DC_BITS, &LUMA_DC_VALS);
static CHROMA_DC_LUT: HuffmanLut = HuffmanLut::new(&CHROMA_DC_BITS, &CHROMA_DC_VALS);
static LUMA_AC_LUT: HuffmanLut = HuffmanLut::new(&LUMA_AC_BITS, &LUMA_AC_VALS);
static CHROMA_AC_LUT: HuffmanLut = HuffmanLut::new(&CHROMA_AC_BITS, &CHROMA_AC_VALS);

/// BitWriter packing bits and emitting byte-stuffed 0xFF bytes.
pub struct BitWriter<'a> {
    out: &'a mut Vec<u8>,
    buffer: u64,
    bits_in_buffer: u8,
}

impl<'a> BitWriter<'a> {
    #[inline(always)]
    pub fn new(out: &'a mut Vec<u8>) -> Self {
        Self {
            out,
            buffer: 0,
            bits_in_buffer: 0,
        }
    }

    #[inline(always)]
    pub fn write_bits(&mut self, code: u16, len: u8) {
        if len == 0 {
            return;
        }
        self.buffer = (self.buffer << len) | (code as u64 & ((1u64 << len) - 1));
        self.bits_in_buffer += len;

        while self.bits_in_buffer >= 8 {
            self.bits_in_buffer -= 8;
            let byte = ((self.buffer >> self.bits_in_buffer) & 0xFF) as u8;
            self.out.push(byte);
            if byte == 0xFF {
                self.out.push(0x00); // JPEG byte stuffing
            }
        }
    }

    #[inline(always)]
    pub fn flush_pad(&mut self) {
        if self.bits_in_buffer > 0 {
            let pad_bits = 8 - self.bits_in_buffer;
            let code = (1u16 << pad_bits) - 1;
            self.write_bits(code, pad_bits);
        }
    }
}

/// Computes bit category and magnitude bits for a signed integer value.
#[inline(always)]
fn get_category_and_bits(val: i32) -> (u8, u16) {
    if val == 0 {
        (0, 0)
    } else {
        let abs_val = val.abs();
        let cat = 32 - abs_val.leading_zeros() as u8;
        let bits = if val > 0 {
            val as u16
        } else {
            (val + (1 << cat) - 1) as u16
        };
        (cat, bits)
    }
}

/// Standard Loeffler / LLM 1D 8-point Forward Discrete Cosine Transform on 16-bit integers.
/// Uses fixed-point scaling factor of 13 bits (8192).
#[inline(always)]
fn fdct_1d(data: &mut [i32; 64], stride: usize, offset: usize) {
    const FIX_0_298631375: i32 = 2446; // FIX(0.298631375)
    const FIX_0_390180644: i32 = 3196; // FIX(0.390180644)
    const FIX_0_541196100: i32 = 4433; // FIX(0.541196100)
    const FIX_0_765366865: i32 = 6270; // FIX(0.765366865)
    const FIX_0_899976223: i32 = 7373; // FIX(0.899976223)
    const FIX_1_175875602: i32 = 9633; // FIX(1.175875602)
    const FIX_1_501321110: i32 = 12299; // FIX(1.501321110)
    const FIX_1_847759065: i32 = 15137; // FIX(1.847759065)
    const FIX_1_961570560: i32 = 16069; // FIX(1.961570560)
    const FIX_2_053119869: i32 = 16819; // FIX(2.053119869)
    const FIX_2_562915447: i32 = 20995; // FIX(2.562915447)
    const FIX_3_072711026: i32 = 25172; // FIX(3.072711026)

    let p0 = data[offset];
    let p1 = data[offset + stride];
    let p2 = data[offset + 2 * stride];
    let p3 = data[offset + 3 * stride];
    let p4 = data[offset + 4 * stride];
    let p5 = data[offset + 5 * stride];
    let p6 = data[offset + 6 * stride];
    let p7 = data[offset + 7 * stride];

    let tmp0 = p0 + p7;
    let tmp7 = p0 - p7;
    let tmp1 = p1 + p6;
    let tmp6 = p1 - p6;
    let tmp2 = p2 + p5;
    let tmp5 = p2 - p5;
    let tmp3 = p3 + p4;
    let tmp4 = p3 - p4;

    // Even part
    let tmp10 = tmp0 + tmp3;
    let tmp13 = tmp0 - tmp3;
    let tmp11 = tmp1 + tmp2;
    let tmp12 = tmp1 - tmp2;

    data[offset] = (tmp10 + tmp11) << 2;
    data[offset + 4 * stride] = (tmp10 - tmp11) << 2;

    let z1 = (tmp12 + tmp13) * FIX_0_541196100;
    data[offset + 2 * stride] = (z1 + tmp13 * FIX_0_765366865 + (1 << 10)) >> 11;
    data[offset + 6 * stride] = (z1 - tmp12 * FIX_1_847759065 + (1 << 10)) >> 11;

    // Odd part
    let z1 = tmp4 + tmp7;
    let z2 = tmp5 + tmp6;
    let z3 = tmp4 + tmp6;
    let z4 = tmp5 + tmp7;
    let z5 = (z3 + z4) * FIX_1_175875602;

    let tmp4 = tmp4 * FIX_0_298631375;
    let tmp5 = tmp5 * FIX_2_053119869;
    let tmp6 = tmp6 * FIX_3_072711026;
    let tmp7 = tmp7 * FIX_1_501321110;
    let z1 = -z1 * FIX_0_899976223;
    let z2 = -z2 * FIX_2_562915447;
    let z3 = -z3 * FIX_1_961570560;
    let z4 = -z4 * FIX_0_390180644;

    let z3 = z3 + z5;
    let z4 = z4 + z5;

    data[offset + 7 * stride] = (tmp4 + z1 + z3 + (1 << 10)) >> 11;
    data[offset + 5 * stride] = (tmp5 + z2 + z4 + (1 << 10)) >> 11;
    data[offset + 3 * stride] = (tmp6 + z2 + z3 + (1 << 10)) >> 11;
    data[offset + stride] = (tmp7 + z1 + z4 + (1 << 10)) >> 11;
}

/// Performs full 2D 8x8 integer DCT on a block.
#[inline(always)]
fn fdct_2d(block: &mut [i32; 64]) {
    // 1D DCT on rows
    for row in 0..8 {
        fdct_1d(block, 1, row * 8);
    }
    // 1D DCT on columns
    for col in 0..8 {
        fdct_1d(block, 8, col);
    }
}

/// Scaled quantization table holder for Luminance and Chrominance.
pub struct QuantizationTables {
    pub y_table: [u8; 64],
    pub c_table: [u8; 64],
}

impl QuantizationTables {
    pub fn new(quality: u8) -> Self {
        let q = quality.clamp(1, 100) as u32;
        let scale = if q < 50 { 5000 / q } else { 200 - 2 * q };

        let mut y_table = [0u8; 64];
        let mut c_table = [0u8; 64];

        for i in 0..64 {
            let y_val = ((STD_LUMA_QTABLE[i] as u32 * scale + 50) / 100).clamp(1, 255) as u8;
            let c_val = ((STD_CHROMA_QTABLE[i] as u32 * scale + 50) / 100).clamp(1, 255) as u8;
            y_table[i] = y_val;
            c_table[i] = c_val;
        }

        Self {
            y_table,
            c_table,
        }
    }
}

/// Quantizes and zig-zag reorders DCT coefficients.
#[inline(always)]
fn quantize_and_zigzag(dct: &[i32; 64], q_table: &[u8; 64], out: &mut [i16; 64]) {
    for k in 0..64 {
        let raster_idx = ZIGZAG[k];
        let val = dct[raster_idx];
        let q = q_table[k] as i32;
        // Divide by 128 (8 * sqrt(2) * 8 * sqrt(2)) because 2D DCT scale adds 128x
        let q128 = q * 128;
        let quantized = if val >= 0 {
            (val + q128 / 2) / q128
        } else {
            (val - q128 / 2) / q128
        };
        out[k] = quantized.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
    }
}

/// Encodes a quantized 8x8 block using standard DC and AC Huffman tables.
#[inline(always)]
fn encode_block(
    block: &[i16; 64],
    last_dc: &mut i32,
    dc_lut: &HuffmanLut,
    ac_lut: &HuffmanLut,
    writer: &mut BitWriter,
) {
    // 1. DC Coefficient
    let dc = block[0] as i32;
    let diff = dc - *last_dc;
    *last_dc = dc;

    let (dc_cat, dc_bits) = get_category_and_bits(diff);
    let (dc_code, dc_len) = dc_lut.get(dc_cat);
    writer.write_bits(dc_code, dc_len);
    writer.write_bits(dc_bits, dc_cat);

    // 2. AC Coefficients
    let mut r = 0u8; // run of zeros
    for k in 1..64 {
        let ac = block[k] as i32;
        if ac == 0 {
            r += 1;
        } else {
            while r >= 16 {
                // ZRL (16 zeros)
                let (zrl_code, zrl_len) = ac_lut.get(0xF0);
                writer.write_bits(zrl_code, zrl_len);
                r -= 16;
            }
            let (ac_cat, ac_bits) = get_category_and_bits(ac);
            let symbol = (r << 4) | ac_cat;
            let (ac_code, ac_len) = ac_lut.get(symbol);
            writer.write_bits(ac_code, ac_len);
            writer.write_bits(ac_bits, ac_cat);
            r = 0;
        }
    }

    if r > 0 {
        // EOB (End of Block)
        let (eob_code, eob_len) = ac_lut.get(0x00);
        writer.write_bits(eob_code, eob_len);
    }
}

/// High-performance zero-allocation Baseline JPEG (YCbCr 4:2:0) encoder.
pub struct JpegEncoder {
    quality: u8,
    q_tables: QuantizationTables,
}

impl JpegEncoder {
    pub fn new(quality: u8) -> Self {
        let q = quality.clamp(1, 100);
        Self {
            quality: q,
            q_tables: QuantizationTables::new(q),
        }
    }

    pub fn set_quality(&mut self, quality: u8) {
        let q = quality.clamp(1, 100);
        if self.quality != q {
            self.quality = q;
            self.q_tables = QuantizationTables::new(q);
        }
    }

    pub fn quality(&self) -> u8 {
        self.quality
    }

    /// Encodes an arbitrary sub-rectangle `(rect_x, rect_y, rect_w, rect_h)` from `full_rgba`
    /// (with stride `full_width * 4`) directly into `output` with zero dynamic allocations.
    pub fn encode_subimage_into(
        &self,
        full_rgba: &[u8],
        full_width: u32,
        rect_x: u32,
        rect_y: u32,
        rect_w: u32,
        rect_h: u32,
        output: &mut Vec<u8>,
    ) {
        if rect_w == 0 || rect_h == 0 {
            return;
        }
        output.clear();

        // 1. SOI (Start of Image)
        output.extend_from_slice(&[0xFF, 0xD8]);

        // 2. APP0 (JFIF Header)
        output.extend_from_slice(&[
            0xFF, 0xE0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0x00, 0x01, 0x01, 0x00, 0x00, 0x01,
            0x00, 0x01, 0x00, 0x00,
        ]);

        // 3. DQT (Quantization Tables: Y and CbCr)
        output.extend_from_slice(&[0xFF, 0xDB, 0x00, 0x84]);
        // Y table (id 0)
        output.push(0x00);
        output.extend_from_slice(&self.q_tables.y_table);
        // CbCr table (id 1)
        output.push(0x01);
        output.extend_from_slice(&self.q_tables.c_table);

        // 4. SOF0 (Start of Frame: Baseline DCT, 8-bit, 3 components, 4:2:0 subsampling)
        output.extend_from_slice(&[
            0xFF, 0xC0, 0x00, 0x11, 0x08, // 8-bit precision
            (rect_h >> 8) as u8, (rect_h & 0xFF) as u8, // Image height
            (rect_w >> 8) as u8, (rect_w & 0xFF) as u8, // Image width
            0x03, // 3 components
            // Component 1: Y, 2x2 sampling, QTable 0
            0x01, 0x22, 0x00,
            // Component 2: Cb, 1x1 sampling, QTable 1
            0x02, 0x11, 0x01,
            // Component 3: Cr, 1x1 sampling, QTable 1
            0x03, 0x11, 0x01,
        ]);

        // 5. DHT (Define Huffman Tables: DC0, AC0, DC1, AC1)
        Self::write_dht(output);

        // 6. SOS (Start of Scan)
        output.extend_from_slice(&[
            0xFF, 0xDA, 0x00, 0x0C, 0x03, // 3 components
            0x01, 0x00, // Y: DC 0, AC 0
            0x02, 0x11, // Cb: DC 1, AC 1
            0x03, 0x11, // Cr: DC 1, AC 1
            0x00, 0x3F, 0x00, // Spectral 0..63, successive 0
        ]);

        // 7. MCU (Macroblock) Processing
        let mcu_cols = (rect_w + 15) / 16;
        let mcu_rows = (rect_h + 15) / 16;

        let mut last_y_dc = 0i32;
        let mut last_cb_dc = 0i32;
        let mut last_cr_dc = 0i32;

        let mut dct_block = [0i32; 64];
        let mut quant_block = [0i16; 64];

        let mut writer = BitWriter::new(output);

        for mcu_y in 0..mcu_rows {
            let base_y = rect_y + mcu_y * 16;
            for mcu_x in 0..mcu_cols {
                let base_x = rect_x + mcu_x * 16;

                // Four 8x8 Y blocks: Y0, Y1, Y2, Y3
                for by in 0..2 {
                    for bx in 0..2 {
                        let block_x = base_x + bx * 8;
                        let block_y = base_y + by * 8;
                        Self::extract_luma_8x8(
                            full_rgba,
                            full_width,
                            block_x,
                            block_y,
                            rect_x + rect_w,
                            rect_y + rect_h,
                            &mut dct_block,
                        );
                        fdct_2d(&mut dct_block);
                        quantize_and_zigzag(&dct_block, &self.q_tables.y_table, &mut quant_block);
                        encode_block(
                            &quant_block,
                            &mut last_y_dc,
                            &LUMA_DC_LUT,
                            &LUMA_AC_LUT,
                            &mut writer,
                        );
                    }
                }

                // One 8x8 Cb block and one 8x8 Cr block (4:2:0 subsampled from 16x16)
                let mut cb_block = [0i32; 64];
                let mut cr_block = [0i32; 64];
                Self::extract_chroma_8x8(
                    full_rgba,
                    full_width,
                    base_x,
                    base_y,
                    rect_x + rect_w,
                    rect_y + rect_h,
                    &mut cb_block,
                    &mut cr_block,
                );

                // Encode Cb
                fdct_2d(&mut cb_block);
                quantize_and_zigzag(&cb_block, &self.q_tables.c_table, &mut quant_block);
                encode_block(
                    &quant_block,
                    &mut last_cb_dc,
                    &CHROMA_DC_LUT,
                    &CHROMA_AC_LUT,
                    &mut writer,
                );

                // Encode Cr
                fdct_2d(&mut cr_block);
                quantize_and_zigzag(&cr_block, &self.q_tables.c_table, &mut quant_block);
                encode_block(
                    &quant_block,
                    &mut last_cr_dc,
                    &CHROMA_DC_LUT,
                    &CHROMA_AC_LUT,
                    &mut writer,
                );
            }
        }

        // 8. Flush bit buffer and append EOI (End of Image)
        writer.flush_pad();
        writer.out.extend_from_slice(&[0xFF, 0xD9]);
    }

    /// Extract 8x8 Luminance (Y) with boundary clamping and level shift (-128).
    #[inline(always)]
    fn extract_luma_8x8(
        rgba: &[u8],
        stride_w: u32,
        start_x: u32,
        start_y: u32,
        max_x: u32,
        max_y: u32,
        out: &mut [i32; 64],
    ) {
        let stride = (stride_w * 4) as usize;
        let limit_x = max_x.saturating_sub(1);
        let limit_y = max_y.saturating_sub(1);

        for y in 0..8 {
            let py = (start_y + y as u32).min(limit_y) as usize;
            let row_offset = py * stride;
            let out_row = y * 8;
            for x in 0..8 {
                let px = (start_x + x as u32).min(limit_x) as usize;
                let idx = row_offset + px * 4;
                if idx + 2 < rgba.len() {
                    let r = rgba[idx] as i32;
                    let g = rgba[idx + 1] as i32;
                    let b = rgba[idx + 2] as i32;
                    // Standard ITU-R BT.601 Y formula: Y = 0.299 R + 0.587 G + 0.114 B - 128
                    // Integer fixed-point: (19595 R + 38470 G + 7471 B + 32768) >> 16 - 128
                    let y_val = ((19595 * r + 38470 * g + 7471 * b + 32768) >> 16) - 128;
                    out[out_row + x] = y_val;
                } else {
                    out[out_row + x] = 0;
                }
            }
        }
    }

    /// Extract 8x8 Chrominance (Cb, Cr) by 2x2 averaging from a 16x16 macroblock with boundary clamping.
    #[inline(always)]
    fn extract_chroma_8x8(
        rgba: &[u8],
        stride_w: u32,
        start_x: u32,
        start_y: u32,
        max_x: u32,
        max_y: u32,
        cb_out: &mut [i32; 64],
        cr_out: &mut [i32; 64],
    ) {
        let stride = (stride_w * 4) as usize;
        let limit_x = max_x.saturating_sub(1);
        let limit_y = max_y.saturating_sub(1);

        for y in 0..8 {
            let py0 = (start_y + (y * 2) as u32).min(limit_y) as usize;
            let py1 = (start_y + (y * 2 + 1) as u32).min(limit_y) as usize;
            let row0 = py0 * stride;
            let row1 = py1 * stride;
            let out_row = y * 8;

            for x in 0..8 {
                let px0 = (start_x + (x * 2) as u32).min(limit_x) as usize;
                let px1 = (start_x + (x * 2 + 1) as u32).min(limit_x) as usize;

                let i00 = row0 + px0 * 4;
                let i01 = row0 + px1 * 4;
                let i10 = row1 + px0 * 4;
                let i11 = row1 + px1 * 4;

                let (r, g, b) = if i11 + 2 < rgba.len() {
                    let r = (rgba[i00] as i32 + rgba[i01] as i32 + rgba[i10] as i32 + rgba[i11] as i32 + 2) >> 2;
                    let g = (rgba[i00 + 1] as i32 + rgba[i01 + 1] as i32 + rgba[i10 + 1] as i32 + rgba[i11 + 1] as i32 + 2) >> 2;
                    let b = (rgba[i00 + 2] as i32 + rgba[i01 + 2] as i32 + rgba[i10 + 2] as i32 + rgba[i11 + 2] as i32 + 2) >> 2;
                    (r, g, b)
                } else if i00 + 2 < rgba.len() {
                    (rgba[i00] as i32, rgba[i00 + 1] as i32, rgba[i00 + 2] as i32)
                } else {
                    (0, 0, 0)
                };

                // Standard ITU-R BT.601 Cb/Cr formulas:
                // Cb = -0.168736 R - 0.331264 G + 0.5 B
                // Cr =  0.5 R - 0.418688 G - 0.081312 B
                // Integer fixed-point:
                let cb = (-11059 * r - 21709 * g + 32768 * b + 32768) >> 16;
                let cr = ( 32768 * r - 27439 * g -  5329 * b + 32768) >> 16;

                cb_out[out_row + x] = cb;
                cr_out[out_row + x] = cr;
            }
        }
    }

    /// Appends standard Annex K DHT (Define Huffman Table) segments.
    fn write_dht(out: &mut Vec<u8>) {
        // DC Luma (id 0)
        out.extend_from_slice(&[0xFF, 0xC4, 0x00, 0x1F, 0x00]);
        out.extend_from_slice(&LUMA_DC_BITS);
        out.extend_from_slice(&LUMA_DC_VALS);

        // AC Luma (id 0)
        out.extend_from_slice(&[0xFF, 0xC4, 0x00, 0xB5, 0x10]);
        out.extend_from_slice(&LUMA_AC_BITS);
        out.extend_from_slice(&LUMA_AC_VALS);

        // DC Chroma (id 1)
        out.extend_from_slice(&[0xFF, 0xC4, 0x00, 0x1F, 0x01]);
        out.extend_from_slice(&CHROMA_DC_BITS);
        out.extend_from_slice(&CHROMA_DC_VALS);

        // AC Chroma (id 1)
        out.extend_from_slice(&[0xFF, 0xC4, 0x00, 0xB5, 0x11]);
        out.extend_from_slice(&CHROMA_AC_BITS);
        out.extend_from_slice(&CHROMA_AC_VALS);
    }
}

/// Static 1x1 white JPEG base64 payload for zero-latency capability query (`a=q,f=101`).
pub const TINY_1X1_JPEG_BASE64: &str =
    "/9j/4AAQSkZJRgABAQAAAQABAAD/2wCEAAYEBQYFBAYGBQYHBwYIChAKCgkJChQODwwQFxQYGBcUFhYaHSUfGhsjHBYWICwgIyYnKSopGR8tMC0oMCUoKSgBBwcKEygoKCgHCAoaKCgoKAoKFigoKCgoExooKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKP/AABEIAAEAAQMBIgACEQEDEQH/xAAfAAABBQEBAQEBAQAAAAAAAAAAAQIDBAUGBwgJCgv/xAC1EAACAQMDAgQDBQUEBAAAAX0BAgMABBEFEiExQQYTUWEHInEUMoGRoQgjQrHBFVLR8CQzYnKCCQoWFxgZGiUmJygpKjQ1Njc4OTpDREVGR0hJSlNUVVZXWFlaY2RlZmdoaWpzdHV2d3h5eoOEhYaHiImKkpOUlZaXmJmaoqOkpaanqKmqsrO0tba3uLm6wsPExcbHyMnK0tPU1dbX2Nna4eLj5OXm5+jp6vHy8/T19vf4+fr/xAAfAQADAQEBAQEBAQEBAAAAAAAAAQIDBAUGBwgJCgv/xAC1EQACAQIEBAMEBwUEBAABAncAAQIDEQQFITEGEkFRB2FxEyIygQgUQpGhscEJIzNS8BVictEKFiQ04SXxFxgZGiYnKCkqNTY3ODk6Q0RFRkdISUpTVFVWV1hZWmNkZWZnaGlqc3R1dnd4eXqCg4SFhoeIiYqSk5SVlpeYmZqio6Slpqeoqaqys7S1tre4ubrCw8TFxsfIycrS09TV1tfY2dri4+Tl5ufo6ery8/T19vf4+fr/2gAMAwEAAhEDEQA/APqmiiigD//Z";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jpeg_encoder_structure_and_markers() {
        let w = 64u32;
        let h = 48u32;
        let mut rgba = vec![128u8; (w * h * 4) as usize];
        for (i, p) in rgba.chunks_exact_mut(4).enumerate() {
            p[0] = (i % 256) as u8;
            p[1] = ((i * 3) % 256) as u8;
            p[2] = ((i * 7) % 256) as u8;
            p[3] = 255;
        }

        let encoder = JpegEncoder::new(80);
        let mut jpeg = Vec::new();
        encoder.encode_subimage_into(&rgba, w, 0, 0, w, h, &mut jpeg);
        assert!(!jpeg.is_empty());
        assert_eq!(jpeg[0], 0xFF);
        assert_eq!(jpeg[1], 0xD8); // SOI
        assert_eq!(jpeg[jpeg.len() - 2], 0xFF);
        assert_eq!(jpeg[jpeg.len() - 1], 0xD9); // EOI

        // Check for APP0 marker
        assert!(jpeg.windows(4).any(|w| w == [0xFF, 0xE0, 0x00, 0x10]));
        // Check for SOF0 marker
        assert!(jpeg.windows(2).any(|w| w == [0xFF, 0xC0]));
        // Check for SOS marker
        assert!(jpeg.windows(2).any(|w| w == [0xFF, 0xDA]));
    }

    #[test]
    fn test_jpeg_encoder_subimage_and_odd_dimensions() {
        let full_w = 100u32;
        let full_h = 100u32;
        let rgba = vec![200u8; (full_w * full_h * 4) as usize];

        let encoder = JpegEncoder::new(75);
        let mut jpeg = Vec::new();

        // Odd sub-rectangle: 13x17 at offset (5, 9)
        encoder.encode_subimage_into(&rgba, full_w, 5, 9, 13, 17, &mut jpeg);
        assert!(!jpeg.is_empty());
        assert_eq!(jpeg[0], 0xFF);
        assert_eq!(jpeg[1], 0xD8);
        assert_eq!(jpeg[jpeg.len() - 2], 0xFF);
        assert_eq!(jpeg[jpeg.len() - 1], 0xD9);

        // Verify dimensions in SOF0 header
        let sof0_idx = jpeg.windows(2).position(|w| w == [0xFF, 0xC0]).expect("SOF0");
        let h = ((jpeg[sof0_idx + 5] as u16) << 8) | (jpeg[sof0_idx + 6] as u16);
        let w = ((jpeg[sof0_idx + 7] as u16) << 8) | (jpeg[sof0_idx + 8] as u16);
        assert_eq!(h, 17);
        assert_eq!(w, 13);
    }

    #[test]
    fn test_color_fidelity() {
        let w = 32u32;
        let h = 32u32;
        let enc = JpegEncoder::new(90);

        // White
        let rgba = vec![255u8; (w * h * 4) as usize];
        let mut out = Vec::new();
        enc.encode_subimage_into(&rgba, w, 0, 0, w, h, &mut out);
        std::fs::write("/tmp/white.jpg", &out).ok();

        // Red: RGBA (255, 0, 0, 255)
        let mut rgba_red = vec![0u8; (w * h * 4) as usize];
        for p in rgba_red.chunks_exact_mut(4) {
            p[0] = 255;
            p[3] = 255;
        }
        out.clear();
        enc.encode_subimage_into(&rgba_red, w, 0, 0, w, h, &mut out);
        std::fs::write("/tmp/red.jpg", &out).ok();

        // Green: RGBA (0, 255, 0, 255)
        let mut rgba_green = vec![0u8; (w * h * 4) as usize];
        for p in rgba_green.chunks_exact_mut(4) {
            p[1] = 255;
            p[3] = 255;
        }
        out.clear();
        enc.encode_subimage_into(&rgba_green, w, 0, 0, w, h, &mut out);
        std::fs::write("/tmp/green.jpg", &out).ok();

        // Blue: RGBA (0, 0, 255, 255)
        let mut rgba_blue = vec![0u8; (w * h * 4) as usize];
        for p in rgba_blue.chunks_exact_mut(4) {
            p[2] = 255;
            p[3] = 255;
        }
        out.clear();
        enc.encode_subimage_into(&rgba_blue, w, 0, 0, w, h, &mut out);
        std::fs::write("/tmp/blue.jpg", &out).ok();

        // 1260x714 pattern
        let w2 = 1260u32;
        let h2 = 714u32;
        let mut rgba_large = vec![0u8; (w2 * h2 * 4) as usize];
        for y in 0..h2 {
            for x in 0..w2 {
                let idx = ((y * w2 + x) * 4) as usize;
                rgba_large[idx] = (x % 256) as u8;
                rgba_large[idx + 1] = (y % 256) as u8;
                rgba_large[idx + 2] = ((x + y) % 256) as u8;
                rgba_large[idx + 3] = 255;
            }
        }
        out.clear();
        enc.encode_subimage_into(&rgba_large, w2, 0, 0, w2, h2, &mut out);
        std::fs::write("/tmp/pattern_1260x714.jpg", &out).ok();
    }

    #[test]
    fn test_tiny_1x1_jpeg_base64_validity() {
        use base64::engine::general_purpose::STANDARD as BASE64;
        use base64::Engine;

        let bytes = BASE64.decode(TINY_1X1_JPEG_BASE64).expect("valid base64");
        assert_eq!(bytes[0], 0xFF);
        assert_eq!(bytes[1], 0xD8);
        assert_eq!(bytes[bytes.len() - 2], 0xFF);
        assert_eq!(bytes[bytes.len() - 1], 0xD9);
    }

    #[test]
    fn test_jpeg_encoder_various_qualities_and_sizes() {
        for &q in &[10, 50, 80, 95] {
            let encoder = JpegEncoder::new(q);
            assert_eq!(encoder.quality(), q);

            for &(w, h) in &[(1, 1), (8, 8), (15, 15), (16, 16), (17, 23), (128, 96)] {
                let rgba = vec![180u8; (w * h * 4) as usize];
                let mut out = Vec::new();
                encoder.encode_subimage_into(&rgba, w, 0, 0, w, h, &mut out);
                assert!(!out.is_empty());
                assert_eq!(out[0], 0xFF);
                assert_eq!(out[1], 0xD8);
                assert_eq!(out[out.len() - 2], 0xFF);
                assert_eq!(out[out.len() - 1], 0xD9);
            }
        }
    }
}
