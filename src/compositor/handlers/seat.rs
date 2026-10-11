//! Input seat focus tracking and clipboard selection bridging
//! (Wayland data device <-> host terminal via OSC 52).

use std::os::unix::io::{FromRawFd, OwnedFd};

use smithay::{
    input::{Seat, SeatHandler, SeatState},
    reexports::wayland_server::{protocol::wl_surface::WlSurface, Resource},
    wayland::selection::{
        data_device::{ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler},
        SelectionHandler, SelectionSource, SelectionTarget,
    },
};

use crate::compositor::state::TermwayState;

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
    }
    fn cursor_image(&mut self, _seat: &Seat<Self>, _image: smithay::input::pointer::CursorImageStatus) {}
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
