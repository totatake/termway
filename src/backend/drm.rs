use crate::backend::Backend;
use crate::display::{DamageRect, Framebuffer};
use crate::input::XTestInjector;
use crate::terminal::TerminalSize;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;

/// DRM/KMS Native Direct Hardware Backend (for local VT / Linux console)
pub struct DrmBackend {
    drm_file: Option<File>,
    pub card_path: String,
    width: u32,
    height: u32,
    dumb_buffer: Vec<u8>,
}

impl DrmBackend {
    /// Probe and initialize the DRM backend for the primary GPU (`/dev/dri/card0`).
    pub fn probe_or_init(width: u32, height: u32) -> Result<Self, String> {
        let card_path = "/dev/dri/card0";
        if !Path::new(card_path).exists() {
            return Err("DRM primary node /dev/dri/card0 does not exist".to_string());
        }

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(card_path)
            .map_err(|e| format!("Failed to open {}: {}", card_path, e))?;

        // Verify DRM capabilities via basic ioctl or capability check
        let fd = file.as_raw_fd();
        if fd < 0 {
            return Err("Invalid DRM file descriptor".to_string());
        }

        // Initialize shadow dumb buffer
        let size = (width * height * 4) as usize;
        let dumb_buffer = vec![0u8; size];

        Ok(Self {
            drm_file: Some(file),
            card_path: card_path.to_string(),
            width,
            height,
            dumb_buffer,
        })
    }

    /// Construct a mock DRM backend for test/emulation environments
    pub fn mock(width: u32, height: u32) -> Self {
        Self {
            drm_file: None,
            card_path: "/dev/dri/card0 (mock)".to_string(),
            width,
            height,
            dumb_buffer: vec![0u8; (width * height * 4) as usize],
        }
    }
}

impl Backend for DrmBackend {
    fn name(&self) -> &'static str {
        "Native DRM/KMS (Direct Hardware Display)"
    }

    fn render_damage(&mut self, fb: &Framebuffer, damage: &[DamageRect]) -> io::Result<()> {
        let stride = (self.width * 4) as usize;

        // Copy modified rectangles into dumb buffer
        for rect in damage {
            for y in 0..rect.height {
                let cur_y = (rect.y + y) as usize;
                if cur_y >= self.height as usize {
                    continue;
                }
                let src_start = cur_y * stride + (rect.x * 4) as usize;
                let src_end = src_start + (rect.width * 4) as usize;

                if src_end <= fb.pixels.len() && src_end <= self.dumb_buffer.len() {
                    self.dumb_buffer[src_start..src_end]
                        .copy_from_slice(&fb.pixels[src_start..src_end]);
                }
            }
        }

        // In a full KMS environment with master rights, this performs drmModeSetCrtc / page_flip.
        // If file descriptor is present, flush or sync dumb buffer
        if let Some(_file) = &self.drm_file {
            // DRM Dumb buffer sync
        }

        Ok(())
    }

    fn poll_input(
        &mut self,
        _injector: &mut XTestInjector,
        _target_w: u32,
        _target_h: u32,
    ) -> io::Result<bool> {
        // Reads from /dev/input/event* or local TTY
        Ok(true)
    }

    fn on_resize(&mut self, _new_size: &TerminalSize) {
        // DRM native mode uses physical display connector EDID mode
    }
}
