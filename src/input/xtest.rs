use crate::protocol::{KeyCode, MouseAction, MouseButton, MouseEvent};
use std::os::raw::c_uint;

/// Injects synthetic mouse and keyboard events into an X11 server using libXtst.
pub struct XTestInjector {
    display: *mut x11::xlib::Display,
}

impl XTestInjector {
    /// Create a new XTest injector for the given X11 display.
    pub fn new(display: *mut x11::xlib::Display) -> Self {
        Self { display }
    }

    /// Inject a mouse event (motion, press, release) at specific X11 pixel coordinates.
    pub fn inject_mouse(&mut self, event: &MouseEvent, px: i32, py: i32) {
        if self.display.is_null() {
            return;
        }

        unsafe {
            // First move pointer to target location
            x11::xtest::XTestFakeMotionEvent(self.display, -1, px, py, x11::xlib::CurrentTime);

            let button_num: Option<c_uint> = match event.button {
                MouseButton::Left => Some(1),
                MouseButton::Middle => Some(2),
                MouseButton::Right => Some(3),
                MouseButton::WheelUp => Some(4),
                MouseButton::WheelDown => Some(5),
                _ => None,
            };

            if let Some(btn) = button_num {
                match event.action {
                    MouseAction::Press => {
                        x11::xtest::XTestFakeButtonEvent(self.display, btn, 1, x11::xlib::CurrentTime);
                        if event.button == MouseButton::WheelUp || event.button == MouseButton::WheelDown {
                            // Wheels are discrete clicks, release immediately
                            x11::xtest::XTestFakeButtonEvent(self.display, btn, 0, x11::xlib::CurrentTime);
                        }
                    }
                    MouseAction::Release => {
                        x11::xtest::XTestFakeButtonEvent(self.display, btn, 0, x11::xlib::CurrentTime);
                    }
                    MouseAction::Drag => {
                        // Already handled by motion event
                    }
                    MouseAction::Move => {
                        // Handled by motion event
                    }
                }
            }

            x11::xlib::XFlush(self.display);
        }
    }

    /// Inject a keyboard event (press and release) into X11.
    pub fn inject_key(&mut self, code: KeyCode) {
        if self.display.is_null() {
            return;
        }

        unsafe {
            let keysym = match code {
                KeyCode::Char(c) => c as u64,
                KeyCode::Enter => x11::keysym::XK_Return as u64,
                KeyCode::Backspace => x11::keysym::XK_BackSpace as u64,
                KeyCode::Tab => x11::keysym::XK_Tab as u64,
                KeyCode::Escape => x11::keysym::XK_Escape as u64,
                KeyCode::Up => x11::keysym::XK_Up as u64,
                KeyCode::Down => x11::keysym::XK_Down as u64,
                KeyCode::Right => x11::keysym::XK_Right as u64,
                KeyCode::Left => x11::keysym::XK_Left as u64,
                KeyCode::Home => x11::keysym::XK_Home as u64,
                KeyCode::End => x11::keysym::XK_End as u64,
                KeyCode::PageUp => x11::keysym::XK_Page_Up as u64,
                KeyCode::PageDown => x11::keysym::XK_Page_Down as u64,
                KeyCode::Delete => x11::keysym::XK_Delete as u64,
                KeyCode::Ctrl(c) => c as u64,
            };

            let keycode = x11::xlib::XKeysymToKeycode(self.display, keysym);
            if keycode != 0 {
                // Press
                x11::xtest::XTestFakeKeyEvent(self.display, keycode as c_uint, 1, x11::xlib::CurrentTime);
                // Release
                x11::xtest::XTestFakeKeyEvent(self.display, keycode as c_uint, 0, x11::xlib::CurrentTime);
                x11::xlib::XFlush(self.display);
            }
        }
    }
}
