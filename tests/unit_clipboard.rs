use x11::xlib;
use termway::display::XvfbServer;

#[test]
fn test_read_clipboard() {
    let xvfb = XvfbServer::spawn(800, 600).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));
    let display = xvfb.raw_display();
    
    unsafe {
        let root = xlib::XDefaultRootWindow(display);
        let win = xlib::XCreateSimpleWindow(display, root, 0, 0, 1, 1, 0, 0, 0);
        
        let clipboard = xlib::XInternAtom(display, c"CLIPBOARD".as_ptr() as _, xlib::False);
        let utf8_string = xlib::XInternAtom(display, c"UTF8_STRING".as_ptr() as _, xlib::False);
        let target_property = xlib::XInternAtom(display, c"XSEL_DATA".as_ptr() as _, xlib::False);

        let mut child = match std::process::Command::new("xclip")
            .args(["-i", "-selection", "clipboard"])
            .env("DISPLAY", xvfb.display_string())
            .stdin(std::process::Stdio::piped())
            .spawn() {
                Ok(c) => c,
                Err(_) => {
                    println!("xclip not installed, skipping test");
                    return;
                }
            };
        
        use std::io::Write;
        child.stdin.as_mut().unwrap().write_all(b"Hello from X11!").unwrap();
        child.wait().unwrap();

        xlib::XConvertSelection(display, clipboard, utf8_string, target_property, win, xlib::CurrentTime);
        xlib::XFlush(display);

        let mut event: xlib::XEvent = std::mem::zeroed();
        let mut got_notify = false;
        
        for _ in 0..50 {
            if xlib::XPending(display) > 0 {
                xlib::XNextEvent(display, &mut event);
                if event.get_type() == xlib::SelectionNotify {
                    let sel = event.selection;
                    if sel.property != 0 {
                        let mut actual_type = 0;
                        let mut actual_format = 0;
                        let mut nitems = 0;
                        let mut bytes_after = 0;
                        let mut prop_return = std::ptr::null_mut();

                        xlib::XGetWindowProperty(
                            display, win, target_property, 0, 1024, xlib::False,
                            xlib::AnyPropertyType as u64,
                            &mut actual_type, &mut actual_format, &mut nitems, &mut bytes_after, &mut prop_return
                        );

                        if !prop_return.is_null() && nitems > 0 {
                            let slice = std::slice::from_raw_parts(prop_return as *const u8, nitems as usize);
                            let text = std::str::from_utf8_unchecked(slice);
                            println!("Clipboard text: {}", text);
                            assert_eq!(text, "Hello from X11!");
                            xlib::XFree(prop_return as *mut _);
                            got_notify = true;
                            break;
                        }
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(got_notify, "Did not successfully read clipboard");
    }
}
