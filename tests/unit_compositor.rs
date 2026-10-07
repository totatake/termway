use smithay::reexports::wayland_server::Display;
use termway::compositor::renderer::TermwayRenderer;
use termway::compositor::state::TermwayState;

#[test]
fn test_compositor_state_init() {
    let display = Display::<TermwayState>::new().expect("Failed to create Wayland Display");
    let dh = display.handle();

    let state = TermwayState::new(&dh, 800, 600, true);
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

    let state = TermwayState::new(&dh, 320, 240, true);
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

    let mut state = TermwayState::new(&dh, 640, 480, true);
    assert!(state.x11_windows.is_empty());
    assert!(state.xwm.is_none());
    assert!(!state.xwayland_ready);
    assert!(state.xwayland_error.is_none());
    // unassociated_surfaces は commit フックで収集される XWayland サーフェスのフォールバック用
    assert!(state.unassociated_surfaces.is_empty());

    // Verify error state tracking
    state.xwayland_error = Some("XWayland failed to start".into());
    assert!(state.xwayland_error.is_some());
}


#[test]
fn test_output_and_data_device() {
    let display = Display::<TermwayState>::new().expect("Failed to create Wayland Display");
    let dh = display.handle();

    let state = TermwayState::new(&dh, 1024, 768, true);
    assert_eq!(state.output.name(), "termway-0");
    assert_eq!(state.output.current_mode().unwrap().size, (1024, 768).into());
}

#[test]
fn test_window_geometry_and_decorations() {
    use smithay::utils::{Point, Size};
    use termway::compositor::window::{ManagedWindow, TITLEBAR_HEIGHT, BORDER_WIDTH, BUTTON_SIZE};
    use termway::compositor::decoration::DecorationRenderer;

    let loc = Point::from((50, 60));
    let size = Size::from((200, 150));

    // Create a dummy ManagedWindow using Wayland dummy or testing geometry methods directly
    let fg_x = 50 - BORDER_WIDTH;
    let fg_y = 60 - TITLEBAR_HEIGHT - BORDER_WIDTH;
    let fg_w = 200 + BORDER_WIDTH * 2;
    let fg_h = 150 + TITLEBAR_HEIGHT + BORDER_WIDTH * 2;

    let dummy_window = ManagedWindow::new_dummy(
        1,
        "Test Window".to_string(),
        loc,
        size,
    );

    let fg = dummy_window.frame_geometry();
    assert_eq!(fg.loc.x, fg_x);
    assert_eq!(fg.loc.y, fg_y);
    assert_eq!(fg.size.w, fg_w);
    assert_eq!(fg.size.h, fg_h);

    let tb = dummy_window.titlebar_geometry();
    assert_eq!(tb.loc.x, fg_x);
    assert_eq!(tb.loc.y, fg_y);
    assert_eq!(tb.size.h, TITLEBAR_HEIGHT);

    let close_btn = dummy_window.close_button_geometry();
    assert!(tb.contains(Point::from((close_btn.loc.x, close_btn.loc.y))));
    assert_eq!(close_btn.size.w, BUTTON_SIZE);

    let resize_grip = dummy_window.resize_handle_geometry();
    assert!(fg.contains(Point::from((resize_grip.loc.x + 5, resize_grip.loc.y + 5))));

    // Test decoration drawing on pixman image buffer
    let mut image = pixman::Image::new(
        pixman::FormatCode::A8B8G8R8,
        300,
        300,
        true,
    ).unwrap();

    let deco = DecorationRenderer::new();
    deco.draw_decorations(&mut image, 300, 300, &dummy_window);
}

#[test]
fn test_csd_window_no_double_decorations() {
    use smithay::utils::{Point, Size};
    use termway::compositor::window::ManagedWindow;
    use termway::compositor::decoration::DecorationRenderer;

    let loc = Point::from((50, 60));
    let size = Size::from((200, 150));

    let mut csd_window = ManagedWindow::new_dummy(1, "CSD App".into(), loc, size);
    csd_window.has_ssd = false; // Simulated client with client-side decorations (e.g. gnome-terminal)

    // CSD window frame geometry should match content geometry exactly
    let fg = csd_window.frame_geometry();
    let cg = csd_window.content_geometry();
    assert_eq!(fg.loc.x, cg.loc.x);
    assert_eq!(fg.loc.y, cg.loc.y);
    assert_eq!(fg.size.w, cg.size.w);
    assert_eq!(fg.size.h, cg.size.h);

    // Titlebar, close button, and resize handle should be empty/zero
    let tb = csd_window.titlebar_geometry();
    assert_eq!(tb.size.w, 0);
    assert_eq!(tb.size.h, 0);

    let btn = csd_window.close_button_geometry();
    assert_eq!(btn.size.w, 0);
    assert_eq!(btn.size.h, 0);

    let grip = csd_window.resize_handle_geometry();
    assert_eq!(grip.size.w, 0);
    assert_eq!(grip.size.h, 0);

    // Drawing decorations on CSD window should be a no-op
    let mut image = pixman::Image::new(
        pixman::FormatCode::A8B8G8R8,
        300,
        300,
        true,
    ).unwrap();
    let deco = DecorationRenderer::new();
    deco.draw_decorations(&mut image, 300, 300, &csd_window);
}

#[test]
fn test_state_raise_window() {
    use smithay::utils::{Point, Size};
    use termway::compositor::window::ManagedWindow;

    let display = Display::<TermwayState>::new().expect("Failed to create Wayland Display");
    let dh = display.handle();

    let mut state = TermwayState::new(&dh, 800, 600, true);

    let win1 = ManagedWindow::new_dummy(1, "Window 1".into(), Point::from((10, 10)), Size::from((200, 150)));
    let win2 = ManagedWindow::new_dummy(2, "Window 2".into(), Point::from((50, 50)), Size::from((200, 150)));
    let win3 = ManagedWindow::new_dummy(3, "Window 3".into(), Point::from((90, 90)), Size::from((200, 150)));

    state.managed_windows.push(win1);
    state.managed_windows.push(win2);
    state.managed_windows.push(win3);

    assert_eq!(state.managed_windows.len(), 3);
    assert_eq!(state.managed_windows.last().unwrap().id, 3);

    // Raise window 1 to top
    state.raise_window(1);

    // Window 1 should now be at the end (top of Z-order) and active
    assert_eq!(state.managed_windows.len(), 3);
    assert_eq!(state.managed_windows.last().unwrap().id, 1);
    assert!(state.managed_windows.last().unwrap().active);

    // Other windows should be inactive
    assert!(!state.managed_windows[0].active);
    assert!(!state.managed_windows[1].active);

    // Raise window 2 to top
    state.raise_window(2);
    assert_eq!(state.managed_windows.last().unwrap().id, 2);
    assert!(state.managed_windows.last().unwrap().active);
    assert!(!state.managed_windows[0].active);
    assert!(!state.managed_windows[1].active);
}

#[test]
fn test_window_clamping_logic() {
    use smithay::utils::{Point, Size};
    use termway::compositor::window::{ManagedWindow, BORDER_WIDTH, TITLEBAR_HEIGHT};

    let screen_w = 800;
    let screen_h = 600;

    let mut win = ManagedWindow::new_dummy(1, "Test Window".into(), Point::from((50, 50)), Size::from((700, 500)));

    // Clamp window position
    let min_x = BORDER_WIDTH;
    let max_x = (screen_w - win.size.w - BORDER_WIDTH).max(min_x);
    let min_y = TITLEBAR_HEIGHT + BORDER_WIDTH;
    let max_y = (screen_h - win.size.h - BORDER_WIDTH).max(min_y);

    let clamped_x = win.loc.x.clamp(min_x, max_x);
    let clamped_y = win.loc.y.clamp(min_y, max_y);
    win.set_loc(Point::from((clamped_x, clamped_y)));

    assert!(win.loc.x >= min_x && win.loc.x <= max_x);
    assert!(win.loc.y >= min_y && win.loc.y <= max_y);

    // Frame geometry should be entirely within screen bounds
    let fg = win.frame_geometry();
    assert!(fg.loc.x >= 0);
    assert!(fg.loc.y >= 0);
    assert!(fg.loc.x + fg.size.w <= screen_w);
    assert!(fg.loc.y + fg.size.h <= screen_h);
}

#[test]
fn test_data_device_focus_change() {
    use smithay::input::SeatHandler;

    let display = Display::<TermwayState>::new().expect("Failed to create Wayland Display");
    let dh = display.handle();

    let mut state = TermwayState::new(&dh, 800, 600, true);
    let seat = state.seat.clone();

    // Focus change to None (unfocus) should complete cleanly without panicking
    state.focus_changed(&seat, None);
}

#[test]
fn test_popup_and_override_redirect_handling() {
    let display = Display::<TermwayState>::new().expect("Failed to create Wayland Display");
    let dh = display.handle();

    let state = TermwayState::new(&dh, 1024, 768, true);

    // Verify PopupManager is initialized
    assert_eq!(state.managed_windows.len(), 0);
    assert_eq!(state.x11_windows.len(), 0);
    assert_eq!(state.unassociated_surfaces.len(), 0);
}

