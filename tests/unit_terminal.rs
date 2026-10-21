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
    assert!((0..800).contains(&tx));
    assert!((0..600).contains(&ty));

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

#[test]
fn test_terminal_backend_mode_names() {
    let sixel_backend = termway::backend::TerminalBackend::new(TerminalSize::fallback(80, 24), true, false, 80);
    assert_eq!(sixel_backend.name(), "Sixel");
    assert!(sixel_backend.is_sixel());
    assert!(!sixel_backend.is_jpeg());

    let kitty_backend = termway::backend::TerminalBackend::new(TerminalSize::fallback(80, 24), false, false, 80);
    assert_eq!(kitty_backend.name(), "Kitty");
    assert!(!kitty_backend.is_sixel());
    assert!(!kitty_backend.is_jpeg());

    let kitty_jpeg_backend = termway::backend::TerminalBackend::new(TerminalSize::fallback(80, 24), false, true, 85);
    assert_eq!(kitty_jpeg_backend.name(), "Kitty-JPEG");
    assert!(!kitty_jpeg_backend.is_sixel());
    assert!(kitty_jpeg_backend.is_jpeg());
}
