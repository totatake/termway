use termway::display::XvfbServer;
use x11::xlib;
use x11::xshm;
use libc;
use std::ptr;

#[test]
fn test_xshm() {
    let xvfb = XvfbServer::spawn(800, 600).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));
    let display = xvfb.raw_display();
    
    unsafe {
        let has_shm = xshm::XShmQueryExtension(display);
        assert!(has_shm != 0, "XShm extension not available!");

        let visual = xlib::XDefaultVisual(display, 0);
        let depth = xlib::XDefaultDepth(display, 0) as u32;

        let mut shminfo: xshm::XShmSegmentInfo = std::mem::zeroed();

        // Create image
        let image = xshm::XShmCreateImage(
            display,
            visual,
            depth,
            xlib::ZPixmap,
            ptr::null_mut(), // data pointer
            &mut shminfo,
            800,
            600,
        );
        assert!(!image.is_null());

        let bytes_per_line = (*image).bytes_per_line;
        let size = (bytes_per_line * 600) as usize;

        // Allocate shared memory
        let shmid = libc::shmget(
            libc::IPC_PRIVATE,
            size,
            libc::IPC_CREAT | 0o600,
        );
        assert!(shmid >= 0);

        let shmaddr = libc::shmat(shmid, ptr::null(), 0);
        assert!(shmaddr != libc::MAP_FAILED);

        shminfo.shmid = shmid;
        shminfo.shmaddr = shmaddr as *mut libc::c_char;
        shminfo.readOnly = xlib::False;
        (*image).data = shmaddr as *mut libc::c_char;

        // Attach to X server
        let attach_res = xshm::XShmAttach(display, &mut shminfo);
        assert!(attach_res != 0);
        xlib::XFlush(display);

        // We can mark SHM segment for destruction immediately so it gets cleaned up automatically when detached
        libc::shmctl(shmid, libc::IPC_RMID, ptr::null_mut());

        // Get image
        let root = xlib::XDefaultRootWindow(display);
        let res = xshm::XShmGetImage(display, root, image, 0, 0, !0u32);
        assert!(res != 0);

        println!("Successfully captured frame using MIT-SHM!");

        // Cleanup
        xshm::XShmDetach(display, &mut shminfo);
        xlib::XDestroyImage(image);
        libc::shmdt(shmaddr);
    }
}
