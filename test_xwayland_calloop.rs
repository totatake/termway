use smithay::xwayland::XWayland;
fn test(xwayland: XWayland) {
    let mut event_loop = smithay::reexports::calloop::EventLoop::<()>::try_new().unwrap();
    let loop_handle = event_loop.handle();
    // loop_handle.insert_source(xwayland, |event, _, _| { ... });
}
