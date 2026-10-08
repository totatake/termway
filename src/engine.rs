use crate::backend::TerminalBackend;
use crate::display::{DamageTracker, Framebuffer, XvfbServer};
use crate::input::XTestInjector;
use crate::terminal::{RawModeGuard, TerminalSize};
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};
use crate::terminal::winsize::WINCH_RECEIVED;

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub cell_size: Option<(u16, u16)>,
    pub fps: u32,
    pub enable_wm: bool,
    pub use_sixel: bool,
    pub command: Vec<String>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            width: None,
            height: None,
            cell_size: None,
            fps: 30,
            enable_wm: false,
            use_sixel: false,
            command: Vec::new(),
        }
    }
}

pub struct TermwayEngine {
    config: EngineConfig,
}

impl TermwayEngine {
    pub fn new(config: EngineConfig) -> Self {
        Self { config }
    }

    pub fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        // 1. Detect terminal size & resolve resolution
        let mut term_size = TerminalSize::query().unwrap_or_else(|_| TerminalSize::fallback(80, 24));
        if let Some((cw, ch)) = self.config.cell_size {
            term_size.cell_width_px = cw;
            term_size.cell_height_px = ch;
            term_size.width_px = term_size.cols * cw;
            term_size.height_px = term_size.rows * ch;
        }
        let width = self.config.width.unwrap_or(term_size.width_px as u32).max(320);
        let height = if let Some(h) = self.config.height {
            h.max(240)
        } else if self.config.use_sixel {
            let avail_rows = term_size.rows.saturating_sub(1).max(1);
            let raw_h = avail_rows as u32 * term_size.cell_height_px as u32;
            (raw_h / 6 * 6).max(240)
        } else {
            (term_size.height_px as u32).max(240)
        };

        let backend_name = if self.config.use_sixel { "Terminal (Sixel Graphics + SGR Mouse)" } else { "Terminal (Kitty Graphics + SGR Mouse)" };
        eprintln!("[termway] Active backend: {}", backend_name);
        eprintln!("[termway] Virtual display resolution: {}x{}", width, height);

        // 3. Start Xvfb virtual display server
        let xvfb = XvfbServer::spawn(width, height)
            .map_err(|e| format!("Xvfb launch error: {}", e))?;
        let display_str = xvfb.display_string();
        eprintln!("[termway] X11 server ready on DISPLAY={}", display_str);

        // 3.5. Start session manager (D-Bus isolation and Polkit authentication agent)
        let mut session = crate::session::SessionManager::new();
        session.update_activation_environment(None, Some(&display_str));
        session.start_polkit(None, Some(&display_str));
        thread::sleep(Duration::from_millis(300));

        // 4. Initialize XTest input injector
        let mut injector = unsafe { XTestInjector::new(xvfb.raw_display()) };

        // 5. Spawn Window Manager if enabled
        let wm_child: Option<Child> = if self.config.enable_wm {
            let mut cmd = Command::new("openbox");
            cmd.env("DISPLAY", &display_str);
            if let Some(dbus) = session.dbus_address() {
                cmd.env("DBUS_SESSION_BUS_ADDRESS", dbus);
            }
            cmd.stdout(std::process::Stdio::null());
            cmd.stderr(std::process::Stdio::null());
            
            match cmd.spawn() {
                Ok(child) => {
                    eprintln!("[termway] Started Window Manager: openbox");
                    Some(child)
                }
                Err(e) => {
                    return Err(format!(
                        "Failed to start Window Manager 'openbox': {}. Please install openbox (e.g. 'sudo apt install openbox') or run without --wm.",
                        e
                    ).into());
                }
            }
        } else {
            None
        };

        // 6. Spawn client GUI application if specified
        let mut client_child: Option<Child> = if !self.config.command.is_empty() {
            let cmd_name = &self.config.command[0];
            let cmd_args = &self.config.command[1..];
            let mut cmd = Command::new(cmd_name);
            cmd.args(cmd_args);
            cmd.env("DISPLAY", &display_str);
            if let Some(dbus) = session.dbus_address_for_client(cmd_name) {
                cmd.env("DBUS_SESSION_BUS_ADDRESS", dbus);
            }
            if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
                cmd.env("XDG_RUNTIME_DIR", runtime_dir);
            }
            // Redirect child stdout/stderr to our stderr so X11 warnings don't corrupt stdout
            cmd.stdout(std::process::Stdio::null());

            let child = cmd.spawn().map_err(|e| {
                format!("Failed to start client command '{}': {}", cmd_name, e)
            })?;
            eprintln!("[termway] Started GUI client: {:?}", self.config.command);
            Some(child)
        } else {
            None
        };

        // 6. Wait for client to map its window and render initial frame
        thread::sleep(Duration::from_millis(500));

        // 7. Create backend instance
        let mut backend = TerminalBackend::new(term_size, self.config.use_sixel, false);

        // 8. Enter Raw mode and terminal alternate screen
        let _raw_guard = RawModeGuard::enter()?;

        // 9. Main event & frame loop
        let frame_interval = Duration::from_millis(1000 / self.config.fps.max(1) as u64);
        let mut fb = Framebuffer::new(width, height);
        let mut damage_tracker = DamageTracker::new(width, height, 32);
        let mut clipboard = unsafe { crate::display::clipboard::ClipboardBridge::new(xvfb.raw_display()) };
        let mut shm_image = unsafe { crate::display::shm::ShmImage::new(xvfb.raw_display(), width, height) };

        let mut running = true;

        let mut has_opened_windows = false;
        let mut all_windows_closed_time: Option<Instant> = None;
        let spawn_time = Instant::now();

        while running {
            if WINCH_RECEIVED.swap(false, std::sync::atomic::Ordering::Relaxed) {
                if let Ok(new_size) = TerminalSize::query() {
                    backend.term_size = new_size;
                }
            }

            let frame_start = Instant::now();

            let active_windows = xvfb.count_client_windows();
            if active_windows > 0 {
                has_opened_windows = true;
            }

            let is_child_alive = client_child
                .as_mut()
                .map(|child| matches!(child.try_wait(), Ok(None)))
                .unwrap_or(false);

            // A. Check exit condition based on window lifecycle
            if has_opened_windows {
                if active_windows == 0 {
                    if is_child_alive {
                        // While the child process is still running, do not exit immediately on 0 windows;
                        // wait up to 3 seconds grace period in case a new window is about to open.
                        if let Some(t) = all_windows_closed_time {
                            if t.elapsed() > Duration::from_millis(3000) {
                                eprintln!("[termway] All X11 windows closed and grace period expired while client running, exiting.");
                                break;
                            }
                        } else {
                            all_windows_closed_time = Some(Instant::now());
                        }
                    } else {
                        eprintln!("[termway] All X11 windows closed and client exited, exiting.");
                        break;
                    }
                } else {
                    all_windows_closed_time = None;
                }
            } else if let Some(ref mut child) = client_child {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        if !status.success() {
                            return Err(format!("Client process exited with error ({:?}) before opening any window.", status).into());
                        }
                        // If parent exited with 0 (e.g. gvim fork or gnome-terminal launcher), wait up to 6s grace period for window to appear
                        if spawn_time.elapsed() > Duration::from_secs(6) {
                            return Err("Client exited and grace period expired with no X11 window opened.".into());
                        }
                    }
                    Ok(None) => {}
                    Err(e) => return Err(format!("Error waiting for client process: {}", e).into()),
                }
            }

            // B. Poll and process terminal input
            match backend.poll_input(&mut injector, width, height) {
                Ok(continue_running) => {
                    if !continue_running {
                        running = false;
                    }
                }
                Err(_) => break,
            }

            if backend.take_screenshot_requested() {
                let screenshot_dir = std::env::var("TERMWAY_SCREENSHOT_DIR")
                    .or_else(|_| std::env::var("TERMWAY_TEST_DUMP_DIR"))
                    .unwrap_or_else(|_| ".".into());
                let _ = std::fs::create_dir_all(&screenshot_dir);
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis();
                let path = format!("{}/termway_screenshot_{}.png", screenshot_dir, ts);
                if let Some(img) = image::RgbaImage::from_raw(fb.width, fb.height, fb.pixels.clone()) {
                    if img.save(&path).is_ok() {
                        eprintln!("[termway] Screenshot saved: {}", path);
                    }
                }
                let latest_path = format!("{}/termway_screenshot_latest.png", screenshot_dir);
                if let Some(img) = image::RgbaImage::from_raw(fb.width, fb.height, fb.pixels.clone()) {
                    let _ = img.save(&latest_path);
                }
            }

            // C. Poll X11 clipboard events
            if let Some(cb) = &mut clipboard {
                cb.poll_events();
            }

            // D. Capture X11 frame
            let mut got_frame = false;
            if let Some(shm) = &mut shm_image {
                if shm.capture() {
                    unsafe { fb.copy_from_ximage(shm.image) };
                    got_frame = true;
                }
            } else {
                // Fallback to slow XGetImage
                let ximage = xvfb.get_image();
                if !ximage.is_null() {
                    unsafe {
                        fb.copy_from_ximage(ximage);
                        x11::xlib::XDestroyImage(ximage);
                    }
                    got_frame = true;
                }
            }

            if got_frame {

                // Composite hardware cursor
                if let Some((mx, my)) = unsafe { crate::display::cursor::query_pointer(xvfb.raw_display()) } {
                    if let Some(cursor) = unsafe { crate::display::cursor::HardwareCursor::query(xvfb.raw_display()) } {
                        cursor.composite(&mut fb, mx, my);
                    }
                }

                // D. Track damage and render
                let damage = damage_tracker.track_damage(&fb.pixels);
                if !damage.is_empty() && backend.render_damage(&fb, &damage).is_err() {
                    break;
                }
            }

            // E. Frame pacing
            let elapsed = frame_start.elapsed();
            if elapsed < frame_interval {
                thread::sleep(frame_interval - elapsed);
            }
        }

        // Cleanup child if still running
        if let Some(mut child) = client_child {
            let _ = child.kill();
            let _ = child.wait();
        }

        // Cleanup WM child if still running
        if let Some(mut wm) = wm_child {
            let _ = wm.kill();
            let _ = wm.wait();
        }

        Ok(())
    }
}
