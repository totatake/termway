use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            element::surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
            pixman::PixmanRenderer,
            utils::draw_render_elements,
            Bind, Color32F, Frame, Offscreen, Renderer,
        },
    },
    utils::{Buffer, Physical, Point, Rectangle, Size, Transform},
};

use crate::error::{Result, TermwayError};
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
    pub fn new(width: u32, height: u32) -> Result<Self> {
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

    pub fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        if self.width == width && self.height == height {
            return Ok(());
        }
        let buffer_size: Size<i32, Buffer> = Size::from((width as i32, height as i32));
        self.buffer = self.renderer.create_buffer(Fourcc::Abgr8888, buffer_size)?;
        self.width = width;
        self.height = height;
        Ok(())
    }

    pub fn render(&mut self, state: &TermwayState) -> Result<&[u8]> {
        let phys_size: Size<i32, Physical> = Size::from((self.width as i32, self.height as i32));
        let screen_damage: Rectangle<i32, Physical> = Rectangle::from_size(phys_size);

        struct ManagedWindowRenderLayer {
            window_index: usize,
            surface_layer: Option<(
                Vec<WaylandSurfaceRenderElement<PixmanRenderer>>,
                Vec<Rectangle<i32, Physical>>,
            )>,
            popup_layers: Vec<(
                Vec<WaylandSurfaceRenderElement<PixmanRenderer>>,
                Rectangle<i32, Physical>,
            )>,
        }

        // 1. Collect render elements from surfaces with their clipping rectangles
        // Collect before creating `frame` to avoid multiple mutable borrows of `self.renderer`.
        let mut managed_layers: Vec<ManagedWindowRenderLayer> = Vec::with_capacity(state.wm.windows.len());
        let mut any_surface_elems = false;

        for (window_index, window) in state.wm.windows.iter().enumerate() {
            if window.is_minimized {
                continue;
            }

            let mut surface_layer = None;
            if let Some(surface) = state.surface_for_managed_window(window) {
                let win_clip: Rectangle<i32, Physical> = Rectangle::new(
                    Point::from((window.loc.x, window.loc.y)),
                    Size::from((window.size.w, window.size.h)),
                );
                // Shaped X11 windows: only draw inside the X Shape bounding region.
                // SSD windows: constrain client drawing inside the server-rendered window frame.
                // CSD windows: allow full client surface tree rendering (including decorations and shadows) clipped to screen.
                let clips: Vec<Rectangle<i32, Physical>> = match &window.x11_shape {
                    Some(rects) => rects
                        .iter()
                        .filter_map(|r| {
                            Rectangle::<i32, Physical>::new(
                                Point::from((window.loc.x + r.loc.x, window.loc.y + r.loc.y)),
                                Size::from((r.size.w, r.size.h)),
                            )
                            .intersection(win_clip)
                        })
                        .collect(),
                    None => {
                        if window.has_ssd {
                            vec![win_clip]
                        } else {
                            vec![screen_damage]
                        }
                    }
                };
                if !clips.is_empty() {
                    let elems: Vec<WaylandSurfaceRenderElement<PixmanRenderer>> = render_elements_from_surface_tree(
                        &mut self.renderer,
                        &surface,
                        (window.loc.x, window.loc.y),
                        1.0,
                        1.0,
                        smithay::backend::renderer::element::Kind::Unspecified,
                    );
                    if !elems.is_empty() {
                        any_surface_elems = true;
                        surface_layer = Some((elems, clips));
                    }
                }
            }

            let mut popup_layers = Vec::new();
            if let WindowSurface::Wayland(ref toplevel) = window.surface {
                for (popup, popup_offset) in smithay::desktop::PopupManager::popups_for_surface(toplevel.wl_surface()) {
                    let popup_x = window.loc.x + popup_offset.x - popup.geometry().loc.x;
                    let popup_y = window.loc.y + popup_offset.y - popup.geometry().loc.y;
                    let bbox = smithay::desktop::utils::bbox_from_surface_tree(popup.wl_surface(), (0, 0));
                    let popup_clip: Rectangle<i32, Physical> = Rectangle::new(
                        Point::from((popup_x, popup_y)),
                        Size::from((bbox.size.w, bbox.size.h)),
                    );
                    let elems: Vec<WaylandSurfaceRenderElement<PixmanRenderer>> = render_elements_from_surface_tree(
                        &mut self.renderer,
                        popup.wl_surface(),
                        (popup_x, popup_y),
                        1.0,
                        1.0,
                        smithay::backend::renderer::element::Kind::Unspecified,
                    );
                    if !elems.is_empty() {
                        popup_layers.push((elems, popup_clip));
                    }
                }
            }

            managed_layers.push(ManagedWindowRenderLayer {
                window_index,
                surface_layer,
                popup_layers,
            });
        }

        let mut x11_layers: Vec<(Vec<WaylandSurfaceRenderElement<PixmanRenderer>>, Rectangle<i32, Physical>)> = Vec::new();
        for xwindow in &state.x11_windows {
            let is_managed = state.wm.windows.iter().any(|mw| matches!(&mw.surface, WindowSurface::X11(w) if w == xwindow));
            if !is_managed {
                let geo = xwindow.geometry();
                if let Some(surface) = state.surface_for_x11(xwindow) {
                    let bbox = smithay::desktop::utils::bbox_from_surface_tree(&surface, (0, 0));
                    let w = if geo.size.w > 0 { geo.size.w } else { bbox.size.w };
                    let h = if geo.size.h > 0 { geo.size.h } else { bbox.size.h };
                    let x11_clip: Rectangle<i32, Physical> = Rectangle::new(
                        Point::from((geo.loc.x, geo.loc.y)),
                        Size::from((w, h)),
                    );

                    let elems: Vec<WaylandSurfaceRenderElement<PixmanRenderer>> = render_elements_from_surface_tree(
                        &mut self.renderer,
                        &surface,
                        (geo.loc.x, geo.loc.y),
                        1.0,
                        1.0,
                        smithay::backend::renderer::element::Kind::Unspecified,
                    );
                    if !elems.is_empty() {
                        x11_layers.push((elems, x11_clip));
                    }
                }
            }
        }

        let any_managed = !state.wm.windows.is_empty();
        let all_managed_minimized = any_managed && state.wm.windows.iter().all(|w| w.is_minimized);
        let mut fallback_layers: Vec<(Vec<WaylandSurfaceRenderElement<PixmanRenderer>>, Rectangle<i32, Physical>)> = Vec::new();

        if !any_surface_elems && !all_managed_minimized {
            let managed_surfaces: Vec<smithay::reexports::wayland_server::protocol::wl_surface::WlSurface> = state
                .wm.windows
                .iter()
                .filter_map(|w| state.surface_for_managed_window(w))
                .collect();

            for surface in &state.unassociated_surfaces {
                if managed_surfaces.contains(surface) {
                    continue;
                }
                let elems: Vec<WaylandSurfaceRenderElement<PixmanRenderer>> = render_elements_from_surface_tree(
                    &mut self.renderer,
                    surface,
                    (0, 0),
                    1.0,
                    1.0,
                    smithay::backend::renderer::element::Kind::Unspecified,
                );
                if !elems.is_empty() {
                    fallback_layers.push((elems, screen_damage));
                }
            }
        }

        // 2. Clear background to dark gray (#14141a)
        {
            let mut target = self.renderer.bind(&mut self.buffer)?;
            let mut frame = self.renderer.render(&mut target, phys_size, Transform::Normal)?;
            frame.clear(Color32F::new(0.08, 0.08, 0.10, 1.0), &[screen_damage])?;
            let _ = frame.finish()?;
        }

        // 3. Draw managed windows in strict bottom-to-top Z-order:
        //    For each window, render its client surface, then its SSD frame/titlebar, then its popups.
        //    This ensures any window higher in the stacking order properly occludes both the content
        //    and the window frame of windows behind it.
        for layer in &managed_layers {
            let window = &state.wm.windows[layer.window_index];

            if let Some((ref elems, ref win_clips)) = layer.surface_layer {
                let mut target = self.renderer.bind(&mut self.buffer)?;
                let mut frame = self.renderer.render(&mut target, phys_size, Transform::Normal)?;
                draw_render_elements(&mut frame, 1.0, elems, win_clips)
                    .map_err(|e| TermwayError::Renderer(format!("{:?}", e)))?;
                let _ = frame.finish()?;
            } else if window.x11_shape.is_none() {
                let win_clip: Rectangle<i32, Physical> = Rectangle::new(
                    Point::from((window.loc.x, window.loc.y)),
                    Size::from((window.size.w, window.size.h)),
                );
                if let Some(clipped) = win_clip.intersection(screen_damage) {
                    let mut target = self.renderer.bind(&mut self.buffer)?;
                    let mut frame = self.renderer.render(&mut target, phys_size, Transform::Normal)?;
                    frame.clear(Color32F::new(0.08, 0.08, 0.10, 1.0), &[clipped])?;
                    let _ = frame.finish()?;
                }
            }

            if state.enable_wm && window.has_ssd() {
                self.decoration_renderer.draw_decorations(
                    &mut self.buffer,
                    self.width,
                    self.height,
                    window,
                );
            }

            if !layer.popup_layers.is_empty() {
                let mut target = self.renderer.bind(&mut self.buffer)?;
                let mut frame = self.renderer.render(&mut target, phys_size, Transform::Normal)?;
                for (elems, popup_clip) in &layer.popup_layers {
                    draw_render_elements(&mut frame, 1.0, elems, &[*popup_clip])
                        .map_err(|e| TermwayError::Renderer(format!("{:?}", e)))?;
                }
                let _ = frame.finish()?;
            }
        }

        // 4. Draw unmanaged X11 popup/override-redirect surfaces and fallback surfaces on top
        if !x11_layers.is_empty() || !fallback_layers.is_empty() {
            let mut target = self.renderer.bind(&mut self.buffer)?;
            let mut frame = self.renderer.render(&mut target, phys_size, Transform::Normal)?;
            for (elems, x11_clip) in &x11_layers {
                draw_render_elements(&mut frame, 1.0, elems, &[*x11_clip])
                    .map_err(|e| TermwayError::Renderer(format!("{:?}", e)))?;
            }
            for (elems, clip) in &fallback_layers {
                draw_render_elements(&mut frame, 1.0, elems, &[*clip])
                    .map_err(|e| TermwayError::Renderer(format!("{:?}", e)))?;
            }
            let _ = frame.finish()?;
        }

        // 5. Draw startup splash screen if active
        if state.show_splash {
            let elapsed = state.start_time.elapsed().as_millis();
            self.decoration_renderer.draw_splash(
                &mut self.buffer,
                self.width,
                self.height,
                elapsed,
                &state.splash_info,
                &state.splash_command,
            );
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
