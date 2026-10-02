use smithay::xwayland::{XWayland, XWMEvent};
fn test(xwayland: XWayland) {
    let mut event_loop = smithay::reexports::calloop::EventLoop::<()>::try_new().unwrap();
    let loop_handle = event_loop.handle();
    let _ = loop_handle.insert_source(xwayland, |event, _, _| {
        // Handle event
    });
}
fn main() {}
