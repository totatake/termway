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
    assert!(state.toplevels.is_empty());
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
    deco.draw_decorations(&mut image, 300, 300, &dummy_window, 1.0);
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
    deco.draw_decorations(&mut image, 300, 300, &csd_window, 1.0);
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

    state.wm.windows.push(win1);
    state.wm.windows.push(win2);
    state.wm.windows.push(win3);

    assert_eq!(state.wm.windows.len(), 3);
    assert_eq!(state.wm.windows.last().unwrap().id, 3);

    // Raise window 1 to top
    state.raise_window(1);

    // Window 1 should now be at the end (top of Z-order) and active
    assert_eq!(state.wm.windows.len(), 3);
    assert_eq!(state.wm.windows.last().unwrap().id, 1);
    assert!(state.wm.windows.last().unwrap().active);

    // Other windows should be inactive
    assert!(!state.wm.windows[0].active);
    assert!(!state.wm.windows[1].active);

    // Raise window 2 to top
    state.raise_window(2);
    assert_eq!(state.wm.windows.last().unwrap().id, 2);
    assert!(state.wm.windows.last().unwrap().active);
    assert!(!state.wm.windows[0].active);
    assert!(!state.wm.windows[1].active);
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
    win.unmaximize(800, 600);
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

    // Test unmaximize without saved_geometry uses 80% screen fallback
    win.is_maximized = true;
    win.saved_geometry = None;
    win.unmaximize(800, 600);
    assert!(!win.is_maximized);
    let expected_80 = ManagedWindow::default_80pct_geometry(800, 600, true);
    assert_eq!(win.loc, expected_80.loc);
    assert_eq!(win.size, expected_80.size);
    // Verify 80% bounds
    assert_eq!(win.size.w, 640);
    assert_eq!(win.size.h, 480);
    let fg = win.frame_geometry();
    assert!(fg.loc.x >= 0 && fg.loc.y >= 0);
    assert!(fg.loc.x + fg.size.w <= 800);
    assert!(fg.loc.y + fg.size.h <= 600);
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
    assert_eq!(close_hit.size.w, 22);
    assert_eq!(max_hit.size.w, 20);
    assert_eq!(min_hit.size.w, 20);

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
    state.wm.windows.push(win1);
    state.wm.windows.push(win2);

    // Win 2 is on top and active
    assert_eq!(state.wm.windows.last().unwrap().id, 2);

    // Minimize Win 2 -> Win 1 becomes active
    state.minimize_window(2);
    let win2_ref = state.wm.windows.iter().find(|w| w.id == 2).unwrap();
    assert!(win2_ref.is_minimized);
    assert!(!win2_ref.active);

    let win1_ref = state.wm.windows.iter().find(|w| w.id == 1).unwrap();
    assert!(!win1_ref.is_minimized);
    assert!(win1_ref.active);

    // Restore Win 2 -> Win 2 becomes un-minimized, active, and top of Z-order
    state.restore_window(2);
    let win2_restored = state.wm.windows.last().unwrap();
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
    state.wm.windows.push(win);

    // Initial render when not minimized succeeds
    let pixels_normal = renderer.render(&state).expect("Render normal failed");
    assert_eq!(pixels_normal.len(), 800 * 600 * 4);

    // Minimize window -> all managed windows are minimized
    state.minimize_window(1);
    assert!(state.wm.windows[0].is_minimized);

    // Rendering when minimized should succeed without errors and not render unassociated fallback
    let pixels_min = renderer.render(&state).expect("Render minimized failed");
    assert_eq!(pixels_min.len(), 800 * 600 * 4);

    // Restore window
    state.restore_window(1);
    assert!(!state.wm.windows[0].is_minimized);
    let pixels_restored = renderer.render(&state).expect("Render restored failed");
    assert_eq!(pixels_restored.len(), 800 * 600 * 4);
}

#[test]
fn test_csd_and_ssd_unmaximize_80pct_centered() {
    use smithay::utils::{Point, Size};
    use termway::compositor::window::ManagedWindow;

    let screen_w = 1000;
    let screen_h = 800;

    // 1. CSD window (like Firefox) initialized maximized
    let saved_geo = ManagedWindow::default_80pct_geometry(screen_w, screen_h, false);
    let mut win_csd = ManagedWindow::new_dummy(1, "Firefox".into(), Point::from((0, 0)), Size::from((screen_w as i32, screen_h as i32)));
    win_csd.has_ssd = false;
    win_csd.is_maximized = true;
    win_csd.saved_geometry = Some(saved_geo);

    // Unmaximize CSD window
    win_csd.unmaximize(screen_w, screen_h);
    assert!(!win_csd.is_maximized);
    assert!(win_csd.pending_unmaximize);
    assert_eq!(win_csd.size.w, 800); // 80% of 1000
    assert_eq!(win_csd.size.h, 640); // 80% of 800
    assert_eq!(win_csd.loc.x, (1000 - 800) / 2); // 100 (centered)
    assert_eq!(win_csd.loc.y, (800 - 640) / 2); // 80 (centered)

    // 2. SSD window initialized maximized
    let saved_geo_ssd = ManagedWindow::default_80pct_geometry(screen_w, screen_h, true);
    let mut win_ssd = ManagedWindow::new_dummy(2, "Gnome-Terminal".into(), Point::from((0, 0)), Size::from((screen_w as i32, screen_h as i32)));
    win_ssd.has_ssd = true;
    win_ssd.is_maximized = true;
    win_ssd.saved_geometry = Some(saved_geo_ssd);

    // Unmaximize SSD window
    win_ssd.unmaximize(screen_w, screen_h);
    assert!(!win_ssd.is_maximized);
    assert!(win_ssd.pending_unmaximize);
    assert_eq!(win_ssd.size.w, 800); // 80% of 1000
    assert_eq!(win_ssd.size.h, 640); // 80% of 800
    let fg = win_ssd.frame_geometry();
    // Frame including SSD titlebar and border should be centered
    let frame_x = (1000 - fg.size.w) / 2;
    let frame_y = (800 - fg.size.h) / 2;
    assert_eq!(fg.loc.x, frame_x);
    assert_eq!(fg.loc.y, frame_y);
}

#[test]
fn test_unmaximized_window_clean_background_outside_bounds() {
    use smithay::reexports::wayland_server::Display;
    use smithay::utils::{Point, Size};
    use termway::compositor::renderer::TermwayRenderer;
    use termway::compositor::state::TermwayState;
    use termway::compositor::window::ManagedWindow;

    let display = Display::<TermwayState>::new().expect("Failed to create Wayland Display");
    let dh = display.handle();

    let mut state = TermwayState::new(&dh, 800, 600, true);
    let mut renderer = TermwayRenderer::new(800, 600).expect("Failed to create TermwayRenderer");

    // Create a window that was maximized, then unmaximized to 80% centered
    let mut win = ManagedWindow::new_dummy(1, "xeyes".into(), Point::from((0, 0)), Size::from((800, 600)));
    win.has_ssd = true;
    win.is_maximized = true;
    win.saved_geometry = Some(ManagedWindow::default_80pct_geometry(800, 600, true));

    // Unmaximize to 80% centered
    win.unmaximize(800, 600);
    assert!(!win.is_maximized);
    let fg = win.frame_geometry();
    state.wm.windows.push(win);

    // Render frame
    let pixels = renderer.render(&state).expect("Render failed");
    assert_eq!(pixels.len(), 800 * 600 * 4);

    let get_pixel = |x: usize, y: usize| -> (u8, u8, u8, u8) {
        let idx = (y * 800 + x) * 4;
        (pixels[idx], pixels[idx + 1], pixels[idx + 2], pixels[idx + 3])
    };

    // Outside the window bounds, the background must be cleanly dark gray (#14141a)
    // Dark gray: R ~20, G ~20, B ~25, A = 255
    let check_background = |x: usize, y: usize| {
        let (r, g, b, a) = get_pixel(x, y);
        assert!(r >= 15 && r <= 25, "pixel at ({}, {}) R channel expected ~20, got {}", x, y, r);
        assert!(g >= 15 && g <= 25, "pixel at ({}, {}) G channel expected ~20, got {}", x, y, g);
        assert!(b >= 20 && b <= 30, "pixel at ({}, {}) B channel expected ~25, got {}", x, y, b);
        assert_eq!(a, 255, "pixel at ({}, {}) A channel expected 255, got {}", x, y, a);
    };

    // Test top-left margin (outside frame)
    assert!(fg.loc.x > 10 && fg.loc.y > 10);
    check_background(5, 5);
    check_background(10, 10);

    // Test right margin (outside frame)
    let right_outside = (fg.loc.x + fg.size.w + 10) as usize;
    assert!(right_outside < 800);
    check_background(right_outside, 300);

    // Test bottom margin (outside frame)
    let bottom_outside = (fg.loc.y + fg.size.h + 10) as usize;
    assert!(bottom_outside < 600);
    check_background(400, bottom_outside);
}

#[test]
fn test_unmaximize_pending_preserves_target_size_and_requests_clear() {
    use smithay::reexports::wayland_server::Display;
    use smithay::utils::{Point, Size};
    use termway::compositor::state::TermwayState;
    use termway::compositor::window::ManagedWindow;

    let display = Display::<TermwayState>::new().expect("Failed to create Wayland Display");
    let dh = display.handle();

    let mut state = TermwayState::new(&dh, 1000, 600, true);
    let mut win = ManagedWindow::new_dummy(1, "xeyes".into(), Point::from((0, 0)), Size::from((1000, 600)));
    win.has_ssd = true;
    win.is_maximized = true;

    // Trigger unmaximize
    win.unmaximize(1000, 600);
    assert!(win.pending_unmaximize);
    assert!(!win.is_maximized);

    let target_size = win.size;
    let target_loc = win.loc;

    // Target size must be ~80%
    assert!(target_size.w <= 800);
    assert!(target_loc.x > 0);

    state.wm.windows.push(win);

    // Verify state needs_clear
    state.needs_clear = true;
    assert!(state.needs_clear);
}

#[test]
fn test_dynamic_resize_updates_maximized_and_floating_windows() {
    use termway::compositor::window::{ManagedWindow, BORDER_WIDTH, TITLEBAR_HEIGHT};
    use smithay::utils::{Point, Size};

    // 1. Maximized CSD window expands to full screen
    let mut win_csd = ManagedWindow::new_dummy(1, "CSD Win".into(), Point::from((0, 0)), Size::from((800, 600)));
    win_csd.has_ssd = false;
    win_csd.is_maximized = true;

    win_csd.update_maximized_size(1280, 720);
    assert_eq!(win_csd.loc.x, 0);
    assert_eq!(win_csd.loc.y, 0);
    assert_eq!(win_csd.size.w, 1280);
    assert_eq!(win_csd.size.h, 720);

    // Unmaximizing after resize MUST restore to 80% and centered on the NEW screen (1280x720)!
    win_csd.unmaximize(1280, 720);
    assert!(!win_csd.is_maximized);
    assert!(win_csd.pending_unmaximize);
    assert_eq!(win_csd.size.w, (1280 * 8) / 10);
    assert_eq!(win_csd.size.h, (720 * 8) / 10);
    assert_eq!(win_csd.loc.x, (1280 - win_csd.size.w) / 2);
    assert_eq!(win_csd.loc.y, (720 - win_csd.size.h) / 2);

    // 2. Maximized SSD window expands to full screen within server-decorated margins
    let mut win_ssd = ManagedWindow::new_dummy(2, "SSD Win".into(), Point::from((0, 0)), Size::from((800, 600)));
    win_ssd.has_ssd = true;
    win_ssd.is_maximized = true;

    win_ssd.update_maximized_size(1280, 720);
    assert_eq!(win_ssd.loc.x, BORDER_WIDTH);
    assert_eq!(win_ssd.loc.y, TITLEBAR_HEIGHT + BORDER_WIDTH);
    assert_eq!(win_ssd.size.w, 1280 - BORDER_WIDTH * 2);
    assert_eq!(win_ssd.size.h, 720 - TITLEBAR_HEIGHT - BORDER_WIDTH * 2);

    // Unmaximizing SSD after resize MUST restore to 80% and centered on the NEW screen (1280x720)!
    win_ssd.unmaximize(1280, 720);
    assert!(!win_ssd.is_maximized);
    assert!(win_ssd.pending_unmaximize);
    let expected_ssd = ManagedWindow::default_80pct_geometry(1280, 720, true);
    assert_eq!(win_ssd.size, expected_ssd.size);
    assert_eq!(win_ssd.loc, expected_ssd.loc);
}

#[test]
fn test_splash_screen_rendering() {
    use termway::compositor::decoration::DecorationRenderer;

    let width: u32 = 640;
    let height: u32 = 480;
    let mut image = pixman::Image::new(
        pixman::FormatCode::A8B8G8R8,
        width as usize,
        height as usize,
        true,
    ).unwrap();

    let deco = DecorationRenderer::new();
    // Render splash at initial timestamp and later timestamp (for spinner/pulse animation)
    deco.draw_splash(&mut image, width, height, 0, "KITTY-ZLIB | wayland-1 | X11:0 | 640x480", "gvim");
    deco.draw_splash(&mut image, width, height, 500, "KITTY-ZLIB | wayland-1 | X11:0 | 640x480", "gvim");

    // Verify buffer data is non-empty / altered from zero
    let data_ptr = unsafe { image.data() };
    let slice = unsafe {
        std::slice::from_raw_parts(data_ptr as *const u8, (height as usize) * image.stride())
    };
    let non_zero_count = slice.iter().filter(|&&b| b > 0).count();
    assert!(non_zero_count > 1000, "Splash card must render pixels into buffer");
}

#[test]
fn test_splash_screen_dismissal() {
    use smithay::reexports::wayland_server::Display;
    use smithay::utils::{Point, Size};
    use termway::compositor::state::TermwayState;
    use termway::compositor::window::ManagedWindow;
    use termway::engine::EngineConfig;

    let display: Display<TermwayState> = Display::new().unwrap();
    let dh = display.handle();
    let mut state = TermwayState::new(&dh, 800, 600, true);

    // Splash is active on startup by default
    assert!(state.show_splash);

    // Verify --no-splash flag disables splash
    let mut cfg = EngineConfig::default();
    assert!(!cfg.no_splash);
    cfg.no_splash = true;
    state.show_splash = !cfg.no_splash;
    assert!(!state.show_splash);

    // Reset and test automatic dismissal when a window opens
    state.show_splash = true;
    let win = ManagedWindow::new_dummy(1, "App".into(), Point::from((50, 50)), Size::from((400, 300)));
    state.wm.windows.push(win);
    state.dismiss_splash();

    assert!(!state.show_splash);
    assert!(state.needs_clear);
}

#[test]
fn test_window_manager_domain_standalone() {
    use smithay::utils::{Point, Size};
    use termway::compositor::window::ManagedWindow;
    use termway::compositor::window_manager::WindowManager;

    let mut wm = WindowManager::new();
    assert!(wm.is_empty());

    let id1 = wm.alloc_id();
    let id2 = wm.alloc_id();
    assert_eq!(id1, 1);
    assert_eq!(id2, 2);

    wm.windows.push(ManagedWindow::new_dummy(id1, "Win1".into(), Point::from((0, 0)), Size::from((200, 150))));
    wm.deactivate_all();
    wm.windows.push(ManagedWindow::new_dummy(id2, "Win2".into(), Point::from((20, 20)), Size::from((200, 150))));

    assert_eq!(wm.top_visible_id(), Some(2));
    assert!(wm.bring_to_top(1));
    assert_eq!(wm.top_visible_id(), Some(1));
    assert!(wm.get(1).unwrap().active);
    assert!(!wm.get(2).unwrap().active);

    assert!(wm.set_minimized(1, true));
    assert_eq!(wm.top_visible_id(), Some(2));
}

#[test]
fn test_termway_error_variants_and_display() {
    use std::error::Error;
    use termway::TermwayError;

    let err_sock = TermwayError::NoFreeWaylandSocket;
    assert_eq!(err_sock.to_string(), "Failed to find free wayland socket name");
    assert!(err_sock.source().is_none());

    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "missing binary");
    let err_spawn = TermwayError::ClientSpawn {
        command: "nonexistent-gui".into(),
        source: io_err,
    };
    assert!(err_spawn.to_string().contains("nonexistent-gui"));
    assert!(err_spawn.source().is_some());

    let parse_res: termway::Result<u32> = "not_a_number".parse::<u32>().map_err(Into::into);
    assert!(matches!(parse_res, Err(TermwayError::ConfigParse(_))));
}

#[test]
fn test_free_x11_display_candidates_never_returns_host_displays() {
    use termway::compositor::TermwayCompositor;

    let candidates = TermwayCompositor::free_x11_display_candidates();
    assert!(!candidates.is_empty(), "Should find free X11 display candidates in 10..=64");
    for &d in &candidates {
        assert!(
            (10..=64).contains(&d),
            "Display candidate :{} must be in 10..=64 (never 0..=9 such as WSLg :0)",
            d
        );
    }
}

#[test]
fn test_foreground_window_occludes_background_ssd_decorations() {
    use smithay::reexports::wayland_server::Display;
    use smithay::utils::{Point, Size};
    use termway::compositor::renderer::TermwayRenderer;
    use termway::compositor::state::TermwayState;
    use termway::compositor::window::ManagedWindow;

    let display = Display::<TermwayState>::new().expect("Failed to create Wayland Display");
    let dh = display.handle();

    let mut state = TermwayState::new(&dh, 800, 600, true);
    state.show_splash = false;
    let mut renderer = TermwayRenderer::new(800, 600).expect("Failed to create TermwayRenderer");

    // Background SSD window (like xeyes) at (400, 100), size 200x150.
    let mut bg_xeyes = ManagedWindow::new_dummy(1, "xeyes".into(), Point::from((400, 100)), Size::from((200, 150)));
    bg_xeyes.has_ssd = true;
    bg_xeyes.active = false;
    let bg_frame = bg_xeyes.frame_geometry();
    let border_y = bg_frame.loc.y as usize;

    // Foreground CSD window (like gnome-terminal) at (50, 50), size 450x400 (x=50..500, y=50..450).
    // This overlaps the left half of xeyes's titlebar and border (x=bg_frame.loc.x..500).
    let mut fg_term = ManagedWindow::new_dummy(2, "gnome-terminal".into(), Point::from((50, 50)), Size::from((450, 400)));
    fg_term.has_ssd = false;
    fg_term.active = true;

    state.wm.windows.push(bg_xeyes);
    state.wm.windows.push(fg_term);

    let pixels = renderer.render(&state).expect("Render failed");

    let get_rgb = |x: usize, y: usize| -> (u8, u8, u8) {
        let idx = (y * 800 + x) * 4;
        (pixels[idx], pixels[idx + 1], pixels[idx + 2])
    };

    // Inside the overlapped region of xeyes's top border (x=450, y=border_y),
    // the foreground window must have occluded xeyes's inactive_border [45, 48, 55].
    let (r_occ, g_occ, b_occ) = get_rgb(450, border_y);
    assert!(
        !(r_occ == 45 && g_occ == 48 && b_occ == 55),
        "Background SSD window top border at (450, {}) should be occluded by foreground window, got {:?}",
        border_y,
        (r_occ, g_occ, b_occ)
    );

    // Outside the foreground window (x=550, y=border_y), xeyes's top border [45, 48, 55] MUST still be visible.
    let (r_vis, g_vis, b_vis) = get_rgb(550, border_y);
    assert_eq!(
        (r_vis, g_vis, b_vis),
        (45, 48, 55),
        "Uncovered portion of background SSD window border at (550, {}) should remain visible",
        border_y
    );
}

#[test]
fn test_sixel_neutral_gray_quantization() {
    use termway::protocol::SixelEncoder;

    // Two rows of 4 pixels:
    // Row 0: warm light gray RGB(225, 221, 223) (previously quantized to pink #251 -> 100;85;100)
    // Row 1: neutral light gray RGB(218, 216, 218) (previously quantized to light purple #219 -> 85;85;100)
    let mut rgba = Vec::with_capacity(4 * 2 * 4);
    for _ in 0..4 {
        rgba.extend_from_slice(&[225, 221, 223, 255]);
    }
    for _ in 0..4 {
        rgba.extend_from_slice(&[218, 216, 218, 255]);
    }

    let encoded = SixelEncoder::encode_fullscreen_frame(&rgba, 4, 2);
    // Both rows have |max - min| <= 12 and G in 192..=223 (g=6 -> 85%), so both map to neutral gray #219;2;85;85;85
    assert!(
        encoded.contains("#219;2;85;85;85"),
        "Expected neutral gray palette #219;2;85;85;85 in Sixel output, got: {}",
        encoded
    );
    assert!(
        !encoded.contains(";2;100;85;100") && !encoded.contains(";2;85;85;100"),
        "Sixel output should not contain pink (100;85;100) or purple (85;85;100) for near-neutral grays: {}",
        encoded
    );
}

#[test]
fn test_utf8_japanese_input_xkb_keymap_and_text_input_v3() {
    use smithay::input::keyboard::Keycode;
    use smithay::reexports::wayland_server::Display;
    use termway::compositor::input::dispatch_utf8_text;
    use termway::compositor::state::TermwayState;

    let display = Display::<TermwayState>::new().expect("Failed to create Wayland Display");
    let dh = display.handle();

    let mut state = TermwayState::new(&dh, 800, 600, true);
    assert!(
        state.unicode_key_slots.len() >= 32,
        "Expected at least 32 spare XKB keycode slots for Unicode input, got {}",
        state.unicode_key_slots.len()
    );
    assert!(!state.base_keymap_string.is_empty());

    // Dispatch Japanese string (Hiragana, Katakana, Kanji, Emoji)
    let sample = "日本語あいうアイウ🎉";
    dispatch_utf8_text(&mut state, sample);

    // Verify that each unique character in `sample` is mapped to a distinct XKB keycode
    // and that the compiled XKB state translates each keycode back to the exact UTF-8 character.
    let keyboard = state.keyboard.clone();
    for expected_ch in sample.chars() {
        let slot = state
            .unicode_key_slots
            .iter()
            .find(|s| s.mapped_char == Some(expected_ch))
            .cloned()
            .unwrap_or_else(|| panic!("Character {:?} was not mapped to any UnicodeKeySlot", expected_ch));
        let kc = Keycode::new(slot.keycode);
        let actual_utf8 = keyboard.with_xkb_state(&mut state, |context| {
            let xkb = context.xkb().lock().unwrap();
            let xkb_state = unsafe { xkb.state() };
            xkb_state.key_get_utf8(kc)
        });
        assert_eq!(
            actual_utf8,
            expected_ch.to_string(),
            "XKB keycode {} (<{}>) should produce {:?}, got {:?}",
            slot.keycode,
            slot.xkb_name,
            expected_ch,
            actual_utf8
        );
    }
}

#[test]
fn test_fractional_scale_state_and_output_scale() {
    let display = smithay::reexports::wayland_server::Display::<TermwayState>::new()
        .expect("Failed to create Wayland Display");
    let dh = display.handle();

    // 1920x1080 physical display at 1.5x fractional scale
    let state = TermwayState::with_scale(&dh, 1920, 1080, true, 1.5);
    assert!((state.scale - 1.5).abs() < 1e-5);
    assert_eq!(state.physical_width, 1920);
    assert_eq!(state.physical_height, 1080);
    // Logical dimensions: 1920 / 1.5 = 1280, 1080 / 1.5 = 720
    assert_eq!(state.width, 1280);
    assert_eq!(state.height, 720);
    assert!((state.output.current_scale().fractional_scale() - 1.5).abs() < 1e-5);
    assert!(state.splash_info.contains("@1.50x"));
}

#[test]
fn test_integer_scale_state_and_output_scale() {
    let display = smithay::reexports::wayland_server::Display::<TermwayState>::new()
        .expect("Failed to create Wayland Display");
    let dh = display.handle();

    // 1600x1200 physical display at 2.0x integer scale
    let state = TermwayState::with_scale(&dh, 1600, 1200, true, 2.0);
    assert!((state.scale - 2.0).abs() < 1e-5);
    assert_eq!(state.physical_width, 1600);
    assert_eq!(state.physical_height, 1200);
    assert_eq!(state.width, 800);
    assert_eq!(state.height, 600);
    assert_eq!(state.output.current_scale().integer_scale(), 2);
    assert!(state.splash_info.contains("@2.00x"));
}

#[test]
fn test_engine_config_scale_and_cli() {
    use termway::engine::EngineConfig;

    let cfg = EngineConfig::default();
    assert_eq!(cfg.scale, 1.0);

    let mut custom_cfg = EngineConfig::default();
    custom_cfg.scale = 1.25;
    assert_eq!(custom_cfg.scale, 1.25);
}

#[test]
fn test_renderer_with_fractional_scale() {
    use termway::compositor::window::ManagedWindow;

    let display = smithay::reexports::wayland_server::Display::<TermwayState>::new()
        .expect("Failed to create Wayland Display");
    let dh = display.handle();

    let mut state = TermwayState::with_scale(&dh, 600, 400, true, 1.5);
    state.show_splash = false;
    let mut renderer = TermwayRenderer::new(600, 400).expect("Renderer should initialize");

    let win_id = state.wm.alloc_id();
    let mut win = ManagedWindow::new_dummy(
        win_id,
        "Scaled Window".into(),
        (50, 50).into(),
        (200, 150).into(),
    );
    win.active = true;
    state.wm.windows.push(win);

    let pixels = renderer.render(&state).expect("Render with scale should succeed");
    assert_eq!(pixels.len(), 600 * 400 * 4);
}
