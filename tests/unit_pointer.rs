use termway::display::XvfbServer;
use x11::xlib;

#[test]
fn test_query_pointer() {
    let xvfb = XvfbServer::spawn(800, 600).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));
    let display = xvfb.raw_display();
    
    unsafe {
        let root = xlib::XDefaultRootWindow(display);
        let mut root_return = 0;
        let mut child_return = 0;
        let mut root_x_return = 0;
        let mut root_y_return = 0;
        let mut win_x_return = 0;
        let mut win_y_return = 0;
        let mut mask_return = 0;

        let res = xlib::XQueryPointer(
            display,
            root,
            &mut root_return,
            &mut child_return,
            &mut root_x_return,
            &mut root_y_return,
            &mut win_x_return,
            &mut win_y_return,
            &mut mask_return,
        );

        assert!(res != 0);
        println!("Pointer: ({}, {})", root_x_return, root_y_return);
    }
}
