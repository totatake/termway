use termway::terminal::{RawModeGuard, TerminalSize};

#[test]
fn test_terminal_size_fallback() {
    let size = TerminalSize::fallback(100, 30);
    assert_eq!(size.cols, 100);
    assert_eq!(size.rows, 30);
    assert_eq!(size.cell_width_px, 9);
    assert_eq!(size.cell_height_px, 18);
    assert_eq!(size.width_px, 900);
    assert_eq!(size.height_px, 540);
}

#[test]
fn test_cell_to_pixel_mapping() {
    let size = TerminalSize::fallback(100, 50); // 900x900 px, cell 9x18
    // col 1, row 1 -> center of first cell: (9/2 = 4, 18/2 = 9)
    let (px, py) = size.cell_to_pixel(1, 1);
    assert_eq!(px, 4);
    assert_eq!(py, 9);

    // col 2, row 1 -> 9 + 4 = 13, 9
    let (px2, py2) = size.cell_to_pixel(2, 1);
    assert_eq!(px2, 13);
    assert_eq!(py2, 9);
}

#[test]
fn test_map_cell_to_target_x11() {
    let size = TerminalSize::fallback(100, 50); // 900x900
    // Target display is 800x600
    let (tx, ty) = size.map_cell_to_target(1, 1, 800, 600);
    assert!(tx >= 0 && tx < 800);
    assert!(ty >= 0 && ty < 600);

    // Bottom right cell
    let (br_x, br_y) = size.map_cell_to_target(100, 50, 800, 600);
    assert!(br_x < 800);
    assert!(br_y < 600);
    assert!(br_x > 750);
    assert!(br_y > 550);
}

#[test]
fn test_raw_mode_guard_dummy_drop() {
    {
        let _guard = RawModeGuard::dummy();
    }
    // Verifies no panic on drop
}
