use std::ptr;
use libc;
use x11::xlib;
use x11::xshm;

pub struct ShmImage {
    depth: u32,
    display: *mut xlib::Display,
    pub image: *mut xlib::XImage,
    shminfo: Box<xshm::XShmSegmentInfo>,
    pub width: u32,
    pub height: u32,
}

impl ShmImage {
    pub fn new(display: *mut xlib::Display, _req_w: u32, _req_h: u32) -> Option<Self> {
        if display.is_null() {
            return None;
        }

        unsafe {
            if xshm::XShmQueryExtension(display) == 0 {
                return None;
            }

            let root_win = xlib::XDefaultRootWindow(display);
            let mut root_ret = 0;
            let mut x_ret = 0;
            let mut y_ret = 0;
            let mut width_ret = 0;
            let mut height_ret = 0;
            let mut bw_ret = 0;
            let mut depth_ret = 0;
            xlib::XGetGeometry(display, root_win, &mut root_ret, &mut x_ret, &mut y_ret, &mut width_ret, &mut height_ret, &mut bw_ret, &mut depth_ret);
            let width = width_ret;
            let height = height_ret;

            let visual = xlib::XDefaultVisual(display, 0);
            let depth = xlib::XDefaultDepth(display, 0) as u32;

            // XShmCreateImage stores a raw pointer to shminfo inside image->obdata.
            // Therefore, shminfo MUST be pinned to a stable heap location (Box)
            // and must NOT move in memory!
            let mut shminfo = Box::new(std::mem::zeroed::<xshm::XShmSegmentInfo>());

            let image = xshm::XShmCreateImage(
                display,
                visual,
                depth,
                xlib::ZPixmap,
                ptr::null_mut(),
                shminfo.as_mut() as *mut _,
                width,
                height,
            );

            if image.is_null() {
                return None;
            }

            // Ensure obdata explicitly points to our heap-allocated shminfo
            (*image).obdata = (shminfo.as_mut() as *mut xshm::XShmSegmentInfo) as *mut libc::c_char;

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

            if xshm::XShmAttach(display, shminfo.as_mut() as *mut _) == 0 {
                libc::shmdt(shmaddr);
                libc::shmctl(shmid, libc::IPC_RMID, ptr::null_mut());
                xlib::XDestroyImage(image);
                return None;
            }

            // MUST wait for the X server to actually process the Attach request!
            xlib::XSync(display, xlib::False);

            // Mark for auto-destruction
            libc::shmctl(shmid, libc::IPC_RMID, ptr::null_mut());

            Some(Self {
                depth,
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
            let plane_mask = (1u32.checked_shl(self.depth).unwrap_or(0)).wrapping_sub(1);
            xshm::XShmGetImage(self.display, root, self.image, 0, 0, plane_mask) != 0
        }
    }
}

impl Drop for ShmImage {
    fn drop(&mut self) {
        unsafe {
            xshm::XShmDetach(self.display, self.shminfo.as_mut() as *mut _);
            xlib::XSync(self.display, xlib::False);
            let addr = self.shminfo.shmaddr;
            // XDestroyImage tries to free `image->data`.
            // Set it to null first so it doesn't free our SHM mapping.
            (*self.image).data = ptr::null_mut();
            xlib::XDestroyImage(self.image);
            libc::shmdt(addr as *mut _);
        }
    }
}
