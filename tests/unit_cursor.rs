use termway::display::XvfbServer;

#[test]
fn test_xfixes_cursor() {
    let xvfb = XvfbServer::spawn(800, 600).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));
    let display = xvfb.raw_display();
    
    unsafe {
        let cursor_image = x11::xfixes::XFixesGetCursorImage(display);
        assert!(!cursor_image.is_null());
        let width = (*cursor_image).width;
        let height = (*cursor_image).height;
        let pixels = std::slice::from_raw_parts((*cursor_image).pixels, (width * height) as usize);
        println!("Cursor pixels (first 10):");
        for i in 0..10 {
            println!("{:08x}", pixels[i]);
        }
        x11::xlib::XFree(cursor_image as *mut _);
    }
}
