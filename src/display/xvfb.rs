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
        let x11_dir = std::path::Path::new("/tmp/.X11-unix");
        if x11_dir.exists() {
            let test_file = format!("/tmp/.X11-unix/.termway_test_{}", std::process::id());
            if let Err(e) = std::fs::OpenOptions::new().write(true).create_new(true).open(&test_file) {
                return Err(format!("Cannot write to /tmp/.X11-unix ({}). Xvfb will not work: {}", e.kind(), e));
            } else {
                let _ = std::fs::remove_file(&test_file);
            }
        }

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
        let c_display = CString::new(display_str.clone()).unwrap();

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
        let c_display = CString::new(display_str).unwrap();
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

    /// Query the number of mapped client windows on the root window
    pub fn count_client_windows(&self) -> usize {
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

        let mut count = 0;
        if status != 0 && !children_return.is_null() {
            let slice = unsafe { std::slice::from_raw_parts(children_return, nchildren_return as usize) };
            for &win in slice {
                let mut attrs: x11::xlib::XWindowAttributes = unsafe { std::mem::zeroed() };
                if unsafe { x11::xlib::XGetWindowAttributes(self.display_ptr, win, &mut attrs) } != 0 {
                    if attrs.map_state == x11::xlib::IsViewable && attrs.override_redirect == 0 && attrs.width > 1 && attrs.height > 1 {
                        count += 1;
                    }
                }
            }
            unsafe {
                x11::xlib::XFree(children_return as *mut std::ffi::c_void);
            }
        }
        count
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
