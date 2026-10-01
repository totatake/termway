use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use x11::xlib;
use x11::xfixes;

pub struct ClipboardBridge {
    display: *mut xlib::Display,
    window: xlib::Window,
    clipboard_atom: xlib::Atom,
    utf8_string_atom: xlib::Atom,
    target_property_atom: xlib::Atom,
    xfixes_event_base: i32,
    awaiting_selection: bool,
    last_clipboard_hash: u64,
}

impl ClipboardBridge {
    pub fn new(display: *mut xlib::Display) -> Option<Self> {
        if display.is_null() {
            return None;
        }
        unsafe {
            let mut event_base = 0;
            let mut error_base = 0;
            if xfixes::XFixesQueryExtension(display, &mut event_base, &mut error_base) == 0 {
                return None;
            }

            let root = xlib::XDefaultRootWindow(display);
            let window = xlib::XCreateSimpleWindow(display, root, 0, 0, 1, 1, 0, 0, 0);

            let clipboard_atom = xlib::XInternAtom(display, b"CLIPBOARD\0".as_ptr() as _, xlib::False);
            let utf8_string_atom = xlib::XInternAtom(display, b"UTF8_STRING\0".as_ptr() as _, xlib::False);
            let target_property_atom = xlib::XInternAtom(display, b"XSEL_DATA\0".as_ptr() as _, xlib::False);

            // Subscribe to CLIPBOARD owner changes (XFixesSetSelectionOwnerNotifyMask = 1<<0)
            xfixes::XFixesSelectSelectionInput(display, root, clipboard_atom, 1);
            xlib::XFlush(display);

            Some(Self {
                display,
                window,
                clipboard_atom,
                utf8_string_atom,
                target_property_atom,
                xfixes_event_base: event_base,
                awaiting_selection: false,
                last_clipboard_hash: 0,
            })
        }
    }

    pub fn poll_events(&mut self) {
        unsafe {
            while xlib::XPending(self.display) > 0 {
                let mut event: xlib::XEvent = std::mem::zeroed();
                xlib::XNextEvent(self.display, &mut event);

                let ev_type = event.get_type();
                if ev_type == self.xfixes_event_base + 0 /* XFixesSelectionNotify */ {
                    // Selection owner changed! Request the new selection.
                    xlib::XConvertSelection(
                        self.display,
                        self.clipboard_atom,
                        self.utf8_string_atom,
                        self.target_property_atom,
                        self.window,
                        xlib::CurrentTime,
                    );
                    self.awaiting_selection = true;
                } else if ev_type == xlib::SelectionNotify && self.awaiting_selection {
                    self.awaiting_selection = false;
                    let sel = event.selection;
                    if sel.property != 0 {
                        let mut actual_type = 0;
                        let mut actual_format = 0;
                        let mut nitems = 0;
                        let mut bytes_after = 0;
                        let mut prop_return = std::ptr::null_mut();

                        xlib::XGetWindowProperty(
                            self.display, self.window, self.target_property_atom, 0, 1024 * 1024, // 1MB max
                            xlib::False,
                            xlib::AnyPropertyType as u64,
                            &mut actual_type, &mut actual_format, &mut nitems, &mut bytes_after, &mut prop_return
                        );

                        if !prop_return.is_null() && nitems > 0 {
                            let slice = std::slice::from_raw_parts(prop_return as *const u8, nitems as usize);
                            if let Ok(text) = std::str::from_utf8(slice) {
                                // Calculate hash to avoid duplicate prints
                                use std::hash::{Hash, Hasher};
                                use std::collections::hash_map::DefaultHasher;
                                let mut hasher = DefaultHasher::new();
                                text.hash(&mut hasher);
                                let hash = hasher.finish();
                                
                                if hash != self.last_clipboard_hash && !text.is_empty() {
                                    self.last_clipboard_hash = hash;
                                    self.send_osc52(text);
                                }
                            }
                            xlib::XFree(prop_return as *mut _);
                        }
                    }
                }
            }
        }
    }

    fn send_osc52(&self, text: &str) {
        let b64 = BASE64.encode(text);
        // Print the OSC 52 sequence directly to stdout
        print!("\x1b]52;c;{}\x07", b64);
        use std::io::Write;
        let _ = std::io::stdout().flush();
    }
}

impl Drop for ClipboardBridge {
    fn drop(&mut self) {
        unsafe {
            xlib::XDestroyWindow(self.display, self.window);
        }
    }
}
