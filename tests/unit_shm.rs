use termway::display::XvfbServer;
use termway::display::shm::ShmImage;

#[test]
fn test_shm_image_struct() {
    let xvfb = XvfbServer::spawn(800, 600).unwrap();
    let display = xvfb.raw_display();
    let mut shm = unsafe { ShmImage::new(display, 800, 600) }.expect("Failed to create ShmImage");
    let res = shm.capture();
    assert!(res, "ShmImage::capture failed!");
}
