use smithay::reexports::wayland_server::Display;
use termway::compositor::renderer::TermwayRenderer;
use termway::compositor::state::TermwayState;

#[test]
fn test_compositor_state_init() {
    let display = Display::<TermwayState>::new().expect("Failed to create Wayland Display");
    let dh = display.handle();

    let state = TermwayState::new(&dh, 800, 600);
    assert_eq!(state.width, 800);
    assert_eq!(state.height, 600);
    assert!(state.dirty);
    assert!(state.running);
    assert!(state.windows.is_empty());
}

#[test]
fn test_compositor_renderer_buffer() {
    let display = Display::<TermwayState>::new().expect("Failed to create Wayland Display");
    let dh = display.handle();

    let state = TermwayState::new(&dh, 320, 240);
    let mut renderer = TermwayRenderer::new(320, 240).expect("Failed to create Pixman renderer");

    assert_eq!(renderer.width(), 320);
    assert_eq!(renderer.height(), 240);

    let pixels = renderer.render(&state).expect("Render failed");
    // 320 * 240 * 4 bytes per RGBA pixel
    assert_eq!(pixels.len(), 320 * 240 * 4);

    // Verify dark gray background (0.08 * 255 ~= 20, 0.10 * 255 ~= 25)
    // Red channel
    assert!(pixels[0] >= 15 && pixels[0] <= 25);
    // Green channel
    assert!(pixels[1] >= 15 && pixels[1] <= 25);
    // Blue channel
    assert!(pixels[2] >= 20 && pixels[2] <= 30);
    // Alpha channel
    assert_eq!(pixels[3], 255);
}

#[test]
fn test_xwayland_state_and_shell() {
    let display = Display::<TermwayState>::new().expect("Failed to create Wayland Display");
    let dh = display.handle();

    let state = TermwayState::new(&dh, 640, 480);
    assert!(state.x11_windows.is_empty());
    assert!(state.xwm.is_none());
}


#[test]
fn test_calloop_handle() {
    let event_loop = smithay::reexports::calloop::EventLoop::<TermwayState>::try_new().unwrap();
    let handle = event_loop.handle();
    let _ = handle.clone();
}
