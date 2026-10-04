use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};

pub static WINCH_RECEIVED: AtomicBool = AtomicBool::new(false);

pub fn setup_sigwinch() {
    unsafe {
        libc::signal(libc::SIGWINCH, handle_sigwinch as *const () as usize);
    }
}

extern "C" fn handle_sigwinch(_: libc::c_int) {
    WINCH_RECEIVED.store(true, Ordering::Relaxed);
}

/// Terminal window size and resolution information
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSize {
    pub cols: u16,
    pub rows: u16,
    pub width_px: u16,
    pub height_px: u16,
    pub cell_width_px: u16,
    pub cell_height_px: u16,
}

impl TerminalSize {
    /// Query the current terminal window size using ioctl(TIOCGWINSZ).
    /// If pixel resolution is not reported by the terminal (returns 0), sensible font cell defaults (9x18) are used.
    pub fn query() -> Result<Self, std::io::Error> {
        let stdout_fd = std::io::stdout().as_raw_fd();
        let mut ws = libc::winsize {
            ws_row: 0,
            ws_col: 0,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        let res = unsafe { libc::ioctl(stdout_fd, libc::TIOCGWINSZ, &mut ws) };
        if res < 0 {
            // Default fallback if not a TTY or error
            return Ok(Self::fallback(80, 24));
        }

        let cols = if ws.ws_col > 0 { ws.ws_col } else { 80 };
        let rows = if ws.ws_row > 0 { ws.ws_row } else { 24 };

        let cell_width_px = if ws.ws_xpixel > 0 && cols > 0 {
            (ws.ws_xpixel / cols).max(1)
        } else {
            9 // Typical monospace font width
        };

        let cell_height_px = if ws.ws_ypixel > 0 && rows > 0 {
            (ws.ws_ypixel / rows).max(1)
        } else {
            18 // Typical monospace font height
        };

        let width_px = if ws.ws_xpixel > 0 {
            ws.ws_xpixel
        } else {
            cols * cell_width_px
        };

        let height_px = if ws.ws_ypixel > 0 {
            ws.ws_ypixel
        } else {
            rows * cell_height_px
        };

        Ok(Self {
            cols,
            rows,
            width_px,
            height_px,
            cell_width_px,
            cell_height_px,
        })
    }

    /// Construct a fallback size with specified columns and rows
    pub fn fallback(cols: u16, rows: u16) -> Self {
        let cell_width_px = 9;
        let cell_height_px = 18;
        Self {
            cols,
            rows,
            width_px: cols * cell_width_px,
            height_px: rows * cell_height_px,
            cell_width_px,
            cell_height_px,
        }
    }

    /// Convert 1-indexed terminal cell coordinates (col, row) to pixel coordinates (x, y)
    /// within this terminal window.
    pub fn cell_to_pixel(&self, col: u16, row: u16) -> (u32, u32) {
        let c = col.saturating_sub(1);
        let r = row.saturating_sub(1);

        let x = (c as u32) * (self.cell_width_px as u32) + (self.cell_width_px as u32 / 2);
        let y = (r as u32) * (self.cell_height_px as u32) + (self.cell_height_px as u32 / 2);

        let max_x = (self.width_px as u32).saturating_sub(1);
        let max_y = (self.height_px as u32).saturating_sub(1);

        (x.min(max_x), y.min(max_y))
    }

    /// Map cell coordinates to a target X11 / virtual display resolution.
    pub fn map_cell_to_target(&self, col: u16, row: u16, target_w: u32, target_h: u32) -> (i32, i32) {
        let (px, py) = self.cell_to_pixel(col, row);
        if self.width_px == 0 || self.height_px == 0 {
            return (0, 0);
        }

        let mapped_x = (px as u64 * target_w as u64 / self.width_px as u64) as i32;
        let mapped_y = (py as u64 * target_h as u64 / self.height_px as u64) as i32;

        (
            mapped_x.clamp(0, (target_w.saturating_sub(1)) as i32),
            mapped_y.clamp(0, (target_h.saturating_sub(1)) as i32),
        )
    }
}
