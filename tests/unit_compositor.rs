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
fn test_dump_state_json() {
    let display = Display::<TermwayState>::new().expect("Failed to create Wayland Display");
    let dh = display.handle();

    let state = TermwayState::new(&dh, 1024, 768, true);
    let json = state.dump_state_json();

    assert!(json.contains("\"screen_width\": 1024"));
    assert!(json.contains("\"screen_height\": 768"));
    assert!(json.contains("\"windows\": ["));
    assert!(json.contains("\"popups\": ["));
}

#[test]
fn test_compositor_clipboard_selection_and_send() {
    use smithay::wayland::selection::SelectionHandler;
    use smithay::wayland::selection::SelectionTarget;
    use std::io::Read;
    use std::os::unix::io::FromRawFd;

    let display = Display::<TermwayState>::new().expect("Failed to create Wayland Display");
    let dh = display.handle();

    let mut state = TermwayState::new(&dh, 800, 600, true);
    let seat = state.seat.clone();

    // Set server-side clipboard selection
    let sample_text = "Hello Wayland Clipboard!";
    smithay::wayland::selection::data_device::set_data_device_selection(
        &dh,
        &seat,
        vec!["text/plain;charset=utf-8".to_string()],
        sample_text.as_bytes().to_vec(),
    );

    // Verify send_selection writes the user_data to the provided fd
    let mut fds = [0i32; 2];
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    let read_fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(fds[0]) };
    let write_fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(fds[1]) };

    state.send_selection(
        SelectionTarget::Clipboard,
        "text/plain;charset=utf-8".to_string(),
        write_fd,
        seat,
        &sample_text.as_bytes().to_vec(),
    );

    let mut file = std::fs::File::from(read_fd);
    let mut read_data = String::new();
    file.read_to_string(&mut read_data).expect("Failed to read from pipe");
    assert_eq!(read_data, sample_text);
}

#[test]
fn test_compositor_mouse_wheel_dispatch() {
    use termway::compositor::input::dispatch_mouse_event;
    use termway::protocol::{MouseAction, MouseButton, MouseEvent};

    let display = Display::<TermwayState>::new().expect("Failed to create Wayland Display");
    let dh = display.handle();

    let mut state = TermwayState::new(&dh, 800, 600, true);
    state.dirty = false;

    let wheel_up = MouseEvent {
        button: MouseButton::WheelUp,
        action: MouseAction::Press,
        col: 40,
        row: 15,
        shift: false,
        meta: false,
        ctrl: false,
    };
    let mut child = None;
    dispatch_mouse_event(&mut state, 800, 600, 80, 24, &wheel_up, &mut child);
    assert!(state.dirty, "WheelUp should set state.dirty = true");

    state.dirty = false;
    let wheel_down = MouseEvent {
        button: MouseButton::WheelDown,
        action: MouseAction::Press,
        col: 40,
        row: 15,
        shift: false,
        meta: false,
        ctrl: false,
    };
    dispatch_mouse_event(&mut state, 800, 600, 80, 24, &wheel_down, &mut child);
    assert!(state.dirty, "WheelDown should set state.dirty = true");
}

#[test]
fn test_window_maximize_and_unmaximize() {
    use smithay::utils::{Point, Size};
    use termway::compositor::window::{ManagedWindow, BORDER_WIDTH, TITLEBAR_HEIGHT};

    let orig_loc = Point::from((50, 60));
    let orig_size = Size::from((400, 300));
    let mut win = ManagedWindow::new_dummy(1, "Test Win".into(), orig_loc, orig_size);

    assert!(!win.is_maximized);
    assert!(win.saved_geometry.is_none());

    // Maximize on 800x600 screen
    win.maximize(800, 600);
    assert!(win.is_maximized);
    assert!(win.saved_geometry.is_some());
    assert_eq!(win.loc.x, BORDER_WIDTH);
    assert_eq!(win.loc.y, TITLEBAR_HEIGHT + BORDER_WIDTH);
    assert_eq!(win.size.w, 800 - BORDER_WIDTH * 2);
    assert_eq!(win.size.h, 600 - TITLEBAR_HEIGHT - BORDER_WIDTH * 2);

    // Unmaximize restores original geometry
    win.unmaximize();
    assert!(!win.is_maximized);
    assert!(win.saved_geometry.is_none());
    assert_eq!(win.loc.x, orig_loc.x);
    assert_eq!(win.loc.y, orig_loc.y);
    assert_eq!(win.size.w, orig_size.w);
    assert_eq!(win.size.h, orig_size.h);

    // Toggle maximize
    win.toggle_maximize(800, 600);
    assert!(win.is_maximized);
    win.toggle_maximize(800, 600);
    assert!(!win.is_maximized);
}

#[test]
fn test_titlebar_buttons_geometry_and_hit_testing() {
    use smithay::utils::{Point, Size};
    use termway::compositor::window::ManagedWindow;

    let loc = Point::from((100, 100));
    let size = Size::from((500, 400));
    let win = ManagedWindow::new_dummy(1, "Test Window".into(), loc, size);

    let close_hit = win.close_button_hit_geometry();
    let max_hit = win.maximize_button_hit_geometry();
    let min_hit = win.minimize_button_hit_geometry();

    // Verify non-overlapping distinct hit regions from right to left
    assert_eq!(close_hit.size.w, 24);
    assert_eq!(max_hit.size.w, 24);
    assert_eq!(min_hit.size.w, 24);

    assert_eq!(max_hit.loc.x + max_hit.size.w, close_hit.loc.x);
    assert_eq!(min_hit.loc.x + min_hit.size.w, max_hit.loc.x);

    // Test hit detection with points inside each button
    let close_pt = Point::from((close_hit.loc.x + 5, close_hit.loc.y + 5));
    let max_pt = Point::from((max_hit.loc.x + 5, max_hit.loc.y + 5));
    let min_pt = Point::from((min_hit.loc.x + 5, min_hit.loc.y + 5));

    assert!(close_hit.contains(close_pt));
    assert!(!max_hit.contains(close_pt));
    assert!(!min_hit.contains(close_pt));

    assert!(max_hit.contains(max_pt));
    assert!(!close_hit.contains(max_pt));
    assert!(!min_hit.contains(max_pt));

    assert!(min_hit.contains(min_pt));
    assert!(!close_hit.contains(min_pt));
    assert!(!max_hit.contains(min_pt));
}

#[test]
fn test_window_minimize_and_restore() {
    use smithay::utils::{Point, Size};
    use termway::compositor::window::ManagedWindow;

    let display = Display::<TermwayState>::new().expect("Failed to create Wayland Display");
    let dh = display.handle();

    let mut state = TermwayState::new(&dh, 800, 600, true);

    let win1 = ManagedWindow::new_dummy(1, "App 1".into(), Point::from((10, 10)), Size::from((200, 150)));
    let win2 = ManagedWindow::new_dummy(2, "App 2".into(), Point::from((50, 50)), Size::from((200, 150)));
    state.managed_windows.push(win1);
    state.managed_windows.push(win2);

    // Win 2 is on top and active
    assert_eq!(state.managed_windows.last().unwrap().id, 2);

    // Minimize Win 2 -> Win 1 becomes active
    state.minimize_window(2);
    let win2_ref = state.managed_windows.iter().find(|w| w.id == 2).unwrap();
    assert!(win2_ref.is_minimized);
    assert!(!win2_ref.active);

    let win1_ref = state.managed_windows.iter().find(|w| w.id == 1).unwrap();
    assert!(!win1_ref.is_minimized);
    assert!(win1_ref.active);

    // Restore Win 2 -> Win 2 becomes un-minimized, active, and top of Z-order
    state.restore_window(2);
    let win2_restored = state.managed_windows.last().unwrap();
    assert_eq!(win2_restored.id, 2);
    assert!(!win2_restored.is_minimized);
    assert!(win2_restored.active);
}

#[test]
fn test_minimized_window_renderer_suppression() {
    use smithay::utils::{Point, Size};
    use termway::compositor::window::ManagedWindow;
    use termway::compositor::renderer::TermwayRenderer;

    let display = Display::<TermwayState>::new().expect("Failed to create Wayland Display");
    let dh = display.handle();

    let mut state = TermwayState::new(&dh, 800, 600, true);
    let mut renderer = TermwayRenderer::new(800, 600).expect("Failed to create TermwayRenderer");

    let win = ManagedWindow::new_dummy(1, "Test App".into(), Point::from((50, 50)), Size::from((300, 200)));
    state.managed_windows.push(win);

    // Initial render when not minimized succeeds
    let pixels_normal = renderer.render(&state).expect("Render normal failed");
    assert_eq!(pixels_normal.len(), 800 * 600 * 4);

    // Minimize window -> all managed windows are minimized
    state.minimize_window(1);
    assert!(state.managed_windows[0].is_minimized);

    // Rendering when minimized should succeed without errors and not render unassociated fallback
    let pixels_min = renderer.render(&state).expect("Render minimized failed");
    assert_eq!(pixels_min.len(), 800 * 600 * 4);

    // Restore window
    state.restore_window(1);
    assert!(!state.managed_windows[0].is_minimized);
    let pixels_restored = renderer.render(&state).expect("Render restored failed");
    assert_eq!(pixels_restored.len(), 800 * 600 * 4);
}



