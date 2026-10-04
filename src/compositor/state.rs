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
        Client, DisplayHandle,
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
        },
        shm::{ShmHandler, ShmState},
    },
};

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

pub struct TermwayState {
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub shm_state: ShmState,
    pub seat_state: SeatState<Self>,
    pub seat: Seat<Self>,
    pub keyboard: KeyboardHandle<Self>,
    pub pointer: PointerHandle<Self>,

    pub output: Output,
    pub data_device_state: DataDeviceState,

    pub windows: Vec<ToplevelSurface>,
    pub x11_windows: Vec<smithay::xwayland::X11Surface>,
    pub unassociated_surfaces: Vec<WlSurface>,
    pub xwayland_shell_state: smithay::wayland::xwayland_shell::XWaylandShellState,
    pub xwm: Option<smithay::xwayland::xwm::X11Wm>,
    pub xwayland_ready: bool,
    pub dirty: bool,
    pub running: bool,
    pub width: u32,
    pub height: u32,
    pub start_time: std::time::Instant,
}

impl TermwayState {
    pub fn new(dh: &DisplayHandle, width: u32, height: u32) -> Self {
        let compositor_state = CompositorState::new::<Self>(dh);
        let xdg_shell_state = XdgShellState::new::<Self>(dh);
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
            compositor_state,
            xdg_shell_state,
            shm_state,
            seat_state,
            seat,
            keyboard,
            pointer,
            output,
            data_device_state,
            windows: Vec::new(),
            x11_windows: Vec::new(),
            unassociated_surfaces: Vec::new(),
            xwayland_shell_state,
            xwm: None,
            xwayland_ready: false,
            dirty: true,
            running: true,
            width,
            height,
            start_time: std::time::Instant::now(),
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
            if let Some(s) = self.unassociated_surfaces.iter().find(|s| s.id().protocol_id() == wid) {
                return Some(s.clone());
            }
        }

        // If this window does not have an ID/serial yet, find an unclaimed surface from unassociated_surfaces
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
        self.dirty = true;
        
        // 破棄済みサーフェスをフォーカス・描画候補から除く
        self.unassociated_surfaces.retain(|s| smithay::reexports::wayland_server::Resource::is_alive(s));

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
        // サブサーフェス（親あり）とカーソル画像は独立サーフェスとして扱わない
        if smithay::wayland::compositor::get_parent(surface).is_some() {
            tracked = true;
        }
        if smithay::wayland::compositor::get_role(surface) == Some("cursor_image") {
            tracked = true;
        }
        if !tracked {
            if !self.unassociated_surfaces.iter().any(|s| s == surface) {
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
}

// 3. XdgShellHandler
impl XdgShellHandler for TermwayState {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Activated);
            state.size = Some((self.width as i32, self.height as i32).into());
        });
        surface.send_configure();
        let wl = surface.wl_surface().clone();
        self.unassociated_surfaces.retain(|s| s != &wl);
        self.windows.push(surface);

        let keyboard = self.keyboard.clone();
        keyboard.set_focus(
            self,
            Some(wl),
            smithay::utils::SERIAL_COUNTER.next_serial(),
        );

        self.dirty = true;
    }

    fn new_popup(&mut self, _surface: PopupSurface, _positioner: PositionerState) {}
    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {}
    fn reposition_request(&mut self, _surface: PopupSurface, _positioner: PositionerState, _token: u32) {}
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

    fn focus_changed(&mut self, _seat: &Seat<Self>, _focused: Option<&WlSurface>) {}
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
