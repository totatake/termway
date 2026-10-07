use std::os::unix::io::OwnedFd;
use smithay::{
    delegate_compositor, delegate_data_device, delegate_output, delegate_seat, delegate_shm,
    delegate_xdg_shell,
    input::{
        keyboard::KeyboardHandle,
        pointer::PointerHandle,
        Seat, SeatHandler, SeatState,
    },
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel,
    reexports::wayland_server::{
        backend::{ClientData, ClientId, DisconnectReason},
        protocol::{
            wl_buffer,
            wl_seat,
            wl_surface::WlSurface,
        },
        Client, DisplayHandle, Resource,
    },
    utils::{Serial, Transform},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            with_surface_tree_downward, CompositorClientState, CompositorHandler, CompositorState,
            SurfaceAttributes, TraversalAction,
        },
        output::OutputHandler,
        selection::{
            data_device::{ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler},
            SelectionHandler,
        },
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
            decoration::{XdgDecorationHandler, XdgDecorationState},
            XDG_POPUP_ROLE,
        },
        shm::{ShmHandler, ShmState},
    },
};

use smithay::desktop::{
    find_popup_root_surface, PopupKind, PopupManager, PopupPointerGrab,
};
use smithay::input::pointer::Focus;
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode;

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

use super::window::{ManagedWindow, WindowDragState, WindowSurface, BORDER_WIDTH, TITLEBAR_HEIGHT};

pub struct TermwayState {
    pub dh: DisplayHandle,
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub xdg_decoration_state: XdgDecorationState,
    pub shm_state: ShmState,
    pub popups: PopupManager,
    pub seat_state: SeatState<Self>,
    pub seat: Seat<Self>,
    pub keyboard: KeyboardHandle<Self>,
    pub pointer: PointerHandle<Self>,

    pub output: Output,
    pub data_device_state: DataDeviceState,

    pub windows: Vec<ToplevelSurface>,
    pub x11_windows: Vec<smithay::xwayland::X11Surface>,
    pub managed_windows: Vec<ManagedWindow>,
    pub drag_state: WindowDragState,
    pub next_window_id: usize,
    pub unassociated_surfaces: Vec<WlSurface>,
    pub xwayland_shell_state: smithay::wayland::xwayland_shell::XWaylandShellState,
    pub xwm: Option<smithay::xwayland::xwm::X11Wm>,
    pub xwayland_ready: bool,
    pub xwayland_error: Option<String>,
    pub dirty: bool,
    pub running: bool,
    pub width: u32,
    pub height: u32,
    pub enable_wm: bool,
    pub start_time: std::time::Instant,
}

impl TermwayState {
    pub fn new(dh: &DisplayHandle, width: u32, height: u32, enable_wm: bool) -> Self {
        let compositor_state = CompositorState::new::<Self>(dh);
        let xdg_shell_state = XdgShellState::new::<Self>(dh);
        let xdg_decoration_state = XdgDecorationState::new::<Self>(dh);
        let shm_state = ShmState::new::<Self>(dh, vec![]);
        let xwayland_shell_state = smithay::wayland::xwayland_shell::XWaylandShellState::new::<Self>(dh);
        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(dh, "termway");

        let keyboard = seat.add_keyboard(Default::default(), 200, 25).unwrap();
        let pointer = seat.add_pointer();

        // Initialize output and advertise it to Wayland clients
        let output = Output::new(
            "termway-0".into(),
            PhysicalProperties {
                size: (200, 150).into(),
                subpixel: Subpixel::HorizontalRgb,
                make: "termway".into(),
                model: "virtual-terminal".into(),
            },
        );
        let _global = output.create_global::<Self>(dh);
        output.change_current_state(
            Some(Mode { size: (width as i32, height as i32).into(), refresh: 60000 }),
            Some(Transform::Normal),
            Some(Scale::Integer(1)),
            Some((0, 0).into()),
        );
        output.set_preferred(Mode { size: (width as i32, height as i32).into(), refresh: 60000 });

        let data_device_state = DataDeviceState::new::<Self>(dh);

        Self {
            dh: dh.clone(),
            compositor_state,
            xdg_shell_state,
            xdg_decoration_state,
            shm_state,
            popups: PopupManager::default(),
            seat_state,
            seat,
            keyboard,
            pointer,
            output,
            data_device_state,
            windows: Vec::new(),
            x11_windows: Vec::new(),
            managed_windows: Vec::new(),
            drag_state: WindowDragState::None,
            next_window_id: 1,
            unassociated_surfaces: Vec::new(),
            xwayland_shell_state,
            xwm: None,
            xwayland_ready: false,
            xwayland_error: None,
            dirty: true,
            running: true,
            width,
            height,
            enable_wm,
            start_time: std::time::Instant::now(),
        }
    }

    pub fn raise_window(&mut self, id: usize) {
        if let Some(pos) = self.managed_windows.iter().position(|w| w.id == id) {
            let mut win = self.managed_windows.remove(pos);
            for w in &mut self.managed_windows {
                w.active = false;
            }
            win.active = true;
            let wl_surface = self.surface_for_managed_window(&win);
            self.managed_windows.push(win);

            if let Some(surface) = wl_surface {
                let keyboard = self.keyboard.clone();
                keyboard.set_focus(
                    self,
                    Some(surface),
                    smithay::utils::SERIAL_COUNTER.next_serial(),
                );
            }
            self.dirty = true;
        }
    }

    pub fn surface_for_managed_window(&self, window: &ManagedWindow) -> Option<WlSurface> {
        match &window.surface {
            super::window::WindowSurface::Wayland(w) => Some(w.wl_surface().clone()),
            super::window::WindowSurface::X11(w) => self.surface_for_x11(w),
            super::window::WindowSurface::Dummy => None,
        }
    }


    /// Retrieve the corresponding WlSurface for an X11Surface.
    /// Matches by wl_surface(), wl_surface_serial(), or wl_surface_id().
    pub fn surface_for_x11(&self, xwindow: &smithay::xwayland::X11Surface) -> Option<WlSurface> {
        use smithay::reexports::wayland_server::Resource;

        if let Some(s) = xwindow.wl_surface() {
            return Some(s);
        }
        if let Some(serial) = xwindow.wl_surface_serial() {
            if let Some(s) = self.xwayland_shell_state.surface_for_serial(serial) {
                return Some(s);
            }
        }
        #[allow(deprecated)]
        if let Some(wid) = xwindow.wl_surface_id() {
            if let Some(s) = self.unassociated_surfaces.iter().rfind(|s| s.id().protocol_id() == wid) {
                return Some(s.clone());
            }
        }

        let claimed_wids: Vec<u32> = self.x11_windows.iter().filter_map(|w| {
            #[allow(deprecated)]
            w.wl_surface_id()
        }).collect();

        let unclaimed: Vec<&WlSurface> = self.unassociated_surfaces.iter().filter(|s| {
            !claimed_wids.contains(&s.id().protocol_id())
        }).collect();

        if let Some(&s) = unclaimed.last() {
            return Some(s.clone());
        }

        if self.x11_windows.len() == 1 {
            return self.unassociated_surfaces.last().cloned();
        }

        None
    }

    /// Dumps current GUI window and popup state as JSON for automated testing and inspection.
    pub fn dump_state_json(&self) -> String {
        use smithay::reexports::wayland_server::Resource;
        let mut out = String::from("{\n");
        out.push_str(&format!("  \"screen_width\": {},\n", self.width));
        out.push_str(&format!("  \"screen_height\": {},\n", self.height));

        // Windows
        out.push_str("  \"windows\": [\n");
        for (i, win) in self.managed_windows.iter().enumerate() {
            let kind = match win.surface {
                super::window::WindowSurface::Wayland(_) => "wayland",
                super::window::WindowSurface::X11(_) => "x11",
                super::window::WindowSurface::Dummy => "dummy",
            };
            let content_geo = win.content_geometry();
            let title_escaped = win.title.replace('\\', "\\\\").replace('\"', "\\\"");
            let comma = if i + 1 < self.managed_windows.len() { "," } else { "" };
            out.push_str(&format!(
                "    {{\"id\": {}, \"kind\": \"{}\", \"title\": \"{}\", \"active\": {}, \"x\": {}, \"y\": {}, \"width\": {}, \"height\": {}}}{}\n",
                win.id, kind, title_escaped, win.active, content_geo.loc.x, content_geo.loc.y, content_geo.size.w, content_geo.size.h, comma
            ));
        }
        out.push_str("  ],\n");

        // Popups
        out.push_str("  \"popups\": [\n");
        let mut popup_entries = Vec::new();
        for win in &self.managed_windows {
            if let super::window::WindowSurface::Wayland(ref toplevel) = win.surface {
                for (popup, popup_offset) in smithay::desktop::PopupManager::popups_for_surface(toplevel.wl_surface()) {
                    let popup_x = win.loc.x + popup_offset.x - popup.geometry().loc.x;
                    let popup_y = win.loc.y + popup_offset.y - popup.geometry().loc.y;
                    let geo = popup.geometry();
                    let id = popup.wl_surface().id().protocol_id();
                    let bbox = smithay::desktop::utils::bbox_from_surface_tree(popup.wl_surface(), (0, 0));
                    popup_entries.push(format!(
                        "    {{\"id\": {}, \"parent_window_id\": {}, \"x\": {}, \"y\": {}, \"width\": {}, \"height\": {}, \"buffer_width\": {}, \"buffer_height\": {}}}",
                        id, win.id, popup_x, popup_y, geo.size.w, geo.size.h, bbox.size.w, bbox.size.h
                    ));
                }
            }
        }
        out.push_str(&popup_entries.join(",\n"));
        if !popup_entries.is_empty() {
            out.push('\n');
        }
        out.push_str("  ]\n");

        out.push('}');
        out
    }
}

// 1. BufferHandler
impl BufferHandler for TermwayState {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

// 2. CompositorHandler
impl CompositorHandler for TermwayState {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        smithay::backend::renderer::utils::on_commit_buffer_handler::<Self>(surface);
        self.popups.commit(surface);
        self.dirty = true;
        
        // 破棄済みサーフェスをフォーカス・描画候補から除く
        self.unassociated_surfaces.retain(smithay::reexports::wayland_server::Resource::is_alive);
        self.managed_windows.retain(|w| {
            w.wl_surface().as_ref().map(smithay::reexports::wayland_server::Resource::is_alive).unwrap_or(true)
        });

        // タイトルや状態の更新
        for w in &mut self.managed_windows {
            if w.wl_surface().as_ref() == Some(surface) {
                w.update_title();
                if self.enable_wm {
                    let bbox = smithay::desktop::utils::bbox_from_surface_tree(surface, (0, 0));
                    if bbox.size.w > 0 && bbox.size.h > 0 {
                        let top_margin = if w.has_ssd { TITLEBAR_HEIGHT + BORDER_WIDTH * 2 } else { 0 };
                        let side_margin = if w.has_ssd { BORDER_WIDTH * 2 } else { 0 };
                        let max_allowed_w = (self.width as i32 - side_margin - 10).max(160);
                        let max_allowed_h = (self.height as i32 - top_margin - 10).max(100);
                        let clamped_w = bbox.size.w.min(max_allowed_w);
                        let clamped_h = bbox.size.h.min(max_allowed_h);
                        w.size = (clamped_w, clamped_h).into();

                        // 画面枠内に確実に収まるよう位置を調整
                        let min_x = if w.has_ssd { BORDER_WIDTH } else { 0 };
                        let max_x = (self.width as i32 - w.size.w - min_x).max(min_x);
                        let min_y = if w.has_ssd { TITLEBAR_HEIGHT + BORDER_WIDTH } else { 0 };
                        let max_y = (self.height as i32 - w.size.h - min_x).max(min_y);
                        w.loc.x = w.loc.x.clamp(min_x, max_x);
                        w.loc.y = w.loc.y.clamp(min_y, max_y);

                        if clamped_w != bbox.size.w || clamped_h != bbox.size.h {
                            w.configure_size(clamped_w, clamped_h);
                        }
                    }
                } else {
                    w.loc = (0, 0).into();
                    let bbox = smithay::desktop::utils::bbox_from_surface_tree(surface, (0, 0));
                    if bbox.size.w > 0 && bbox.size.h > 0 {
                        w.size = (bbox.size.w.min(self.width as i32), bbox.size.h.min(self.height as i32)).into();
                        if bbox.size.w > self.width as i32 || bbox.size.h > self.height as i32 {
                            w.configure_size(self.width as i32, self.height as i32);
                        }
                    } else {
                        w.size = (self.width as i32, self.height as i32).into();
                    }
                }
            }
        }

        let mut tracked = false;
        for w in &self.windows {
            if w.wl_surface() == surface {
                tracked = true; break;
            }
        }
        for w in &self.x11_windows {
            if let Some(s) = w.wl_surface() {
                if &s == surface {
                    tracked = true; break;
                }
            }
        }
        for w in &self.managed_windows {
            if w.wl_surface().as_ref() == Some(surface) {
                tracked = true; break;
            }
        }
        // サブサーフェス（親あり）とカーソル画像は独立サーフェスとして扱わない
        if smithay::wayland::compositor::get_parent(surface).is_some() {
            tracked = true;
        }
        if smithay::wayland::compositor::get_role(surface) == Some("cursor_image") {
            tracked = true;
        }
        if smithay::wayland::compositor::get_role(surface) == Some(XDG_POPUP_ROLE) {
            tracked = true;
        }
        if !tracked {
            use smithay::reexports::wayland_server::Resource;
            let pid = surface.id().protocol_id();
            // 同一 ID の古いサーフェスがあれば破棄して最新版に置き換える
            self.unassociated_surfaces.retain(|s| s.id().protocol_id() != pid);
            self.unassociated_surfaces.push(surface.clone());
            
            let keyboard = self.keyboard.clone();
            keyboard.set_focus(
                self,
                Some(surface.clone()),
                smithay::utils::SERIAL_COUNTER.next_serial(),
            );
        }
    }
}

// 3. XdgShellHandler
impl XdgShellHandler for TermwayState {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let id = self.next_window_id;
        self.next_window_id += 1;
        let wl_id = surface.wl_surface().id().protocol_id();
        crate::compositor::log_line(format!("[compositor] new_toplevel id={} surface={}", id, wl_id));

        let (loc, configure_size, init_window_size) = if self.enable_wm {
            let offset = ((self.managed_windows.len() % 8) as i32) * 20;
            let loc_x = (20 + offset).max(0);
            let loc_y = (10 + offset).max(0);
            let loc = smithay::utils::Point::from((loc_x, loc_y));

            let max_w = (self.width as i32 - loc_x - 10).max(320);
            let max_h = (self.height as i32 - loc_y - 10).max(200);

            let init_w = (self.width as i32 * 8 / 10).clamp(320, max_w);
            let init_h = (self.height as i32 * 8 / 10).clamp(240, max_h);
            let configure_size = smithay::utils::Size::from((init_w, init_h));
            let init_window_size = configure_size;

            (loc, configure_size, init_window_size)
        } else {
            let s = smithay::utils::Size::from((self.width as i32, self.height as i32));
            (smithay::utils::Point::from((0, 0)), s, s)
        };

        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Activated);
            if !self.enable_wm {
                state.states.set(xdg_toplevel::State::Maximized);
            }
            state.size = Some(configure_size);
        });
        surface.send_configure();

        let wl = surface.wl_surface().clone();
        self.unassociated_surfaces.retain(|s| s != &wl);
        self.windows.push(surface.clone());

        for w in &mut self.managed_windows {
            w.active = false;
        }
        // Wayland clients default to CSD unless ServerSide is negotiated via zxdg_decoration
        let win = ManagedWindow::new_wayland(id, surface, loc, init_window_size, false);
        self.managed_windows.push(win);

        let keyboard = self.keyboard.clone();
        keyboard.set_focus(
            self,
            Some(wl),
            smithay::utils::SERIAL_COUNTER.next_serial(),
        );

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
        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Maximized);
            state.size = Some((self.width as i32, self.height as i32).into());
        });
        surface.send_configure();
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        if !self.enable_wm {
            surface.with_pending_state(|state| {
                state.states.set(xdg_toplevel::State::Maximized);
                state.size = Some((self.width as i32, self.height as i32).into());
            });
            surface.send_configure();
        }
    }

    fn move_request(&mut self, surface: ToplevelSurface, _seat: wl_seat::WlSeat, _serial: Serial) {
        if !self.enable_wm {
            return;
        }
        if let Some(win) = self.managed_windows.iter().find(|w| match &w.surface {
            WindowSurface::Wayland(t) => t == &surface,
            _ => false,
        }) {
            let cur_loc = self.pointer.current_location();
            let win_loc = win.loc;
            self.drag_state = WindowDragState::Moving {
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
        if let Some(win) = self.managed_windows.iter().find(|w| match &w.surface {
            WindowSurface::Wayland(t) => t == &surface,
            _ => false,
        }) {
            let cur_loc = self.pointer.current_location();
            let win_geo = win.content_geometry();
            self.drag_state = WindowDragState::Resizing {
                start_cursor: (cur_loc.x, cur_loc.y),
                start_window_geo: win_geo,
            };
        }
    }
}

// 4. ShmHandler
impl ShmHandler for TermwayState {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

// 5. SeatHandler
impl SeatHandler for TermwayState {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let client = focused.and_then(|s| self.dh.get_client(s.id()).ok());
        smithay::wayland::selection::data_device::set_data_device_focus(&self.dh, seat, client);
    }
    fn cursor_image(&mut self, _seat: &Seat<Self>, _image: smithay::input::pointer::CursorImageStatus) {}
}

// 6. OutputHandler
impl OutputHandler for TermwayState {}

// 7. SelectionHandler & DataDeviceHandler
impl SelectionHandler for TermwayState {
    type SelectionUserData = ();
}

impl DataDeviceHandler for TermwayState {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for TermwayState {}
impl ServerDndGrabHandler for TermwayState {
    fn send(&mut self, _mime_type: String, _fd: OwnedFd, _seat: Seat<Self>) {}
}

pub fn send_frames_surface_tree(surface: &WlSurface, time: u32) {
    with_surface_tree_downward(
        surface,
        (),
        |_, _, &()| TraversalAction::DoChildren(()),
        |_surf, states, &()| {
            for callback in states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .frame_callbacks
                .drain(..)
            {
                callback.done(time);
            }
        },
        |_, _, &()| true,
    );
}

// Protocol delegates
delegate_compositor!(TermwayState);
delegate_xdg_shell!(TermwayState);
delegate_shm!(TermwayState);
delegate_seat!(TermwayState);
delegate_output!(TermwayState);
delegate_data_device!(TermwayState);
smithay::delegate_xdg_decoration!(TermwayState);

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
        toplevel.send_configure();

        for w in &mut self.managed_windows {
            if let WindowSurface::Wayland(ref s) = w.surface {
                if s == &toplevel {
                    w.has_ssd = self.enable_wm && mode == DecorationMode::ServerSide;
                }
            }
        }
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
        toplevel.send_configure();

        for w in &mut self.managed_windows {
            if let WindowSurface::Wayland(ref s) = w.surface {
                if s == &toplevel {
                    w.has_ssd = self.enable_wm && chosen_mode == DecorationMode::ServerSide;
                }
            }
        }
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
        toplevel.send_configure();

        for w in &mut self.managed_windows {
            if let WindowSurface::Wayland(ref s) = w.surface {
                if s == &toplevel {
                    w.has_ssd = self.enable_wm && mode == DecorationMode::ServerSide;
                }
            }
        }
    }
}
