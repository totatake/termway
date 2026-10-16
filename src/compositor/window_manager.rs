//! Window management domain: Z-order stack, activation, minimize state, drag/resize
//! interaction state and window ID allocation.
//!
//! `WindowManager` intentionally has no dependency on the Wayland `Display` or the
//! Smithay `Seat`, so its logic can be unit-tested in isolation. Operations that must
//! also move keyboard focus (e.g. raising a window) live on `TermwayState`, which
//! orchestrates `WindowManager` together with the seat.

use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::wayland::shell::xdg::ToplevelSurface;

use super::window::{ManagedWindow, WindowDragState, WindowSurface};

pub struct WindowManager {
    /// Managed windows in Z-order (index 0 = bottom, last = top).
    pub windows: Vec<ManagedWindow>,
    /// Current pointer-driven move/resize interaction.
    pub drag_state: WindowDragState,
    /// Last titlebar click (time, window id) for double-click detection.
    pub last_titlebar_click: Option<(std::time::Instant, usize)>,
    next_id: usize,
}

impl Default for WindowManager {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowManager {
    pub fn new() -> Self {
        Self {
            windows: Vec::new(),
            drag_state: WindowDragState::None,
            last_titlebar_click: None,
            next_id: 1,
        }
    }

    /// Allocates a new unique window ID (monotonically increasing, starting at 1).
    pub fn alloc_id(&mut self) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn len(&self) -> usize {
        self.windows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    pub fn get(&self, id: usize) -> Option<&ManagedWindow> {
        self.windows.iter().find(|w| w.id == id)
    }

    pub fn get_mut(&mut self, id: usize) -> Option<&mut ManagedWindow> {
        self.windows.iter_mut().find(|w| w.id == id)
    }

    /// Finds the managed window backing the given xdg toplevel.
    pub fn find_toplevel(&self, surface: &ToplevelSurface) -> Option<&ManagedWindow> {
        self.windows
            .iter()
            .find(|w| matches!(&w.surface, WindowSurface::Wayland(t) if t == surface))
    }

    /// Finds the managed window backing the given xdg toplevel (mutable).
    pub fn find_toplevel_mut(&mut self, surface: &ToplevelSurface) -> Option<&mut ManagedWindow> {
        self.windows
            .iter_mut()
            .find(|w| matches!(&w.surface, WindowSurface::Wayland(t) if t == surface))
    }

    /// Removes the managed window backing the given xdg toplevel.
    pub fn remove_toplevel(&mut self, surface: &ToplevelSurface) {
        self.windows
            .retain(|w| !matches!(&w.surface, WindowSurface::Wayland(t) if t == surface));
    }

    /// Marks every managed window as inactive.
    pub fn deactivate_all(&mut self) {
        for w in &mut self.windows {
            w.active = false;
        }
    }

    /// Marks the topmost window as active (used after a window has been removed).
    pub fn activate_top(&mut self) {
        if let Some(top) = self.windows.last_mut() {
            top.active = true;
        }
    }

    /// Moves the window to the top of the Z-order and makes it the only active window.
    /// Returns `false` if the ID is unknown.
    pub fn bring_to_top(&mut self, id: usize) -> bool {
        let Some(pos) = self.windows.iter().position(|w| w.id == id) else {
            return false;
        };
        let mut win = self.windows.remove(pos);
        self.deactivate_all();
        win.active = true;
        self.windows.push(win);
        true
    }

    /// Returns the ID of the topmost window that is not minimized.
    pub fn top_visible_id(&self) -> Option<usize> {
        self.windows.iter().rev().find(|w| !w.is_minimized).map(|w| w.id)
    }

    /// Sets the minimized flag and notifies the client of its activation state.
    /// Returns `false` if the ID is unknown.
    pub fn set_minimized(&mut self, id: usize, minimized: bool) -> bool {
        let Some(win) = self.get_mut(id) else {
            return false;
        };
        win.is_minimized = minimized;
        if minimized {
            win.active = false;
        }
        let activated = !minimized;
        match &win.surface {
            WindowSurface::Wayland(toplevel) => {
                toplevel.with_pending_state(|state| {
                    if activated {
                        state.states.set(xdg_toplevel::State::Activated);
                    } else {
                        state.states.unset(xdg_toplevel::State::Activated);
                    }
                });
                toplevel.send_configure();
            }
            WindowSurface::X11(xsurface) => {
                let _ = xsurface.set_activated(activated);
            }
            WindowSurface::Dummy => {}
        }
        true
    }

    /// Offset used to cascade newly opened non-maximized windows.
    pub fn cascade_offset(&self) -> i32 {
        ((self.windows.len() % 8) as i32) * 20
    }

    /// Updates geometries and constraints of managed windows when terminal/screen resolution changes.
    pub fn update_layout_for_resize(&mut self, width: u32, height: u32) {
        use super::window::{BORDER_WIDTH, TITLEBAR_HEIGHT};

        for m_win in &mut self.windows {
            if m_win.is_maximized {
                m_win.update_maximized_size(width, height);
            } else {
                let top_margin = if m_win.has_ssd { TITLEBAR_HEIGHT + BORDER_WIDTH * 2 } else { 0 };
                let side_margin = if m_win.has_ssd { BORDER_WIDTH * 2 } else { 0 };
                let max_w = (width as i32 - side_margin).max(160);
                let max_h = (height as i32 - top_margin).max(100);
                let new_w = m_win.size.w.min(max_w);
                let new_h = m_win.size.h.min(max_h);

                let min_x = if m_win.has_ssd { BORDER_WIDTH } else { 0 };
                let max_x = (width as i32 - new_w - min_x).max(min_x);
                let min_y = if m_win.has_ssd { TITLEBAR_HEIGHT + BORDER_WIDTH } else { 0 };
                let max_y = (height as i32 - new_h - min_x).max(min_y);
                let new_loc = smithay::utils::Point::from((
                    m_win.loc.x.clamp(min_x, max_x),
                    m_win.loc.y.clamp(min_y, max_y),
                ));

                let loc_changed = m_win.loc != new_loc;
                let size_changed = new_w != m_win.size.w || new_h != m_win.size.h;
                m_win.loc = new_loc;
                m_win.size = (new_w, new_h).into();

                if loc_changed || size_changed {
                    match &m_win.surface {
                        WindowSurface::Wayland(win) => {
                            if size_changed {
                                win.with_pending_state(|state| {
                                    state.size = Some((new_w, new_h).into());
                                });
                                win.send_configure();
                            }
                        }
                        WindowSurface::X11(win) => {
                            let rect = smithay::utils::Rectangle::new(m_win.loc, m_win.size);
                            let _ = win.configure(rect);
                        }
                        WindowSurface::Dummy => {}
                    }
                }
            }
        }
    }
}
