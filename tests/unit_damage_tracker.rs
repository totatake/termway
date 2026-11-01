use termway::display::DamageTracker;

#[test]
fn test_damage_tracker_initial_frame() {
    let mut tracker = DamageTracker::new(64, 64, 32); // 2x2 = 4 tiles
    let mut frame = vec![0u8; 64 * 64 * 4];

    // First frame: all tiles are dirty compared to initial 0-hash
    frame[0] = 255;
    let damage = tracker.track_damage(&frame);
    assert!(!damage.is_empty());
}

#[test]
fn test_damage_tracker_no_change() {
    let mut tracker = DamageTracker::new(64, 64, 32);
    let mut frame = vec![0u8; 64 * 64 * 4];
    frame[0] = 100;

    // First check
    let _ = tracker.track_damage(&frame);

    // Second check with unchanged data
    let damage2 = tracker.track_damage(&frame);
    assert!(damage2.is_empty(), "Should report no damage when pixels are identical");
}

#[test]
fn test_damage_tracker_partial_change() {
    let mut tracker = DamageTracker::new(64, 64, 32); // 4 tiles (32x32 each)
    let mut frame = vec![0u8; 64 * 64 * 4];
    let _ = tracker.track_damage(&frame);

    // Modify a pixel in bottom-right tile (tile at x=32, y=32)
    let idx = (32 * 64 + 32) * 4;
    frame[idx] = 200;

    let damage = tracker.track_damage(&frame);
    assert_eq!(damage.len(), 1, "Should report exactly 1 modified tile");
    assert_eq!(damage[0].x, 32);
    assert_eq!(damage[0].y, 32);
    assert_eq!(damage[0].width, 32);
    assert_eq!(damage[0].height, 32);
}

#[test]
fn test_damage_tracker_reset() {
    let mut tracker = DamageTracker::new(64, 64, 32);
    let mut frame = vec![0u8; 64 * 64 * 4];
    frame[0] = 50;
    let _ = tracker.track_damage(&frame);

    // Identical frame produces no damage
    assert!(tracker.track_damage(&frame).is_empty());

    // Reset forces next frame to be marked as dirty
    tracker.reset();
    assert!(!tracker.track_damage(&frame).is_empty());
}

#[test]
fn test_damage_tracker_resize() {
    let mut tracker = DamageTracker::new(64, 64, 32);
    let frame1 = vec![128u8; 64 * 64 * 4];
    let _ = tracker.track_damage(&frame1);

    // Resize to 96x96
    tracker.resize(96, 96);
    let frame2 = vec![128u8; 96 * 96 * 4];
    let damage = tracker.track_damage(&frame2);
    assert!(!damage.is_empty());
}

#[test]
fn test_damage_tracker_simd_edge_tiles_and_benchmark_1080x720() {
    let w = 1080u32; // 1080 = 33 * 32 + 24 (tests both 32px full tiles and 24px edge tiles)
    let h = 720u32;
    let mut tracker = DamageTracker::new(w, h, 32);
    let mut frame = vec![42u8; (w * h * 4) as usize];

    // Warmup initial frame
    let initial_damage = tracker.track_damage(&frame);
    assert!(!initial_damage.is_empty());

    // Benchmark unchanged frame scan across 1080x720 (3.1 MB)
    let start = std::time::Instant::now();
    for _ in 0..20 {
        let d = tracker.track_damage(&frame);
        assert!(d.is_empty());
    }
    let elapsed = start.elapsed() / 20;
    eprintln!(
        "\n[BENCHMARK] 1080x720 DamageTracker SIMD scan avg took: {:?}",
        elapsed
    );

    // Mutate the last pixel in the rightmost 24px partial edge tile (x=1079, y=719)
    let last_pixel_idx = ((719 * w + 1079) * 4) as usize;
    frame[last_pixel_idx] ^= 0xFF;
    let edge_damage = tracker.track_damage(&frame);
    assert_eq!(edge_damage.len(), 1);
    assert_eq!(edge_damage[0].x, 33 * 32);
    assert_eq!(edge_damage[0].width, 24);
}

#[test]
fn test_damage_tracker_rgb565_noise_filtering() {
    let mut tracker = DamageTracker::new(64, 64, 32);
    let mut frame = vec![0u8; 64 * 64 * 4];

    // Establish baseline frame
    for b in &mut frame {
        *b = 100; // 100 & 0xF8 = 96
    }
    let _ = tracker.track_damage(&frame);

    // Perturb lowest bits: 100 (0b01100100) -> 102 (0b01100110)
    // For R (& 0xF8), G (& 0xFC), B (& 0xF8), these small perturbations are masked to 0
    frame[0] = 102; // R: 102 & 0xF8 = 96
    frame[1] = 101; // G: 101 & 0xFC = 100
    frame[2] = 103; // B: 103 & 0xF8 = 96

    let damage = tracker.track_damage(&frame);
    assert!(damage.is_empty(), "Noise in lower bits masked by RGB565 must not trigger damage");

    // But perturbation in high bits must trigger damage
    frame[0] = 120; // 120 & 0xF8 = 120 != 96
    let damage_high = tracker.track_damage(&frame);
    assert_eq!(damage_high.len(), 1, "Significant color change must trigger damage");
}

#[test]
fn test_damage_tracker_adjacent_and_sparse_coalescing() {
    // 320x320 with 32px tile size -> 10x10 = 100 tiles
    let mut tracker = DamageTracker::new(320, 320, 32);
    let mut frame = vec![0u8; 320 * 320 * 4];
    let _ = tracker.track_damage(&frame);

    // Modify a 2x2 contiguous tile block at (0, 0) -> (tile x: 0..2, y: 0..2)
    // and a separate 1x1 tile at bottom right (tile x: 9, y: 9)
    for y in 0..64 {
        for x in 0..64 {
            frame[(y * 320 + x) * 4] = 200;
        }
    }
    frame[((9 * 32 * 320) + (9 * 32)) * 4] = 200;

    let damage = tracker.track_damage(&frame);
    // Before this fix (>16 threshold or no 2D merging), disjoint tiles would either stay unmerged or inflate.
    // Here, the 2x2 contiguous tiles should be merged into a single 64x64 rect,
    // and the bottom-right tile remains a separate 32x32 rect (total 2 rects, NOT inflated to full screen).
    assert_eq!(damage.len(), 2, "Expected 2 coalesced damage rectangles");
    assert!(damage.iter().any(|r| r.x == 0 && r.y == 0 && r.width == 64 && r.height == 64));
    assert!(damage.iter().any(|r| r.x == 288 && r.y == 288 && r.width == 32 && r.height == 32));
}


