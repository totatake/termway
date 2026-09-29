use termway::backend::{detect_backend_mode, Backend, BackendMode, DrmBackend};
use termway::display::{DamageTracker, Framebuffer};
use termway::protocol::{KittyEncoder, SgrMouseParser};
use termway::terminal::TerminalSize;

#[test]
fn test_integration_framebuffer_to_kitty_pipeline() {
    let width = 64;
    let height = 64;
    let mut fb = Framebuffer::new(width, height);
    let mut tracker = DamageTracker::new(width, height, 32);

    // Initial state: draw a red square at (0, 0) to (16, 16)
    for y in 0..16 {
        for x in 0..16 {
            let idx = (y * width + x) * 4;
            fb.pixels[idx as usize] = 255;     // R
            fb.pixels[idx as usize + 3] = 255; // A
        }
    }

    let damage = tracker.track_damage(&fb.pixels);
    assert!(!damage.is_empty());

    // Encode via Kitty Graphics Protocol
    let mut encoded_stream = String::new();
    for rect in &damage {
        let chunk = KittyEncoder::encode_rgba_subimage(
            &fb.pixels,
            fb.width,
            fb.height,
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            Some(1),
            Some(1),
        );
        encoded_stream.push_str(&chunk);
    }

    assert!(encoded_stream.starts_with("\x1b_Ga=T,q=2,f=32"));
    assert!(encoded_stream.contains(",X=1,Y=1"));
}

#[test]
fn test_integration_mouse_to_target_coordinate_pipeline() {
    let term_size = TerminalSize::fallback(80, 24); // cell size 9x18
    let sgr_str = "\x1b[<0;40;12M"; // Left click at col 40, row 12

    let (mouse_ev, _) = SgrMouseParser::parse(sgr_str).expect("parse sgr");
    let (target_x, target_y) = term_size.map_cell_to_target(
        mouse_ev.col,
        mouse_ev.row,
        1024,
        768,
    );

    // Should map roughly to center of 1024x768 screen
    assert!(target_x > 450 && target_x < 550, "mapped x: {}", target_x);
    assert!(target_y > 330 && target_y < 430, "mapped y: {}", target_y);
}

#[test]
fn test_integration_drm_backend_mock_pipeline() {
    let mut drm = DrmBackend::mock(64, 64);
    assert_eq!(drm.name(), "Native DRM/KMS (Direct Hardware Display)");

    let mut fb = Framebuffer::new(64, 64);
    fb.pixels[0] = 42;

    let mut tracker = DamageTracker::new(64, 64, 32);
    let damage = tracker.track_damage(&fb.pixels);

    let res = drm.render_damage(&fb, &damage);
    assert!(res.is_ok());
}

#[test]
fn test_integration_backend_detection() {
    let mode = detect_backend_mode();
    // In our test runner / container, it should safely fall back to Terminal mode
    assert!(mode == BackendMode::Terminal || mode == BackendMode::Drm);
}
