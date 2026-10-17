/// A dirty bounding rectangle to be redrawn
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DamageRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Tile-based damage tracker for screen updates.
/// Divides the screen into small tiles (e.g. 32x32) and computes fast hashes
/// to detect which tiles have changed between consecutive frames.
pub struct DamageTracker {
    width: u32,
    height: u32,
    tile_size: u32,
    tiles_x: u32,
    tiles_y: u32,
    prev_tile_hashes: Vec<u64>,
}

impl DamageTracker {
    pub fn new(width: u32, height: u32, tile_size: u32) -> Self {
        let tile_size = tile_size.max(8);
        let tiles_x = width.div_ceil(tile_size);
        let tiles_y = height.div_ceil(tile_size);
        let num_tiles = (tiles_x * tiles_y) as usize;

        Self {
            width,
            height,
            tile_size,
            tiles_x,
            tiles_y,
            prev_tile_hashes: vec![0; num_tiles],
        }
    }

    /// Fast 64-bit block hash using 8-byte word folding (with tail handling)
    #[inline(always)]
    pub fn fnv1a_hash(data: &[u8]) -> u64 {
        Self::hash_row_words(0xcbf29ce484222325u64, data)
    }

    #[inline(always)]
    fn hash_row_words(mut hash: u64, row: &[u8]) -> u64 {
        const PRIME: u64 = 0x100000001b3u64;
        let mut chunks = row.chunks_exact(8);
        for chunk in &mut chunks {
            let word = u64::from_le_bytes(chunk.try_into().unwrap());
            hash = (hash ^ word).wrapping_mul(PRIME);
        }
        for &byte in chunks.remainder() {
            hash = (hash ^ (byte as u64)).wrapping_mul(PRIME);
        }
        hash
    }

    /// AVX2 SIMD accelerated tile hash for 32-pixel wide rows (128 bytes per row)
    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx2")]
    unsafe fn hash_tile_avx2(
        rgba_data: &[u8],
        stride: usize,
        rect_x: usize,
        rect_y: usize,
        rect_w: usize,
        rect_h: usize,
    ) -> u64 {
        use std::arch::x86_64::*;
        let mut acc0 = _mm256_set1_epi64x(0x517cc1b727220a95u64 as i64);
        let mut acc1 = _mm256_set1_epi64x(0x9e3779b97f4a7c15u64 as i64);
        let row_bytes = rect_w * 4;
        let base_ptr = rgba_data.as_ptr();

        for y in 0..rect_h {
            let row_start = (rect_y + y) * stride + rect_x * 4;
            let mut ptr = base_ptr.add(row_start);
            let mut rem = row_bytes;

            while rem >= 64 {
                let v0 = _mm256_loadu_si256(ptr as *const __m256i);
                let v1 = _mm256_loadu_si256(ptr.add(32) as *const __m256i);
                acc0 = _mm256_xor_si256(acc0, v0);
                acc0 = _mm256_add_epi64(_mm256_slli_epi64(acc0, 13), _mm256_srli_epi64(acc0, 51));
                acc0 = _mm256_add_epi64(acc0, v1);

                acc1 = _mm256_xor_si256(acc1, v1);
                acc1 = _mm256_add_epi64(_mm256_slli_epi64(acc1, 17), _mm256_srli_epi64(acc1, 47));
                acc1 = _mm256_add_epi64(acc1, v0);

                ptr = ptr.add(64);
                rem -= 64;
            }
            while rem >= 32 {
                let v0 = _mm256_loadu_si256(ptr as *const __m256i);
                acc0 = _mm256_xor_si256(acc0, v0);
                acc0 = _mm256_add_epi64(_mm256_slli_epi64(acc0, 13), _mm256_srli_epi64(acc0, 51));
                ptr = ptr.add(32);
                rem -= 32;
            }
            if rem > 0 {
                let tail_slice = std::slice::from_raw_parts(ptr, rem);
                let tail_h = Self::hash_row_words(0xcbf29ce484222325u64, tail_slice);
                acc1 = _mm256_xor_si256(acc1, _mm256_set1_epi64x(tail_h as i64));
            }
        }

        let combined = _mm256_xor_si256(acc0, acc1);
        let mut lanes = [0u64; 4];
        _mm256_storeu_si256(lanes.as_mut_ptr() as *mut __m256i, combined);
        let mut h = 0xcbf29ce484222325u64;
        for &lane in &lanes {
            h = (h ^ lane).wrapping_mul(0x100000001b3u64);
        }
        if h == 0 { 1 } else { h }
    }

    /// ARM NEON SIMD accelerated tile hash for 64-bit lane accumulation
    #[cfg(target_arch = "aarch64")]
    unsafe fn hash_tile_neon(
        rgba_data: &[u8],
        stride: usize,
        rect_x: usize,
        rect_y: usize,
        rect_w: usize,
        rect_h: usize,
    ) -> u64 {
        use std::arch::aarch64::*;
        let mut acc0 = vdupq_n_u64(0x517cc1b727220a95u64);
        let mut acc1 = vdupq_n_u64(0x9e3779b97f4a7c15u64);
        let row_bytes = rect_w * 4;
        let base_ptr = rgba_data.as_ptr();

        for y in 0..rect_h {
            let row_start = (rect_y + y) * stride + rect_x * 4;
            let mut ptr = base_ptr.add(row_start);
            let mut rem = row_bytes;

            while rem >= 64 {
                let v0 = vld1q_u64(ptr as *const u64);
                let v1 = vld1q_u64(ptr.add(16) as *const u64);
                let v2 = vld1q_u64(ptr.add(32) as *const u64);
                let v3 = vld1q_u64(ptr.add(48) as *const u64);

                acc0 = veorq_u64(acc0, v0);
                acc0 = vaddq_u64(vaddq_u64(vshlq_n_u64(acc0, 13), vshrq_n_u64(acc0, 51)), v1);

                acc1 = veorq_u64(acc1, v2);
                acc1 = vaddq_u64(vaddq_u64(vshlq_n_u64(acc1, 17), vshrq_n_u64(acc1, 47)), v3);

                ptr = ptr.add(64);
                rem -= 64;
            }
            while rem >= 32 {
                let v0 = vld1q_u64(ptr as *const u64);
                let v1 = vld1q_u64(ptr.add(16) as *const u64);
                acc0 = veorq_u64(acc0, v0);
                acc0 = vaddq_u64(vaddq_u64(vshlq_n_u64(acc0, 13), vshrq_n_u64(acc0, 51)), v1);
                ptr = ptr.add(32);
                rem -= 32;
            }
            while rem >= 16 {
                let v0 = vld1q_u64(ptr as *const u64);
                acc0 = veorq_u64(acc0, v0);
                acc0 = vaddq_u64(vshlq_n_u64(acc0, 13), vshrq_n_u64(acc0, 51));
                ptr = ptr.add(16);
                rem -= 16;
            }
            if rem > 0 {
                let tail_slice = std::slice::from_raw_parts(ptr, rem);
                let tail_h = Self::hash_row_words(0xcbf29ce484222325u64, tail_slice);
                acc1 = veorq_u64(acc1, vdupq_n_u64(tail_h));
            }
        }

        let combined = veorq_u64(acc0, acc1);
        let l0 = vgetq_lane_u64(combined, 0);
        let l1 = vgetq_lane_u64(combined, 1);
        let mut h = 0xcbf29ce484222325u64;
        h = (h ^ l0).wrapping_mul(0x100000001b3u64);
        h = (h ^ l1).wrapping_mul(0x100000001b3u64);
        if h == 0 { 1 } else { h }
    }

    /// Resize damage tracker to new resolution
    pub fn resize(&mut self, width: u32, height: u32) {
        if self.width == width && self.height == height {
            return;
        }
        *self = Self::new(width, height, self.tile_size);
    }

    /// Compare current frame buffer against previous frame and return modified bounding rectangles.
    /// Uses AVX2 SIMD vector hashing on x86_64, NEON vector hashing on aarch64, and falls back to 64-bit word folding.
    pub fn track_damage(&mut self, rgba_data: &[u8]) -> Vec<DamageRect> {
        let stride = (self.width * 4) as usize;
        let mut dirty_tiles = Vec::new();
        #[cfg(target_arch = "x86_64")]
        let has_avx2 = std::is_x86_feature_detected!("avx2");

        for ty in 0..self.tiles_y {
            for tx in 0..self.tiles_x {
                let tile_idx = (ty * self.tiles_x + tx) as usize;

                let rect_x = tx * self.tile_size;
                let rect_y = ty * self.tile_size;
                let rect_w = (self.tile_size).min(self.width.saturating_sub(rect_x));
                let rect_h = (self.tile_size).min(self.height.saturating_sub(rect_y));

                let last_row_end = ((rect_y + rect_h.saturating_sub(1)) as usize) * stride
                    + ((rect_x + rect_w) * 4) as usize;

                let current_hash = if last_row_end <= rgba_data.len() && rect_h > 0 && rect_w > 0 {
                    #[cfg(target_arch = "x86_64")]
                    {
                        if has_avx2 {
                            unsafe {
                                Self::hash_tile_avx2(
                                    rgba_data,
                                    stride,
                                    rect_x as usize,
                                    rect_y as usize,
                                    rect_w as usize,
                                    rect_h as usize,
                                )
                            }
                        } else {
                            let mut h = 0xcbf29ce484222325u64;
                            for y in 0..rect_h {
                                let cur_y = (rect_y + y) as usize;
                                let row_start = cur_y * stride + (rect_x * 4) as usize;
                                let row_end = row_start + (rect_w * 4) as usize;
                                h = Self::hash_row_words(h, &rgba_data[row_start..row_end]);
                            }
                            if h == 0 { 1 } else { h }
                        }
                    }
                    #[cfg(target_arch = "aarch64")]
                    {
                        unsafe {
                            Self::hash_tile_neon(
                                rgba_data,
                                stride,
                                rect_x as usize,
                                rect_y as usize,
                                rect_w as usize,
                                rect_h as usize,
                            )
                        }
                    }
                    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
                    {
                        let mut h = 0xcbf29ce484222325u64;
                        for y in 0..rect_h {
                            let cur_y = (rect_y + y) as usize;
                            let row_start = cur_y * stride + (rect_x * 4) as usize;
                            let row_end = row_start + (rect_w * 4) as usize;
                            h = Self::hash_row_words(h, &rgba_data[row_start..row_end]);
                        }
                        if h == 0 { 1 } else { h }
                    }
                } else {
                    0
                };

                if current_hash != self.prev_tile_hashes[tile_idx] {
                    self.prev_tile_hashes[tile_idx] = current_hash;
                    dirty_tiles.push(DamageRect {
                        x: rect_x,
                        y: rect_y,
                        width: rect_w,
                        height: rect_h,
                    });
                }
            }
        }

        Self::coalesce_rects(dirty_tiles)
    }

    /// Reset tracked state (forces all tiles to be considered dirty on next check)
    pub fn reset(&mut self) {
        self.prev_tile_hashes.fill(0);
    }

    /// Coalesce adjacent dirty tiles into larger rectangles to minimize escape sequence overhead
    fn coalesce_rects(dirty_tiles: Vec<DamageRect>) -> Vec<DamageRect> {
        if dirty_tiles.is_empty() {
            return Vec::new();
        }

        // If more than 50% of screen is dirty, coalesce into a single full update
        if dirty_tiles.len() > 16 {
            let mut min_x = u32::MAX;
            let mut min_y = u32::MAX;
            let mut max_x = 0;
            let mut max_y = 0;

            for r in &dirty_tiles {
                min_x = min_x.min(r.x);
                min_y = min_y.min(r.y);
                max_x = max_x.max(r.x + r.width);
                max_y = max_y.max(r.y + r.height);
            }

            return vec![DamageRect {
                x: min_x,
                y: min_y,
                width: max_x - min_x,
                height: max_y - min_y,
            }];
        }

        dirty_tiles
    }
}
