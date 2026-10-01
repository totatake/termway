use std::ptr;
use libc;
use x11::xlib;
use x11::xshm;

pub struct ShmImage {
    display: *mut xlib::Display,
    pub image: *mut xlib::XImage,
    shminfo: xshm::XShmSegmentInfo,
    pub width: u32,
    pub height: u32,
}

impl ShmImage {
    pub fn new(display: *mut xlib::Display, width: u32, height: u32) -> Option<Self> {
        if display.is_null() {
            return None;
        }

        unsafe {
            if xshm::XShmQueryExtension(display) == 0 {
                return None;
            }

            let visual = xlib::XDefaultVisual(display, 0);
            let depth = xlib::XDefaultDepth(display, 0) as u32;

            let mut shminfo: xshm::XShmSegmentInfo = std::mem::zeroed();

            let image = xshm::XShmCreateImage(
                display,
                visual,
                depth,
                xlib::ZPixmap,
                ptr::null_mut(),
                &mut shminfo,
                width,
                height,
            );

            if image.is_null() {
                return None;
            }

            let bytes_per_line = (*image).bytes_per_line;
            let size = (bytes_per_line * height as i32) as usize;

            let shmid = libc::shmget(
                libc::IPC_PRIVATE,
                size,
                libc::IPC_CREAT | 0o600,
            );

            if shmid < 0 {
                xlib::XDestroyImage(image);
                return None;
            }

            let shmaddr = libc::shmat(shmid, ptr::null(), 0);
            if shmaddr == libc::MAP_FAILED {
                libc::shmctl(shmid, libc::IPC_RMID, ptr::null_mut());
                xlib::XDestroyImage(image);
                return None;
            }

            shminfo.shmid = shmid;
            shminfo.shmaddr = shmaddr as *mut libc::c_char;
            shminfo.readOnly = xlib::False;
            (*image).data = shmaddr as *mut libc::c_char;

            if xshm::XShmAttach(display, &mut shminfo) == 0 {
                libc::shmdt(shmaddr);
                libc::shmctl(shmid, libc::IPC_RMID, ptr::null_mut());
                xlib::XDestroyImage(image);
                return None;
            }

            // Mark for auto-destruction
            libc::shmctl(shmid, libc::IPC_RMID, ptr::null_mut());

            Some(Self {
                display,
                image,
                shminfo,
                width,
                height,
            })
        }
    }

    pub fn capture(&mut self) -> bool {
        unsafe {
            let root = xlib::XDefaultRootWindow(self.display);
            xshm::XShmGetImage(self.display, root, self.image, 0, 0, !0u32) != 0
        }
    }
}

impl Drop for ShmImage {
    fn drop(&mut self) {
        unsafe {
            xshm::XShmDetach(self.display, &mut self.shminfo);
            let addr = self.shminfo.shmaddr;
            xlib::XDestroyImage(self.image); // Also frees data? No, it usually frees data if we allocated it, but we provided it. Wait!
            // Actually, XDestroyImage tries to free `image->data`.
            // Let's detach it first to avoid segfault.
            (*self.image).data = ptr::null_mut();
            xlib::XDestroyImage(self.image);
            libc::shmdt(addr as *mut _);
        }
    }
}
