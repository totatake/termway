use crate::backend::Backend;
use crate::display::{DamageRect, Framebuffer};
use crate::input::XTestInjector;
use crate::protocol::{KeyCode, KeyboardParser, KittyEncoder, SgrMouseParser};
use crate::terminal::TerminalSize;
use std::io::{self, Read, Write};

pub struct TerminalBackend {
    term_size: TerminalSize,
    input_buffer: Vec<u8>,
}

impl TerminalBackend {
    pub fn new(term_size: TerminalSize) -> Self {
        Self {
            term_size,
            input_buffer: Vec::with_capacity(1024),
        }
    }
}

impl Backend for TerminalBackend {
    fn name(&self) -> &'static str {
        "Terminal (Kitty Graphics + SGR Mouse)"
    }

    fn render_damage(&mut self, fb: &Framebuffer, damage: &[DamageRect]) -> io::Result<()> {
        let mut stdout = io::stdout();

        for rect in damage {
            // Encode sub-image rectangle
            let encoded = KittyEncoder::encode_rgba_subimage(
                &fb.pixels,
                fb.width,
                fb.height,
                rect.x,
                rect.y,
                rect.width,
                rect.height,
                Some(1),
                Some(1),
            );
            write!(stdout, "{}", encoded)?;
        }

        stdout.flush()
    }

    fn poll_input(
        &mut self,
        injector: &mut XTestInjector,
        target_w: u32,
        target_h: u32,
    ) -> io::Result<bool> {
        let mut chunk = [0u8; 512];
        let mut stdin = io::stdin();

        // Note: stdin is set to raw/non-blocking or polled via select/calloop
        // Here we read whatever bytes are available
        match stdin.read(&mut chunk) {
            Ok(0) => return Ok(true), // EOF
            Ok(n) => {
                self.input_buffer.extend_from_slice(&chunk[..n]);
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                return Ok(true);
            }
            Err(e) => return Err(e),
        }

        // Process buffer
        while !self.input_buffer.is_empty() {
            // 1. Try parsing SGR mouse sequence
            if let Ok(input_str) = std::str::from_utf8(&self.input_buffer) {
                if let Some((mouse_ev, consumed)) = SgrMouseParser::parse(input_str) {
                    let (mapped_x, mapped_y) = self.term_size.map_cell_to_target(
                        mouse_ev.col,
                        mouse_ev.row,
                        target_w,
                        target_h,
                    );
                    injector.inject_mouse(&mouse_ev, mapped_x, mapped_y);
                    self.input_buffer.drain(..consumed);
                    continue;
                }
            }

            // 2. Try parsing keyboard sequence
            if let Some((key_ev, consumed)) = KeyboardParser::parse(&self.input_buffer) {
                // Check exit hotkeys: Ctrl+C or 'q'
                if key_ev.code == KeyCode::Ctrl('c') || key_ev.code == KeyCode::Char('q') {
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

    fn on_resize(&mut self, new_size: &TerminalSize) {
        self.term_size = *new_size;
    }
}
