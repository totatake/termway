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

    fn find_free_display() -> u32 {
        for num in 99..200 {
            let socket_path = format!("/tmp/.X11-unix/X{}", num);
            if !std::path::Path::new(&socket_path).exists() {
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
