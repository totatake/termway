use smithay::utils::{Logical, Rectangle};
use smithay::xwayland::X11Surface;
fn configure(window: X11Surface) {
    let _ = window.configure(Rectangle::new((0, 0).into(), (400, 400).into()));
}
fn main() {}
