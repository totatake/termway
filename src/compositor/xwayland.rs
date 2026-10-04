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
use super::window::{ManagedWindow, WindowSurface, BORDER_WIDTH, TITLEBAR_HEIGHT};

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
    fn new_override_redirect_window(&mut self, _xwm: XwmId, _window: X11Surface) {
        crate::compositor::log_line("[xwayland] new_override_redirect_window");
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
            let already_managed = self.managed_windows.iter().any(|w| matches!(&w.surface, WindowSurface::X11(x) if x == &window));
            if !already_managed {
                let id = self.next_window_id;
                self.next_window_id += 1;
                let geo = window.geometry();
                let (loc, size) = if self.enable_wm {
                    let loc = if geo.loc.x == 0 && geo.loc.y == 0 {
                        let offset = ((self.managed_windows.len() % 8) as i32) * 20;
                        smithay::utils::Point::from((10 + offset, TITLEBAR_HEIGHT + 6 + offset))
                    } else {
                        smithay::utils::Point::from((
                            geo.loc.x.max(BORDER_WIDTH),
                            geo.loc.y.max(TITLEBAR_HEIGHT + BORDER_WIDTH),
                        ))
                    };
                    let max_w = (self.width as i32 - loc.x - BORDER_WIDTH * 2 - 20).max(160);
                    let max_h = (self.height as i32 - loc.y - BORDER_WIDTH * 2 - 20).max(100);
                    let init_w = if geo.size.w > 0 { (geo.size.w as i32).min(max_w) } else { max_w };
                    let init_h = if geo.size.h > 0 { (geo.size.h as i32).min(max_h) } else { max_h };
                    (loc, smithay::utils::Size::from((init_w, init_h)))
                } else {
                    (
                        smithay::utils::Point::from((0, 0)),
                        smithay::utils::Size::from((self.width as i32, self.height as i32)),
                    )
                };

                for w in &mut self.managed_windows {
                    w.active = false;
                }
                let win = ManagedWindow::new_x11(id, window.clone(), loc, size);
                let rect = Rectangle::new(loc, size);
                let _ = window.configure(rect);
                self.managed_windows.push(win);
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
        self.managed_windows.retain(|w| !matches!(&w.surface, WindowSurface::X11(x) if x == &window));
        if let Some(top) = self.managed_windows.last_mut() {
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
        self.managed_windows.retain(|w| !matches!(&w.surface, WindowSurface::X11(x) if x == &window));
        if let Some(top) = self.managed_windows.last_mut() {
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

        let rect = if self.enable_wm {
            let cur_loc = self.managed_windows.iter().find(|mw| matches!(&mw.surface, WindowSurface::X11(ref xwin) if xwin == &window))
                .map(|mw| mw.loc)
                .unwrap_or_else(|| {
                    smithay::utils::Point::from((
                        x.unwrap_or(BORDER_WIDTH).max(BORDER_WIDTH),
                        y.unwrap_or(TITLEBAR_HEIGHT + BORDER_WIDTH).max(TITLEBAR_HEIGHT + BORDER_WIDTH),
                    ))
                });
            let max_w = (self.width as i32 - cur_loc.x - BORDER_WIDTH * 2).max(160);
            let max_h = (self.height as i32 - cur_loc.y - BORDER_WIDTH * 2).max(100);
            let target_w = w.map(|v| (v as i32).min(max_w)).unwrap_or(max_w);
            let target_h = h.map(|v| (v as i32).min(max_h)).unwrap_or(max_h);
            Rectangle::new(cur_loc, (target_w, target_h).into())
        } else {
            Rectangle::new(
                (x.unwrap_or(0), y.unwrap_or(0)).into(),
                (w.unwrap_or(self.width) as i32, h.unwrap_or(self.height) as i32).into(),
            )
        };

        let _ = window.configure(rect);
        for mw in &mut self.managed_windows {
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
        for w in &mut self.managed_windows {
            if matches!(&w.surface, WindowSurface::X11(x) if x == &window) {
                w.update_title();
                if self.enable_wm {
                    let max_w = (self.width as i32 - w.loc.x - BORDER_WIDTH * 2).max(160);
                    let max_h = (self.height as i32 - w.loc.y - BORDER_WIDTH * 2).max(100);
                    w.size = (geometry.size.w.min(max_w), geometry.size.h.min(max_h)).into();
                    let max_x = (self.width as i32 - w.size.w - BORDER_WIDTH).max(BORDER_WIDTH);
                    let max_y = (self.height as i32 - w.size.h - BORDER_WIDTH).max(TITLEBAR_HEIGHT + BORDER_WIDTH);
                    if w.loc.x > max_x { w.loc.x = max_x; }
                    if w.loc.y > max_y { w.loc.y = max_y; }
                } else {
                    w.size = geometry.size;
                }
            }
        }
        self.dirty = true;
    }

    fn resize_request(&mut self, _xwm: XwmId, _window: X11Surface, _button: u32, _resize_edge: ResizeEdge) {}
    fn move_request(&mut self, _xwm: XwmId, _window: X11Surface, _button: u32) {}
    fn send_selection(&mut self, _xwm: XwmId, _selection: SelectionTarget, _mime_type: String, _fd: OwnedFd) {}
}

delegate_xwayland_shell!(TermwayState);
