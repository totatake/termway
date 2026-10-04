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
    pub fn new_wayland(id: usize, surface: ToplevelSurface, loc: Point<i32, Logical>, size: Size<i32, Logical>) -> Self {
        let title = get_wayland_title(&surface).unwrap_or_else(|| "Wayland Application".to_string());
        Self {
            id,
            title,
            surface: WindowSurface::Wayland(surface),
            loc,
            size,
            active: true,
        }
    }

    pub fn new_x11(id: usize, surface: X11Surface, loc: Point<i32, Logical>, size: Size<i32, Logical>) -> Self {
        let t = surface.title();
        let title = if t.is_empty() { "X11 Application".to_string() } else { t };
        Self {
            id,
            title,
            surface: WindowSurface::X11(surface),
            loc,
            size,
            active: true,
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
        }
    }

    pub fn wl_surface(&self) -> Option<WlSurface> {
        match &self.surface {
            WindowSurface::Wayland(w) => Some(w.wl_surface().clone()),
            WindowSurface::X11(w) => w.wl_surface(),
            WindowSurface::Dummy => None,
        }
    }

    /// Full frame geometry including titlebar and border
    pub fn frame_geometry(&self) -> Rectangle<i32, Logical> {
        Rectangle::new(
            (self.loc.x - BORDER_WIDTH, self.loc.y - TITLEBAR_HEIGHT - BORDER_WIDTH).into(),
            (
                self.size.w + BORDER_WIDTH * 2,
                self.size.h + TITLEBAR_HEIGHT + BORDER_WIDTH * 2,
            ).into(),
        )
    }

    /// Content geometry (the client surface area)
    pub fn content_geometry(&self) -> Rectangle<i32, Logical> {
        Rectangle::new(self.loc, self.size)
    }

    /// Titlebar geometry
    pub fn titlebar_geometry(&self) -> Rectangle<i32, Logical> {
        Rectangle::new(
            (self.loc.x - BORDER_WIDTH, self.loc.y - TITLEBAR_HEIGHT - BORDER_WIDTH).into(),
            (self.size.w + BORDER_WIDTH * 2, TITLEBAR_HEIGHT).into(),
        )
    }

    /// Close button geometry located at top-right of titlebar (for visual rendering)
    pub fn close_button_geometry(&self) -> Rectangle<i32, Logical> {
        let tb = self.titlebar_geometry();
        let btn_x = tb.loc.x + tb.size.w - BUTTON_SIZE - 4;
        let btn_y = tb.loc.y + (TITLEBAR_HEIGHT - BUTTON_SIZE) / 2;
        Rectangle::new((btn_x, btn_y).into(), (BUTTON_SIZE, BUTTON_SIZE).into())
    }

    /// Expanded hit test area for close button (generous 32px width and full titlebar height for terminal mouse cells)
    pub fn close_button_hit_geometry(&self) -> Rectangle<i32, Logical> {
        let tb = self.titlebar_geometry();
        let hit_w = 32;
        let btn_x = tb.loc.x + tb.size.w - hit_w;
        Rectangle::new((btn_x, tb.loc.y).into(), (hit_w, TITLEBAR_HEIGHT).into())
    }

    /// Resize handle at bottom-right corner (for visual rendering)
    pub fn resize_handle_geometry(&self) -> Rectangle<i32, Logical> {
        let fg = self.frame_geometry();
        Rectangle::new(
            (fg.loc.x + fg.size.w - 12, fg.loc.y + fg.size.h - 12).into(),
            (12, 12).into(),
        )
    }

    /// Expanded hit test area for resize handle (28x28 corner area for terminal mouse cells)
    pub fn resize_handle_hit_geometry(&self) -> Rectangle<i32, Logical> {
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
                    (w as i32, h as i32).into(),
                );
                let _ = win.configure(rect);
            }
            WindowSurface::Dummy => {}
        }
    }
}

