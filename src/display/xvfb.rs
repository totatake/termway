use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};
use std::ffi::CString;

pub struct XvfbServer {
    display_num: u32,
    process: Option<Child>,
    display_ptr: *mut x11::xlib::Display,
    root_window: x11::xlib::Window,
    pub width: u32,
    pub height: u32,
}

impl XvfbServer {
    /// Spawn an Xvfb instance on an available display number.
    pub fn spawn(width: u32, height: u32) -> Result<Self, String> {
        crate::session::check_x11_unix_writability()?;

        let display_num = Self::find_free_display();
        let display_str = format!(":{}", display_num);
        let screen_geom = format!("{}x{}x24", width, height);

        let child = Command::new("Xvfb")
            .arg(&display_str)
            .arg("-screen")
            .arg("0")
            .arg(&screen_geom)
            .arg("-nocursor")
            .spawn()
            .map_err(|e| format!("Failed to spawn Xvfb: {}", e))?;

        // Wait for X server to be ready
        let start = Instant::now();
        let mut dpy = std::ptr::null_mut();
        let c_display = CString::new(display_str.clone()).map_err(|e| format!("Invalid display string: {}", e))?;

        while start.elapsed() < Duration::from_secs(5) {
            unsafe {
                dpy = x11::xlib::XOpenDisplay(c_display.as_ptr());
                if !dpy.is_null() {
                    break;
                }
            }
            thread::sleep(Duration::from_millis(50));
        }

        if dpy.is_null() {
            let mut child = child;
            let _ = child.kill();
            return Err(format!("Timeout waiting for Xvfb on display {}", display_str));
        }

        let root_window = unsafe { x11::xlib::XDefaultRootWindow(dpy) };

        Ok(Self {
            display_num,
            process: Some(child),
            display_ptr: dpy,
            root_window,
            width,
            height,
        })
    }

    /// Connect to an existing X11 display (e.g. :0 or DISPLAY env var)
    pub fn connect_existing(display_str: &str, width: u32, height: u32) -> Result<Self, String> {
        let c_display = CString::new(display_str).map_err(|e| format!("Invalid display string: {}", e))?;
        let dpy = unsafe { x11::xlib::XOpenDisplay(c_display.as_ptr()) };
        if dpy.is_null() {
            return Err(format!("Could not open X11 display: {}", display_str));
        }

        let root_window = unsafe { x11::xlib::XDefaultRootWindow(dpy) };
        Ok(Self {
            display_num: 0,
            process: None,
            display_ptr: dpy,
            root_window,
            width,
            height,
        })
    }

    pub fn display_string(&self) -> String {
        format!(":{}", self.display_num)
    }

    pub fn raw_display(&self) -> *mut x11::xlib::Display {
        self.display_ptr
    }

    pub fn root_window(&self) -> x11::xlib::Window {
        self.root_window
    }

    /// Capture current screen into an XImage
    pub fn get_image(&self) -> *mut x11::xlib::XImage {
        unsafe {
            x11::xlib::XGetImage(
                self.display_ptr,
                self.root_window,
                0,
                0,
                self.width,
                self.height,
                x11::xlib::XAllPlanes(),
                x11::xlib::ZPixmap,
            )
        }
    }

    /// Query the number of active/open client windows (including minimized ones managed by WM)
    pub fn count_client_windows(&self) -> usize {
        let mut ewmh_count = 0;
        let mut ewmh_available = false;

        let net_client_list_atom = unsafe {
            let name = c"_NET_CLIENT_LIST";
            x11::xlib::XInternAtom(self.display_ptr, name.as_ptr(), x11::xlib::False)
        };

        if net_client_list_atom != 0 {
            let mut actual_type = 0;
            let mut actual_format = 0;
            let mut nitems = 0;
            let mut bytes_after = 0;
            let mut prop_data: *mut u8 = std::ptr::null_mut();

            let res = unsafe {
                x11::xlib::XGetWindowProperty(
                    self.display_ptr,
                    self.root_window,
                    net_client_list_atom,
                    0,
                    1024,
                    x11::xlib::False,
                    x11::xlib::XA_WINDOW,
                    &mut actual_type,
                    &mut actual_format,
                    &mut nitems,
                    &mut bytes_after,
                    &mut prop_data,
                )
            };

            if res == x11::xlib::Success as i32 && !prop_data.is_null() {
                ewmh_available = true;
                ewmh_count = nitems as usize;
                unsafe {
                    x11::xlib::XFree(prop_data as *mut std::ffi::c_void);
                }
            }
        }

        if ewmh_available && ewmh_count > 0 {
            return ewmh_count;
        }

        let mut root_return = 0;
        let mut parent_return = 0;
        let mut children_return: *mut x11::xlib::Window = std::ptr::null_mut();
        let mut nchildren_return: u32 = 0;

        let status = unsafe {
            x11::xlib::XQueryTree(
                self.display_ptr,
                self.root_window,
                &mut root_return,
                &mut parent_return,
                &mut children_return,
                &mut nchildren_return,
            )
        };

        let mut tree_count = 0;
        if status != 0 && !children_return.is_null() {
            let slice = unsafe { std::slice::from_raw_parts(children_return, nchildren_return as usize) };
            for &win in slice {
                let mut attrs = std::mem::MaybeUninit::<x11::xlib::XWindowAttributes>::uninit();
                if unsafe { x11::xlib::XGetWindowAttributes(self.display_ptr, win, attrs.as_mut_ptr()) } != 0 {
                    let attrs = unsafe { attrs.assume_init() };
                    if attrs.map_state == x11::xlib::IsViewable
                        && attrs.override_redirect == 0
                        && attrs.width > 1
                        && attrs.height > 1
                    {
                        tree_count += 1;
                    }
                }
            }
            unsafe {
                x11::xlib::XFree(children_return as *mut std::ffi::c_void);
            }
        }

        if ewmh_available {
            std::cmp::max(ewmh_count, tree_count)
        } else {
            tree_count
        }
    }

    fn find_free_display() -> u32 {
        for num in 99..200 {
            let socket_path = format!("/tmp/.X11-unix/X{}", num);
            let lock_path = format!("/tmp/.X{}-lock", num);
            if !std::path::Path::new(&socket_path).exists() && !std::path::Path::new(&lock_path).exists() {
                return num;
            }
        }
        99
    }
}

impl Drop for XvfbServer {
    fn drop(&mut self) {
        if !self.display_ptr.is_null() {
            unsafe {
                x11::xlib::XCloseDisplay(self.display_ptr);
            }
            self.display_ptr = std::ptr::null_mut();
        }

        if let Some(mut child) = self.process.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
