use std::io::{self, Write};
use std::os::fd::AsRawFd;

/// RAII guard that configures the terminal into raw mode with mouse tracking,
/// and restores the original terminal state upon drop (even if panicking).
pub struct RawModeGuard {
    stdin_fd: i32,
    original_termios: Option<libc::termios>,
    active: bool,
}

impl RawModeGuard {
    /// Enter raw mode, enable alternate screen and SGR 1006 mouse tracking.
    pub fn enter() -> io::Result<Self> {
        let stdin_fd = io::stdin().as_raw_fd();
        let is_atty = unsafe { libc::isatty(stdin_fd) == 1 };

        let mut original_termios = None;

        if is_atty {
            let mut termios = std::mem::MaybeUninit::<libc::termios>::uninit();
            let res = unsafe { libc::tcgetattr(stdin_fd, termios.as_mut_ptr()) };
            if res == 0 {
                let orig = unsafe { termios.assume_init() };
                original_termios = Some(orig);

                // Set raw mode
                let mut raw = orig;
                unsafe { libc::cfmakeraw(&mut raw) };
                unsafe { libc::tcsetattr(stdin_fd, libc::TCSANOW, &raw) };
            }
        }

        let mut guard = Self {
            stdin_fd,
            original_termios,
            active: true,
        };

        guard.setup_terminal_sequences()?;
        Ok(guard)
    }

    /// Construct a dummy guard for testing (does not modify terminal)
    pub fn dummy() -> Self {
        Self {
            stdin_fd: 0,
            original_termios: None,
            active: false,
        }
    }

    fn setup_terminal_sequences(&mut self) -> io::Result<()> {
        let mut stdout = io::stdout();
        // \x1b[?1049h: Alternate Screen Buffer
        // \x1b[?25l: Hide Cursor
        // \x1b[?1000h: Normal mouse tracking
        // \x1b[?1002h: Button-event tracking (motion while button pressed)
        // \x1b[?1003h: Any-event mouse tracking (optional, useful for hover)
        // \x1b[?1006h: SGR extended mode
        // \x1b[2J\x1b[H: Clear screen and move to home
        write!(
            stdout,
            "\x1b[?1049h\x1b[?25l\x1b[?1000h\x1b[?1002h\x1b[?1006h\x1b[2J\x1b[H"
        )?;
        stdout.flush()
    }

    /// Explicitly restore the terminal ahead of drop
    pub fn restore(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;

        let mut stdout = io::stdout();
        // Disable SGR mouse tracking, show cursor, leave alternate screen
        let _ = write!(
            stdout,
            "\x1b[?1006l\x1b[?1002l\x1b[?1000l\x1b[?25h\x1b[?1049l"
        );
        let _ = stdout.flush();

        if let Some(orig) = self.original_termios {
            unsafe {
                libc::tcsetattr(self.stdin_fd, libc::TCSANOW, &orig);
            }
        }
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        self.restore();
    }
}
