use crate::backend::{detect_backend_mode, Backend, BackendMode, DrmBackend, TerminalBackend};
use crate::display::{DamageTracker, Framebuffer, XvfbServer};
use crate::input::XTestInjector;
use crate::terminal::{RawModeGuard, TerminalSize};
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub backend_mode: Option<BackendMode>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: u32,
    pub command: Vec<String>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            backend_mode: None,
            width: None,
            height: None,
            fps: 30,
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

    pub fn run(&self) -> Result<(), Box<dyn std::error::Error>> {
        // 1. Detect terminal size & resolve resolution
        let term_size = TerminalSize::query().unwrap_or_else(|_| TerminalSize::fallback(80, 24));
        let width = self.config.width.unwrap_or(term_size.width_px as u32).max(320);
        let height = self.config.height.unwrap_or(term_size.height_px as u32).max(240);

        // 2. Select backend
        let mode = self.config.backend_mode.unwrap_or_else(detect_backend_mode);

        if mode == BackendMode::Terminal {
            if !crate::terminal::is_kitty_graphics_supported() {
                return Err("Terminal does not support Kitty Graphics Protocol, and not running in local DRM/KMS mode. Please use Kitty, WezTerm, Ghostty, or run on a local virtual console.".into());
            }
        }

        let mut backend: Box<dyn Backend> = match mode {
            BackendMode::Terminal => {
                Box::new(TerminalBackend::new(term_size))
            }
            BackendMode::Drm => {
                match DrmBackend::probe_or_init(width, height) {
                    Ok(drm) => Box::new(drm),
                    Err(e) => {
                        eprintln!("[termway] DRM initialization failed ({}), falling back to Terminal mode", e);
                        if !crate::terminal::is_kitty_graphics_supported() {
                            return Err("Terminal does not support Kitty Graphics Protocol, and DRM fallback failed. Please use Kitty, WezTerm, Ghostty, or fix DRM.".into());
                        }
                        Box::new(TerminalBackend::new(term_size))
                    }
                }
            }
        };

        println!("[termway] Active backend: {}", backend.name());
        println!("[termway] Virtual display resolution: {}x{}", width, height);

        // 3. Start Xvfb virtual display server
        let xvfb = XvfbServer::spawn(width, height)
            .map_err(|e| format!("Xvfb launch error: {}", e))?;
        let display_str = xvfb.display_string();
        println!("[termway] X11 server ready on DISPLAY={}", display_str);

        // 4. Initialize XTest input injector
        let mut injector = XTestInjector::new(xvfb.raw_display());

        // 5. Spawn client GUI application if specified
        let mut client_child: Option<Child> = if !self.config.command.is_empty() {
            let cmd_name = &self.config.command[0];
            let cmd_args = &self.config.command[1..];
            let mut cmd = Command::new(cmd_name);
            cmd.args(cmd_args);
            cmd.env("DISPLAY", &display_str);

            let child = cmd.spawn().map_err(|e| {
                format!("Failed to start client command '{}': {}", cmd_name, e)
            })?;
            println!("[termway] Started GUI client: {:?}", self.config.command);
            Some(child)
        } else {
            None
        };

        // 6. Enter Raw mode and terminal alternate screen if in Terminal mode
        let _raw_guard = if mode == BackendMode::Terminal {
            Some(RawModeGuard::enter()?)
        } else {
            None
        };

        // 7. Main event & frame loop
        let frame_interval = Duration::from_millis(1000 / self.config.fps.max(1) as u64);
        let mut fb = Framebuffer::new(width, height);
        let mut damage_tracker = DamageTracker::new(width, height, 32);

        let mut running = true;

        while running {
            let frame_start = Instant::now();

            // A. Check if client process exited
            if let Some(ref mut child) = client_child {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        // Client GUI finished
                        println!("\r\n[termway] GUI client process exited with status: {}", status);
                        break;
                    }
                    Ok(None) => {}
                    Err(e) => {
                        eprintln!("\r\n[termway] Error checking client status: {}", e);
                        break;
                    }
                }
            }

            // B. Poll and process terminal input
            match backend.poll_input(&mut injector, width, height) {
                Ok(continue_running) => {
                    if !continue_running {
                        running = false;
                    }
                }
                Err(e) => {
                    eprintln!("\r\n[termway] Input poll error: {}", e);
                    break;
                }
            }

            // C. Capture X11 frame
            let ximage = xvfb.get_image();
            if !ximage.is_null() {
                unsafe {
                    fb.copy_from_ximage(ximage);
                    x11::xlib::XDestroyImage(ximage);
                }

                // D. Track damage and render
                let damage = damage_tracker.track_damage(&fb.pixels);
                if !damage.is_empty() {
                    if let Err(e) = backend.render_damage(&fb, &damage) {
                        eprintln!("\r\n[termway] Render error: {}", e);
                        break;
                    }
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

        Ok(())
    }
}
