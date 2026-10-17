//! `xdg_shell` (toplevels, popups, move/resize/maximize requests) and
//! `zxdg_decoration_manager_v1` (server-side vs client-side decoration negotiation).

use smithay::{
    desktop::{find_popup_root_surface, PopupKind, PopupPointerGrab},
    input::{pointer::Focus, Seat},
    reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode,
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel,
    reexports::wayland_server::{protocol::wl_seat, Resource},
    utils::Serial,
    wayland::shell::xdg::{
        decoration::XdgDecorationHandler, PopupSurface, PositionerState, ToplevelSurface,
        XdgShellHandler, XdgShellState,
    },
};

use crate::compositor::state::TermwayState;
use crate::compositor::window::{ManagedWindow, WindowDragState, BORDER_WIDTH, TITLEBAR_HEIGHT};

// 3. XdgShellHandler
impl XdgShellHandler for TermwayState {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let id = self.wm.alloc_id();
        let wl_id = surface.wl_surface().id().protocol_id();
        crate::compositor::log_line(format!("[compositor] new_toplevel id={} surface={}", id, wl_id));

        let has_parent = surface.parent().is_some();
        let should_maximize = !has_parent;

        let (loc, configure_size, init_window_size, is_maximized, saved_geometry) = if self.enable_wm {
            if should_maximize {
                let saved_geo = ManagedWindow::default_80pct_geometry(self.width, self.height, false);
                let loc = smithay::utils::Point::from((0, 0));
                let configure_size = smithay::utils::Size::from((self.width as i32, self.height as i32));
                let init_window_size = configure_size;
                (loc, configure_size, init_window_size, true, Some(saved_geo))
            } else {
                let offset = self.wm.cascade_offset();
                let loc_x = (20 + offset).max(0);
                let loc_y = (10 + offset).max(0);
                let loc = smithay::utils::Point::from((loc_x, loc_y));

                let max_w = (self.width as i32 - loc_x - 10).max(320);
                let max_h = (self.height as i32 - loc_y - 10).max(200);

                let init_w = (self.width as i32 * 8 / 10).clamp(320, max_w);
                let init_h = (self.height as i32 * 8 / 10).clamp(240, max_h);
                let configure_size = smithay::utils::Size::from((init_w, init_h));
                let init_window_size = configure_size;

                (loc, configure_size, init_window_size, false, None)
            }
        } else {
            let s = smithay::utils::Size::from((self.width as i32, self.height as i32));
            (smithay::utils::Point::from((0, 0)), s, s, true, None)
        };

        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Activated);
            if is_maximized {
                state.states.set(xdg_toplevel::State::Maximized);
            }
            state.size = Some(configure_size);
        });
        surface.send_configure();

        let wl = surface.wl_surface().clone();
        self.unassociated_surfaces.retain(|s| s != &wl);
        self.toplevels.push(surface.clone());

        self.wm.deactivate_all();
        // Wayland clients default to CSD unless ServerSide is negotiated via zxdg_decoration
        let mut win = ManagedWindow::new_wayland(id, surface, loc, init_window_size, false);
        win.is_maximized = is_maximized;
        win.saved_geometry = saved_geometry;
        self.wm.windows.push(win);
        self.update_active_window_title();

        self.dismiss_splash();
        self.set_keyboard_focus(Some(wl));

        self.dirty = true;
    }

    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        let geo = positioner.get_geometry();
        crate::compositor::log_line(format!(
            "[compositor] new_popup id={} geo={:?}",
            surface.wl_surface().id().protocol_id(),
            geo
        ));
        surface.with_pending_state(|state| {
            state.geometry = geo;
        });
        if let Err(err) = self.popups.track_popup(PopupKind::Xdg(surface.clone())) {
            crate::compositor::log_line(format!("[compositor] Failed to track popup: {:?}", err));
        }
        let _ = surface.send_configure();
        // NOTE: キーボードフォーカスは toplevel に残し、ポップアップへは移さない。
        // Qt はメニュー切替時に旧ポップアップを自ら destroy するため、フォーカスが
        // 旧ポップアップ上にあると「アプリが非アクティブ化した」と判断され、
        // 直後に生成された新しいポップアップも閉じられてしまう。
        // (Qt/GTK はキー入力をアプリ内部で最前面ポップアップへ配送する)
        self.dirty = true;
    }
    fn grab(&mut self, surface: PopupSurface, seat: wl_seat::WlSeat, serial: Serial) {
        crate::compositor::log_line(format!(
            "[compositor] grab request id={} serial={:?}",
            surface.wl_surface().id().protocol_id(),
            serial
        ));
        let seat: Seat<Self> = match Seat::from_resource(&seat) {
            Some(s) => s,
            None => {
                crate::compositor::log_line("[compositor] grab: Seat::from_resource returned None!");
                return;
            }
        };
        let kind = PopupKind::Xdg(surface.clone());
        if let Ok(root) = find_popup_root_surface(&kind) {
            match self.popups.grab_popup::<Self>(root, kind, &seat, serial) {
                Ok(grab) => {
                    crate::compositor::log_line(format!(
                        "[compositor] grab_popup succeeded for id={}",
                        surface.wl_surface().id().protocol_id()
                    ));
                    if let Some(pointer) = seat.get_pointer() {
                        pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
                    }
                }
                Err(err) => {
                    crate::compositor::log_line(format!("[compositor] grab_popup error: {:?}", err));
                }
            }
        } else {
            crate::compositor::log_line("[compositor] grab: find_popup_root_surface failed!");
        }
    }
    fn reposition_request(&mut self, surface: PopupSurface, positioner: PositionerState, token: u32) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
        });
        let _ = surface.send_repositioned(token);
        self.dirty = true;
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        let w = self.width;
        let h = self.height;
        if let Some(win) = self.wm.find_toplevel_mut(&surface) {
            win.maximize(w, h);
        } else {
            surface.with_pending_state(|state| {
                state.states.set(xdg_toplevel::State::Maximized);
                state.size = Some((w as i32, h as i32).into());
            });
            surface.send_configure();
        }
        self.needs_clear = true;
        self.dirty = true;
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        let w = self.width;
        let h = self.height;
        if self.enable_wm {
            if let Some(win) = self.wm.find_toplevel_mut(&surface) {
                win.unmaximize(w, h);
            }
        } else {
            surface.with_pending_state(|state| {
                state.states.set(xdg_toplevel::State::Maximized);
                state.size = Some((w as i32, h as i32).into());
            });
            surface.send_configure();
        }
        self.needs_clear = true;
        self.dirty = true;
    }

    fn minimize_request(&mut self, surface: ToplevelSurface) {
        if let Some(id) = self.wm.find_toplevel(&surface).map(|w| w.id) {
            self.minimize_window(id);
        }
        self.needs_clear = true;
        self.dirty = true;
    }

    fn move_request(&mut self, surface: ToplevelSurface, _seat: wl_seat::WlSeat, _serial: Serial) {
        if !self.enable_wm {
            return;
        }
        if let Some(win) = self.wm.find_toplevel(&surface) {
            let cur_loc = self.pointer.current_location();
            let win_loc = win.loc;
            self.wm.drag_state = WindowDragState::Moving {
                start_cursor: (cur_loc.x, cur_loc.y),
                start_window_pos: (win_loc.x, win_loc.y),
            };
        }
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        _seat: wl_seat::WlSeat,
        _serial: Serial,
        _edges: xdg_toplevel::ResizeEdge,
    ) {
        if !self.enable_wm {
            return;
        }
        if let Some(win) = self.wm.find_toplevel(&surface) {
            let cur_loc = self.pointer.current_location();
            let win_geo = win.content_geometry();
            self.wm.drag_state = WindowDragState::Resizing {
                start_cursor: (cur_loc.x, cur_loc.y),
                start_window_geo: win_geo,
            };
        }
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        self.toplevels.retain(|w| w != &surface);
        self.wm.remove_toplevel(&surface);
        self.focus_top_window();
        self.dirty = true;
    }
}

impl TermwayState {
    fn update_toplevel_decoration_mode(&mut self, toplevel: &ToplevelSurface, mode: DecorationMode) {
        let new_has_ssd = self.enable_wm && mode == DecorationMode::ServerSide;
        let screen_w = self.width;
        let screen_h = self.height;
        if let Some(w) = self.wm.find_toplevel_mut(toplevel) {
            if w.has_ssd != new_has_ssd {
                w.has_ssd = new_has_ssd;
                if w.is_maximized {
                    let (new_loc, new_w, new_h) = if w.has_ssd {
                        (
                            smithay::utils::Point::from((BORDER_WIDTH, TITLEBAR_HEIGHT + BORDER_WIDTH)),
                            (screen_w as i32 - BORDER_WIDTH * 2).max(100),
                            (screen_h as i32 - TITLEBAR_HEIGHT - BORDER_WIDTH * 2).max(60),
                        )
                    } else {
                        (
                            smithay::utils::Point::from((0, 0)),
                            (screen_w as i32).max(100),
                            (screen_h as i32).max(60),
                        )
                    };
                    w.loc = new_loc;
                    w.size = (new_w, new_h).into();
                    w.saved_geometry = Some(ManagedWindow::default_80pct_geometry(screen_w, screen_h, w.has_ssd));
                    toplevel.with_pending_state(|state| {
                        state.size = Some((new_w, new_h).into());
                    });
                }
            }
        }
    }
}

impl XdgDecorationHandler for TermwayState {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        let mode = if self.enable_wm {
            DecorationMode::ServerSide
        } else {
            DecorationMode::ClientSide
        };
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(mode);
        });
        self.update_toplevel_decoration_mode(&toplevel, mode);
        toplevel.send_configure();
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: DecorationMode) {
        let chosen_mode = if self.enable_wm {
            if mode == DecorationMode::ClientSide {
                DecorationMode::ClientSide
            } else {
                DecorationMode::ServerSide
            }
        } else {
            DecorationMode::ClientSide
        };
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(chosen_mode);
        });
        self.update_toplevel_decoration_mode(&toplevel, chosen_mode);
        toplevel.send_configure();
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        let mode = if self.enable_wm {
            DecorationMode::ServerSide
        } else {
            DecorationMode::ClientSide
        };
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(mode);
        });
        self.update_toplevel_decoration_mode(&toplevel, mode);
        toplevel.send_configure();
    }
}
