use crate::display::{DamageRect, Framebuffer};
use crate::input::XTestInjector;
use crate::protocol::{KeyCode, KeyboardParser, KittyEncoder, SixelEncoder, SgrMouseParser};
use crate::terminal::TerminalSize;
use std::io::{self, Read, Write};

use std::sync::{Arc, Mutex, Condvar};
use std::thread::JoinHandle;

struct RenderJob {
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    term_cols: u16,
    term_rows: u16,
    taskbar: Option<String>,
    needs_clear: bool,
}

struct AsyncRenderState {
    pending_job: Option<RenderJob>,
    spare_buffer: Option<Vec<u8>>,
    shutdown: bool,
}

pub struct TerminalBackend {
    pub term_size: TerminalSize,
    input_buffer: Vec<u8>,
    use_sixel: bool,
    is_wayland: bool,
    pub screenshot_requested: bool,
    pub needs_clear: bool,
    async_state: Arc<(Mutex<AsyncRenderState>, Condvar)>,
    worker_handle: Option<JoinHandle<()>>,
}

impl TerminalBackend {
    pub fn new(term_size: TerminalSize, use_sixel: bool, is_wayland: bool) -> Self {
        let async_state = Arc::new((
            Mutex::new(AsyncRenderState {
                pending_job: None,
                spare_buffer: None,
                shutdown: false,
            }),
            Condvar::new(),
        ));

        let state_clone = Arc::clone(&async_state);
        let mode_desc = if is_wayland {
            if use_sixel { "Wayland (Sixel)" } else { "Wayland (Kitty)" }
        } else {
            if use_sixel { "X11/Xvfb (Sixel)" } else { "X11/Xvfb (Kitty)" }
        };

        let worker_handle = std::thread::Builder::new()
            .name("termway-async-render".into())
            .spawn(move || {
                let mut stdout = io::stdout();
                let mut sixel_out = Vec::new();
                let mut sixel_quantized = Vec::new();
                let mut kitty_b64 = String::new();
                let mut kitty_out = String::new();

                let (lock, cvar) = &*state_clone;
                loop {
                    let job = {
                        let mut guard = lock.lock().unwrap();
                        while guard.pending_job.is_none() && !guard.shutdown {
                            guard = cvar.wait(guard).unwrap();
                        }
                        if guard.shutdown && guard.pending_job.is_none() {
                            break;
                        }
                        guard.pending_job.take().unwrap()
                    };

                    let overlay_text = format!("\x1b[40;37m [{}] [Ctrl-Q: Exit] [F12: Screenshot] \x1b[0m", mode_desc);
                    let overlay_len = mode_desc.len() + 37;
                    let overlay_row = job.term_rows;
                    let overlay_col = job.term_cols.saturating_sub(overlay_len as u16 + 2).max(1);
                    let overlay_seq = format!("\x1b[{};{}H{}", overlay_row, overlay_col, overlay_text);

                    let taskbar_str = job.taskbar.as_deref().unwrap_or("");
                    let taskbar_seq = format!("\x1b[{};1H\x1b[2K{}", overlay_row, taskbar_str);
                    let clear_seq = if job.needs_clear {
                        if use_sixel {
                            "\x1b[2J"
                        } else {
                            "\x1b[2J\x1b_Ga=d,d=a\x1b\\"
                        }
                    } else {
                        ""
                    };

                    if use_sixel {
                        SixelEncoder::encode_fullscreen_frame_into(
                            &job.pixels,
                            job.width,
                            job.height,
                            &mut sixel_out,
                            &mut sixel_quantized,
                        );
                        let _ = write!(stdout, "{}\x1b[H", clear_seq);
                        let _ = stdout.write_all(&sixel_out);
                        let _ = write!(stdout, "{}{}", taskbar_seq, overlay_seq);
                    } else {
                        KittyEncoder::encode_fullscreen_frame_into(
                            &job.pixels,
                            job.width,
                            job.height,
                            job.term_cols,
                            job.term_rows,
                            1,
                            &mut kitty_b64,
                            &mut kitty_out,
                        );
                        let _ = write!(stdout, "{}{}{}{}", clear_seq, kitty_out, taskbar_seq, overlay_seq);
                    }
                    let _ = stdout.flush();

                    let mut guard = lock.lock().unwrap();
                    guard.spare_buffer = Some(job.pixels);
                }
            })
            .ok();

        Self {
            term_size,
            input_buffer: Vec::with_capacity(1024),
            use_sixel,
            is_wayland,
            screenshot_requested: false,
            needs_clear: false,
            async_state,
            worker_handle,
        }
    }

    pub fn resize(&mut self, term_size: TerminalSize) {
        self.term_size = term_size;
    }

    pub fn term_size(&self) -> TerminalSize {
        self.term_size
    }

    pub fn is_sixel(&self) -> bool {
        self.use_sixel
    }

    pub fn name(&self) -> &'static str {
        if self.is_wayland {
            if self.use_sixel {
                "Wayland (Sixel)"
            } else {
                "Wayland (Kitty)"
            }
        } else {
            if self.use_sixel {
                "X11/Xvfb (Sixel)"
            } else {
                "X11/Xvfb (Kitty)"
            }
        }
    }

    pub fn render_damage(&mut self, fb: &Framebuffer, damage: &[DamageRect]) -> io::Result<()> {
        if damage.is_empty() {
            return Ok(());
        }
        self.render_pixels(&fb.pixels, fb.width, fb.height, None)
    }

    pub fn render_pixels(
        &mut self,
        pixels: &[u8],
        width: u32,
        height: u32,
        taskbar: Option<&str>,
    ) -> io::Result<()> {
        let (lock, cvar) = &*self.async_state;
        let mut guard = lock.lock().unwrap();

        let needs_clear = self.needs_clear;
        self.needs_clear = false;

        if let Some(ref mut job) = guard.pending_job {
            // Worker is still busy encoding/flushing previous frame.
            // Drop outdated pending frame and replace with this latest frame (Frame Dropping!).
            job.pixels.clear();
            job.pixels.extend_from_slice(pixels);
            job.width = width;
            job.height = height;
            job.term_cols = self.term_size.cols;
            job.term_rows = self.term_size.rows;
            job.taskbar = taskbar.map(String::from);
            if needs_clear {
                job.needs_clear = true;
            }
        } else {
            let mut buf = guard.spare_buffer.take().unwrap_or_else(|| Vec::with_capacity(pixels.len()));
            buf.clear();
            buf.extend_from_slice(pixels);
            guard.pending_job = Some(RenderJob {
                pixels: buf,
                width,
                height,
                term_cols: self.term_size.cols,
                term_rows: self.term_size.rows,
                taskbar: taskbar.map(String::from),
                needs_clear,
            });
        }
        cvar.notify_one();
        Ok(())
    }

    pub fn poll_input(
        &mut self,
        injector: &mut XTestInjector,
        target_w: u32,
        target_h: u32,
    ) -> io::Result<bool> {
        let mut chunk = [0u8; 512];
        let stdin = io::stdin();

        // Non-blocking poll on stdin (timeout: 0ms) to avoid blocking the frame loop
        let mut poll_fds = [nix::poll::PollFd::new(
            std::os::fd::AsFd::as_fd(&stdin),
            nix::poll::PollFlags::POLLIN,
        )];

        let poll_res = nix::poll::poll(&mut poll_fds, nix::poll::PollTimeout::ZERO)
            .map_err(io::Error::other)?;

        if poll_res > 0 {
            if let Some(revents) = poll_fds[0].revents() {
                if revents.contains(nix::poll::PollFlags::POLLIN) {
                    let mut stdin_lock = stdin.lock();
                    match stdin_lock.read(&mut chunk) {
                        Ok(0) => return Ok(true), // EOF
                        Ok(n) => {
                            self.input_buffer.extend_from_slice(&chunk[..n]);
                        }
                        Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {}
                        Err(e) => return Err(e),
                    }
                }
            }
        }

        // Process buffer
        while !self.input_buffer.is_empty() {
            // 1. Try parsing window pixel size response (\x1b[4;height;widtht)
            if let Ok(input_str) = std::str::from_utf8(&self.input_buffer) {
                if let Some(start) = input_str.find("\x1b[4;") {
                    if let Some(end_offset) = input_str[start..].find('t') {
                        let inner = &input_str[start+4 .. start+end_offset];
                        let parts: Vec<&str> = inner.split(';').collect();
                        if parts.len() == 2 {
                            if let (Ok(h), Ok(w)) = (parts[0].parse::<u16>(), parts[1].parse::<u16>()) {
                                self.term_size.width_px = w;
                                self.term_size.height_px = h;
                                if let Some(res) = w.checked_div(self.term_size.cols) {
                                    self.term_size.cell_width_px = res.max(1);
                                }
                                if let Some(res) = h.checked_div(self.term_size.rows) {
                                    self.term_size.cell_height_px = res.max(1);
                                }
                            }
                        }
                        self.input_buffer.drain(start .. start + end_offset + 1);
                        continue;
                    }
                }

                // 2. Try parsing SGR mouse sequence
                if let Some((mouse_ev, consumed)) = SgrMouseParser::parse(input_str) {
                    let (mapped_x, mapped_y) = if self.use_sixel {
                        // Sixel draws 1:1 pixels, so do NOT stretch to target_w
                        let (px, py) = self.term_size.cell_to_pixel(mouse_ev.col, mouse_ev.row);
                        (px as i32, py as i32)
                    } else {
                        self.term_size.map_cell_to_target(
                            mouse_ev.col,
                            mouse_ev.row,
                            target_w,
                            target_h,
                        )
                    };
                    injector.inject_mouse(&mouse_ev, mapped_x, mapped_y);
                    self.input_buffer.drain(..consumed);
                    continue;
                }
            }

            // 2. Try parsing keyboard sequence
            if let Some((key_ev, consumed)) = KeyboardParser::parse(&self.input_buffer) {
                // Use Ctrl+Q as the emergency exit hotkey
                if key_ev.code == KeyCode::Ctrl('q') {
                    return Ok(false); // Signal to terminate
                }
                if key_ev.code == KeyCode::F12 {
                    self.screenshot_requested = true;
                    self.input_buffer.drain(..consumed);
                    continue;
                }

                injector.inject_key(key_ev.code);
                self.input_buffer.drain(..consumed);
                continue;
            }

            // If sequence cannot be parsed yet, wait for more bytes (or drain invalid single byte)
            if self.input_buffer.len() > 32 {
                self.input_buffer.remove(0);
            } else {
                break;
            }
        }

        Ok(true)
    }

    pub fn take_screenshot_requested(&mut self) -> bool {
        let req = self.screenshot_requested;
        self.screenshot_requested = false;
        req
    }
}

impl Drop for TerminalBackend {
    fn drop(&mut self) {
        {
            let (lock, cvar) = &*self.async_state;
            let mut guard = lock.lock().unwrap();
            guard.shutdown = true;
            cvar.notify_one();
        }
        if let Some(handle) = self.worker_handle.take() {
            let _ = handle.join();
        }
    }
}

