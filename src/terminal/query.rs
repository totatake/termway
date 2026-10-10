use std::env;
use std::io::{self, Write};
use std::os::fd::AsRawFd;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphicsProtocol {
    Kitty,
    Sixel,
    None,
}

pub fn query_graphics_protocol() -> GraphicsProtocol {
    // 1. Fast path: Environment variables
    if let Ok(term) = env::var("TERM") {
        let term_lower = term.to_lowercase();
        if term_lower.contains("kitty") || term_lower.contains("wezterm") || term_lower.contains("ghostty") {
            return GraphicsProtocol::Kitty;
        }
        if term_lower.contains("foot") || term_lower.contains("mlterm") {
            return GraphicsProtocol::Sixel;
        }
    }
    if let Ok(tp) = env::var("TERM_PROGRAM") {
        let tp = tp.to_lowercase();
        if tp.contains("wezterm") || tp.contains("ghostty") || tp.contains("kitty") {
            return GraphicsProtocol::Kitty;
        }
        if tp.contains("iterm.app") || tp.contains("mintty") {
            return GraphicsProtocol::Sixel;
        }
    }
    if env::var("KITTY_WINDOW_ID").is_ok()
        || env::var("KITTY_PID").is_ok()
        || env::var("WEZTERM_PANE").is_ok()
        || env::var("GHOSTTY_RESOURCES_DIR").is_ok()
    {
        return GraphicsProtocol::Kitty;
    }
    // WT_SESSION is set by Windows Terminal, FOOT_SERVER_SOCKET by foot
    if env::var("WT_SESSION").is_ok() || env::var("FOOT_SERVER_SOCKET").is_ok() {
        return GraphicsProtocol::Sixel;
    }

    // 2. Terminal querying via escape sequence
    let stdin_fd = io::stdin().as_raw_fd();
    if unsafe { libc::isatty(stdin_fd) } != 1 {
        return GraphicsProtocol::None; // Not a TTY, can't query
    }

    let mut termios = std::mem::MaybeUninit::<libc::termios>::uninit();
    if unsafe { libc::tcgetattr(stdin_fd, termios.as_mut_ptr()) } != 0 {
        return GraphicsProtocol::None;
    }
    let orig_termios = unsafe { termios.assume_init() };
    let mut raw = orig_termios;
    unsafe { libc::cfmakeraw(&mut raw) };
    unsafe { libc::tcsetattr(stdin_fd, libc::TCSANOW, &raw) };

    // Send Kitty query (a=q) followed by Primary DA (\x1b[c)
    // The DA acts as a flush/barrier and also queries Sixel (DA response has '4' if Sixel is supported)
    let mut stdout = io::stdout();
    let _ = write!(
        stdout,
        "\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[c"
    );
    let _ = stdout.flush();

    let mut buffer = Vec::new();
    let mut chunk = [0u8; 256];
    let mut found_kitty = false;
    let mut found_da = false;
    let mut has_sixel = false;

    let start_time = std::time::Instant::now();
    let max_timeout_ms = 200;
    let mut da_received_time: Option<std::time::Instant> = None;

    while start_time.elapsed().as_millis() < max_timeout_ms {
        let mut poll_fds = [nix::poll::PollFd::new(
            unsafe { std::os::fd::BorrowedFd::borrow_raw(stdin_fd) },
            nix::poll::PollFlags::POLLIN,
        )];

        if let Ok(poll_res) = nix::poll::poll(&mut poll_fds, nix::poll::PollTimeout::try_from(40).unwrap_or(nix::poll::PollTimeout::NONE)) {
            if poll_res > 0 {
                if let Some(revents) = poll_fds[0].revents() {
                    if revents.contains(nix::poll::PollFlags::POLLIN) {
                        let n = unsafe {
                            libc::read(
                                stdin_fd,
                                chunk.as_mut_ptr() as *mut libc::c_void,
                                chunk.len(),
                            )
                        };
                        if n > 0 {
                            buffer.extend_from_slice(&chunk[..n as usize]);
                            let resp = String::from_utf8_lossy(&buffer);

                            // Check if we got a Kitty OK response
                            if resp.contains("\x1b_Gi=31;OK\x1b\\") {
                                found_kitty = true;
                            }

                            // Check if we got the DA response `\x1b[? ... c`
                            if let Some(start) = resp.find("\x1b[?") {
                                if let Some(end) = resp[start..].find('c') {
                                    found_da = true;
                                    if da_received_time.is_none() {
                                        da_received_time = Some(std::time::Instant::now());
                                    }
                                    let da_inner = &resp[start + 3..start + end];
                                    has_sixel = da_inner.split(';').any(|p| p == "4");
                                }
                            }

                            // If both responses received, break immediately
                            if found_kitty && found_da {
                                break;
                            }
                        }
                    }
                }
            }
        }

        // If Kitty is confirmed, give up to 20ms to drain any accompanying DA response
        if found_kitty {
            if let Some(da_time) = da_received_time {
                if da_time.elapsed().as_millis() > 20 {
                    break;
                }
            } else if start_time.elapsed().as_millis() > 60 {
                break;
            }
        }

        // If DA is received without Kitty response, wait up to 50ms more for Kitty response
        if let Some(da_time) = da_received_time {
            if da_time.elapsed().as_millis() >= 50 {
                break;
            }
        }
    }

    // Drain any remaining bytes from stdin with 0-timeout poll to avoid leaking escape sequences
    loop {
        let mut poll_drain = [nix::poll::PollFd::new(
            unsafe { std::os::fd::BorrowedFd::borrow_raw(stdin_fd) },
            nix::poll::PollFlags::POLLIN,
        )];
        if let Ok(1) = nix::poll::poll(&mut poll_drain, nix::poll::PollTimeout::try_from(0).unwrap_or(nix::poll::PollTimeout::NONE)) {
            let mut drain_buf = [0u8; 256];
            let n = unsafe {
                libc::read(
                    stdin_fd,
                    drain_buf.as_mut_ptr() as *mut libc::c_void,
                    drain_buf.len(),
                )
            };
            if n <= 0 {
                break;
            }
        } else {
            break;
        }
    }

    // Restore terminal
    unsafe { libc::tcsetattr(stdin_fd, libc::TCSANOW, &orig_termios) };

    if found_kitty {
        GraphicsProtocol::Kitty
    } else if has_sixel {
        GraphicsProtocol::Sixel
    } else {
        GraphicsProtocol::None
    }
}
