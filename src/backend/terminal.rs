use crate::display::{DamageRect, Framebuffer};
use crate::input::XTestInjector;
use crate::protocol::{KeyCode, KeyboardParser, KittyEncoder, SixelEncoder, SgrMouseParser};
use crate::terminal::TerminalSize;
use std::io::{self, Read, Write};

pub struct TerminalBackend {
    term_size: TerminalSize,
    input_buffer: Vec<u8>,
    use_sixel: bool,
}

impl TerminalBackend {
    pub fn new(term_size: TerminalSize, use_sixel: bool) -> Self {
        Self {
            term_size,
            input_buffer: Vec::with_capacity(1024),
            use_sixel,
        }
    }

    pub fn name(&self) -> &'static str {
        if self.use_sixel {
            "Terminal (Sixel Graphics + SGR Mouse)"
        } else {
            "Terminal (Kitty Graphics + SGR Mouse)"
        }
    }

    pub fn render_damage(&mut self, fb: &Framebuffer, damage: &[DamageRect]) -> io::Result<()> {
        if damage.is_empty() {
            return Ok(());
        }

        let mut stdout = io::stdout();

        let overlay_text = "\x1b[40;37m [Ctrl-Q: Exit] \x1b[0m";
        let overlay_row = self.term_size.rows;
        let overlay_col = self.term_size.cols.saturating_sub(16).max(1);
        let overlay_seq = format!("\x1b[{};{}H{}", overlay_row, overlay_col, overlay_text);

        if self.use_sixel {
            let encoded = SixelEncoder::encode_fullscreen_frame(&fb.pixels, fb.width, fb.height);
            // Sixel doesn't inherently position itself at the top left without a cursor move
            write!(stdout, "\x1b[H{}{}", encoded, overlay_seq)?;
        } else {
            // Render full frame with image replacement ID=1 and cursor suppression C=1
            // to avoid scrolling and flickering
            let encoded = KittyEncoder::encode_fullscreen_frame(
                &fb.pixels,
                fb.width,
                fb.height,
                self.term_size.cols,
                self.term_size.rows,
                1, // Fixed image ID for the viewport
            );
            write!(stdout, "{}{}", encoded, overlay_seq)?;
        }
        stdout.flush()
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
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

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
                                if self.term_size.cols > 0 {
                                    self.term_size.cell_width_px = (w / self.term_size.cols).max(1);
                                }
                                if self.term_size.rows > 0 {
                                    self.term_size.cell_height_px = (h / self.term_size.rows).max(1);
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
}
