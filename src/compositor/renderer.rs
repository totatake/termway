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

use super::state::TermwayState;

pub struct TermwayRenderer {
    renderer: PixmanRenderer,
    buffer: pixman::Image<'static, 'static>,
    width: u32,
    height: u32,
}

impl TermwayRenderer {
    pub fn new(width: u32, height: u32) -> Result<Self, Box<dyn std::error::Error>> {
        let mut renderer = PixmanRenderer::new()?;
        let buffer_size: Size<i32, Buffer> = Size::from((width as i32, height as i32));
        // Use Abgr8888 so that in little-endian the bytes in memory are [R, G, B, A]
        let buffer = renderer.create_buffer(Fourcc::Abgr8888, buffer_size)?;

        Ok(Self {
            renderer,
            buffer,
            width,
            height,
        })
    }

    pub fn render(&mut self, state: &TermwayState) -> Result<&[u8], Box<dyn std::error::Error>> {
        let phys_size: Size<i32, Physical> = Size::from((self.width as i32, self.height as i32));
        let damage: Rectangle<i32, Physical> = Rectangle::from_size(phys_size);

        // 1. Collect render elements from all toplevel surfaces (Wayland XDG & XWayland)
        let mut elements: Vec<WaylandSurfaceRenderElement<PixmanRenderer>> = Vec::new();
        for window in &state.windows {
            let elems = render_elements_from_surface_tree(
                &mut self.renderer,
                window.wl_surface(),
                (0, 0),
                1.0,
                1.0,
                Kind::Unspecified,
            );
            elements.extend(elems);
        }

        let mut unassoc = state.unassociated_surfaces.clone();

        for xwindow in &state.x11_windows {
            let geo = xwindow.geometry();
            if let Some(surface) = xwindow.wl_surface().or_else(|| unassoc.pop()) {
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
        for surface in &unassoc {
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

        // 2. Bind and render to offscreen pixman buffer
        let mut target = self.renderer.bind(&mut self.buffer)?;
        let mut frame = self.renderer.render(&mut target, phys_size, Transform::Normal)?;
        
        // Clear background to dark gray
        frame.clear(Color32F::new(0.08, 0.08, 0.10, 1.0), &[damage])?;
        draw_render_elements(&mut frame, 1.0, &elements, &[damage]).map_err(|e| format!("{:?}", e))?;
        let _ = frame.finish()?;

        // 3. Extract RGBA pixel slice
        let stride = self.buffer.stride();
        let data_ptr = unsafe { self.buffer.data() };
        let slice = unsafe {
            std::slice::from_raw_parts(data_ptr as *const u8, (self.height as usize) * (stride as usize))
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
