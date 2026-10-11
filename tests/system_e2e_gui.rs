use termway::compositor::renderer::TermwayRenderer;
use termway::compositor::state::TermwayState;
use termway::compositor::window::ManagedWindow;
use termway::display::{DamageTracker, Framebuffer};
use termway::protocol::KittyEncoder;

#[test]
fn test_system_e2e_wayland_compositor_rendering_and_damage_pipeline() {
    let width = 640;
    let height = 480;

    let display = smithay::reexports::wayland_server::Display::<TermwayState>::new()
        .expect("Failed to create Wayland Display");
    let dh = display.handle();
    let mut state = TermwayState::new(&dh, width, height, true);
    state.show_splash = false;

    let mut renderer = TermwayRenderer::new(width, height).expect("Renderer should initialize");
    let mut tracker = DamageTracker::new(width, height, 32);

    // 1. Render initial desktop with one active window
    let win_id = state.wm.alloc_id();
    let mut m_win = ManagedWindow::new_dummy(
        win_id,
        "E2E Test Window".to_string(),
        (100, 100).into(),
        (240, 180).into(),
    );
    m_win.active = true;
    state.wm.windows.push(m_win);

    let pixels_initial = renderer
        .render(&state)
        .expect("Initial render should succeed")
        .to_vec();
    let initial_damage = tracker.track_damage(&pixels_initial);
    assert!(!initial_damage.is_empty(), "First frame should have damage rects");

    let kitty_seq = KittyEncoder::encode_rgba_image(&pixels_initial, width, height, Some(1), Some(1));
    assert!(kitty_seq.starts_with("\x1b_Ga=T,q=2,f=32"));

    // 2. Move the window and verify damage tracking detects the repainted region
    state.wm.windows[0].loc = (220, 160).into();
    let pixels_moved = renderer
        .render(&state)
        .expect("Second render should succeed")
        .to_vec();
    let damage_after = tracker.track_damage(&pixels_moved);
    assert!(
        !damage_after.is_empty(),
        "Damage tracker should detect repainted window region"
    );

    // 3. Verify Framebuffer BGRX-to-RGBA conversion (SIMD fast-path)
    let mut bgrx_scanlines = vec![0u8; (width * height * 4) as usize];
    // Pixel (10, 10): B=30, G=140, R=250, X=0
    let idx = ((10 * width + 10) * 4) as usize;
    bgrx_scanlines[idx] = 30;
    bgrx_scanlines[idx + 1] = 140;
    bgrx_scanlines[idx + 2] = 250;
    bgrx_scanlines[idx + 3] = 0;

    let mut fb = Framebuffer::new(width, height);
    fb.copy_from_bgrx(&bgrx_scanlines, (width * 4) as usize, width, height);
    assert_eq!(fb.pixels[idx], 250);
    assert_eq!(fb.pixels[idx + 1], 140);
    assert_eq!(fb.pixels[idx + 2], 30);
    assert_eq!(fb.pixels[idx + 3], 255);
}

#[test]
fn test_gui_test_driver_signals_and_state() {
    use std::fs;
    use std::time::SystemTime;

    let temp_dir = std::env::temp_dir().join(format!(
        "termway_gui_test_{}",
        SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis()
    ));
    fs::create_dir_all(&temp_dir).unwrap();

    let state_file = temp_dir.join("gui_state.json");
    std::env::set_var("TERMWAY_TEST_STATE_FILE", &state_file);
    std::env::set_var("TERMWAY_TEST_DUMP_DIR", &temp_dir);

    let display = smithay::reexports::wayland_server::Display::<termway::compositor::state::TermwayState>::new()
        .expect("Failed to create Wayland Display");
    let dh = display.handle();
    let state = termway::compositor::state::TermwayState::new(&dh, 800, 600, true);

    // Test JSON state dump serialization
    let json = state.dump_state_json();
    fs::write(&state_file, json).unwrap();
    assert!(state_file.exists());

    let content = fs::read_to_string(&state_file).unwrap();
    assert!(content.contains("\"screen_width\": 800"));
    assert!(content.contains("\"screen_height\": 600"));
    assert!(content.contains("\"windows\": ["));
    assert!(content.contains("\"popups\": ["));

    // Cleanup
    let _ = fs::remove_dir_all(&temp_dir);
}
