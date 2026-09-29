use std::env;
use std::io::{self, Read, Write};
use std::os::fd::AsFd;
use std::os::fd::AsRawFd;

pub fn is_kitty_graphics_supported() -> bool {
    // 1. Fast path: Environment variables
    if let Ok(term) = env::var("TERM") {
        if term.contains("kitty") || term.contains("wezterm") {
            return true;
        }
    }
    if let Ok(tp) = env::var("TERM_PROGRAM") {
        let tp = tp.to_lowercase();
        if tp.contains("wezterm") || tp.contains("ghostty") || tp.contains("kitty") {
            return true;
        }
    }
    if env::var("KITTY_WINDOW_ID").is_ok() || env::var("WEZTERM_PANE").is_ok() {
        return true;
    }

    // 2. Terminal querying via escape sequence
    let stdin_fd = io::stdin().as_raw_fd();
    if unsafe { libc::isatty(stdin_fd) } != 1 {
        return false; // Not a TTY, can't query
    }

    let mut termios = std::mem::MaybeUninit::<libc::termios>::uninit();
    if unsafe { libc::tcgetattr(stdin_fd, termios.as_mut_ptr()) } != 0 {
        return false;
    }
    let orig_termios = unsafe { termios.assume_init() };
    let mut raw = orig_termios;
    unsafe { libc::cfmakeraw(&mut raw) };
    unsafe { libc::tcsetattr(stdin_fd, libc::TCSANOW, &raw) };

    // Send Kitty query (a=q) followed by Primary DA (\x1b[c)
    // The DA acts as a flush/barrier: once we receive the DA response, 
    // we know the terminal has processed our query.
    let mut stdout = io::stdout();
    let _ = write!(
        stdout,
        "\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[c"
    );
    let _ = stdout.flush();

    let stdin = io::stdin();
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 128];
    let mut supported = false;

    // Poll for response (timeout 200ms)
    let mut poll_fds = [nix::poll::PollFd::new(
        stdin.as_fd(),
        nix::poll::PollFlags::POLLIN,
    )];

    let start_time = std::time::Instant::now();
    
    while start_time.elapsed().as_millis() < 200 {
        if let Ok(poll_res) = nix::poll::poll(&mut poll_fds, nix::poll::PollTimeout::try_from(50).unwrap()) {
            if poll_res > 0 {
                if let Some(revents) = poll_fds[0].revents() {
                    if revents.contains(nix::poll::PollFlags::POLLIN) {
                        if let Ok(n) = stdin.lock().read(&mut chunk) {
                            if n > 0 {
                                buffer.extend_from_slice(&chunk[..n]);
                                let resp = String::from_utf8_lossy(&buffer);
                                
                                // Check if we got a Kitty OK response
                                if resp.contains("\x1b_Gi=31;OK\x1b\\") {
                                    supported = true;
                                    break;
                                }
                                
                                // If we received DA response `\x1b[?` without Kitty response, it's not supported
                                if resp.contains("\x1b[?") && resp.contains("c") {
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Restore terminal
    unsafe { libc::tcsetattr(stdin_fd, libc::TCSANOW, &orig_termios) };

    supported
}
