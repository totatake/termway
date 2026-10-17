//! Central compositor state.
//!
//! `TermwayState` is the single state type required by Smithay's event loop and protocol
//! delegates. It acts as an orchestrator that owns:
//! - Wayland protocol globals (compositor, xdg-shell, shm, seat, output, data device)
//! - [`WindowManager`]: the managed window stack, activation and drag interaction state
//! - XWayland integration state
//! - Render loop flags and splash screen state
//!
//! Protocol handler trait implementations live in `compositor::handlers`.

use std::io::Write;

use smithay::{
    desktop::PopupManager,
    input::{keyboard::KeyboardHandle, pointer::PointerHandle, Seat, SeatState},
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::wayland_server::{
        backend::{ClientData, ClientId, DisconnectReason},
        protocol::wl_surface::WlSurface,
        DisplayHandle,
    },
    utils::Transform,
    wayland::{
        compositor::{
            with_surface_tree_downward, CompositorClientState, CompositorState, SurfaceAttributes,
            TraversalAction,
        },
        selection::data_device::DataDeviceState,
        shell::xdg::{decoration::XdgDecorationState, ToplevelSurface, XdgShellState},
        shm::ShmState,
    },
    reexports::wayland_protocols::wp::text_input::zv3::server::{
        zwp_text_input_manager_v3::ZwpTextInputManagerV3,
        zwp_text_input_v3::ZwpTextInputV3,
    },
};

#[derive(Debug, Clone)]
pub struct TrackedTextInput {
    pub instance: ZwpTextInputV3,
    pub serial: u32,
    pub enabled: bool,
    pub pending_enable: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct UnicodeKeySlot {
    pub keycode: u32,
    pub xkb_name: String,
    pub mapped_char: Option<char>,
}

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

use super::window::{ManagedWindow, WindowSurface};
use super::window_manager::WindowManager;

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
    pub focused_surface: Option<WlSurface>,
    pub text_inputs: Vec<TrackedTextInput>,
    pub clipboard_text: Vec<u8>,
    pub base_keymap_string: String,
    pub unicode_key_slots: Vec<UnicodeKeySlot>,
    pub unicode_slot_cursor: usize,

    /// Raw xdg toplevels known to the compositor (used when the built-in WM is disabled
    /// and for liveness tracking). Managed window state lives in `wm`.
    pub toplevels: Vec<ToplevelSurface>,
    pub x11_windows: Vec<smithay::xwayland::X11Surface>,
    /// Window management domain (Z-order, activation, minimize, drag interaction).
    pub wm: WindowManager,
    pub unassociated_surfaces: Vec<WlSurface>,
    pub xwayland_shell_state: smithay::wayland::xwayland_shell::XWaylandShellState,
    pub xwm: Option<smithay::xwayland::xwm::X11Wm>,
    /// Separate X11 client connection to XWayland used to query X Shape regions.
    pub x11_shape_conn: Option<x11rb::rust_connection::RustConnection>,
    pub xwayland_ready: bool,
    pub xwayland_error: Option<String>,
    pub fractional_scale_manager_state: smithay::wayland::fractional_scale::FractionalScaleManagerState,
    pub viewporter_state: smithay::wayland::viewporter::ViewporterState,
    pub dirty: bool,
    pub needs_clear: bool,
    pub running: bool,
    pub scale: f64,
    pub physical_width: u32,
    pub physical_height: u32,
    pub width: u32,
    pub height: u32,
    pub enable_wm: bool,
    pub show_splash: bool,
    pub splash_info: String,
    pub splash_command: String,
    pub start_time: std::time::Instant,
    pub pixel_mouse: bool,
    pub current_cursor_shape: String,
}

impl TermwayState {
    pub fn new(dh: &DisplayHandle, width: u32, height: u32, enable_wm: bool) -> Self {
        Self::with_scale(dh, width, height, enable_wm, 1.0)
    }

    pub fn with_scale(dh: &DisplayHandle, width: u32, height: u32, enable_wm: bool, scale: f64) -> Self {
        let scale = if scale <= 0.0 { 1.0 } else { scale };
        let compositor_state = CompositorState::new::<Self>(dh);
        let xdg_shell_state = XdgShellState::new::<Self>(dh);
        let xdg_decoration_state = XdgDecorationState::new::<Self>(dh);
        let shm_state = ShmState::new::<Self>(dh, vec![]);
        let xwayland_shell_state = smithay::wayland::xwayland_shell::XWaylandShellState::new::<Self>(dh);
        let fractional_scale_manager_state = smithay::wayland::fractional_scale::FractionalScaleManagerState::new::<Self>(dh);
        let viewporter_state = smithay::wayland::viewporter::ViewporterState::new::<Self>(dh);
        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(dh, "termway");

        let keyboard = seat.add_keyboard(Default::default(), 200, 25).unwrap();
        let pointer = seat.add_pointer();

        // Advertise zwp_text_input_manager_v3 for native Wayland IME / UTF-8 text input
        dh.create_global::<Self, ZwpTextInputManagerV3, _>(1, ());

        let output_scale = if (scale.fract()).abs() < 1e-5 {
            Scale::Integer(scale as i32)
        } else {
            Scale::Fractional(scale)
        };

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
            Some(output_scale),
            Some((0, 0).into()),
        );
        output.set_preferred(Mode { size: (width as i32, height as i32).into(), refresh: 60000 });

        let data_device_state = DataDeviceState::new::<Self>(dh);

        let logical_width = ((width as f64 / scale).round() as u32).max(1);
        let logical_height = ((height as f64 / scale).round() as u32).max(1);

        let mut state = Self {
            dh: dh.clone(),
            compositor_state,
            xdg_shell_state,
            xdg_decoration_state,
            shm_state,
            popups: PopupManager::default(),
            fractional_scale_manager_state,
            viewporter_state,
            seat_state,
            seat,
            keyboard,
            pointer,
            output,
            data_device_state,
            focused_surface: None,
            text_inputs: Vec::new(),
            clipboard_text: Vec::new(),
            base_keymap_string: String::new(),
            unicode_key_slots: Vec::new(),
            unicode_slot_cursor: 0,
            toplevels: Vec::new(),
            x11_windows: Vec::new(),
            wm: WindowManager::new(),
            unassociated_surfaces: Vec::new(),
            xwayland_shell_state,
            xwm: None,
            x11_shape_conn: None,
            xwayland_ready: false,
            xwayland_error: None,
            dirty: true,
            needs_clear: false,
            running: true,
            scale,
            physical_width: width,
            physical_height: height,
            width: logical_width,
            height: logical_height,
            enable_wm,
            show_splash: true,
            splash_info: if (scale - 1.0).abs() < 1e-5 {
                format!("Wayland + XWayland | {}x{}", width, height)
            } else {
                format!("Wayland + XWayland | {}x{} (@{:.2}x)", width, height, scale)
            },
            splash_command: String::from("gui-app"),
            start_time: std::time::Instant::now(),
            pixel_mouse: false,
            current_cursor_shape: String::from("default"),
        };
        state.init_unicode_keymap_cache();
        state
    }

    /// Dynamically synchronizes host terminal mouse cursor shape via OSC 22 (`\x1b]22;<name>\x07`).
    /// Deduplicates emissions to avoid unnecessary terminal I/O when cursor shape is unchanged.
    pub fn update_cursor_shape(&mut self, shape: &str) {
        if self.current_cursor_shape != shape {
            self.current_cursor_shape = shape.to_string();
            let mut stdout = std::io::stdout();
            let _ = write!(stdout, "\x1b]22;{}\x07", shape);
            let _ = stdout.flush();
        }
    }

    /// Initializes the base XKB keymap string and discovers spare XKB keycodes (140..=247)
    /// that can be dynamically remapped to Unicode keysyms (`Uxxxx`) for non-ASCII text input.
    fn init_unicode_keymap_cache(&mut self) {
        use smithay::input::keyboard::{xkb, Keycode};
        let keyboard = self.keyboard.clone();
        let (base_keymap, slots) = keyboard.with_xkb_state(self, |context| {
            let xkb = context.xkb().lock().unwrap();
            let keymap = unsafe { xkb.keymap() };
            let base_str = keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
            let mut slots = Vec::new();
            for kc in 140..=247u32 {
                if let Some(name) = keymap.key_get_name(Keycode::new(kc)) {
                    let syms = keymap.key_get_syms_by_level(Keycode::new(kc), 0, 0);
                    if syms.is_empty() {
                        slots.push(UnicodeKeySlot {
                            keycode: kc,
                            xkb_name: name.to_string(),
                            mapped_char: None,
                        });
                        if slots.len() >= 64 {
                            break;
                        }
                    }
                }
            }
            if slots.len() < 64 {
                for kc in (180..=247u32).rev() {
                    if slots.iter().any(|s| s.keycode == kc) {
                        continue;
                    }
                    if let Some(name) = keymap.key_get_name(Keycode::new(kc)) {
                        slots.push(UnicodeKeySlot {
                            keycode: kc,
                            xkb_name: name.to_string(),
                            mapped_char: None,
                        });
                        if slots.len() >= 64 {
                            break;
                        }
                    }
                }
            }
            (base_str, slots)
        });
        self.base_keymap_string = base_keymap;
        self.unicode_key_slots = slots;
    }

    /// Moves keyboard focus to the given surface (or clears it).
    pub fn set_keyboard_focus(&mut self, surface: Option<WlSurface>) {
        let keyboard = self.keyboard.clone();
        keyboard.set_focus(self, surface, smithay::utils::SERIAL_COUNTER.next_serial());
    }

    /// Dismisses the startup splash screen (no-op if already hidden).
    pub fn dismiss_splash(&mut self) {
        if self.show_splash {
            self.show_splash = false;
            self.needs_clear = true;
        }
    }

    pub fn minimize_window(&mut self, id: usize) {
        self.wm.set_minimized(id, true);
        // Focus the next available non-minimized window (from top to bottom of Z-order)
        if let Some(top_id) = self.wm.top_visible_id() {
            self.raise_window(top_id);
        } else {
            self.set_keyboard_focus(None);
        }
        self.needs_clear = true;
        self.dirty = true;
    }

    pub fn restore_window(&mut self, id: usize) {
        self.wm.set_minimized(id, false);
        self.raise_window(id);
        self.needs_clear = true;
        self.dirty = true;
    }

    /// Raises the window to the top of the Z-order, activates it and gives it keyboard focus.
    pub fn raise_window(&mut self, id: usize) {
        if !self.wm.bring_to_top(id) {
            return;
        }
        let wl_surface = self.wm.windows.last().and_then(|w| self.surface_for_managed_window(w));
        if let Some(surface) = wl_surface {
            self.set_keyboard_focus(Some(surface));
        }
        self.dirty = true;
    }

    /// Re-activates the topmost window and focuses it (used after a window is destroyed).
    pub fn focus_top_window(&mut self) {
        self.wm.activate_top();
        let top_surface = self.wm.windows.last().and_then(|top| self.surface_for_managed_window(top));
        if let Some(s) = top_surface {
            self.set_keyboard_focus(Some(s));
        }
    }


    pub fn surface_for_managed_window(&self, window: &ManagedWindow) -> Option<WlSurface> {
        match &window.surface {
            WindowSurface::Wayland(w) => Some(w.wl_surface().clone()),
            WindowSurface::X11(w) => w
                .wl_surface()
                .or_else(|| window.cached_wl_surface())
                .or_else(|| self.surface_for_x11(w)),
            WindowSurface::Dummy => None,
        }
    }

    /// Query the X Shape extension bounding region of an X11 window.
    /// Returns `None` if the window is unshaped (single rect covering the whole window)
    /// or if the query is unavailable.
    ///
    /// Shaped windows (e.g. xeyes) never repaint pixels outside their shape. When XWayland
    /// (Composite) reallocates the window pixmap on resize, it copies the old contents, so
    /// pixels outside the shape keep stale data. A real X server hides them via the shape,
    /// so we must clip rendering to the shape region as well.
    pub fn query_x11_shape(&self, xwindow: &smithay::xwayland::X11Surface) -> Option<Vec<smithay::utils::Rectangle<i32, smithay::utils::Logical>>> {
        use x11rb::protocol::shape::{ConnectionExt as _, SK};
        let conn = self.x11_shape_conn.as_ref()?;
        let reply = conn
            .shape_get_rectangles(xwindow.window_id(), SK::BOUNDING)
            .ok()?
            .reply()
            .ok()?;
        let rects: Vec<smithay::utils::Rectangle<i32, smithay::utils::Logical>> = reply
            .rectangles
            .iter()
            .map(|r| smithay::utils::Rectangle::new((r.x as i32, r.y as i32).into(), (r.width as i32, r.height as i32).into()))
            .collect();
        let geo = xwindow.geometry();
        if rects.len() == 1 && rects[0].loc.x <= 0 && rects[0].loc.y <= 0
            && rects[0].loc.x + rects[0].size.w >= geo.size.w
            && rects[0].loc.y + rects[0].size.h >= geo.size.h
        {
            return None;
        }
        Some(rects)
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
        for (i, win) in self.wm.windows.iter().enumerate() {
            let kind = match win.surface {
                WindowSurface::Wayland(_) => "wayland",
                WindowSurface::X11(_) => "x11",
                WindowSurface::Dummy => "dummy",
            };
            let content_geo = win.content_geometry();
            let title_escaped = win.title.replace('\\', "\\\\").replace('\"', "\\\"");
            let comma = if i + 1 < self.wm.windows.len() { "," } else { "" };
            out.push_str(&format!(
                "    {{\"id\": {}, \"kind\": \"{}\", \"title\": \"{}\", \"active\": {}, \"is_maximized\": {}, \"is_minimized\": {}, \"x\": {}, \"y\": {}, \"width\": {}, \"height\": {}}}{}\n",
                win.id, kind, title_escaped, win.active, win.is_maximized, win.is_minimized, content_geo.loc.x, content_geo.loc.y, content_geo.size.w, content_geo.size.h, comma
            ));
        }
        out.push_str("  ],\n");

        // Popups
        out.push_str("  \"popups\": [\n");
        let mut popup_entries = Vec::new();
        for win in &self.wm.windows {
            if let WindowSurface::Wayland(ref toplevel) = win.surface {
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
