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
