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
        let tiles_x = (width + tile_size - 1) / tile_size;
        let tiles_y = (height + tile_size - 1) / tile_size;
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

    /// Fast FNV-1a 64-bit hash
    #[inline(always)]
    fn fnv1a_hash(data: &[u8]) -> u64 {
        let mut hash = 0xcbf29ce484222325u64;
        for &byte in data {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x100000001b3u64);
        }
        hash
    }

    /// Compare current frame buffer against previous frame and return modified bounding rectangles.
    pub fn track_damage(&mut self, rgba_data: &[u8]) -> Vec<DamageRect> {
        let stride = (self.width * 4) as usize;
        let mut dirty_tiles = Vec::new();

        for ty in 0..self.tiles_y {
            for tx in 0..self.tiles_x {
                let tile_idx = (ty * self.tiles_x + tx) as usize;

                let rect_x = tx * self.tile_size;
                let rect_y = ty * self.tile_size;
                let rect_w = (self.tile_size).min(self.width.saturating_sub(rect_x));
                let rect_h = (self.tile_size).min(self.height.saturating_sub(rect_y));

                // Compute hash of this tile
                let mut tile_bytes = Vec::with_capacity((rect_w * rect_h * 4) as usize);
                for y in 0..rect_h {
                    let cur_y = (rect_y + y) as usize;
                    let row_start = cur_y * stride + (rect_x * 4) as usize;
                    let row_end = row_start + (rect_w * 4) as usize;
                    if row_end <= rgba_data.len() {
                        tile_bytes.extend_from_slice(&rgba_data[row_start..row_end]);
                    }
                }

                let current_hash = Self::fnv1a_hash(&tile_bytes);
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
