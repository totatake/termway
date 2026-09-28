pub mod terminal;
pub mod drm;

pub use terminal::TerminalBackend;
pub use drm::DrmBackend;

use crate::display::{DamageRect, Framebuffer};
use crate::input::XTestInjector;
use crate::terminal::TerminalSize;
use std::env;
use std::io;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendMode {
    Terminal,
    Drm,
}

/// Abstract common backend interface for both In-Terminal and DRM modes.
pub trait Backend {
    fn name(&self) -> &'static str;
    fn render_damage(&mut self, fb: &Framebuffer, damage: &[DamageRect]) -> io::Result<()>;
    fn poll_input(&mut self, injector: &mut XTestInjector, target_w: u32, target_h: u32) -> io::Result<bool>;
    fn on_resize(&mut self, new_size: &TerminalSize);
}

/// Dynamically detect the optimal backend mode based on environment variables and TTY device.
pub fn detect_backend_mode() -> BackendMode {
    // 1. If SSH connection indicators exist, definitely use Terminal mode
    if env::var("SSH_CONNECTION").is_ok()
        || env::var("SSH_CLIENT").is_ok()
        || env::var("SSH_TTY").is_ok()
    {
        return BackendMode::Terminal;
    }

    // 2. Check if stdin is a pseudo-terminal (pty) or real virtual console (tty)
    let stdin = io::stdin();
    if let Ok(tty_path) = nix::unistd::ttyname(&stdin) {
        let tty_str = tty_path.to_string_lossy();
        if tty_str.starts_with("/dev/pts/") {
            // Under tmux / terminal emulator: Terminal mode
            return BackendMode::Terminal;
        }
    }

    // 3. If running directly on a Linux virtual console (Ctrl+Alt+F1 /dev/tty1) with DRM node available
    // and no existing X11/Wayland display server running
    if Path::new("/dev/dri/card0").exists() && env::var("DISPLAY").is_err() && env::var("WAYLAND_DISPLAY").is_err() {
        return BackendMode::Drm;
    }

    // Default fallback
    BackendMode::Terminal
}
