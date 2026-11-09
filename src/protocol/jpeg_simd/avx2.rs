//! AVX2 SIMD acceleration for Baseline JPEG (x86_64).
//!
//! Provides fused RGBA-to-YCbCr MCU extraction, 2D integer FDCT, and quantization
//! with strict bit-exact equivalence (INV-4) to scalar reference algorithms.

#![cfg(target_arch = "x86_64")]

use std::arch::x86_64::*;

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

/// In-place 8x8 transposition of 32-bit integer matrix.
#[target_feature(enable = "avx2")]
pub unsafe fn transpose_8x8_epi32(r: &mut [__m256i; 8]) {
    let t0 = _mm256_unpacklo_epi32(r[0], r[1]);
    let t1 = _mm256_unpackhi_epi32(r[0], r[1]);
    let t2 = _mm256_unpacklo_epi32(r[2], r[3]);
    let t3 = _mm256_unpackhi_epi32(r[2], r[3]);
    let t4 = _mm256_unpacklo_epi32(r[4], r[5]);
    let t5 = _mm256_unpackhi_epi32(r[4], r[5]);
    let t6 = _mm256_unpacklo_epi32(r[6], r[7]);
    let t7 = _mm256_unpackhi_epi32(r[6], r[7]);

    let u0 = _mm256_unpacklo_epi64(t0, t2);
    let u1 = _mm256_unpackhi_epi64(t0, t2);
    let u2 = _mm256_unpacklo_epi64(t1, t3);
    let u3 = _mm256_unpackhi_epi64(t1, t3);
    let u4 = _mm256_unpacklo_epi64(t4, t6);
    let u5 = _mm256_unpackhi_epi64(t4, t6);
    let u6 = _mm256_unpacklo_epi64(t5, t7);
    let u7 = _mm256_unpackhi_epi64(t5, t7);

    r[0] = _mm256_permute2x128_si256(u0, u4, 0x20);
    r[1] = _mm256_permute2x128_si256(u1, u5, 0x20);
    r[2] = _mm256_permute2x128_si256(u2, u6, 0x20);
    r[3] = _mm256_permute2x128_si256(u3, u7, 0x20);
    r[4] = _mm256_permute2x128_si256(u0, u4, 0x31);
    r[5] = _mm256_permute2x128_si256(u1, u5, 0x31);
    r[6] = _mm256_permute2x128_si256(u2, u6, 0x31);
    r[7] = _mm256_permute2x128_si256(u3, u7, 0x31);
}

/// 8-lane parallel 1D 8-point Forward DCT (Loeffler/LLM) in AVX2.
/// Matches scalar `fdct_1d` bit-for-bit.
#[target_feature(enable = "avx2")]
unsafe fn fdct_1d_lanes(r: &mut [__m256i; 8]) {
    const FIX_0_298631375: i32 = 2446;
    const FIX_0_390180644: i32 = 3196;
    const FIX_0_541196100: i32 = 4433;
    const FIX_0_765366865: i32 = 6270;
    const FIX_0_899976223: i32 = 7373;
    const FIX_1_175875602: i32 = 9633;
    const FIX_1_501321110: i32 = 12299;
    const FIX_1_847759065: i32 = 15137;
    const FIX_1_961570560: i32 = 16069;
    const FIX_2_053119869: i32 = 16819;
    const FIX_2_562915447: i32 = 20995;
    const FIX_3_072711026: i32 = 25172;

    let c1024 = _mm256_set1_epi32(1 << 10);

    let p0 = r[0];
    let p1 = r[1];
    let p2 = r[2];
    let p3 = r[3];
    let p4 = r[4];
    let p5 = r[5];
    let p6 = r[6];
    let p7 = r[7];

    let tmp0 = _mm256_add_epi32(p0, p7);
    let tmp7 = _mm256_sub_epi32(p0, p7);
    let tmp1 = _mm256_add_epi32(p1, p6);
    let tmp6 = _mm256_sub_epi32(p1, p6);
    let tmp2 = _mm256_add_epi32(p2, p5);
    let tmp5 = _mm256_sub_epi32(p2, p5);
    let tmp3 = _mm256_add_epi32(p3, p4);
    let tmp4 = _mm256_sub_epi32(p3, p4);

    // Even part
    let tmp10 = _mm256_add_epi32(tmp0, tmp3);
    let tmp13 = _mm256_sub_epi32(tmp0, tmp3);
    let tmp11 = _mm256_add_epi32(tmp1, tmp2);
    let tmp12 = _mm256_sub_epi32(tmp1, tmp2);

    r[0] = _mm256_slli_epi32(_mm256_add_epi32(tmp10, tmp11), 2);
    r[4] = _mm256_slli_epi32(_mm256_sub_epi32(tmp10, tmp11), 2);

    let z1 = _mm256_mullo_epi32(_mm256_add_epi32(tmp12, tmp13), _mm256_set1_epi32(FIX_0_541196100));
    let term_p2 = _mm256_add_epi32(_mm256_add_epi32(z1, _mm256_mullo_epi32(tmp13, _mm256_set1_epi32(FIX_0_765366865))), c1024);
    r[2] = _mm256_srai_epi32(term_p2, 11);

    let term_p6 = _mm256_add_epi32(_mm256_sub_epi32(z1, _mm256_mullo_epi32(tmp12, _mm256_set1_epi32(FIX_1_847759065))), c1024);
    r[6] = _mm256_srai_epi32(term_p6, 11);

    // Odd part
    let oz1 = _mm256_add_epi32(tmp4, tmp7);
    let oz2 = _mm256_add_epi32(tmp5, tmp6);
    let oz3 = _mm256_add_epi32(tmp4, tmp6);
    let oz4 = _mm256_add_epi32(tmp5, tmp7);
    let oz5 = _mm256_mullo_epi32(_mm256_add_epi32(oz3, oz4), _mm256_set1_epi32(FIX_1_175875602));

    let otmp4 = _mm256_mullo_epi32(tmp4, _mm256_set1_epi32(FIX_0_298631375));
    let otmp5 = _mm256_mullo_epi32(tmp5, _mm256_set1_epi32(FIX_2_053119869));
    let otmp6 = _mm256_mullo_epi32(tmp6, _mm256_set1_epi32(FIX_3_072711026));
    let otmp7 = _mm256_mullo_epi32(tmp7, _mm256_set1_epi32(FIX_1_501321110));

    let oz1 = _mm256_mullo_epi32(oz1, _mm256_set1_epi32(-FIX_0_899976223));
    let oz2 = _mm256_mullo_epi32(oz2, _mm256_set1_epi32(-FIX_2_562915447));
    let oz3 = _mm256_add_epi32(_mm256_mullo_epi32(oz3, _mm256_set1_epi32(-FIX_1_961570560)), oz5);
    let oz4 = _mm256_add_epi32(_mm256_mullo_epi32(oz4, _mm256_set1_epi32(-FIX_0_390180644)), oz5);

    let term_p7 = _mm256_add_epi32(_mm256_add_epi32(_mm256_add_epi32(otmp4, oz1), oz3), c1024);
    r[7] = _mm256_srai_epi32(term_p7, 11);

    let term_p5 = _mm256_add_epi32(_mm256_add_epi32(_mm256_add_epi32(otmp5, oz2), oz4), c1024);
    r[5] = _mm256_srai_epi32(term_p5, 11);

    let term_p3 = _mm256_add_epi32(_mm256_add_epi32(_mm256_add_epi32(otmp6, oz2), oz3), c1024);
    r[3] = _mm256_srai_epi32(term_p3, 11);

    let term_p1 = _mm256_add_epi32(_mm256_add_epi32(_mm256_add_epi32(otmp7, oz1), oz4), c1024);
    r[1] = _mm256_srai_epi32(term_p1, 11);
}

/// 2D 8x8 Forward DCT in AVX2 (Rows first, then Columns).
/// Operates on 8 x `__m256i` representing rows of an 8x8 block.
#[target_feature(enable = "avx2")]
pub unsafe fn fdct_2d_avx2(r: &mut [__m256i; 8]) {
    // 1. Transpose rows into registers to process rows in parallel
    transpose_8x8_epi32(r);
    fdct_1d_lanes(r);
    // 2. Transpose intermediate coefficients into registers to process columns
    transpose_8x8_epi32(r);
    fdct_1d_lanes(r);
}

/// Quantizes and zig-zag reorders DCT coefficients in AVX2.
/// Returns the index of the last non-zero AC coefficient in zig-zag order.
#[target_feature(enable = "avx2")]
pub unsafe fn quantize_and_zigzag_avx2(
    r: &[__m256i; 8],
    mult_r: &[u32; 64],
    half_r: &[i32; 64],
    out: &mut [i16; 64],
) -> u8 {
    let mut quant_raster = [0i32; 64];

    for row in 0..8 {
        let v = r[row];
        let mult = _mm256_loadu_si256(mult_r.as_ptr().add(row * 8) as *const __m256i);
        let half = _mm256_loadu_si256(half_r.as_ptr().add(row * 8) as *const __m256i);

        let abs_v = _mm256_abs_epi32(v);
        let a = _mm256_add_epi32(abs_v, half);

        // 64-bit product for even lanes (0, 2, 4, 6)
        let prod_even = _mm256_mul_epu32(a, mult);
        let q_even = _mm256_srli_epi64(prod_even, 33);

        // 64-bit product for odd lanes (1, 3, 5, 7)
        let a_odd = _mm256_srli_epi64(a, 32);
        let mult_odd = _mm256_srli_epi64(mult, 32);
        let prod_odd = _mm256_mul_epu32(a_odd, mult_odd);
        let q_odd = _mm256_srli_epi64(prod_odd, 33);
        let q_odd_shifted = _mm256_slli_epi64(q_odd, 32);

        let q_mag = _mm256_or_si256(q_even, q_odd_shifted);
        let signed_q = _mm256_sign_epi32(q_mag, v);

        _mm256_storeu_si256(quant_raster.as_mut_ptr().add(row * 8) as *mut __m256i, signed_q);
    }

    let mut last_nonzero = 0u8;
    for k in 0..64 {
        let q = quant_raster[ZIGZAG[k]].clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        out[k] = q;
        if k > 0 && q != 0 {
            last_nonzero = k as u8;
        }
    }
    last_nonzero
}

/// Fused MCU extraction in AVX2.
/// Reads 16x16 RGBA pixels with a single memory pass and generates
/// all 4 Y blocks (Y0, Y1, Y2, Y3) and both Cb and Cr blocks.
#[target_feature(enable = "avx2")]
pub unsafe fn extract_mcu_avx2(
    rgba: &[u8],
    stride_bytes: usize,
    base_offset: usize,
    y_out: &mut [[__m256i; 8]; 4],
    cb_out: &mut [__m256i; 8],
    cr_out: &mut [__m256i; 8],
) {
    let mask_ff = _mm256_set1_epi32(0xFF);
    let c19595 = _mm256_set1_epi32(19595);
    let c38470 = _mm256_set1_epi32(38470);
    let c7471 = _mm256_set1_epi32(7471);
    let c32768 = _mm256_set1_epi32(32768);
    let c128 = _mm256_set1_epi32(128);
    let c2 = _mm256_set1_epi32(2);

    let cb_r_coef = _mm256_set1_epi32(-11058);
    let cb_g_coef = _mm256_set1_epi32(-21710);
    let cr_r_coef = _mm256_set1_epi32(32768);
    let cr_g_coef = _mm256_set1_epi32(-27439);
    let cr_b_coef = _mm256_set1_epi32(-5329);

    let mut prev_r_l = _mm256_setzero_si256();
    let mut prev_g_l = _mm256_setzero_si256();
    let mut prev_b_l = _mm256_setzero_si256();
    let mut prev_r_r = _mm256_setzero_si256();
    let mut prev_g_r = _mm256_setzero_si256();
    let mut prev_b_r = _mm256_setzero_si256();

    for y in 0..16 {
        let row_offset = base_offset + y * stride_bytes;
        let ptr_l = rgba.as_ptr().add(row_offset);
        let ptr_r = ptr_l.add(32);

        let v_l = _mm256_loadu_si256(ptr_l as *const __m256i);
        let v_r = _mm256_loadu_si256(ptr_r as *const __m256i);

        // Extract R, G, B for left 8 pixels
        let r_l = _mm256_and_si256(v_l, mask_ff);
        let g_l = _mm256_and_si256(_mm256_srli_epi32(v_l, 8), mask_ff);
        let b_l = _mm256_and_si256(_mm256_srli_epi32(v_l, 16), mask_ff);

        // Compute Y for left 8 pixels
        let y_l_sum = _mm256_add_epi32(
            _mm256_add_epi32(
                _mm256_mullo_epi32(r_l, c19595),
                _mm256_mullo_epi32(g_l, c38470),
            ),
            _mm256_add_epi32(
                _mm256_mullo_epi32(b_l, c7471),
                c32768,
            ),
        );
        let y_l = _mm256_sub_epi32(_mm256_srai_epi32(y_l_sum, 16), c128);

        // Extract R, G, B for right 8 pixels
        let r_r = _mm256_and_si256(v_r, mask_ff);
        let g_r = _mm256_and_si256(_mm256_srli_epi32(v_r, 8), mask_ff);
        let b_r = _mm256_and_si256(_mm256_srli_epi32(v_r, 16), mask_ff);

        // Compute Y for right 8 pixels
        let y_r_sum = _mm256_add_epi32(
            _mm256_add_epi32(
                _mm256_mullo_epi32(r_r, c19595),
                _mm256_mullo_epi32(g_r, c38470),
            ),
            _mm256_add_epi32(
                _mm256_mullo_epi32(b_r, c7471),
                c32768,
            ),
        );
        let y_r = _mm256_sub_epi32(_mm256_srai_epi32(y_r_sum, 16), c128);

        if y < 8 {
            y_out[0][y] = y_l;
            y_out[1][y] = y_r;
        } else {
            y_out[2][y - 8] = y_l;
            y_out[3][y - 8] = y_r;
        }

        // Chroma subsampling (4:2:0)
        if (y & 1) == 0 {
            prev_r_l = r_l;
            prev_g_l = g_l;
            prev_b_l = b_l;
            prev_r_r = r_r;
            prev_g_r = g_r;
            prev_b_r = b_r;
        } else {
            let cy = y >> 1;

            // Vertical sums (2 rows)
            let r_v_l = _mm256_add_epi32(prev_r_l, r_l);
            let r_v_r = _mm256_add_epi32(prev_r_r, r_r);
            let g_v_l = _mm256_add_epi32(prev_g_l, g_l);
            let g_v_r = _mm256_add_epi32(prev_g_r, g_r);
            let b_v_l = _mm256_add_epi32(prev_b_l, b_l);
            let b_v_r = _mm256_add_epi32(prev_b_r, b_r);

            // Horizontal pairwise sum (across 16 pixels -> 8 values)
            let r_sum = _mm256_permute4x64_epi64(_mm256_hadd_epi32(r_v_l, r_v_r), 0xD8);
            let g_sum = _mm256_permute4x64_epi64(_mm256_hadd_epi32(g_v_l, g_v_r), 0xD8);
            let b_sum = _mm256_permute4x64_epi64(_mm256_hadd_epi32(b_v_l, b_v_r), 0xD8);

            // Averaging: (sum + 2) >> 2
            let r_avg = _mm256_srai_epi32(_mm256_add_epi32(r_sum, c2), 2);
            let g_avg = _mm256_srai_epi32(_mm256_add_epi32(g_sum, c2), 2);
            let b_avg = _mm256_srai_epi32(_mm256_add_epi32(b_sum, c2), 2);

            // Cb = (-11058 * r - 21710 * g + 32768 * b + 32768) >> 16
            let cb_term = _mm256_add_epi32(
                _mm256_add_epi32(
                    _mm256_mullo_epi32(r_avg, cb_r_coef),
                    _mm256_mullo_epi32(g_avg, cb_g_coef),
                ),
                _mm256_add_epi32(
                    _mm256_mullo_epi32(b_avg, c32768),
                    c32768,
                ),
            );
            cb_out[cy] = _mm256_srai_epi32(cb_term, 16);

            // Cr = ( 32768 * r - 27439 * g -  5329 * b + 32768) >> 16
            let cr_term = _mm256_add_epi32(
                _mm256_add_epi32(
                    _mm256_mullo_epi32(r_avg, cr_r_coef),
                    _mm256_mullo_epi32(g_avg, cr_g_coef),
                ),
                _mm256_add_epi32(
                    _mm256_mullo_epi32(b_avg, cr_b_coef),
                    c32768,
                ),
            );
            cr_out[cy] = _mm256_srai_epi32(cr_term, 16);
        }
    }
}
