use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            element::{
                surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
                Kind,
            },
            pixman::PixmanRenderer,
            utils::draw_render_elements,
            Bind, Color32F, Frame, Offscreen, Renderer,
        },
    },
    utils::{Buffer, Physical, Rectangle, Size, Transform},
};

use super::decoration::DecorationRenderer;
use super::state::TermwayState;
use super::window::WindowSurface;

pub struct TermwayRenderer {
    renderer: PixmanRenderer,
    buffer: pixman::Image<'static, 'static>,
    decoration_renderer: DecorationRenderer,
    width: u32,
    height: u32,
}

impl TermwayRenderer {
    pub fn new(width: u32, height: u32) -> Result<Self, Box<dyn std::error::Error>> {
        let mut renderer = PixmanRenderer::new()?;
        let buffer_size: Size<i32, Buffer> = Size::from((width as i32, height as i32));
        // Use Abgr8888 so that in little-endian the bytes in memory are [R, G, B, A]
        let buffer = renderer.create_buffer(Fourcc::Abgr8888, buffer_size)?;
        let decoration_renderer = DecorationRenderer::new();

        Ok(Self {
            renderer,
            buffer,
            decoration_renderer,
            width,
            height,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), Box<dyn std::error::Error>> {
        if self.width == width && self.height == height {
            return Ok(());
        }
        let buffer_size: Size<i32, Buffer> = Size::from((width as i32, height as i32));
        self.buffer = self.renderer.create_buffer(Fourcc::Abgr8888, buffer_size)?;
        self.width = width;
        self.height = height;
        Ok(())
    }

    pub fn render(&mut self, state: &TermwayState) -> Result<&[u8], Box<dyn std::error::Error>> {
        let phys_size: Size<i32, Physical> = Size::from((self.width as i32, self.height as i32));
        let damage: Rectangle<i32, Physical> = Rectangle::from_size(phys_size);

        // 1. Collect render elements from surfaces (Front to Back order for draw_render_elements)
        let mut elements: Vec<WaylandSurfaceRenderElement<PixmanRenderer>> = Vec::new();

        // A. Render Wayland XDG popups on top
        for window in state.managed_windows.iter().rev() {
            if let WindowSurface::Wayland(ref toplevel) = window.surface {
                for (popup, popup_offset) in smithay::desktop::PopupManager::popups_for_surface(toplevel.wl_surface()) {
                    let popup_x = window.loc.x + popup_offset.x - popup.geometry().loc.x;
                    let popup_y = window.loc.y + popup_offset.y - popup.geometry().loc.y;
                    let elems = render_elements_from_surface_tree(
                        &mut self.renderer,
                        popup.wl_surface(),
                        (popup_x, popup_y),
                        1.0,
                        1.0,
                        Kind::Unspecified,
                    );
                    elements.extend(elems);
                }
            }
        }

        // B. Render unmanaged X11 popup/override-redirect surfaces on top
        for xwindow in state.x11_windows.iter().rev() {
            let is_managed = state.managed_windows.iter().any(|mw| matches!(&mw.surface, WindowSurface::X11(w) if w == xwindow));
            if !is_managed {
                let geo = xwindow.geometry();
                if let Some(surface) = state.surface_for_x11(xwindow) {
                    let elems = render_elements_from_surface_tree(
                        &mut self.renderer,
                        &surface,
                        (geo.loc.x, geo.loc.y),
                        1.0,
                        1.0,
                        Kind::Unspecified,
                    );
                    elements.extend(elems);
                }
            }
        }

        // B. Render managed windows in top-to-bottom order (rev() since managed_windows last is front)
        for window in state.managed_windows.iter().rev() {
            if let Some(surface) = state.surface_for_managed_window(window) {
                let elems = render_elements_from_surface_tree(
                    &mut self.renderer,
                    &surface,
                    (window.loc.x, window.loc.y),
                    1.0,
                    1.0,
                    Kind::Unspecified,
                );
                elements.extend(elems);
            }
        }

        // C. Fallback: if no elements collected from managed or X11 windows, render unassociated surfaces at (0, 0)
        if elements.is_empty() {
            for surface in &state.unassociated_surfaces {
                let elems = render_elements_from_surface_tree(
                    &mut self.renderer,
                    surface,
                    (0, 0),
                    1.0,
                    1.0,
                    Kind::Unspecified,
                );
                elements.extend(elems);
            }
        }

        // 2. Bind and render to offscreen pixman buffer
        let mut target = self.renderer.bind(&mut self.buffer)?;
        let mut frame = self.renderer.render(&mut target, phys_size, Transform::Normal)?;
        
        // Clear background to dark gray (#14141a)
        frame.clear(Color32F::new(0.08, 0.08, 0.10, 1.0), &[damage])?;
        draw_render_elements(&mut frame, 1.0, &elements, &[damage]).map_err(|e| format!("{:?}", e))?;
        let _ = frame.finish()?;

        // 3. Draw Openbox/Fluxbox window decorations (Bottom to Top order) if WM is enabled
        if state.enable_wm {
            for window in &state.managed_windows {
                if window.has_ssd() {
                    self.decoration_renderer.draw_decorations(
                        &mut self.buffer,
                        self.width,
                        self.height,
                        window,
                    );
                }
            }
        }

        // 3. Extract RGBA pixel slice
        let stride = self.buffer.stride();
        let data_ptr = unsafe { self.buffer.data() };
        let slice = unsafe {
            std::slice::from_raw_parts(data_ptr as *const u8, (self.height as usize) * stride)
        };

        Ok(slice)
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }
}
