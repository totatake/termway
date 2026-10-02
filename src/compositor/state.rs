use smithay::{
    delegate_compositor, delegate_seat, delegate_shm, delegate_xdg_shell,
    input::{
        keyboard::KeyboardHandle,
        pointer::PointerHandle,
        Seat, SeatHandler, SeatState,
    },
    reexports::wayland_server::{
        backend::{ClientData, ClientId, DisconnectReason},
        protocol::{
            wl_buffer,
            wl_seat,
            wl_surface::WlSurface,
        },
        Client, DisplayHandle,
    },
    utils::Serial,
    wayland::{
        buffer::BufferHandler,
        compositor::{
            with_surface_tree_downward, CompositorClientState, CompositorHandler, CompositorState,
            SurfaceAttributes, TraversalAction,
        },
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
        },
        shm::{ShmHandler, ShmState},
    },
};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;

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

    pub windows: Vec<ToplevelSurface>,
    pub x11_windows: Vec<smithay::xwayland::X11Surface>,
    pub xwayland_shell_state: smithay::wayland::xwayland_shell::XWaylandShellState,
    pub xwm: Option<smithay::xwayland::xwm::X11Wm>,
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

        Self {
            compositor_state,
            xdg_shell_state,
            shm_state,
            seat_state,
            seat,
            keyboard,
            pointer,
            windows: Vec::new(),
            x11_windows: Vec::new(),
            xwayland_shell_state,
            xwm: None,
            dirty: true,
            running: true,
            width,
            height,
            start_time: std::time::Instant::now(),
        }
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
        self.windows.push(surface);
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
