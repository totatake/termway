pub mod state;
pub mod renderer;
pub mod xwayland;

use std::io::{self, Read};
use std::process::{Child, Command};
use std::sync::Arc;

use smithay::{
    backend::input::{ButtonState, KeyState, Keycode},
    input::{
        keyboard::FilterResult,
        pointer::{ButtonEvent, MotionEvent},
    },
    reexports::wayland_server::{Display, ListeningSocket},
    utils::{Logical, Point, SERIAL_COUNTER},
};

use crate::backend::TerminalBackend;
use crate::engine::EngineConfig;
use crate::protocol::{KeyCode, KeyboardParser, MouseAction, MouseButton, SgrMouseParser};
use crate::terminal::{RawModeGuard, TerminalSize};

use self::renderer::TermwayRenderer;
use self::state::{send_frames_surface_tree, ClientState, TermwayState};

pub struct TermwayCompositor {
    display: Display<TermwayState>,
    listener: ListeningSocket,
    state: TermwayState,
    renderer: TermwayRenderer,
    backend: TerminalBackend,
    socket_name: String,
    x11_display: Option<String>,
    input_buffer: Vec<u8>,
    pending_command: Option<Vec<String>>,
    client_child: Option<Child>,
    xwayland_source: Option<smithay::xwayland::XWayland>,
    _xwayland_client: Option<smithay::reexports::wayland_server::Client>,
}

impl TermwayCompositor {
    pub fn new(config: EngineConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let term_size = TerminalSize::query().unwrap_or_else(|_| TerminalSize::fallback(80, 24));
        let width = config.width.unwrap_or(term_size.width_px as u32).max(320);
        let height = config.height.unwrap_or(term_size.height_px as u32).max(240);

        let display = Display::new()?;
        let dh = display.handle();

        // 1. Create Wayland listening socket
        let (listener, socket_name) = {
            let mut id = 1;
            loop {
                let name = format!("wayland-{}", id);
                if let Ok(socket) = ListeningSocket::bind(&name) {
                    break (socket, name);
                }
                id += 1;
                if id > 32 {
                    return Err("Failed to find free wayland socket name".into());
                }
            }
        };

        eprintln!("[termway] Headless Wayland Compositor (Smithay)");
        eprintln!("[termway] Socket: WAYLAND_DISPLAY={}", socket_name);
        eprintln!("[termway] Resolution: {}x{}", width, height);

        // 2. Initialize Compositor State
        let state = TermwayState::new(&dh, width, height);

        // 3. Initialize Pixman Offscreen Renderer
        let renderer = TermwayRenderer::new(width, height)?;

        // 4. Initialize Terminal Output Backend
        let backend = TerminalBackend::new(term_size, config.use_sixel);

        // 5. Try starting XWayland for legacy X11 compatibility
        // Workaround for environments where WAYLAND_SOCKET FD gets closed (e.g. by security wrappers):
        // We create a wrapper script that unsets WAYLAND_SOCKET and uses WAYLAND_DISPLAY instead.
        let xwayland_real = String::from_utf8(
            std::process::Command::new("which").arg("Xwayland").output().unwrap().stdout
        ).unwrap();
        let xwayland_real = xwayland_real.trim();
        let wrap_dir = "/tmp/termway_wrap";
        let _ = std::fs::create_dir_all(wrap_dir);
        let wrapper_path = format!("{}/Xwayland", wrap_dir);
        let wrapper_script = format!(
            "#!/bin/bash\nunset WAYLAND_SOCKET\nexport WAYLAND_DISPLAY={}\nexec {} \"$@\"\n",
            socket_name, xwayland_real
        );
        std::fs::write(&wrapper_path, wrapper_script).unwrap();
        std::fs::set_permissions(&wrapper_path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        
        let old_path = std::env::var("PATH").unwrap_or_default();
        std::env::set_var("PATH", format!("{}:{}", wrap_dir, old_path));

        let (xwayland, x11_display) = match smithay::xwayland::XWayland::spawn(
            &dh,
            None,
            std::iter::once((std::ffi::OsString::from("WAYLAND_DEBUG"), std::ffi::OsString::from("1"))),
            true,
            std::process::Stdio::null(),
            std::process::Stdio::inherit(),
            |_| (),
        ) {
            Ok((xwayland, client)) => {
                let disp_num = xwayland.display_number();
                let disp_str = format!(":{}", disp_num);
                eprintln!("[termway] XWayland initialized on DISPLAY={}", disp_str);
                std::env::set_var("DISPLAY", &disp_str);
                (Some((xwayland, client)), Some(disp_str))
            }
            Err(e) => {
                eprintln!("[termway] XWayland not available: {}.", e);
                eprintln!("[termway] Note: Running in native Wayland mode. Wayland applications will run natively.");
                eprintln!("[termway]       Pure X11 applications (e.g. xeyes, xclock) require an X server.");
                eprintln!("[termway]       To run pure X11 apps in this environment, run without '--wayland':");
                eprintln!("[termway]           termway -- {}", config.command.first().map(|s| s.as_str()).unwrap_or("xeyes"));
                (None, None)
            }
        };

        // 6. We will spawn the child process inside `run()` to avoid blocking the Wayland event loop.
        let pending_command = if !config.command.is_empty() {
            Some(config.command.clone())
        } else {
            None
        };

        let (xwayland_source, _xwayland_client) = match xwayland {
            Some((source, client)) => (Some(source), Some(client)),
            None => (None, None),
        };

        Ok(Self {
            display,
            listener,
            state,
            renderer,
            backend,
            client_child: None,
            pending_command,
            xwayland_source,
            _xwayland_client,
            socket_name,
            x11_display,
            input_buffer: Vec::with_capacity(1024),
        })
    }

    pub fn socket_name(&self) -> &str {
        &self.socket_name
    }

    pub fn x11_display(&self) -> Option<&str> {
        self.x11_display.as_deref()
    }

    pub fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        // Raw mode guard ensures terminal is restored upon exit
        let _guard = RawModeGuard::enter()?;

        let mut chunk = [0u8; 512];
        let stdin = io::stdin();

        let mut event_loop = smithay::reexports::calloop::EventLoop::<TermwayState>::try_new()?;
        let loop_handle = event_loop.handle();

        if let Some(xwayland) = self.xwayland_source.take() {
            let _ = loop_handle.insert_source(xwayland, |event, _, state| {
                // Poll XWM events
            });
        }

        let mut loop_count = 0;
        while self.state.running {
            loop_count += 1;
            
            // Dispatch calloop (processes XWM and other registered sources) with 0 timeout
            let _ = event_loop.dispatch(Some(std::time::Duration::ZERO), &mut self.state);
            loop_count += 1;
            // Spawn pending command after 5 iterations (50ms) to ensure Wayland globals are negotiated
            if loop_count == 5 {
                if let Some(cmd_args) = self.pending_command.take() {
                    let mut cmd = Command::new(&cmd_args[0]);
                    if cmd_args.len() > 1 {
                        cmd.args(&cmd_args[1..]);
                    }
                    cmd.env("WAYLAND_DISPLAY", &self.socket_name);
                    cmd.env("GDK_BACKEND", "wayland");
                    cmd.env("QT_QPA_PLATFORM", "wayland");
                    cmd.env("CLUTTER_BACKEND", "wayland");
                    cmd.env("SDL_VIDEODRIVER", "wayland");
                    if let Some(ref disp) = self.x11_display {
                        cmd.env("DISPLAY", disp);
                    }
                    match cmd.spawn() {
                        Ok(child) => {
                            eprintln!("[termway] Spawned client: {:?}", cmd_args);
                            self.client_child = Some(child);
                        }
                        Err(e) => {
                            eprintln!("[termway] Failed to spawn client {:?}: {}", cmd_args, e);
                        }
                    }
                }
            }

            // A. Check if client process exited
            if let Some(ref mut child) = self.client_child {
                match child.try_wait() {
                    Ok(Some(_status)) => break,
                    Ok(None) => {}
                    Err(_) => break,
                }
            }

            // B. Accept new Wayland clients
            while let Ok(Some(stream)) = self.listener.accept() {
                let _ = self
                    .display
                    .handle()
                    .insert_client(stream, Arc::new(ClientState::default()));
            }

            // C. Non-blocking read from terminal stdin
            let mut poll_fds = [nix::poll::PollFd::new(
                std::os::fd::AsFd::as_fd(&stdin),
                nix::poll::PollFlags::POLLIN,
            )];

            if let Ok(res) = nix::poll::poll(&mut poll_fds, nix::poll::PollTimeout::try_from(10).unwrap()) {
                if res > 0 {
                    if let Some(revents) = poll_fds[0].revents() {
                        if revents.contains(nix::poll::PollFlags::POLLIN) {
                            if let Ok(n) = stdin.lock().read(&mut chunk) {
                                if n > 0 {
                                    self.input_buffer.extend_from_slice(&chunk[..n]);
                                }
                            }
                        }
                    }
                }
            }

            // D. Process terminal input events
            self.process_terminal_input();

            // E. Dispatch Wayland client protocol messages
            let _ = self.display.dispatch_clients(&mut self.state);
            let _ = self.display.flush_clients();

            // F. Render if damaged
            if self.state.dirty {
                let w = self.renderer.width();
                let h = self.renderer.height();
                if let Ok(pixels) = self.renderer.render(&self.state) {
                    let _ = self.backend.render_pixels(pixels, w, h);

                    let time = self.state.start_time.elapsed().as_millis() as u32;
                    for window in &self.state.windows {
                        send_frames_surface_tree(window.wl_surface(), time);
                    }
                }
                self.state.dirty = false;
            }
        }

        Ok(())
    }

    fn process_terminal_input(&mut self) {
        while !self.input_buffer.is_empty() {
            // 1. Try parsing SGR mouse sequence
            if let Ok(input_str) = std::str::from_utf8(&self.input_buffer) {
                if let Some((mouse_ev, consumed)) = SgrMouseParser::parse(input_str) {
                    self.dispatch_mouse_event(&mouse_ev);
                    self.input_buffer.drain(..consumed);
                    continue;
                }
            }

            // 2. Try parsing keyboard sequence
            if let Some((key_ev, consumed)) = KeyboardParser::parse(&self.input_buffer) {
                if key_ev.code == KeyCode::Ctrl('q') {
                    self.state.running = false;
                    return;
                }
                self.dispatch_key_event(key_ev.code);
                self.input_buffer.drain(..consumed);
                continue;
            }

            if self.input_buffer.len() > 32 {
                self.input_buffer.remove(0);
            } else {
                break;
            }
        }
    }

    fn dispatch_mouse_event(&mut self, ev: &crate::protocol::MouseEvent) {
        let serial = SERIAL_COUNTER.next_serial();
        let time = self.state.start_time.elapsed().as_millis() as u32;

        let px = ev.col as f64 * (self.renderer.width() as f64 / 80.0);
        let py = ev.row as f64 * (self.renderer.height() as f64 / 24.0);
        let loc: Point<f64, Logical> = Point::from((px, py));

        // Focus toplevel window under pointer
        let focus = self.state.windows.first().map(|w| (w.wl_surface().clone(), Point::from((0.0, 0.0))));
        let pointer = self.state.pointer.clone();
        let keyboard = self.state.keyboard.clone();

        match ev.action {
            MouseAction::Move | MouseAction::Drag => {
                let motion = MotionEvent {
                    location: loc,
                    serial,
                    time,
                };
                pointer.motion(&mut self.state, focus, &motion);
                pointer.frame(&mut self.state);
            }
            MouseAction::Press => {
                let btn = match ev.button {
                    MouseButton::Left => 0x110,   // BTN_LEFT
                    MouseButton::Middle => 0x112, // BTN_MIDDLE
                    MouseButton::Right => 0x111,  // BTN_RIGHT
                    _ => 0x110,
                };
                let btn_ev = ButtonEvent {
                    serial,
                    time,
                    button: btn,
                    state: ButtonState::Pressed,
                };
                pointer.button(&mut self.state, &btn_ev);
                pointer.frame(&mut self.state);

                // Set keyboard focus on click
                if let Some(window) = self.state.windows.first() {
                    let surface = window.wl_surface().clone();
                    keyboard.set_focus(&mut self.state, Some(surface), serial);
                }
            }
            MouseAction::Release => {
                let btn = match ev.button {
                    MouseButton::Left => 0x110,
                    MouseButton::Middle => 0x112,
                    MouseButton::Right => 0x111,
                    _ => 0x110,
                };
                let btn_ev = ButtonEvent {
                    serial,
                    time,
                    button: btn,
                    state: ButtonState::Released,
                };
                pointer.button(&mut self.state, &btn_ev);
                pointer.frame(&mut self.state);
            }
        }
    }

    fn dispatch_key_event(&mut self, code: KeyCode) {
        let serial = SERIAL_COUNTER.next_serial();
        let time = self.state.start_time.elapsed().as_millis() as u32;

        // Map standard keys to Linux evdev keycodes
        let evdev_code: u32 = match code {
            KeyCode::Char(c) => match c.to_ascii_lowercase() {
                'a' => 30, 'b' => 48, 'c' => 46, 'd' => 32, 'e' => 18,
                'f' => 33, 'g' => 34, 'h' => 35, 'i' => 23, 'j' => 36,
                'k' => 37, 'l' => 38, 'm' => 50, 'n' => 49, 'o' => 24,
                'p' => 25, 'q' => 16, 'r' => 19, 's' => 31, 't' => 20,
                'u' => 22, 'v' => 47, 'w' => 17, 'x' => 45, 'y' => 21,
                'z' => 44, '1' => 2,  '2' => 3,  '3' => 4,  '4' => 5,
                '5' => 6,  '6' => 7,  '7' => 8,  '8' => 9,  '9' => 10,
                '0' => 11, ' ' => 57, '\n' | '\r' => 28, '\t' => 15,
                _ => 30,
            },
            KeyCode::Enter => 28,
            KeyCode::Backspace => 14,
            KeyCode::Tab => 15,
            KeyCode::Escape => 1,
            KeyCode::Up => 103,
            KeyCode::Down => 108,
            KeyCode::Left => 105,
            KeyCode::Right => 106,
            _ => return,
        };

        let key = Keycode::new(evdev_code);
        let keyboard = self.state.keyboard.clone();

        // Send Press then Release
        keyboard.input::<(), _>(
            &mut self.state,
            key,
            KeyState::Pressed,
            serial,
            time,
            |_, _, _| FilterResult::Forward,
        );
        let release_serial = SERIAL_COUNTER.next_serial();
        keyboard.input::<(), _>(
            &mut self.state,
            key,
            KeyState::Released,
            release_serial,
            time,
            |_, _, _| FilterResult::Forward,
        );
    }
}
