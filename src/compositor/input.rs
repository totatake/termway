use smithay::{
    backend::input::{Axis, AxisSource, ButtonState, KeyState, Keycode},
    input::{
        keyboard::FilterResult,
        pointer::{AxisFrame, ButtonEvent, MotionEvent},
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER},
};
use std::process::Child;

use super::state::TermwayState;
use super::window::{WindowDragState, WindowSurface, BORDER_WIDTH, TITLEBAR_HEIGHT};
use crate::protocol::{KeyCode, MouseAction, MouseButton, MouseEvent};

/// Map KeyCode / Char to Linux evdev scancode and modifier requirements (need_shift, need_ctrl).
pub fn map_keycode_to_evdev(code: KeyCode) -> Option<(u32, bool, bool)> {
    let result = match code {
        KeyCode::Char(c) => match c {
            'a'..='z' => {
                let ev = match c {
                    'a' => 30, 'b' => 48, 'c' => 46, 'd' => 32, 'e' => 18,
                    'f' => 33, 'g' => 34, 'h' => 35, 'i' => 23, 'j' => 36,
                    'k' => 37, 'l' => 38, 'm' => 50, 'n' => 49, 'o' => 24,
                    'p' => 25, 'q' => 16, 'r' => 19, 's' => 31, 't' => 20,
                    'u' => 22, 'v' => 47, 'w' => 17, 'x' => 45, 'y' => 21,
                    'z' => 44,
                    _ => unreachable!(),
                };
                (ev, false, false)
            }
            'A'..='Z' => {
                let ev = match c.to_ascii_lowercase() {
                    'a' => 30, 'b' => 48, 'c' => 46, 'd' => 32, 'e' => 18,
                    'f' => 33, 'g' => 34, 'h' => 35, 'i' => 23, 'j' => 36,
                    'k' => 37, 'l' => 38, 'm' => 50, 'n' => 49, 'o' => 24,
                    'p' => 25, 'q' => 16, 'r' => 19, 's' => 31, 't' => 20,
                    'u' => 22, 'v' => 47, 'w' => 17, 'x' => 45, 'y' => 21,
                    'z' => 44,
                    _ => unreachable!(),
                };
                (ev, true, false)
            }
            '1' => (2, false, false), '2' => (3, false, false), '3' => (4, false, false),
            '4' => (5, false, false), '5' => (6, false, false), '6' => (7, false, false),
            '7' => (8, false, false), '8' => (9, false, false), '9' => (10, false, false),
            '0' => (11, false, false),
            '!' => (2, true, false), '@' => (3, true, false), '#' => (4, true, false),
            '$' => (5, true, false), '%' => (6, true, false), '^' => (7, true, false),
            '&' => (8, true, false), '*' => (9, true, false), '(' => (10, true, false),
            ')' => (11, true, false),
            '-' => (12, false, false), '_' => (12, true, false),
            '=' => (13, false, false), '+' => (13, true, false),
            '[' => (26, false, false), '{' => (26, true, false),
            ']' => (27, false, false), '}' => (27, true, false),
            '\\' => (43, false, false), '|' => (43, true, false),
            ';' => (39, false, false), ':' => (39, true, false),
            '\'' => (40, false, false), '"' => (40, true, false),
            '`' => (41, false, false), '~' => (41, true, false),
            ',' => (51, false, false), '<' => (51, true, false),
            '.' => (52, false, false), '>' => (52, true, false),
            '/' => (53, false, false), '?' => (53, true, false),
            ' ' => (57, false, false),
            '\n' | '\r' => (28, false, false),
            '\t' => (15, false, false),
            _ => return None,
        },
        KeyCode::Enter => (28, false, false),
        KeyCode::Backspace => (14, false, false),
        KeyCode::Tab => (15, false, false),
        KeyCode::Escape => (1, false, false),
        KeyCode::Up => (103, false, false),
        KeyCode::Down => (108, false, false),
        KeyCode::Left => (105, false, false),
        KeyCode::Right => (106, false, false),
        KeyCode::Home => (102, false, false),
        KeyCode::End => (107, false, false),
        KeyCode::PageUp => (104, false, false),
        KeyCode::PageDown => (109, false, false),
        KeyCode::Delete => (111, false, false),
        KeyCode::F12 => (88, false, false),
        KeyCode::Ctrl(c) => match c.to_ascii_lowercase() {
            'a' => (30, false, true), 'b' => (48, false, true), 'c' => (46, false, true), 'd' => (32, false, true),
            'e' => (18, false, true), 'f' => (33, false, true), 'g' => (34, false, true), 'h' => (35, false, true),
            'i' => (23, false, true), 'j' => (36, false, true), 'k' => (37, false, true), 'l' => (38, false, true),
            'm' => (50, false, true), 'n' => (49, false, true), 'o' => (24, false, true), 'p' => (25, false, true),
            'q' => (16, false, true), 'r' => (19, false, true), 's' => (31, false, true), 't' => (20, false, true),
            'u' => (22, false, true), 'v' => (47, false, true), 'w' => (17, false, true), 'x' => (45, false, true),
            'y' => (21, false, true), 'z' => (44, false, true),
            _ => return None,
        },
        KeyCode::ShiftUp => (103, true, false),
        KeyCode::ShiftDown => (108, true, false),
        KeyCode::ShiftLeft => (105, true, false),
        KeyCode::ShiftRight => (106, true, false),
        KeyCode::ShiftHome => (102, true, false),
        KeyCode::ShiftEnd => (107, true, false),
        KeyCode::ShiftPageUp => (104, true, false),
        KeyCode::ShiftPageDown => (109, true, false),
        KeyCode::ShiftDelete => (111, true, false),
        KeyCode::ShiftTab => (15, true, false),
        KeyCode::CtrlUp => (103, false, true),
        KeyCode::CtrlDown => (108, false, true),
        KeyCode::CtrlLeft => (105, false, true),
        KeyCode::CtrlRight => (106, false, true),
        KeyCode::CtrlHome => (102, false, true),
        KeyCode::CtrlEnd => (107, false, true),
        KeyCode::CtrlShiftUp => (103, true, true),
        KeyCode::CtrlShiftDown => (108, true, true),
        KeyCode::CtrlShiftLeft => (105, true, true),
        KeyCode::CtrlShiftRight => (106, true, true),
        KeyCode::CtrlShiftHome => (102, true, true),
        KeyCode::CtrlShiftEnd => (107, true, true),
        KeyCode::Ignore => return None,
    };
    Some(result)
}

/// Dispatches a key event to the compositor's seat keyboard.
pub fn dispatch_key_event(state: &mut TermwayState, code: KeyCode) {
    let (evdev_code, need_shift, need_ctrl) = match map_keycode_to_evdev(code) {
        Some(mapping) => mapping,
        None => return,
    };

    let mut time = state.start_time.elapsed().as_millis() as u32;

    // Smithay Keycode expects XKB keycode (evdev scancode + 8).
    let key = Keycode::new(evdev_code + 8);
    let shift_key = Keycode::new(42 + 8); // KEY_LEFTSHIFT = 42
    let ctrl_key = Keycode::new(29 + 8);  // KEY_LEFTCTRL = 29
    let keyboard = state.keyboard.clone();

    if need_shift {
        let shift_serial = SERIAL_COUNTER.next_serial();
        keyboard.input::<(), _>(
            state,
            shift_key,
            KeyState::Pressed,
            shift_serial,
            time,
            |_, _, _| FilterResult::Forward,
        );
        time += 1;
    }

    if need_ctrl {
        let ctrl_serial = SERIAL_COUNTER.next_serial();
        keyboard.input::<(), _>(
            state,
            ctrl_key,
            KeyState::Pressed,
            ctrl_serial,
            time,
            |_, _, _| FilterResult::Forward,
        );
        time += 1;
    }

    // Send Press then Release
    let press_serial = SERIAL_COUNTER.next_serial();
    keyboard.input::<(), _>(
        state,
        key,
        KeyState::Pressed,
        press_serial,
        time,
        |_, _, _| FilterResult::Forward,
    );
    time += 1;

    let release_serial = SERIAL_COUNTER.next_serial();
    keyboard.input::<(), _>(
        state,
        key,
        KeyState::Released,
        release_serial,
        time,
        |_, _, _| FilterResult::Forward,
    );
    time += 1;

    if need_ctrl {
        let ctrl_release_serial = SERIAL_COUNTER.next_serial();
        keyboard.input::<(), _>(
            state,
            ctrl_key,
            KeyState::Released,
            ctrl_release_serial,
            time,
            |_, _, _| FilterResult::Forward,
        );
        time += 1;
    }

    if need_shift {
        let shift_release_serial = SERIAL_COUNTER.next_serial();
        keyboard.input::<(), _>(
            state,
            shift_key,
            KeyState::Released,
            shift_release_serial,
            time,
            |_, _, _| FilterResult::Forward,
        );
    }
}

/// Helper to find any active popup at loc_i32 (Wayland XDG popups or unmanaged X11 popups)
fn find_popup_focus(
    state: &TermwayState,
    loc_i32: Point<i32, Logical>,
) -> Option<(WlSurface, Point<f64, Logical>)> {
    // 1. Wayland XDG popups (topmost)
    for win in state.managed_windows.iter().rev() {
        if let WindowSurface::Wayland(ref toplevel) = win.surface {
            for (popup, popup_offset) in smithay::desktop::PopupManager::popups_for_surface(toplevel.wl_surface()) {
                let popup_loc = Point::from((
                    win.loc.x + popup_offset.x - popup.geometry().loc.x,
                    win.loc.y + popup_offset.y - popup.geometry().loc.y,
                ));
                if let Some((surf, pt)) = smithay::desktop::utils::under_from_surface_tree(
                    popup.wl_surface(),
                    loc_i32.to_f64(),
                    popup_loc,
                    smithay::desktop::WindowSurfaceType::ALL,
                ) {
                    return Some((surf, pt.to_f64()));
                }
                let bbox = smithay::desktop::utils::bbox_from_surface_tree(popup.wl_surface(), (0, 0));
                let popup_geo = Rectangle::new(popup_loc, bbox.size);
                if popup_geo.contains(loc_i32) {
                    return Some((
                        popup.wl_surface().clone(),
                        Point::from((popup_loc.x as f64, popup_loc.y as f64)),
                    ));
                }
            }
        }
    }

    // 2. Unmanaged X11 override-redirect popups (topmost)
    for xwin in state.x11_windows.iter().rev() {
        let is_managed = state.managed_windows.iter().any(|mw| matches!(&mw.surface, WindowSurface::X11(w) if w == xwin));
        if !is_managed {
            let geo = xwin.geometry();
            if geo.contains(loc_i32) {
                if let Some(s) = state.surface_for_x11(xwin) {
                    return Some((s, Point::from((geo.loc.x as f64, geo.loc.y as f64))));
                }
            }
        }
    }

    None
}

/// Helper to find pointer focus across popups, managed windows, and unassociated surfaces
fn find_pointer_focus(
    state: &TermwayState,
    loc_i32: Point<i32, Logical>,
) -> Option<(WlSurface, Point<f64, Logical>)> {
    if let Some(popup) = find_popup_focus(state, loc_i32) {
        return Some(popup);
    }

    // Check managed windows content
    for win in state.managed_windows.iter().rev() {
        if win.is_minimized {
            continue;
        }
        let content_geo = win.content_geometry();
        if content_geo.contains(loc_i32) {
            if let Some(surface) = state.surface_for_managed_window(win) {
                return Some((surface, Point::from((win.loc.x as f64, win.loc.y as f64))));
            }
        }
    }

    // Fallback: unassociated surfaces (do not focus if all managed windows are minimized)
    let any_managed = !state.managed_windows.is_empty();
    let all_managed_minimized = any_managed && state.managed_windows.iter().all(|w| w.is_minimized);
    if !all_managed_minimized {
        let minimized_surfaces: Vec<WlSurface> = state
            .managed_windows
            .iter()
            .filter(|w| w.is_minimized)
            .filter_map(|w| state.surface_for_managed_window(w))
            .collect();

        if let Some(surface) = state.unassociated_surfaces.iter().rev().find(|s| !minimized_surfaces.contains(s)) {
            return Some((surface.clone(), Point::from((0.0, 0.0))));
        }
    }

    None
}

/// Dispatches a mouse event (motion, press, release) to the compositor.
pub fn dispatch_mouse_event(
    state: &mut TermwayState,
    renderer_width: u32,
    renderer_height: u32,
    term_cols: u16,
    avail_rows: u16,
    ev: &MouseEvent,
    client_child: &mut Option<Child>,
) {
    let serial = SERIAL_COUNTER.next_serial();
    let time = state.start_time.elapsed().as_millis() as u32;

    let cell_w = renderer_width as f64 / term_cols.max(1) as f64;
    let cell_h = renderer_height as f64 / avail_rows.max(1) as f64;
    let px = (ev.col.saturating_sub(1)) as f64 * cell_w + (cell_w * 0.5);
    let py = (ev.row.saturating_sub(1)) as f64 * cell_h + (cell_h * 0.5);
    let loc: Point<f64, Logical> = Point::from((px, py));
    let loc_i32 = Point::<i32, Logical>::from((px as i32, py as i32));

    let pointer = state.pointer.clone();
    let keyboard = state.keyboard.clone();

    match ev.action {
        MouseAction::Move | MouseAction::Drag => {
            // 1. Process active window dragging (Moving or Resizing)
            match state.drag_state {
                WindowDragState::Moving { start_cursor, start_window_pos } => {
                    let dx = (px - start_cursor.0).round() as i32;
                    let dy = (py - start_cursor.1).round() as i32;
                    if dx.abs() > 3 || dy.abs() > 3 {
                        state.last_titlebar_click = None;
                    }
                    if let Some(top_win) = state.managed_windows.last_mut() {
                        let min_x = if top_win.has_ssd { BORDER_WIDTH } else { 0 };
                        let max_x = (renderer_width as i32 - top_win.size.w - min_x).max(min_x);
                        let min_y = if top_win.has_ssd { TITLEBAR_HEIGHT + BORDER_WIDTH } else { 0 };
                        let max_y = (renderer_height as i32 - top_win.size.h - min_x).max(min_y);

                        let new_x = (start_window_pos.0 + dx).clamp(min_x, max_x);
                        let new_y = (start_window_pos.1 + dy).clamp(min_y, max_y);
                        top_win.set_loc(Point::from((new_x, new_y)));
                        state.dirty = true;
                    }
                    return;
                }
                WindowDragState::Resizing { start_cursor, start_window_geo } => {
                    let dw = (px - start_cursor.0).round() as i32;
                    let dh = (py - start_cursor.1).round() as i32;
                    if let Some(top_win) = state.managed_windows.last_mut() {
                        let max_w = (renderer_width as i32 - top_win.loc.x - BORDER_WIDTH).max(100);
                        let max_h = (renderer_height as i32 - top_win.loc.y - BORDER_WIDTH).max(60);

                        let new_w = (start_window_geo.size.w + dw).clamp(100, max_w);
                        let new_h = (start_window_geo.size.h + dh).clamp(60, max_h);
                        top_win.configure_size(new_w, new_h);
                        state.dirty = true;
                    }
                    return;
                }
                WindowDragState::None => {}
            }

            // 2. Normal pointer motion: check popups, managed windows, and fallback
            let focus = find_pointer_focus(state, loc_i32);
            let motion = MotionEvent { location: loc, serial, time };
            pointer.motion(state, focus, &motion);
            pointer.frame(state);
        }

        MouseAction::Press => {
            // Handle mouse wheel scrolling (Wayland pointer axis scroll events)
            if matches!(ev.button, MouseButton::WheelUp | MouseButton::WheelDown) {
                let focus = find_pointer_focus(state, loc_i32);
                let motion = MotionEvent { location: loc, serial, time };
                pointer.motion(state, focus, &motion);

                let value = if matches!(ev.button, MouseButton::WheelUp) { -15.0 } else { 15.0 };
                let v120 = if matches!(ev.button, MouseButton::WheelUp) { -120 } else { 120 };

                let frame = AxisFrame::new(time)
                    .source(AxisSource::Wheel)
                    .value(Axis::Vertical, value)
                    .v120(Axis::Vertical, v120);
                pointer.axis(state, frame);
                pointer.frame(state);
                state.dirty = true;
                return;
            }

            let is_left = matches!(ev.button, MouseButton::Left);

            // 0. Check popups first (topmost over all windows)
            if let Some((popup_surface, origin)) = find_popup_focus(state, loc_i32) {
                let focus = Some((popup_surface.clone(), origin));
                let motion = MotionEvent { location: loc, serial, time };
                pointer.motion(state, focus.clone(), &motion);

                let btn = match ev.button {
                    MouseButton::Left => 0x110,
                    MouseButton::Middle => 0x112,
                    MouseButton::Right => 0x111,
                    _ => 0x110,
                };
                let btn_ev = ButtonEvent { serial, time, button: btn, state: ButtonState::Pressed };
                pointer.button(state, &btn_ev);
                pointer.frame(state);

                // xdg_popup にはキーボードフォーカスを移さない (state.rs new_popup 参照)
                let is_xdg_popup = smithay::wayland::compositor::get_role(&popup_surface)
                    == Some(smithay::wayland::shell::xdg::XDG_POPUP_ROLE);
                if !is_xdg_popup {
                    keyboard.set_focus(state, Some(popup_surface), serial);
                }
                return;
            }

            // Clicked outside popups: dismiss any active Wayland popups
            for win in &state.managed_windows {
                if let WindowSurface::Wayland(ref toplevel) = win.surface {
                    for (popup, _) in smithay::desktop::PopupManager::popups_for_surface(toplevel.wl_surface()) {
                        if let smithay::desktop::PopupKind::Xdg(ref p) = popup {
                            p.send_popup_done();
                        }
                    }
                }
            }

            // Hit test managed windows from front to back
            let mut hit_window_id = None;
            let mut clicked_close = false;
            let mut clicked_maximize = false;
            let mut clicked_minimize = false;
            let mut clicked_titlebar = false;
            let mut clicked_resize = false;
            let mut clicked_content = false;
            let mut win_loc = Point::from((0, 0));
            let mut win_geo = smithay::utils::Rectangle::new((0, 0).into(), (0, 0).into());

            for win in state.managed_windows.iter().rev() {
                if win.is_minimized {
                    continue;
                }
                if state.enable_wm && win.has_ssd {
                    if is_left && win.close_button_hit_geometry().contains(loc_i32) {
                        hit_window_id = Some(win.id);
                        clicked_close = true;
                        break;
                    }
                    if is_left && win.maximize_button_hit_geometry().contains(loc_i32) {
                        hit_window_id = Some(win.id);
                        clicked_maximize = true;
                        break;
                    }
                    if is_left && win.minimize_button_hit_geometry().contains(loc_i32) {
                        hit_window_id = Some(win.id);
                        clicked_minimize = true;
                        break;
                    }
                    if is_left && !win.is_maximized && win.resize_handle_hit_geometry().contains(loc_i32) {
                        hit_window_id = Some(win.id);
                        clicked_resize = true;
                        win_geo = win.content_geometry();
                        break;
                    }
                    if win.titlebar_geometry().contains(loc_i32) {
                        hit_window_id = Some(win.id);
                        clicked_titlebar = true;
                        win_loc = win.loc;
                        break;
                    }
                }
                if win.content_geometry().contains(loc_i32) {
                    hit_window_id = Some(win.id);
                    clicked_content = true;
                    win_loc = win.loc;
                    break;
                }
            }

            if let Some(id) = hit_window_id {
                if clicked_close {
                    // Close target window
                    if let Some(pos) = state.managed_windows.iter().position(|w| w.id == id) {
                        let win = state.managed_windows.remove(pos);
                        win.close();

                        // If this was an X11 surface, send SIGTERM to its process if PID is available
                        if let WindowSurface::X11(ref xwin) = win.surface {
                            if let Ok(pid) = xwin.get_client_pid() {
                                if pid > 0 {
                                    let _ = nix::sys::signal::kill(
                                        nix::unistd::Pid::from_raw(pid as i32),
                                        nix::sys::signal::Signal::SIGTERM,
                                    );
                                }
                            }
                        }

                        // If all managed windows closed, terminate compositor or client process
                        if state.managed_windows.is_empty() {
                            if let Some(ref mut child) = client_child {
                                let _ = child.kill();
                            }
                            state.running = false;
                        } else {
                            if let Some(top) = state.managed_windows.last_mut() {
                                top.active = true;
                            }
                            if let Some(top) = state.managed_windows.last() {
                                if let Some(s) = state.surface_for_managed_window(top) {
                                    keyboard.set_focus(state, Some(s), serial);
                                }
                            }
                        }
                        state.dirty = true;
                    }
                    return;
                }

                if clicked_maximize {
                    if let Some(win) = state.managed_windows.iter_mut().find(|w| w.id == id) {
                        win.toggle_maximize(renderer_width, renderer_height);
                    }
                    state.raise_window(id);
                    state.dirty = true;
                    return;
                }

                if clicked_minimize {
                    state.minimize_window(id);
                    return;
                }

                // Raise clicked window to top of Z-order & activate
                state.raise_window(id);

                if clicked_titlebar && is_left {
                    // Check titlebar double-click for maximize/restore toggle
                    let now = std::time::Instant::now();
                    if let Some((last_time, last_id)) = state.last_titlebar_click {
                        if last_id == id && now.duration_since(last_time).as_millis() < 400 {
                            if let Some(win) = state.managed_windows.iter_mut().find(|w| w.id == id) {
                                win.toggle_maximize(renderer_width, renderer_height);
                            }
                            state.last_titlebar_click = None;
                            state.drag_state = WindowDragState::None;
                            state.dirty = true;
                            return;
                        }
                    }
                    state.last_titlebar_click = Some((now, id));

                    // Begin window drag/move only if not maximized
                    let is_max = state.managed_windows.iter().find(|w| w.id == id).map(|w| w.is_maximized).unwrap_or(false);
                    if !is_max {
                        state.drag_state = WindowDragState::Moving {
                            start_cursor: (px, py),
                            start_window_pos: (win_loc.x, win_loc.y),
                        };
                    }
                    state.dirty = true;
                    return;
                }

                if clicked_resize && is_left {
                    state.drag_state = WindowDragState::Resizing {
                        start_cursor: (px, py),
                        start_window_geo: win_geo,
                    };
                    state.dirty = true;
                    return;
                }

                if clicked_content {
                    // Forward press to client surface
                    let win = state.managed_windows.last().unwrap();
                    let focus = state.surface_for_managed_window(win).map(|s| (s, Point::from((win.loc.x as f64, win.loc.y as f64))));

                    let motion = MotionEvent { location: loc, serial, time };
                    pointer.motion(state, focus.clone(), &motion);

                    let btn = match ev.button {
                        MouseButton::Left => 0x110,
                        MouseButton::Middle => 0x112,
                        MouseButton::Right => 0x111,
                        _ => 0x110,
                    };
                    let btn_ev = ButtonEvent { serial, time, button: btn, state: ButtonState::Pressed };
                    pointer.button(state, &btn_ev);
                    pointer.frame(state);

                    if let Some((s, _)) = focus {
                        keyboard.set_focus(state, Some(s), serial);
                    }
                    return;
                }
            }

            // Fallback: outside managed windows
            let focus = find_pointer_focus(state, loc_i32);
            let motion = MotionEvent { location: loc, serial, time };
            pointer.motion(state, focus.clone(), &motion);
            let btn = match ev.button {
                MouseButton::Left => 0x110,
                MouseButton::Middle => 0x112,
                MouseButton::Right => 0x111,
                _ => 0x110,
            };
            let btn_ev = ButtonEvent { serial, time, button: btn, state: ButtonState::Pressed };
            pointer.button(state, &btn_ev);
            pointer.frame(state);

            if let Some((s, _)) = focus {
                keyboard.set_focus(state, Some(s), serial);
            }
        }

        MouseAction::Release => {
            if matches!(ev.button, MouseButton::WheelUp | MouseButton::WheelDown) {
                return;
            }

            let was_dragging = !matches!(state.drag_state, WindowDragState::None);
            state.drag_state = WindowDragState::None;

            if was_dragging {
                state.dirty = true;
                return;
            }

            // Forward normal release event
            let focus = find_pointer_focus(state, loc_i32);
            let motion = MotionEvent { location: loc, serial, time };
            pointer.motion(state, focus, &motion);
            let btn = match ev.button {
                MouseButton::Left => 0x110,
                MouseButton::Middle => 0x112,
                MouseButton::Right => 0x111,
                _ => 0x110,
            };
            let btn_ev = ButtonEvent { serial, time, button: btn, state: ButtonState::Released };
            pointer.button(state, &btn_ev);
            pointer.frame(state);
        }
    }
}

