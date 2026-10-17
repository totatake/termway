//! Input seat focus tracking, zwp_text_input_v3 IME protocol, and clipboard selection bridging
//! (Wayland data device <-> host terminal via OSC 52).

use std::os::unix::io::{FromRawFd, OwnedFd};

use smithay::{
    input::{Seat, SeatHandler, SeatState},
    reexports::{
        wayland_protocols::wp::text_input::zv3::server::{
            zwp_text_input_manager_v3::{self, ZwpTextInputManagerV3},
            zwp_text_input_v3::{self, ZwpTextInputV3},
        },
        wayland_server::{
            protocol::wl_surface::WlSurface, Client, DataInit, DisplayHandle, GlobalDispatch,
            New, Resource,
        },
    },
    wayland::selection::{
        data_device::{ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler},
        SelectionHandler, SelectionSource, SelectionTarget,
    },
};

use crate::compositor::state::{TermwayState, TrackedTextInput};

// 5. SeatHandler
impl SeatHandler for TermwayState {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let client = focused.and_then(|s| self.dh.get_client(s.id()).ok());
        smithay::wayland::selection::data_device::set_data_device_focus(&self.dh, seat, client);

        self.text_inputs.retain(|ti| ti.instance.is_alive());
        let new_focus = focused.cloned();
        if self.focused_surface != new_focus {
            if let Some(ref old_surface) = self.focused_surface {
                if old_surface.is_alive() {
                    for ti in &mut self.text_inputs {
                        if ti.instance.id().same_client_as(&old_surface.id()) {
                            ti.instance.leave(old_surface);
                            ti.enabled = false;
                            ti.pending_enable = None;
                        }
                    }
                }
            }
            self.focused_surface = new_focus;
            if let Some(ref new_surface) = self.focused_surface {
                if new_surface.is_alive() {
                    for ti in &mut self.text_inputs {
                        if ti.instance.id().same_client_as(&new_surface.id()) {
                            ti.instance.enter(new_surface);
                        }
                    }
                }
            }
        }
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, image: smithay::input::pointer::CursorImageStatus) {
        if !matches!(self.wm.drag_state, crate::compositor::window::WindowDragState::None) {
            return;
        }
        match image {
            smithay::input::pointer::CursorImageStatus::Hidden => {
                self.update_cursor_shape("none");
            }
            smithay::input::pointer::CursorImageStatus::Named(icon) => {
                let name = icon.name();
                self.update_cursor_shape(name);
            }
            smithay::input::pointer::CursorImageStatus::Surface(_) => {
                // Client provides custom surface; default cursor is fallback
            }
        }
    }
}

// 6. zwp_text_input_v3 (Wayland native IME / UTF-8 text input)
impl GlobalDispatch<ZwpTextInputManagerV3, ()> for TermwayState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZwpTextInputManagerV3>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl smithay::reexports::wayland_server::Dispatch<ZwpTextInputManagerV3, ()> for TermwayState {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &ZwpTextInputManagerV3,
        request: zwp_text_input_manager_v3::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let zwp_text_input_manager_v3::Request::GetTextInput { id, .. } = request {
            let instance: ZwpTextInputV3 = data_init.init(id, ());
            if let Some(ref surface) = state.focused_surface {
                if surface.is_alive() && surface.id().same_client_as(&instance.id()) {
                    instance.enter(surface);
                }
            }
            state.text_inputs.push(TrackedTextInput {
                instance,
                serial: 0,
                enabled: false,
                pending_enable: None,
            });
        }
    }
}

impl smithay::reexports::wayland_server::Dispatch<ZwpTextInputV3, ()> for TermwayState {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &ZwpTextInputV3,
        request: zwp_text_input_v3::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if matches!(request, zwp_text_input_v3::Request::Destroy) {
            state.text_inputs.retain(|ti| &ti.instance != resource);
            return;
        }
        if let Some(ti) = state.text_inputs.iter_mut().find(|ti| &ti.instance == resource) {
            match request {
                zwp_text_input_v3::Request::Enable => {
                    ti.pending_enable = Some(true);
                }
                zwp_text_input_v3::Request::Disable => {
                    ti.pending_enable = Some(false);
                }
                zwp_text_input_v3::Request::Commit => {
                    ti.serial = ti.serial.wrapping_add(1);
                    if let Some(en) = ti.pending_enable.take() {
                        ti.enabled = en;
                    }
                }
                _ => {}
            }
        }
    }
}

// 7. SelectionHandler & DataDeviceHandler
impl SelectionHandler for TermwayState {
    type SelectionUserData = Vec<u8>;

    fn new_selection(&mut self, ty: SelectionTarget, source: Option<SelectionSource>, seat: Seat<Self>) {
        if ty != SelectionTarget::Clipboard {
            return;
        }
        let Some(source) = source else {
            return;
        };
        let mimes = source.mime_types();
        let text_mime = mimes.iter().find(|m| {
            m.as_str() == "text/plain;charset=utf-8"
                || m.as_str() == "UTF8_STRING"
                || m.as_str() == "text/plain"
                || m.starts_with("text/")
        }).cloned();

        if let Some(mime) = text_mime {
            let mut fds = [0i32; 2];
            if unsafe { libc::pipe(fds.as_mut_ptr()) } == 0 {
                let read_fd = unsafe { OwnedFd::from_raw_fd(fds[0]) };
                let write_fd = unsafe { OwnedFd::from_raw_fd(fds[1]) };

                if smithay::wayland::selection::data_device::request_data_device_client_selection(
                    &seat,
                    mime,
                    write_fd,
                ).is_ok() {
                    std::thread::spawn(move || {
                        use std::io::{Read, Write};
                        use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
                        let mut file = std::fs::File::from(read_fd);
                        let mut data = Vec::new();
                        if file.read_to_end(&mut data).is_ok() {
                            if let Ok(text) = std::str::from_utf8(&data) {
                                if !text.is_empty() {
                                    let b64 = BASE64.encode(text);
                                    eprintln!("[termway] Copied to host clipboard: {:?}", text);
                                    print!("\x1b]52;c;{}\x07", b64);
                                    let _ = std::io::stdout().flush();
                                }
                            }
                        }
                    });
                }
            }
        }
    }

    fn send_selection(
        &mut self,
        _ty: SelectionTarget,
        _mime_type: String,
        fd: OwnedFd,
        _seat: Seat<Self>,
        user_data: &Self::SelectionUserData,
    ) {
        use std::io::Write;
        let mut file = std::fs::File::from(fd);
        let _ = file.write_all(user_data);
    }
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
