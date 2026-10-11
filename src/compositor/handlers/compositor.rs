//! Core Wayland surface protocols: `wl_compositor` commit handling, `wl_buffer`,
//! `wl_shm` and `wl_output`.

use smithay::{
    reexports::wayland_server::{protocol::{wl_buffer, wl_surface::WlSurface}, Client},
    wayland::{
        buffer::BufferHandler,
        compositor::{CompositorClientState, CompositorHandler, CompositorState},
        output::OutputHandler,
        shell::xdg::XDG_POPUP_ROLE,
        shm::{ShmHandler, ShmState},
    },
};

use crate::compositor::state::{ClientState, TermwayState};
use crate::compositor::window::{WindowSurface, BORDER_WIDTH, TITLEBAR_HEIGHT};

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
        self.wm.windows.retain(|w| {
            w.wl_surface().as_ref().map(smithay::reexports::wayland_server::Resource::is_alive).unwrap_or(true)
        });

        // Dismiss splash screen upon first real client surface commit
        self.dismiss_splash();

        // タイトルや状態の更新
        let matched_win_id = self.wm.windows.iter().find(|w| {
            w.wl_surface().as_ref() == Some(surface) || self.surface_for_managed_window(w).as_ref() == Some(surface)
        }).map(|w| w.id);

        if let Some(win_id) = matched_win_id {
            // X11: remember the resolved surface and refresh the shape region
            let x11_shape = self.wm.get(win_id).and_then(|w| match &w.surface {
                WindowSurface::X11(x) => Some(self.query_x11_shape(x)),
                _ => None,
            });
            if let Some(w) = self.wm.get_mut(win_id) {
                if let Some(shape) = x11_shape {
                    w.associated_wl_surface = Some(surface.clone());
                    w.x11_shape = shape;
                }
                w.update_title();
                if self.enable_wm {
                    let bbox = smithay::desktop::utils::bbox_from_surface_tree(surface, (0, 0));
                    if bbox.size.w > 0 && bbox.size.h > 0 {
                        let top_margin = if w.has_ssd { TITLEBAR_HEIGHT + BORDER_WIDTH * 2 } else { 0 };
                        let side_margin = if w.has_ssd { BORDER_WIDTH * 2 } else { 0 };
                        let max_allowed_w = (self.width as i32 - side_margin).max(160);
                        let max_allowed_h = (self.height as i32 - top_margin).max(100);

                        if !w.is_maximized {
                            if w.pending_unmaximize {
                                // While pending_unmaximize is true, the target unmaximized geometry is already set in w.size/w.loc.
                                // If the client commits a buffer that is still approximately the maximized size,
                                // it is an in-flight buffer from the previous maximized state. Skip at most 2 frames.
                                let is_old_large_buffer = (bbox.size.w >= self.width as i32 - 10 && bbox.size.h >= self.height as i32 - top_margin - 10)
                                    && w.unmaximize_in_flight_count < 30;
                                if is_old_large_buffer {
                                    w.unmaximize_in_flight_count += 1;
                                    return;
                                }
                                w.pending_unmaximize = false;

                                let clamped_w = bbox.size.w.min(max_allowed_w);
                                let clamped_h = bbox.size.h.min(max_allowed_h);
                                w.size = (clamped_w, clamped_h).into();

                                // Center the unmaximized window based on its actual frame size
                                let total_frame_w = if w.has_ssd { w.size.w + BORDER_WIDTH * 2 } else { w.size.w };
                                let total_frame_h = if w.has_ssd { w.size.h + TITLEBAR_HEIGHT + BORDER_WIDTH * 2 } else { w.size.h };
                                let frame_x = ((self.width as i32 - total_frame_w) / 2).max(0);
                                let frame_y = ((self.height as i32 - total_frame_h) / 2).max(0);
                                w.loc.x = if w.has_ssd { frame_x + BORDER_WIDTH } else { frame_x };
                                w.loc.y = if w.has_ssd { frame_y + TITLEBAR_HEIGHT + BORDER_WIDTH } else { frame_y };

                                if clamped_w != bbox.size.w || clamped_h != bbox.size.h {
                                    w.configure_size(clamped_w, clamped_h);
                                }
                            } else {
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
        for w in &self.toplevels {
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
        for w in &self.wm.windows {
            if w.wl_surface().as_ref() == Some(surface) || self.surface_for_managed_window(w).as_ref() == Some(surface) {
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

// 4. ShmHandler
impl ShmHandler for TermwayState {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

// 6. OutputHandler
impl OutputHandler for TermwayState {}
