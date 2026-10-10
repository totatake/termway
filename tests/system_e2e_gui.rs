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

    // 3. Capture initial frame buffer (FR-1)
    let mut fb = Framebuffer::new(width, height);
    let mut tracker = DamageTracker::new(width, height, 32);

    let pixel_idx = (150 * width + 150) * 4;
    let mut r_init = 0;
    let mut g_init = 0;

    // Poll until Tkinter maps its window and draws the red canvas
    for _ in 0..40 {
        thread::sleep(Duration::from_millis(100));
        let ximage = xvfb.get_image();
        if !ximage.is_null() {
            unsafe {
                fb.copy_from_ximage(ximage);
                x11::xlib::XDestroyImage(ximage);
            }
            r_init = fb.pixels[pixel_idx as usize];
            g_init = fb.pixels[pixel_idx as usize + 1];
            if r_init > 150 && g_init < 100 {
                break;
            }
        }
    }
    assert!(r_init > 150, "Initial canvas pixel R should be red (>150), got {}", r_init);
    assert!(g_init < 100, "Initial canvas pixel G should be low (<100), got {}", g_init);

    // Initial damage tracking
    let initial_damage = tracker.track_damage(&fb.pixels);
    assert!(!initial_damage.is_empty(), "First frame should have damage rects");

    // Verify Kitty encoding of initial frame (FR-1)
    let kitty_seq = KittyEncoder::encode_rgba_image(&fb.pixels, width, height, Some(1), Some(1));
    assert!(kitty_seq.starts_with("\x1b_Ga=T,q=2,f=32"));

    // 4. Inject synthetic mouse click event using XTest (FR-3)
    let mut injector = unsafe { XTestInjector::new(xvfb.raw_display()) };
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

#[test]
fn test_gui_test_driver_signals_and_state() {
    use std::fs;
    use std::time::SystemTime;

    let temp_dir = std::env::temp_dir().join(format!(
        "termway_gui_test_{}",
        SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis()
    ));
    fs::create_dir_all(&temp_dir).unwrap();

    let state_file = temp_dir.join("gui_state.json");
    std::env::set_var("TERMWAY_TEST_STATE_FILE", &state_file);
    std::env::set_var("TERMWAY_TEST_DUMP_DIR", &temp_dir);

    let display = smithay::reexports::wayland_server::Display::<termway::compositor::state::TermwayState>::new()
        .expect("Failed to create Wayland Display");
    let dh = display.handle();
    let state = termway::compositor::state::TermwayState::new(&dh, 800, 600, true);

    // Test JSON state dump serialization
    let json = state.dump_state_json();
    fs::write(&state_file, json).unwrap();
    assert!(state_file.exists());

    let content = fs::read_to_string(&state_file).unwrap();
    assert!(content.contains("\"screen_width\": 800"));
    assert!(content.contains("\"screen_height\": 600"));
    assert!(content.contains("\"windows\": ["));
    assert!(content.contains("\"popups\": ["));

    // Cleanup
    let _ = fs::remove_dir_all(&temp_dir);
}
