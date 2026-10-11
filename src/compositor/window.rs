use smithay::{
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, Rectangle, Size},
    wayland::shell::xdg::ToplevelSurface,
    xwayland::X11Surface,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowDragState {
    None,
    Moving {
        start_cursor: (f64, f64),
        start_window_pos: (i32, i32),
    },
    Resizing {
        start_cursor: (f64, f64),
        start_window_geo: Rectangle<i32, Logical>,
    },
}

#[derive(Clone)]
pub enum WindowSurface {
    Wayland(ToplevelSurface),
    X11(X11Surface),
    #[allow(dead_code)]
    Dummy,
}

#[derive(Clone)]
pub struct ManagedWindow {
    pub id: usize,
    pub title: String,
    pub surface: WindowSurface,
    pub loc: Point<i32, Logical>,
    pub size: Size<i32, Logical>,
    pub active: bool,
    pub has_ssd: bool,
    pub is_maximized: bool,
    pub is_minimized: bool,
    pub pending_unmaximize: bool,
    pub unmaximize_in_flight_count: u8,
    pub saved_geometry: Option<Rectangle<i32, Logical>>,
    pub associated_wl_surface: Option<WlSurface>,
    /// X11 Shape extension bounding rectangles (window-local). `None` means unshaped.
    pub x11_shape: Option<Vec<Rectangle<i32, Logical>>>,
}

pub const TITLEBAR_HEIGHT: i32 = 22;
pub const BORDER_WIDTH: i32 = 1;
pub const BUTTON_SIZE: i32 = 16;

fn get_wayland_title(surface: &ToplevelSurface) -> Option<String> {
    smithay::wayland::compositor::with_states(surface.wl_surface(), |states| {
        states
            .data_map
            .get::<smithay::wayland::shell::xdg::XdgToplevelSurfaceData>()
            .and_then(|d| d.lock().ok())
            .and_then(|attrs| attrs.title.clone())
    })
}

impl ManagedWindow {
    pub fn new_wayland(
        id: usize,
        surface: ToplevelSurface,
        loc: Point<i32, Logical>,
        size: Size<i32, Logical>,
        has_ssd: bool,
    ) -> Self {
        let title = get_wayland_title(&surface).unwrap_or_else(|| "Wayland Application".to_string());
        let wl_surf = Some(surface.wl_surface().clone());
        Self {
            id,
            title,
            surface: WindowSurface::Wayland(surface),
            loc,
            size,
            active: true,
            has_ssd,
            is_maximized: false,
            is_minimized: false,
            pending_unmaximize: false,
            unmaximize_in_flight_count: 0,
            saved_geometry: None,
            associated_wl_surface: wl_surf,
            x11_shape: None,
        }
    }

    pub fn new_x11(id: usize, surface: X11Surface, loc: Point<i32, Logical>, size: Size<i32, Logical>) -> Self {
        let t = surface.title();
        let title = if t.is_empty() { "X11 Application".to_string() } else { t };
        let has_ssd = !surface.is_decorated();
        let wl_surf = surface.wl_surface();
        Self {
            id,
            title,
            surface: WindowSurface::X11(surface),
            loc,
            size,
            active: true,
            has_ssd,
            is_maximized: false,
            is_minimized: false,
            pending_unmaximize: false,
            unmaximize_in_flight_count: 0,
            saved_geometry: None,
            associated_wl_surface: wl_surf,
            x11_shape: None,
        }
    }

    pub fn has_ssd(&self) -> bool {
        self.has_ssd
    }

    pub fn is_polkit(&self, polkit_pid: Option<u32>) -> bool {
        match &self.surface {
            WindowSurface::X11(xwin) => {
                if let (Some(ppid), Some(wpid)) = (polkit_pid, xwin.pid()) {
                    if ppid == wpid {
                        return true;
                    }
                }
                let title = xwin.title().to_lowercase();
                let class = xwin.class().to_lowercase();
                title.contains("authentication")
                    || title.contains("polkit")
                    || class.contains("polkit")
                    || class.contains("policykit")
            }
            WindowSurface::Wayland(_) => {
                let title = self.title.to_lowercase();
                title.contains("authentication")
                    || title.contains("polkit")
                    || title.contains("policykit")
            }
            _ => false,
        }
    }

    #[allow(dead_code)]
    pub fn new_dummy(id: usize, title: String, loc: Point<i32, Logical>, size: Size<i32, Logical>) -> Self {
        Self {
            id,
            title,
            surface: WindowSurface::Dummy,
            loc,
            size,
            active: true,
            has_ssd: true,
            is_maximized: false,
            is_minimized: false,
            pending_unmaximize: false,
            unmaximize_in_flight_count: 0,
            saved_geometry: None,
            associated_wl_surface: None,
            x11_shape: None,
        }
    }

    pub fn wl_surface(&self) -> Option<WlSurface> {
        match &self.surface {
            WindowSurface::Wayland(w) => Some(w.wl_surface().clone()),
            WindowSurface::X11(w) => w.wl_surface().or_else(|| self.cached_wl_surface()),
            WindowSurface::Dummy => None,
        }
    }

    /// Returns the cached associated WlSurface if it is still alive.
    pub fn cached_wl_surface(&self) -> Option<WlSurface> {
        self.associated_wl_surface
            .as_ref()
            .filter(|s| smithay::reexports::wayland_server::Resource::is_alive(*s))
            .cloned()
    }


    /// Full frame geometry including titlebar and border (if has_ssd is true)
    pub fn frame_geometry(&self) -> Rectangle<i32, Logical> {
        if self.has_ssd {
            Rectangle::new(
                (self.loc.x - BORDER_WIDTH, self.loc.y - TITLEBAR_HEIGHT - BORDER_WIDTH).into(),
                (
                    self.size.w + BORDER_WIDTH * 2,
                    self.size.h + TITLEBAR_HEIGHT + BORDER_WIDTH * 2,
                ).into(),
            )
        } else {
            Rectangle::new(self.loc, self.size)
        }
    }

    /// Content geometry (the client surface area)
    pub fn content_geometry(&self) -> Rectangle<i32, Logical> {
        Rectangle::new(self.loc, self.size)
    }

    /// Titlebar geometry
    pub fn titlebar_geometry(&self) -> Rectangle<i32, Logical> {
        if self.has_ssd {
            Rectangle::new(
                (self.loc.x - BORDER_WIDTH, self.loc.y - TITLEBAR_HEIGHT - BORDER_WIDTH).into(),
                (self.size.w + BORDER_WIDTH * 2, TITLEBAR_HEIGHT).into(),
            )
        } else {
            Rectangle::new((0, 0).into(), (0, 0).into())
        }
    }

    /// Close button geometry located at top-right of titlebar (for visual rendering)
    pub fn close_button_geometry(&self) -> Rectangle<i32, Logical> {
        if !self.has_ssd {
            return Rectangle::new((0, 0).into(), (0, 0).into());
        }
        let tb = self.titlebar_geometry();
        let btn_x = tb.loc.x + tb.size.w - (BUTTON_SIZE + 4);
        let btn_y = tb.loc.y + (TITLEBAR_HEIGHT - BUTTON_SIZE) / 2;
        Rectangle::new((btn_x, btn_y).into(), (BUTTON_SIZE, BUTTON_SIZE).into())
    }

    /// Expanded hit test area for close button (spans from right edge to divider before maximize button)
    pub fn close_button_hit_geometry(&self) -> Rectangle<i32, Logical> {
        if !self.has_ssd {
            return Rectangle::new((0, 0).into(), (0, 0).into());
        }
        let tb = self.titlebar_geometry();
        let hit_w = BUTTON_SIZE + 6; // 22px: covers tb.size.w - 22 to tb.size.w
        let btn_x = tb.loc.x + tb.size.w - hit_w;
        Rectangle::new((btn_x, tb.loc.y).into(), (hit_w, TITLEBAR_HEIGHT).into())
    }

    /// Maximize / Restore button geometry (to the left of close button)
    pub fn maximize_button_geometry(&self) -> Rectangle<i32, Logical> {
        if !self.has_ssd {
            return Rectangle::new((0, 0).into(), (0, 0).into());
        }
        let tb = self.titlebar_geometry();
        let btn_x = tb.loc.x + tb.size.w - (BUTTON_SIZE + 4) * 2;
        let btn_y = tb.loc.y + (TITLEBAR_HEIGHT - BUTTON_SIZE) / 2;
        Rectangle::new((btn_x, btn_y).into(), (BUTTON_SIZE, BUTTON_SIZE).into())
    }

    /// Expanded hit test area for maximize button (centered on maximize button, immediately adjacent to close hit area)
    pub fn maximize_button_hit_geometry(&self) -> Rectangle<i32, Logical> {
        if !self.has_ssd {
            return Rectangle::new((0, 0).into(), (0, 0).into());
        }
        let tb = self.titlebar_geometry();
        let close_w = BUTTON_SIZE + 6; // 22px
        let hit_w = BUTTON_SIZE + 4;  // 20px: covers tb.size.w - 42 to tb.size.w - 22
        let btn_x = tb.loc.x + tb.size.w - close_w - hit_w;
        Rectangle::new((btn_x, tb.loc.y).into(), (hit_w, TITLEBAR_HEIGHT).into())
    }

    /// Minimize button geometry (to the left of maximize button)
    pub fn minimize_button_geometry(&self) -> Rectangle<i32, Logical> {
        if !self.has_ssd {
            return Rectangle::new((0, 0).into(), (0, 0).into());
        }
        let tb = self.titlebar_geometry();
        let btn_x = tb.loc.x + tb.size.w - (BUTTON_SIZE + 4) * 3;
        let btn_y = tb.loc.y + (TITLEBAR_HEIGHT - BUTTON_SIZE) / 2;
        Rectangle::new((btn_x, btn_y).into(), (BUTTON_SIZE, BUTTON_SIZE).into())
    }

    /// Expanded hit test area for minimize button (to the left of maximize hit area)
    pub fn minimize_button_hit_geometry(&self) -> Rectangle<i32, Logical> {
        if !self.has_ssd {
            return Rectangle::new((0, 0).into(), (0, 0).into());
        }
        let tb = self.titlebar_geometry();
        let close_w = BUTTON_SIZE + 6; // 22px
        let max_w = BUTTON_SIZE + 4;   // 20px
        let hit_w = BUTTON_SIZE + 4;   // 20px: covers tb.size.w - 62 to tb.size.w - 42
        let btn_x = tb.loc.x + tb.size.w - close_w - max_w - hit_w;
        Rectangle::new((btn_x, tb.loc.y).into(), (hit_w, TITLEBAR_HEIGHT).into())
    }

    /// Resize handle at bottom-right corner (for visual rendering)
    pub fn resize_handle_geometry(&self) -> Rectangle<i32, Logical> {
        if !self.has_ssd {
            return Rectangle::new((0, 0).into(), (0, 0).into());
        }
        let fg = self.frame_geometry();
        Rectangle::new(
            (fg.loc.x + fg.size.w - 12, fg.loc.y + fg.size.h - 12).into(),
            (12, 12).into(),
        )
    }

    /// Expanded hit test area for resize handle (28x28 corner area for terminal mouse cells)
    pub fn resize_handle_hit_geometry(&self) -> Rectangle<i32, Logical> {
        if !self.has_ssd {
            return Rectangle::new((0, 0).into(), (0, 0).into());
        }
        let fg = self.frame_geometry();
        let hit_size = 28;
        Rectangle::new(
            (fg.loc.x + fg.size.w - hit_size, fg.loc.y + fg.size.h - hit_size).into(),
            (hit_size, hit_size).into(),
        )
    }

    pub fn update_title(&mut self) {
        match &self.surface {
            WindowSurface::Wayland(w) => {
                if let Some(t) = get_wayland_title(w) {
                    if !t.is_empty() {
                        self.title = t;
                    }
                }
            }
            WindowSurface::X11(w) => {
                let t = w.title();
                if !t.is_empty() {
                    self.title = t;
                }
            }
            WindowSurface::Dummy => {}
        }
    }

    pub fn close(&self) {
        match &self.surface {
            WindowSurface::Wayland(w) => {
                w.send_close();
            }
            WindowSurface::X11(w) => {
                let _ = w.close();
            }
            WindowSurface::Dummy => {}
        }
    }

    pub fn set_loc(&mut self, new_loc: Point<i32, Logical>) {
        self.loc = new_loc;
        if let WindowSurface::X11(win) = &self.surface {
            let rect = Rectangle::new(
                self.loc,
                self.size,
            );
            let _ = win.configure(rect);
        }
    }

    pub fn configure_size(&mut self, new_w: i32, new_h: i32) {
        let min_w = 100;
        let min_h = 60;
        let w = new_w.max(min_w);
        let h = new_h.max(min_h);
        self.size = (w, h).into();

        match &self.surface {
            WindowSurface::Wayland(win) => {
                win.with_pending_state(|state| {
                    state.size = Some((w, h).into());
                });
                win.send_configure();
            }
            WindowSurface::X11(win) => {
                let rect = Rectangle::new(
                    (self.loc.x, self.loc.y).into(),
                    (w, h).into(),
                );
                let _ = win.configure(rect);
            }
            WindowSurface::Dummy => {}
        }
    }

    /// Computes the default 80% centered geometry for a window within the given screen dimensions
    pub fn default_80pct_geometry(screen_w: u32, screen_h: u32, has_ssd: bool) -> Rectangle<i32, Logical> {
        let max_w = if has_ssd {
            (screen_w as i32 - BORDER_WIDTH * 2).max(100)
        } else {
            (screen_w as i32).max(100)
        };
        let max_h = if has_ssd {
            (screen_h as i32 - TITLEBAR_HEIGHT - BORDER_WIDTH * 2).max(60)
        } else {
            (screen_h as i32).max(60)
        };

        let w = ((screen_w as i32 * 8) / 10).clamp(320.min(max_w), max_w);
        let h = ((screen_h as i32 * 8) / 10).clamp(240.min(max_h), max_h);

        let total_frame_w = if has_ssd { w + BORDER_WIDTH * 2 } else { w };
        let total_frame_h = if has_ssd { h + TITLEBAR_HEIGHT + BORDER_WIDTH * 2 } else { h };

        let frame_x = ((screen_w as i32 - total_frame_w) / 2).max(0);
        let frame_y = ((screen_h as i32 - total_frame_h) / 2).max(0);

        let loc_x = if has_ssd { frame_x + BORDER_WIDTH } else { frame_x };
        let loc_y = if has_ssd { frame_y + TITLEBAR_HEIGHT + BORDER_WIDTH } else { frame_y };

        Rectangle::new(Point::from((loc_x, loc_y)), Size::from((w, h)))
    }

    /// Maximize window to fill the available display area
    pub fn maximize(&mut self, screen_w: u32, screen_h: u32) {
        if self.is_maximized {
            return;
        }
        self.saved_geometry = Some(Rectangle::new(self.loc, self.size));
        self.is_maximized = true;
        self.pending_unmaximize = false;
        self.unmaximize_in_flight_count = 0;

        let (new_loc, new_w, new_h) = if self.has_ssd {
            let loc = Point::from((BORDER_WIDTH, TITLEBAR_HEIGHT + BORDER_WIDTH));
            let w = (screen_w as i32 - BORDER_WIDTH * 2).max(100);
            let h = (screen_h as i32 - TITLEBAR_HEIGHT - BORDER_WIDTH * 2).max(60);
            (loc, w, h)
        } else {
            let loc = Point::from((0, 0));
            let w = (screen_w as i32).max(100);
            let h = (screen_h as i32).max(60);
            (loc, w, h)
        };

        self.loc = new_loc;
        self.size = (new_w, new_h).into();

        match &self.surface {
            WindowSurface::Wayland(win) => {
                win.with_pending_state(|state| {
                    state.states.set(smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Maximized);
                    state.size = Some((new_w, new_h).into());
                });
                win.send_configure();
            }
            WindowSurface::X11(win) => {
                let rect = Rectangle::new(self.loc, self.size);
                let _ = win.configure(rect);
                let _ = win.set_maximized(true);
            }
            WindowSurface::Dummy => {}
        }
    }

    /// Update maximized window dimensions when display/terminal resolution changes
    pub fn update_maximized_size(&mut self, screen_w: u32, screen_h: u32) {
        if !self.is_maximized {
            return;
        }
        let (new_loc, new_w, new_h) = if self.has_ssd {
            let loc = Point::from((BORDER_WIDTH, TITLEBAR_HEIGHT + BORDER_WIDTH));
            let w = (screen_w as i32 - BORDER_WIDTH * 2).max(100);
            let h = (screen_h as i32 - TITLEBAR_HEIGHT - BORDER_WIDTH * 2).max(60);
            (loc, w, h)
        } else {
            let loc = Point::from((0, 0));
            let w = (screen_w as i32).max(100);
            let h = (screen_h as i32).max(60);
            (loc, w, h)
        };

        self.loc = new_loc;
        self.size = (new_w, new_h).into();

        // When display resolution changes while maximized, update saved_geometry to 80% centered on the new resolution
        self.saved_geometry = Some(Self::default_80pct_geometry(screen_w, screen_h, self.has_ssd));

        match &self.surface {
            WindowSurface::Wayland(win) => {
                win.with_pending_state(|state| {
                    state.states.set(smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Maximized);
                    state.size = Some((new_w, new_h).into());
                });
                win.send_configure();
            }
            WindowSurface::X11(win) => {
                let rect = Rectangle::new(self.loc, self.size);
                let _ = win.configure(rect);
                let _ = win.set_maximized(true);
            }
            WindowSurface::Dummy => {}
        }
    }

    /// Restore window from maximized state to its previous saved geometry or 80% screen fallback
    pub fn unmaximize(&mut self, screen_w: u32, screen_h: u32) {
        if !self.is_maximized {
            return;
        }
        self.is_maximized = false;
        self.pending_unmaximize = true;
        self.unmaximize_in_flight_count = 0;
        let raw_geo = self.saved_geometry.take().unwrap_or_else(|| {
            Self::default_80pct_geometry(screen_w, screen_h, self.has_ssd)
        });
        let target_geo = if raw_geo.size.w >= screen_w as i32 - 10 || raw_geo.size.h >= screen_h as i32 - 40 {
            Self::default_80pct_geometry(screen_w, screen_h, self.has_ssd)
        } else {
            raw_geo
        };

        self.loc = target_geo.loc;
        self.size = target_geo.size;

        match &self.surface {
            WindowSurface::Wayland(win) => {
                win.with_pending_state(|state| {
                    state.states.unset(smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Maximized);
                    state.size = Some(target_geo.size);
                });
                win.send_configure();
            }
            WindowSurface::X11(win) => {
                let rect = Rectangle::new(self.loc, self.size);
                let _ = win.configure(rect);
                let _ = win.set_maximized(false);
            }
            WindowSurface::Dummy => {}
        }
    }

    /// Toggle maximized state
    pub fn toggle_maximize(&mut self, screen_w: u32, screen_h: u32) {
        if self.is_maximized {
            self.unmaximize(screen_w, screen_h);
        } else {
            self.maximize(screen_w, screen_h);
        }
    }
}

