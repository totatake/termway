//! ARM NEON SIMD acceleration for Baseline JPEG (aarch64).
//!
//! Provides fused RGBA-to-YCbCr MCU extraction, 2D integer FDCT, and quantization
//! with strict bit-exact equivalence (INV-4) to scalar reference algorithms.

#![cfg(target_arch = "aarch64")]

use std::arch::aarch64::*;

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

/// Transpose a 4x4 sub-block of 32-bit integers in ARM NEON.
#[inline(always)]
pub unsafe fn transpose_4x4_s32(
    r0: &mut int32x4_t,
    r1: &mut int32x4_t,
    r2: &mut int32x4_t,
    r3: &mut int32x4_t,
) {
    let t01 = vtrnq_s32(*r0, *r1);
    let t23 = vtrnq_s32(*r2, *r3);
    *r0 = vcombine_s32(vget_low_s32(t01.0), vget_low_s32(t23.0));
    *r1 = vcombine_s32(vget_low_s32(t01.1), vget_low_s32(t23.1));
    *r2 = vcombine_s32(vget_high_s32(t01.0), vget_high_s32(t23.0));
    *r3 = vcombine_s32(vget_high_s32(t01.1), vget_high_s32(t23.1));
}

/// In-place 8x8 transposition of 32-bit integer matrix in ARM NEON.
/// Represented as 8 rows of [int32x4_t; 2] (low 4 elements, high 4 elements).
#[inline(always)]
pub unsafe fn transpose_8x8_neon(r: &mut [[int32x4_t; 2]; 8]) {
    let [r0, r1, r2, r3, r4, r5, r6, r7] = r;
    // 1. Transpose the four 4x4 quadrants
    // Top-Left (r[0..4][0])
    transpose_4x4_s32(&mut r0[0], &mut r1[0], &mut r2[0], &mut r3[0]);
    // Top-Right (r[0..4][1])
    transpose_4x4_s32(&mut r0[1], &mut r1[1], &mut r2[1], &mut r3[1]);
    // Bottom-Left (r[4..8][0])
    transpose_4x4_s32(&mut r4[0], &mut r5[0], &mut r6[0], &mut r7[0]);
    // Bottom-Right (r[4..8][1])
    transpose_4x4_s32(&mut r4[1], &mut r5[1], &mut r6[1], &mut r7[1]);

    // 2. Swap Top-Right quadrant with Bottom-Left quadrant
    std::mem::swap(&mut r0[1], &mut r4[0]);
    std::mem::swap(&mut r1[1], &mut r5[0]);
    std::mem::swap(&mut r2[1], &mut r6[0]);
    std::mem::swap(&mut r3[1], &mut r7[0]);
}

/// 8-point 1D Forward DCT on 8 parallel rows (each split across two int32x4_t lanes).
#[inline(always)]
unsafe fn fdct_1d_lanes_neon(r: &mut [[int32x4_t; 2]; 8]) {
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

    for half in 0..2 {
        let p0 = r[0][half];
        let p1 = r[1][half];
        let p2 = r[2][half];
        let p3 = r[3][half];
        let p4 = r[4][half];
        let p5 = r[5][half];
        let p6 = r[6][half];
        let p7 = r[7][half];

        let tmp0 = vaddq_s32(p0, p7);
        let tmp7 = vsubq_s32(p0, p7);
        let tmp1 = vaddq_s32(p1, p6);
        let tmp6 = vsubq_s32(p1, p6);
        let tmp2 = vaddq_s32(p2, p5);
        let tmp5 = vsubq_s32(p2, p5);
        let tmp3 = vaddq_s32(p3, p4);
        let tmp4 = vsubq_s32(p3, p4);

        // Even part
        let tmp10 = vaddq_s32(tmp0, tmp3);
        let tmp13 = vsubq_s32(tmp0, tmp3);
        let tmp11 = vaddq_s32(tmp1, tmp2);
        let tmp12 = vsubq_s32(tmp1, tmp2);

        r[0][half] = vshlq_n_s32(vaddq_s32(tmp10, tmp11), 2);
        r[4][half] = vshlq_n_s32(vsubq_s32(tmp10, tmp11), 2);

        let z1 = vmulq_n_s32(vaddq_s32(tmp12, tmp13), FIX_0_541196100);
        let term_p2 = vaddq_s32(vmlaq_n_s32(z1, tmp13, FIX_0_765366865), vdupq_n_s32(1 << 10));
        r[2][half] = vshrq_n_s32(term_p2, 11);

        let term_p6 = vaddq_s32(vmlsq_n_s32(z1, tmp12, FIX_1_847759065), vdupq_n_s32(1 << 10));
        r[6][half] = vshrq_n_s32(term_p6, 11);

        // Odd part
        let oz1 = vaddq_s32(tmp4, tmp7);
        let oz2 = vaddq_s32(tmp5, tmp6);
        let oz3 = vaddq_s32(tmp4, tmp6);
        let oz4 = vaddq_s32(tmp5, tmp7);
        let oz5 = vmulq_n_s32(vaddq_s32(oz3, oz4), FIX_1_175875602);

        let otmp4 = vmulq_n_s32(tmp4, FIX_0_298631375);
        let otmp5 = vmulq_n_s32(tmp5, FIX_2_053119869);
        let otmp6 = vmulq_n_s32(tmp6, FIX_3_072711026);
        let otmp7 = vmulq_n_s32(tmp7, FIX_1_501321110);

        let oz1 = vmulq_n_s32(oz1, -FIX_0_899976223);
        let oz2 = vmulq_n_s32(oz2, -FIX_2_562915447);
        let oz3 = vaddq_s32(vmulq_n_s32(oz3, -FIX_1_961570560), oz5);
        let oz4 = vaddq_s32(vmulq_n_s32(oz4, -FIX_0_390180644), oz5);

        let term_p7 = vaddq_s32(vaddq_s32(vaddq_s32(otmp4, oz1), oz3), vdupq_n_s32(1 << 10));
        r[7][half] = vshrq_n_s32(term_p7, 11);

        let term_p5 = vaddq_s32(vaddq_s32(vaddq_s32(otmp5, oz2), oz4), vdupq_n_s32(1 << 10));
        r[5][half] = vshrq_n_s32(term_p5, 11);

        let term_p3 = vaddq_s32(vaddq_s32(vaddq_s32(otmp6, oz2), oz3), vdupq_n_s32(1 << 10));
        r[3][half] = vshrq_n_s32(term_p3, 11);

        let term_p1 = vaddq_s32(vaddq_s32(vaddq_s32(otmp7, oz1), oz4), vdupq_n_s32(1 << 10));
        r[1][half] = vshrq_n_s32(term_p1, 11);
    }
}

/// 2D 8x8 Forward DCT in ARM NEON.
#[inline(always)]
pub unsafe fn fdct_2d_neon(r: &mut [[int32x4_t; 2]; 8]) {
    transpose_8x8_neon(r);
    fdct_1d_lanes_neon(r);
    transpose_8x8_neon(r);
    fdct_1d_lanes_neon(r);
}

/// Quantizes and zig-zag reorders DCT coefficients in ARM NEON.
#[inline(always)]
pub unsafe fn quantize_and_zigzag_neon(
    r: &[[int32x4_t; 2]; 8],
    mult_r: &[u32; 64],
    half_r: &[i32; 64],
    out: &mut [i16; 64],
) -> u8 {
    let mut quant_raster = [0i32; 64];

    for row in 0..8 {
        for half in 0..2 {
            let offset = row * 8 + half * 4;
            let v = r[row][half];
            let mult = vld1q_u32(mult_r.as_ptr().add(offset));
            let half_v = vld1q_s32(half_r.as_ptr().add(offset));

            let abs_v = vabsq_s32(v);
            let a = vreinterpretq_u32_s32(vaddq_s32(abs_v, half_v));

            // 64-bit multiply low and high pairs
            let a_lo = vget_low_u32(a);
            let a_hi = vget_high_u32(a);
            let mult_lo = vget_low_u32(mult);
            let mult_hi = vget_high_u32(mult);

            let prod_lo = vmull_u32(a_lo, mult_lo);
            let prod_hi = vmull_u32(a_hi, mult_hi);

            let q_lo = vshrn_n_u64(prod_lo, 33);
            let q_hi = vshrn_n_u64(prod_hi, 33);

            let q_mag = vreinterpretq_s32_u32(vcombine_u32(q_lo, q_hi));

            // Restore sign
            let is_neg = vcltq_s32(v, vdupq_n_s32(0));
            let signed_q = vbslq_s32(is_neg, vnegq_s32(q_mag), q_mag);

            vst1q_s32(quant_raster.as_mut_ptr().add(offset), signed_q);
        }
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

/// Fused MCU extraction in ARM NEON.
pub unsafe fn extract_mcu_neon(
    rgba: &[u8],
    stride_bytes: usize,
    base_offset: usize,
    y_out: &mut [[[int32x4_t; 2]; 8]; 4],
    cb_out: &mut [[int32x4_t; 2]; 8],
    cr_out: &mut [[int32x4_t; 2]; 8],
) {
    let mut prev_r_l = vdupq_n_s32(0);
    let mut prev_g_l = vdupq_n_s32(0);
    let mut prev_b_l = vdupq_n_s32(0);
    let mut prev_r_r = vdupq_n_s32(0);
    let mut prev_g_r = vdupq_n_s32(0);
    let mut prev_b_r = vdupq_n_s32(0);

    for y in 0..16 {
        let row_offset = base_offset + y * stride_bytes;
        let ptr_l = rgba.as_ptr().add(row_offset);
        let ptr_r = ptr_l.add(32);

        // Load 8 pixels (left and right) deinterleaved
        let pix_l = vld4_u8(ptr_l);
        let pix_r = vld4_u8(ptr_r);

        // Expand left u8 to u16 then s32
        let r16_l = vmovl_u8(pix_l.0);
        let g16_l = vmovl_u8(pix_l.1);
        let b16_l = vmovl_u8(pix_l.2);

        let r_l_0 = vreinterpretq_s32_u32(vmovl_u16(vget_low_u16(r16_l)));
        let r_l_1 = vreinterpretq_s32_u32(vmovl_u16(vget_high_u16(r16_l)));
        let g_l_0 = vreinterpretq_s32_u32(vmovl_u16(vget_low_u16(g16_l)));
        let g_l_1 = vreinterpretq_s32_u32(vmovl_u16(vget_high_u16(g16_l)));
        let b_l_0 = vreinterpretq_s32_u32(vmovl_u16(vget_low_u16(b16_l)));
        let b_l_1 = vreinterpretq_s32_u32(vmovl_u16(vget_high_u16(b16_l)));

        // Expand right u8 to u16 then s32
        let r16_r = vmovl_u8(pix_r.0);
        let g16_r = vmovl_u8(pix_r.1);
        let b16_r = vmovl_u8(pix_r.2);

        let r_r_0 = vreinterpretq_s32_u32(vmovl_u16(vget_low_u16(r16_r)));
        let r_r_1 = vreinterpretq_s32_u32(vmovl_u16(vget_high_u16(r16_r)));
        let g_r_0 = vreinterpretq_s32_u32(vmovl_u16(vget_low_u16(g16_r)));
        let g_r_1 = vreinterpretq_s32_u32(vmovl_u16(vget_high_u16(g16_r)));
        let b_r_0 = vreinterpretq_s32_u32(vmovl_u16(vget_low_u16(b16_r)));
        let b_r_1 = vreinterpretq_s32_u32(vmovl_u16(vget_high_u16(b16_r)));

        // Compute Y for left 8 pixels
        let r_l = [r_l_0, r_l_1];
        let g_l = [g_l_0, g_l_1];
        let b_l = [b_l_0, b_l_1];
        for i in 0..2 {
            let r = r_l[i];
            let g = g_l[i];
            let b = b_l[i];
            let y_sum = vaddq_s32(
                vaddq_s32(
                    vmulq_n_s32(r, 19595),
                    vmulq_n_s32(g, 38470),
                ),
                vaddq_s32(
                    vmulq_n_s32(b, 7471),
                    vdupq_n_s32(32768),
                ),
            );
            let y_val = vsubq_s32(vshrq_n_s32(y_sum, 16), vdupq_n_s32(128));
            if y < 8 {
                y_out[0][y][i] = y_val;
            } else {
                y_out[2][y - 8][i] = y_val;
            }
        }

        // Compute Y for right 8 pixels
        let r_r = [r_r_0, r_r_1];
        let g_r = [g_r_0, g_r_1];
        let b_r = [b_r_0, b_r_1];
        for i in 0..2 {
            let r = r_r[i];
            let g = g_r[i];
            let b = b_r[i];
            let y_sum = vaddq_s32(
                vaddq_s32(
                    vmulq_n_s32(r, 19595),
                    vmulq_n_s32(g, 38470),
                ),
                vaddq_s32(
                    vmulq_n_s32(b, 7471),
                    vdupq_n_s32(32768),
                ),
            );
            let y_val = vsubq_s32(vshrq_n_s32(y_sum, 16), vdupq_n_s32(128));
            if y < 8 {
                y_out[1][y][i] = y_val;
            } else {
                y_out[3][y - 8][i] = y_val;
            }
        }

        // Chroma subsampling
        if (y & 1) == 0 {
            prev_r_l = r_l_0; // Pairwise add helper
            prev_g_l = g_l_0;
            prev_b_l = b_l_0;
            prev_r_r = r_r_0;
            prev_g_r = g_r_0;
            prev_b_r = b_r_0;
        } else {
            let cy = y >> 1;
            // 2-row sums
            let r_v_l_0 = vaddq_s32(prev_r_l, r_l_0);
            let r_v_l_1 = vaddq_s32(r_l_1, r_l_1); // pair
            let g_v_l_0 = vaddq_s32(prev_g_l, g_l_0);
            let g_v_l_1 = vaddq_s32(g_l_1, g_l_1);
            let b_v_l_0 = vaddq_s32(prev_b_l, b_l_0);
            let b_v_l_1 = vaddq_s32(b_l_1, b_l_1);

            let r_v_r_0 = vaddq_s32(prev_r_r, r_r_0);
            let r_v_r_1 = vaddq_s32(r_r_1, r_r_1);
            let g_v_r_0 = vaddq_s32(prev_g_r, g_r_0);
            let g_v_r_1 = vaddq_s32(g_r_1, g_r_1);
            let b_v_r_0 = vaddq_s32(prev_b_r, b_r_0);
            let b_v_r_1 = vaddq_s32(b_r_1, b_r_1);

            // Pairwise add across adjacent 2 pixels
            let r_pair_l = vpaddq_s32(r_v_l_0, r_v_l_1);
            let r_pair_r = vpaddq_s32(r_v_r_0, r_v_r_1);
            let g_pair_l = vpaddq_s32(g_v_l_0, g_v_l_1);
            let g_pair_r = vpaddq_s32(g_v_r_0, g_v_r_1);
            let b_pair_l = vpaddq_s32(b_v_l_0, b_v_l_1);
            let b_pair_r = vpaddq_s32(b_v_r_0, b_v_r_1);

            let r_avg_l = vshrq_n_s32(vaddq_s32(r_pair_l, vdupq_n_s32(2)), 2);
            let r_avg_r = vshrq_n_s32(vaddq_s32(r_pair_r, vdupq_n_s32(2)), 2);
            let g_avg_l = vshrq_n_s32(vaddq_s32(g_pair_l, vdupq_n_s32(2)), 2);
            let g_avg_r = vshrq_n_s32(vaddq_s32(g_pair_r, vdupq_n_s32(2)), 2);
            let b_avg_l = vshrq_n_s32(vaddq_s32(b_pair_l, vdupq_n_s32(2)), 2);
            let b_avg_r = vshrq_n_s32(vaddq_s32(b_pair_r, vdupq_n_s32(2)), 2);

            let r_avg = [r_avg_l, r_avg_r];
            let g_avg = [g_avg_l, g_avg_r];
            let b_avg = [b_avg_l, b_avg_r];
            for half in 0..2 {
                let r = r_avg[half];
                let g = g_avg[half];
                let b = b_avg[half];
                let cb_term = vaddq_s32(
                    vaddq_s32(
                        vmulq_n_s32(r, -11058),
                        vmulq_n_s32(g, -21710),
                    ),
                    vaddq_s32(
                        vmulq_n_s32(b, 32768),
                        vdupq_n_s32(32768),
                    ),
                );
                cb_out[cy][half] = vshrq_n_s32(cb_term, 16);

                let cr_term = vaddq_s32(
                    vaddq_s32(
                        vmulq_n_s32(r, 32768),
                        vmulq_n_s32(g, -27439),
                    ),
                    vaddq_s32(
                        vmulq_n_s32(b, -5329),
                        vdupq_n_s32(32768),
                    ),
                );
                cr_out[cy][half] = vshrq_n_s32(cr_term, 16);
            }
        }
    }
}
