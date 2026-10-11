use std::os::unix::io::OwnedFd;
use smithay::{
    delegate_xwayland_shell,
    utils::{Logical, Rectangle},
    wayland::{
        selection::SelectionTarget,
        xwayland_shell::{XWaylandShellHandler, XWaylandShellState},
    },
    xwayland::{
        xwm::{Reorder, ResizeEdge, X11Wm, XwmHandler, XwmId},
        X11Surface,
    },
};

use super::state::TermwayState;
use super::window::{ManagedWindow, WindowDragState, WindowSurface, BORDER_WIDTH, TITLEBAR_HEIGHT};

impl XWaylandShellHandler for TermwayState {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland_shell_state
    }
}

impl XwmHandler for TermwayState {
    fn xwm_state(&mut self, _xwm: XwmId) -> &mut X11Wm {
        self.xwm.as_mut().expect("X11Wm not initialized")
    }

    fn new_window(&mut self, _xwm: XwmId, _window: X11Surface) {
        crate::compositor::log_line("[xwayland] new_window");
    }
    fn new_override_redirect_window(&mut self, _xwm: XwmId, window: X11Surface) {
        crate::compositor::log_line(format!("[xwayland] new_override_redirect_window id={:?} geo={:?}", window.window_id(), window.geometry()));
    }

    fn map_window_request(&mut self, _xwm: XwmId, window: X11Surface) {
        crate::compositor::log_line(format!("[xwayland] map_window_request id={:?} geo={:?}", window.window_id(), window.geometry()));
        let _ = window.set_mapped(true);
        let _ = window.set_activated(true);

        let wl_surface = self.surface_for_x11(&window);
        if let Some(wl) = wl_surface {
            let keyboard = self.keyboard.clone();
            keyboard.set_focus(
                self,
                Some(wl),
                smithay::utils::SERIAL_COUNTER.next_serial(),
            );
        }

        if !self.x11_windows.iter().any(|w| w == &window) {
            self.x11_windows.push(window.clone());
        }

        // Manage as Openbox/Fluxbox managed window if not override-redirect
        if !window.is_override_redirect() {
            let already_managed = self.wm.windows.iter().any(|w| matches!(&w.surface, WindowSurface::X11(x) if x == &window));
            if !already_managed {
                let id = self.wm.alloc_id();
                let geo = window.geometry();
                let is_first_window = self.wm.windows.is_empty();
                let is_transient_or_popup = window.is_popup() || window.is_transient_for().is_some();
                let should_maximize = is_first_window || !is_transient_or_popup;
                let has_ssd = !window.is_decorated();
                let (loc, size, is_maximized, saved_geometry) = if self.enable_wm {
                    if should_maximize {
                        let saved_geo = ManagedWindow::default_80pct_geometry(self.width, self.height, has_ssd);
                        let (max_loc, max_w, max_h) = if has_ssd {
                            let loc = smithay::utils::Point::from((BORDER_WIDTH, TITLEBAR_HEIGHT + BORDER_WIDTH));
                            let w = (self.width as i32 - BORDER_WIDTH * 2).max(100);
                            let h = (self.height as i32 - TITLEBAR_HEIGHT - BORDER_WIDTH * 2).max(60);
                            (loc, w, h)
                        } else {
                            let loc = smithay::utils::Point::from((0, 0));
                            let w = (self.width as i32).max(100);
                            let h = (self.height as i32).max(60);
                            (loc, w, h)
                        };
                        (max_loc, smithay::utils::Size::from((max_w, max_h)), true, Some(saved_geo))
                    } else {
                        let loc = if geo.loc.x == 0 && geo.loc.y == 0 {
                            let offset = ((self.wm.windows.len() % 8) as i32) * 20;
                            smithay::utils::Point::from((10 + offset, TITLEBAR_HEIGHT + 6 + offset))
                        } else {
                            smithay::utils::Point::from((
                                geo.loc.x.max(BORDER_WIDTH),
                                geo.loc.y.max(TITLEBAR_HEIGHT + BORDER_WIDTH),
                            ))
                        };
                        let max_w = (self.width as i32 - loc.x - BORDER_WIDTH * 2 - 20).max(160);
                        let max_h = (self.height as i32 - loc.y - BORDER_WIDTH * 2 - 20).max(100);
                        let init_w = if geo.size.w > 0 { geo.size.w.min(max_w) } else { max_w };
                        let init_h = if geo.size.h > 0 { geo.size.h.min(max_h) } else { max_h };
                        (loc, smithay::utils::Size::from((init_w, init_h)), false, None)
                    }
                } else {
                    (
                        smithay::utils::Point::from((0, 0)),
                        smithay::utils::Size::from((self.width as i32, self.height as i32)),
                        true,
                        None,
                    )
                };

                for w in &mut self.wm.windows {
                    w.active = false;
                }
                let mut win = ManagedWindow::new_x11(id, window.clone(), loc, size);
                win.is_maximized = is_maximized;
                win.saved_geometry = saved_geometry;
                let rect = Rectangle::new(loc, size);
                let _ = window.configure(rect);
                if is_maximized {
                    let _ = window.set_maximized(true);
                }
                self.wm.windows.push(win);

                if self.show_splash {
                    self.show_splash = false;
                    self.needs_clear = true;
                }
            }
        }

        self.dirty = true;
    }

    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, window: X11Surface) {
        crate::compositor::log_line(format!("[xwayland] mapped_override_redirect_window id={:?} geo={:?}", window.window_id(), window.geometry()));
        if !self.x11_windows.iter().any(|w| w == &window) {
            self.x11_windows.push(window);
        }
        self.dirty = true;
    }

    fn unmapped_window(&mut self, _xwm: XwmId, window: X11Surface) {
        #[allow(deprecated)]
        if let Some(wid) = window.wl_surface_id() {
            use smithay::reexports::wayland_server::Resource;
            self.unassociated_surfaces.retain(|s| s.id().protocol_id() != wid);
        }
        self.x11_windows.retain(|w| w != &window);
        self.wm.windows.retain(|w| !matches!(&w.surface, WindowSurface::X11(x) if x == &window));
        if let Some(top) = self.wm.windows.last_mut() {
            top.active = true;
        }
        self.dirty = true;
    }

    fn destroyed_window(&mut self, _xwm: XwmId, window: X11Surface) {
        #[allow(deprecated)]
        if let Some(wid) = window.wl_surface_id() {
            use smithay::reexports::wayland_server::Resource;
            self.unassociated_surfaces.retain(|s| s.id().protocol_id() != wid);
        }
        self.x11_windows.retain(|w| w != &window);
        self.wm.windows.retain(|w| !matches!(&w.surface, WindowSurface::X11(x) if x == &window));
        if let Some(top) = self.wm.windows.last_mut() {
            top.active = true;
        }
        self.dirty = true;
    }

    fn configure_request(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        x: Option<i32>,
        y: Option<i32>,
        w: Option<u32>,
        h: Option<u32>,
        _reorder: Option<Reorder>,
    ) {
        crate::compositor::log_line(format!(
            "[xwayland] configure_request id={:?} req=({:?},{:?},{:?},{:?}) current={:?}",
            window.window_id(), x, y, w, h, window.geometry()
        ));

        if window.is_override_redirect() {
            let _ = window.configure(None);
            self.dirty = true;
            return;
        }

        let rect = if self.enable_wm {
            let managed = self.wm.windows.iter().find(|mw| matches!(&mw.surface, WindowSurface::X11(ref xwin) if xwin == &window));
            let cur_loc = managed
                .map(|mw| mw.loc)
                .unwrap_or_else(|| {
                    smithay::utils::Point::from((
                        x.unwrap_or(BORDER_WIDTH).max(BORDER_WIDTH),
                        y.unwrap_or(TITLEBAR_HEIGHT + BORDER_WIDTH).max(TITLEBAR_HEIGHT + BORDER_WIDTH),
                    ))
                });
            let side_margin = managed.map(|mw| if mw.has_ssd { BORDER_WIDTH * 2 } else { 0 }).unwrap_or(0);
            let top_margin = managed.map(|mw| if mw.has_ssd { TITLEBAR_HEIGHT + BORDER_WIDTH * 2 } else { 0 }).unwrap_or(0);
            let mut max_w = (self.width as i32 - cur_loc.x - side_margin).max(160);
            let mut max_h = (self.height as i32 - cur_loc.y - top_margin).max(100);
            if let Some(mw) = managed {
                if mw.is_maximized {
                    // Do not allow client configure request to resize away from maximized state
                    max_w = max_w.max(mw.size.w);
                    max_h = max_h.max(mw.size.h);
                } else if mw.pending_unmaximize {
                    // Do not allow in-flight configure requests to enlarge the window back to maximized
                    let is_old_maximized = w.map(|v| v as i32 >= self.width as i32 - 10).unwrap_or(false);
                    if is_old_maximized && mw.unmaximize_in_flight_count < 30 {
                        max_w = max_w.min(mw.size.w);
                        max_h = max_h.min(mw.size.h);
                    }
                }
            }
            // Unspecified width/height keeps the current size (e.g. move-only requests)
            let cur_w = managed.map(|mw| mw.size.w).unwrap_or(max_w);
            let cur_h = managed.map(|mw| mw.size.h).unwrap_or(max_h);
            let target_w = if managed.map(|mw| mw.is_maximized).unwrap_or(false) {
                cur_w
            } else {
                w.map(|v| v as i32).unwrap_or(cur_w).min(max_w)
            };
            let target_h = if managed.map(|mw| mw.is_maximized).unwrap_or(false) {
                cur_h
            } else {
                h.map(|v| v as i32).unwrap_or(cur_h).min(max_h)
            };
            Rectangle::new(cur_loc, (target_w, target_h).into())
        } else {
            Rectangle::new(
                (0, 0).into(),
                (self.width as i32, self.height as i32).into(),
            )
        };

        let _ = window.configure(rect);
        for mw in &mut self.wm.windows {
            if matches!(&mw.surface, WindowSurface::X11(ref xwin) if xwin == &window) {
                mw.size = rect.size;
            }
        }
        self.dirty = true;
    }

    fn configure_notify(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        geometry: Rectangle<i32, Logical>,
        _above: Option<u32>,
    ) {
        crate::compositor::log_line(format!(
            "[xwayland] configure_notify id={:?} geo={:?}",
            window.window_id(), geometry
        ));
        for w in &mut self.wm.windows {
            if matches!(&w.surface, WindowSurface::X11(x) if x == &window) {
                w.update_title();
                if self.enable_wm {
                    let top_margin = if w.has_ssd { TITLEBAR_HEIGHT + BORDER_WIDTH } else { 0 };
                    let side_margin = if w.has_ssd { BORDER_WIDTH } else { 0 };
                    let mut was_pending_unmaximize = false;
                    if w.pending_unmaximize {
                        // While waiting for unmaximize to complete, ignore in-flight old maximized geometry from X11!
                        let is_old_max = (geometry.size.w >= self.width as i32 - 10 && geometry.size.h >= self.height as i32 - top_margin - 10)
                            && w.unmaximize_in_flight_count < 30;
                        if is_old_max {
                            w.unmaximize_in_flight_count += 1;
                            continue;
                        }
                        w.pending_unmaximize = false;
                        was_pending_unmaximize = true;
                    }
                    let max_w = (self.width as i32 - side_margin * 2).max(160);
                    let max_h = (self.height as i32 - top_margin - side_margin).max(100);
                    w.size = (geometry.size.w.min(max_w), geometry.size.h.min(max_h)).into();
                    if was_pending_unmaximize {
                        let total_frame_w = if w.has_ssd { w.size.w + BORDER_WIDTH * 2 } else { w.size.w };
                        let total_frame_h = if w.has_ssd { w.size.h + TITLEBAR_HEIGHT + BORDER_WIDTH * 2 } else { w.size.h };
                        let frame_x = ((self.width as i32 - total_frame_w) / 2).max(0);
                        let frame_y = ((self.height as i32 - total_frame_h) / 2).max(0);
                        w.loc.x = if w.has_ssd { frame_x + BORDER_WIDTH } else { frame_x };
                        w.loc.y = if w.has_ssd { frame_y + TITLEBAR_HEIGHT + BORDER_WIDTH } else { frame_y };
                    } else {
                        let max_x = (self.width as i32 - w.size.w - side_margin).max(side_margin);
                        let max_y = (self.height as i32 - w.size.h - side_margin).max(top_margin);
                        if w.loc.x > max_x { w.loc.x = max_x; }
                        if w.loc.y > max_y { w.loc.y = max_y; }
                    }
                } else {
                    w.loc = (0, 0).into();
                    w.size = (geometry.size.w.min(self.width as i32), geometry.size.h.min(self.height as i32)).into();
                    if geometry.size.w > self.width as i32 || geometry.size.h > self.height as i32 {
                        let rect = Rectangle::new((0, 0).into(), (self.width as i32, self.height as i32).into());
                        let _ = window.configure(rect);
                    }
                }
            }
        }
        self.dirty = true;
    }

    fn resize_request(&mut self, _xwm: XwmId, window: X11Surface, _button: u32, _resize_edge: ResizeEdge) {
        if !self.enable_wm {
            return;
        }
        if let Some(win) = self.wm.windows.iter().find(|w| match &w.surface {
            WindowSurface::X11(t) => t == &window,
            _ => false,
        }) {
            let cur_loc = self.pointer.current_location();
            let win_geo = win.content_geometry();
            self.wm.drag_state = WindowDragState::Resizing {
                start_cursor: (cur_loc.x, cur_loc.y),
                start_window_geo: win_geo,
            };
        }
    }

    fn move_request(&mut self, _xwm: XwmId, window: X11Surface, _button: u32) {
        if !self.enable_wm {
            return;
        }
        if let Some(win) = self.wm.windows.iter().find(|w| match &w.surface {
            WindowSurface::X11(t) => t == &window,
            _ => false,
        }) {
            let cur_loc = self.pointer.current_location();
            let win_loc = win.loc;
            self.wm.drag_state = WindowDragState::Moving {
                start_cursor: (cur_loc.x, cur_loc.y),
                start_window_pos: (win_loc.x, win_loc.y),
            };
        }
    }

    fn maximize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Some(win) = self.wm.windows.iter_mut().find(|w| match &w.surface {
            WindowSurface::X11(t) => t == &window,
            _ => false,
        }) {
            win.maximize(self.width, self.height);
            let _ = window.set_maximized(true);
        }
        self.dirty = true;
    }

    fn unmaximize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if self.enable_wm {
            if let Some(win) = self.wm.windows.iter_mut().find(|w| match &w.surface {
                WindowSurface::X11(t) => t == &window,
                _ => false,
            }) {
                win.unmaximize(self.width, self.height);
                let _ = window.set_maximized(false);
            }
        }
        self.dirty = true;
    }

    fn minimize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Some(id) = self.wm.windows.iter().find(|w| match &w.surface {
            WindowSurface::X11(t) => t == &window,
            _ => false,
        }).map(|w| w.id) {
            self.minimize_window(id);
        }
    }

    fn unminimize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Some(id) = self.wm.windows.iter().find(|w| match &w.surface {
            WindowSurface::X11(t) => t == &window,
            _ => false,
        }).map(|w| w.id) {
            self.restore_window(id);
        }
    }

    fn send_selection(&mut self, _xwm: XwmId, _selection: SelectionTarget, _mime_type: String, _fd: OwnedFd) {}
}

delegate_xwayland_shell!(TermwayState);
