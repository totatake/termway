use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::sync::{Mutex, Once};

static PANIC_HOOK_SET: Once = Once::new();
static EMERGENCY_STATE: Mutex<Option<(i32, Option<libc::termios>, bool)>> = Mutex::new(None);

/// RAII guard that configures the terminal into raw mode with mouse tracking,
/// and restores the original terminal state upon drop (even if panicking).
pub struct RawModeGuard {
    stdin_fd: i32,
    original_termios: Option<libc::termios>,
    active: bool,
    pixel_mouse: bool,
}

impl RawModeGuard {
    /// Enter raw mode, enable alternate screen and mouse tracking (default: SGR cell mode).
    pub fn enter() -> io::Result<Self> {
        Self::enter_with_options(false)
    }

    /// Enter raw mode with explicit pixel mouse tracking configuration.
    /// When `pixel_mouse` is true, enables DECSET 1016 (SGR-Pixels) for 1px coordinate precision.
    pub fn enter_with_options(pixel_mouse: bool) -> io::Result<Self> {
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
            pixel_mouse,
        };

        if let Ok(mut state) = EMERGENCY_STATE.lock() {
            *state = Some((stdin_fd, original_termios, pixel_mouse));
        }

        PANIC_HOOK_SET.call_once(|| {
            let default_hook = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                if let Ok(mut state) = EMERGENCY_STATE.lock() {
                    if let Some((fd, orig, pixel_mouse)) = state.take() {
                        let reset_seq: &[u8] = if pixel_mouse {
                            b"\x1b[?2026l\x1b]2;\x07\x1b[<u\x1b]22;\x07\x1b[?2004l\x1b[?1016l\x1b[?1006l\x1b[?1003l\x1b[?1000l\x1b[?7h\x1b[?25h\x1b[?1049l\n"
                        } else {
                            b"\x1b[?2026l\x1b]2;\x07\x1b[<u\x1b]22;\x07\x1b[?2004l\x1b[?1006l\x1b[?1003l\x1b[?1000l\x1b[?7h\x1b[?25h\x1b[?1049l\n"
                        };
                        unsafe {
                            libc::write(1, reset_seq.as_ptr() as *const libc::c_void, reset_seq.len());
                            if let Some(orig) = orig {
                                libc::tcsetattr(fd, libc::TCSAFLUSH, &orig);
                            }
                        }
                    }
                }
                default_hook(info);
            }));
        });

        guard.setup_terminal_sequences()?;
        Ok(guard)
    }

    /// Construct a dummy guard for testing (does not modify terminal)
    pub fn dummy() -> Self {
        Self {
            stdin_fd: 0,
            original_termios: None,
            active: false,
            pixel_mouse: false,
        }
    }

    fn setup_terminal_sequences(&mut self) -> io::Result<()> {
        let mut stdout = io::stdout();
        // \x1b[?1049h: Alternate Screen Buffer
        // \x1b[?25l: Hide Cursor
        // \x1b[?1000h: Normal mouse tracking
        // \x1b[?1003h: Any-event mouse tracking (motion even without button pressed)
        // \x1b[?1006h: SGR extended mode
        // \x1b[?1016h: SGR pixel mode (1px precision on xterm/kitty/wezterm/foot/ghostty)
        // \x1b]2;termway\x07: Set terminal window/tab title
        // \x1b]22;default\x07: Set mouse cursor shape to default pointer
        // \x1b[>1u: Enable Kitty Keyboard Protocol (progressive enhancement: disambiguate escape codes)
        // \x1b[?2004h: Enable bracketed paste mode
        // \x1b[?7l: Disable auto-wrap to prevent scrolling on bottom-right text/images
        // \x1b[2J\x1b[H\x1b[16t\x1b[14t: Clear screen, home, and query cell/window size
        let mouse_mode_seq = if self.pixel_mouse {
            "\x1b[?1000h\x1b[?1003h\x1b[?1006h\x1b[?1016h"
        } else {
            "\x1b[?1000h\x1b[?1003h\x1b[?1006h"
        };
        write!(
            stdout,
            "\x1b]2;termway\x07\x1b[?1049h\x1b[?25l{}\x1b]22;default\x07\x1b[>1u\x1b[?2004h\x1b[?7l\x1b[2J\x1b[H\x1b[16t\x1b[14t",
            mouse_mode_seq
        )?;
        stdout.flush()
    }

    /// Explicitly restore the terminal ahead of drop
    pub fn restore(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;

        if let Ok(mut state) = EMERGENCY_STATE.lock() {
            *state = None;
        }

        let mut stdout = io::stdout();
        // Reset synchronized update (?2026l), terminal title (OSC 2), restore keyboard flags (\x1b[<u), reset mouse cursor shape (OSC 22), disable bracketed paste, disable SGR pixel & cell mouse tracking, enable auto-wrap, show cursor, leave alternate screen
        let mouse_disable_seq = if self.pixel_mouse {
            "\x1b[?2004l\x1b[?1016l\x1b[?1006l\x1b[?1003l\x1b[?1000l"
        } else {
            "\x1b[?2004l\x1b[?1006l\x1b[?1003l\x1b[?1000l"
        };
        let _ = write!(
            stdout,
            "\x1b[?2026l\x1b]2;\x07\x1b[<u\x1b]22;\x07{}\x1b[?7h\x1b[?25h\x1b[?1049l",
            mouse_disable_seq
        );
        let _ = stdout.flush();

        // Allow terminal emulator to process tracking disable, and drain any in-flight mouse events
        std::thread::sleep(std::time::Duration::from_millis(25));
        unsafe {
            let flags = libc::fcntl(self.stdin_fd, libc::F_GETFL);
            if flags >= 0 {
                libc::fcntl(self.stdin_fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
                let mut buf = [0u8; 1024];
                while libc::read(self.stdin_fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) > 0 {}
                libc::fcntl(self.stdin_fd, libc::F_SETFL, flags);
            }
            libc::tcflush(self.stdin_fd, libc::TCIFLUSH);
        }

        if let Some(orig) = self.original_termios {
            unsafe {
                libc::tcsetattr(self.stdin_fd, libc::TCSAFLUSH, &orig);
                libc::tcflush(self.stdin_fd, libc::TCIFLUSH);
            }
        }
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        self.restore();
    }
}
