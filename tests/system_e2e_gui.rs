use std::process::{Child, Command};
use std::thread;
use std::time::Duration;
use termway::display::{DamageTracker, Framebuffer, XvfbServer};
use termway::input::XTestInjector;
use termway::protocol::{KeyCode, KittyEncoder, MouseAction, MouseButton, MouseEvent};

struct TestApp {
    child: Child,
}

impl Drop for TestApp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn test_system_e2e_x11_gui_lifecycle_and_interaction() {
    // 1. Launch Xvfb virtual display server (FR-6)
    let width = 640;
    let height = 480;
    let xvfb = XvfbServer::spawn(width, height)
        .expect("Xvfb server should start cleanly");
    let display_str = xvfb.display_string();

    // 2. Launch real Tkinter GUI app on this DISPLAY
    // The Python GUI has a prominent red canvas button that turns green when clicked
    let script = r#"
import tkinter as tk

root = tk.Tk()
root.title("E2E Test Window")
root.geometry("640x480+0+0")

clicked = False

def handle_click(event):
    global clicked
    clicked = True
    canvas.config(bg="green")

canvas = tk.Canvas(root, width=200, height=200, bg="red")
canvas.place(x=100, y=100)
canvas.bind("<Button-1>", handle_click)

root.update()
root.mainloop()
"#;

    let mut cmd = Command::new("python3");
    cmd.arg("-c").arg(script);
    cmd.env("DISPLAY", &display_str);
    let child = cmd.spawn().expect("Tkinter test app should launch");
    let _app_guard = TestApp { child };

    // Allow window manager / X11 client to render initial frame
    thread::sleep(Duration::from_millis(600));

    // 3. Capture initial frame buffer (FR-1)
    let mut fb = Framebuffer::new(width, height);
    let mut tracker = DamageTracker::new(width, height, 32);

    let ximage = xvfb.get_image();
    assert!(!ximage.is_null(), "XGetImage should return valid image pointer");
    unsafe {
        fb.copy_from_ximage(ximage);
        x11::xlib::XDestroyImage(ximage);
    }

    // Verify initial red canvas exists at (100, 100) - RGBA red should have high R, low G
    let pixel_idx = (150 * width + 150) * 4;
    let r_init = fb.pixels[pixel_idx as usize];
    let g_init = fb.pixels[pixel_idx as usize + 1];
    assert!(r_init > 150, "Initial canvas pixel R should be red (>150), got {}", r_init);
    assert!(g_init < 100, "Initial canvas pixel G should be low (<100), got {}", g_init);

    // Initial damage tracking
    let initial_damage = tracker.track_damage(&fb.pixels);
    assert!(!initial_damage.is_empty(), "First frame should have damage rects");

    // Verify Kitty encoding of initial frame (FR-1)
    let kitty_seq = KittyEncoder::encode_rgba_image(&fb.pixels, width, height, Some(1), Some(1));
    assert!(kitty_seq.starts_with("\x1b_Ga=T,q=2,f=32"));

    // 4. Inject synthetic mouse click event using XTest (FR-3)
    let mut injector = XTestInjector::new(xvfb.raw_display());
    let click_ev = MouseEvent {
        button: MouseButton::Left,
        action: MouseAction::Press,
        col: 0,
        row: 0,
        shift: false,
        meta: false,
        ctrl: false,
    };
    // Click at center of the canvas: (150, 150)
    injector.inject_mouse(&click_ev, 150, 150);

    let release_ev = MouseEvent {
        button: MouseButton::Left,
        action: MouseAction::Release,
        col: 0,
        row: 0,
        shift: false,
        meta: false,
        ctrl: false,
    };
    injector.inject_mouse(&release_ev, 150, 150);

    // Wait for GUI event processing and repaint
    thread::sleep(Duration::from_millis(500));

    // 5. Capture updated frame buffer and verify damage tracking (FR-2)
    let ximage_after = xvfb.get_image();
    assert!(!ximage_after.is_null());
    unsafe {
        fb.copy_from_ximage(ximage_after);
        x11::xlib::XDestroyImage(ximage_after);
    }

    // Verify color changed to green: high G, low R
    let r_after = fb.pixels[pixel_idx as usize];
    let g_after = fb.pixels[pixel_idx as usize + 1];
    assert!(g_after > 100, "Updated canvas pixel G should be green (>100), got {}", g_after);
    assert!(r_after < 100, "Updated canvas pixel R should be low (<100), got {}", r_after);

    // Check damage detection: should report damage in canvas region (100..300)
    let damage_after = tracker.track_damage(&fb.pixels);
    assert!(!damage_after.is_empty(), "Damage tracker should detect repainted region");

    let touched_canvas = damage_after.iter().any(|r| {
        r.x + r.width >= 100 && r.x <= 300 && r.y + r.height >= 100 && r.y <= 300
    });
    assert!(touched_canvas, "Damage rects must include the clicked canvas area");

    // 6. Test Key injection (FR-4)
    injector.inject_key(KeyCode::Char('q'));
    // Successfully injected key without crash
}
