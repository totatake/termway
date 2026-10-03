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

// Raw mode では `\n` だけだと行頭に戻らず表示が崩れるため、CRLF で出力する。
fn log_line(msg: impl AsRef<str>) {
    eprint!("{}\r\n", msg.as_ref());
}

pub struct TermwayCompositor {
    display: Display<TermwayState>,
    listener: ListeningSocket,
    state: TermwayState,
    renderer: TermwayRenderer,
    backend: TerminalBackend,
    socket_name: String,
    x11_display: Option<String>,
    // ターミナルのセル数（マウス座標変換に使用）
    term_cols: u16,
    term_rows: u16,
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

        log_line("[termway] Headless Wayland Compositor (Smithay)");
        log_line(format!("[termway] Socket: WAYLAND_DISPLAY={}", socket_name));
        log_line(format!("[termway] Resolution: {}x{}", width, height));

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
            std::iter::empty::<(std::ffi::OsString, std::ffi::OsString)>(),
            true,
            std::process::Stdio::null(),
            std::process::Stdio::null(),
            |_| (),
        ) {
            Ok((xwayland, client)) => {
                let disp_num = xwayland.display_number();
                let disp_str = format!(":{}", disp_num);
                log_line(format!("[termway] XWayland initialized on DISPLAY={}", disp_str));
                std::env::set_var("DISPLAY", &disp_str);
                (Some((xwayland, client)), Some(disp_str))
            }
            Err(e) => {
                log_line(format!("[termway] XWayland not available: {}.", e));
                log_line("[termway] Note: Running in native Wayland mode. Wayland applications will run natively.");
                log_line("[termway]       Pure X11 applications (e.g. xeyes, xclock) require an X server.");
                log_line("[termway]       To run pure X11 apps in this environment, run without '--wayland':");
                log_line(format!("[termway]           termway -- {}", config.command.first().map(|s| s.as_str()).unwrap_or("xeyes")));
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
            term_cols: term_size.cols.max(1),
            term_rows: term_size.rows.max(1),
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
            let client = self._xwayland_client.clone().unwrap();
            let handle = loop_handle.clone();
            let _ = loop_handle.insert_source(xwayland, move |event, _, state| {
                match event {
                    smithay::xwayland::XWaylandEvent::Ready {
                        x11_socket,
                        display_number: _,
                    } => {
                        log_line("[termway] XWayland ready, starting XWM...");
                        if let Ok(wm) = smithay::xwayland::xwm::X11Wm::start_wm(
                            handle.clone(),
                            x11_socket,
                            client.clone(),
                        ) {
                            state.xwm = Some(wm);
                            state.xwayland_ready = true;
                            log_line("[termway] XWM started successfully.");
                        } else {
                            state.xwayland_ready = true;
                            log_line("[termway] Failed to start XWM.");
                        }
                    }
                    smithay::xwayland::XWaylandEvent::Error => {
                        state.xwayland_ready = true;
                        log_line("[termway] XWayland encountered an error.");
                    }
                }
            });
        } else {
            self.state.xwayland_ready = true;
        }

        let mut loop_count = 0;
        while self.state.running {
            loop_count += 1;
            
            // Dispatch calloop (processes XWM and other registered sources) with 0 timeout
            let _ = event_loop.dispatch(Some(std::time::Duration::ZERO), &mut self.state);
            
            // Spawn pending command after Wayland globals are advertised and XWayland (if present) is ready
            let xwayland_is_ready = self.state.xwayland_ready || self.x11_display.is_none();
            if loop_count >= 5 && xwayland_is_ready {
                if let Some(cmd_args) = self.pending_command.take() {
                    let mut cmd = Command::new(&cmd_args[0]);
                    if cmd_args.len() > 1 {
                        cmd.args(&cmd_args[1..]);
                    }
                    cmd.env("WAYLAND_DISPLAY", &self.socket_name);
                    cmd.env("GDK_BACKEND", "wayland,x11");
                    cmd.env("QT_QPA_PLATFORM", "wayland;xcb");
                    cmd.env("CLUTTER_BACKEND", "wayland");
                    cmd.env("SDL_VIDEODRIVER", "wayland,x11");
                    if let Some(ref disp) = self.x11_display {
                        cmd.env("DISPLAY", disp);
                    }
                    match cmd.spawn() {
                        Ok(child) => {
                            log_line(format!("[termway] Spawned client: {:?}", cmd_args));
                            self.client_child = Some(child);
                        }
                        Err(e) => {
                            log_line(format!("[termway] Failed to spawn client {:?}: {}", cmd_args, e));
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
                    for window in &self.state.x11_windows {
                        if let Some(surface) = window.wl_surface() {
                            send_frames_surface_tree(&surface, time);
                        }
                    }
                    for surface in &self.state.unassociated_surfaces {
                        send_frames_surface_tree(surface, time);
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

        // col/row は 1 始まりのターミナルセル座標。
        // 1 セルのピクセルサイズ = コンポジター解像度 / ターミナルのセル数。
        // これを掛けてコンポジター空間のピクセル座標に変換する。
        let cell_w = self.renderer.width() as f64 / self.term_cols as f64;
        let cell_h = self.renderer.height() as f64 / self.term_rows as f64;
        let px = (ev.col.saturating_sub(1)) as f64 * cell_w;
        let py = (ev.row.saturating_sub(1)) as f64 * cell_h;
        let loc: Point<f64, Logical> = Point::from((px, py));

        // 優先度：
        // 1. Wayland ネイティブ Toplevel ウィンドウ (Gtkデモなど)
        // 2. X11 関連ウィンドウ (XWayland)
        // 3. その他関連付け前のサーフェス (unassociated_surfaces)
        // focus の Point は「サーフェス原点のコンポジタ座標」。クライアントには
        // (motion.location - 原点) が届く。ウィンドウは全画面 (0,0) 配置なので原点は (0,0)。
        let origin: Point<f64, Logical> = Point::from((0.0_f64, 0.0_f64));
        let mut focus = None;
        if let Some(window) = self.state.windows.first() {
            focus = Some((window.wl_surface().clone(), origin));
        } else {
            if let Some(xwin) = self.state.x11_windows.first() {
                if let Some(s) = xwin.wl_surface() {
                    focus = Some((s, origin));
                }
            }
            if focus.is_none() {
                if let Some(surface) = self.state.unassociated_surfaces.last() {
                    focus = Some((surface.clone(), origin));
                }
            }
        }

        let pointer = self.state.pointer.clone();
        let keyboard = self.state.keyboard.clone();

        match ev.action {
            MouseAction::Move | MouseAction::Drag => {
                let motion = MotionEvent { location: loc, serial, time };
                pointer.motion(&mut self.state, focus, &motion);
                pointer.frame(&mut self.state);
            }
            MouseAction::Press => {
                let btn = match ev.button {
                    MouseButton::Left => 0x110,
                    MouseButton::Middle => 0x112,
                    MouseButton::Right => 0x111,
                    _ => 0x110,
                };
                let btn_ev = ButtonEvent { serial, time, button: btn, state: ButtonState::Pressed };
                pointer.button(&mut self.state, &btn_ev);
                pointer.frame(&mut self.state);

                // キーボードフォーカスもポインタと同じ優先順位で選ぶ。
                let mut kb_surface = None;
                if let Some(window) = self.state.windows.first() {
                    kb_surface = Some(window.wl_surface().clone());
                } else {
                    if let Some(xwin) = self.state.x11_windows.first() {
                        if let Some(s) = xwin.wl_surface() {
                            kb_surface = Some(s);
                        }
                    }
                    if kb_surface.is_none() {
                        if let Some(surface) = self.state.unassociated_surfaces.last() {
                            kb_surface = Some(surface.clone());
                        }
                    }
                }
                if let Some(surface) = kb_surface {
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
                let btn_ev = ButtonEvent { serial, time, button: btn, state: ButtonState::Released };
                pointer.button(&mut self.state, &btn_ev);
                pointer.frame(&mut self.state);
            }
        }
    }

    fn dispatch_key_event(&mut self, code: KeyCode) {
        let serial = SERIAL_COUNTER.next_serial();
        let mut time = self.state.start_time.elapsed().as_millis() as u32;

        // Map standard keys to Linux evdev scancodes.
        // Also determine if Shift modifier is required (e.g. for uppercase letters or symbols).
        let (evdev_code, need_shift): (u32, bool) = match code {
            KeyCode::Char(c) => match c {
                'a'..='z' => {
                    let ev = match c {
                        'a' => 30, 'b' => 48, 'c' => 46, 'd' => 32, 'e' => 18,
                        'f' => 33, 'g' => 34, 'h' => 35, 'i' => 23, 'j' => 36,
                        'k' => 37, 'l' => 38, 'm' => 50, 'n' => 49, 'o' => 24,
                        'p' => 25, 'q' => 16, 'r' => 19, 's' => 31, 't' => 20,
                        'u' => 22, 'v' => 47, 'w' => 17, 'x' => 45, 'y' => 21,
                        'z' => 44,
                        _ => unreachable!(),
                    };
                    (ev, false)
                }
                'A'..='Z' => {
                    let ev = match c.to_ascii_lowercase() {
                        'a' => 30, 'b' => 48, 'c' => 46, 'd' => 32, 'e' => 18,
                        'f' => 33, 'g' => 34, 'h' => 35, 'i' => 23, 'j' => 36,
                        'k' => 37, 'l' => 38, 'm' => 50, 'n' => 49, 'o' => 24,
                        'p' => 25, 'q' => 16, 'r' => 19, 's' => 31, 't' => 20,
                        'u' => 22, 'v' => 47, 'w' => 17, 'x' => 45, 'y' => 21,
                        'z' => 44,
                        _ => unreachable!(),
                    };
                    (ev, true)
                }
                '1' => (2, false), '2' => (3, false), '3' => (4, false),
                '4' => (5, false), '5' => (6, false), '6' => (7, false),
                '7' => (8, false), '8' => (9, false), '9' => (10, false),
                '0' => (11, false),
                '!' => (2, true), '@' => (3, true), '#' => (4, true),
                '$' => (5, true), '%' => (6, true), '^' => (7, true),
                '&' => (8, true), '*' => (9, true), '(' => (10, true),
                ')' => (11, true),
                '-' => (12, false), '_' => (12, true),
                '=' => (13, false), '+' => (13, true),
                '[' => (26, false), '{' => (26, true),
                ']' => (27, false), '}' => (27, true),
                '\\' => (43, false), '|' => (43, true),
                ';' => (39, false), ':' => (39, true),
                '\'' => (40, false), '"' => (40, true),
                '`' => (41, false), '~' => (41, true),
                ',' => (51, false), '<' => (51, true),
                '.' => (52, false), '>' => (52, true),
                '/' => (53, false), '?' => (53, true),
                ' ' => (57, false),
                '\n' | '\r' => (28, false),
                '\t' => (15, false),
                _ => return,
            },
            KeyCode::Enter => (28, false),
            KeyCode::Backspace => (14, false),
            KeyCode::Tab => (15, false),
            KeyCode::Escape => (1, false),
            KeyCode::Up => (103, false),
            KeyCode::Down => (108, false),
            KeyCode::Left => (105, false),
            KeyCode::Right => (106, false),
            KeyCode::Home => (102, false),
            KeyCode::End => (107, false),
            KeyCode::PageUp => (104, false),
            KeyCode::PageDown => (109, false),
            KeyCode::Delete => (111, false),
            KeyCode::Ctrl(c) => match c.to_ascii_lowercase() {
                'a' => (30, false), 'b' => (48, false), 'c' => (46, false), 'd' => (32, false),
                'e' => (18, false), 'f' => (33, false), 'g' => (34, false), 'h' => (35, false),
                'i' => (23, false), 'j' => (36, false), 'k' => (37, false), 'l' => (38, false),
                'm' => (50, false), 'n' => (49, false), 'o' => (24, false), 'p' => (25, false),
                'q' => (16, false), 'r' => (19, false), 's' => (31, false), 't' => (20, false),
                'u' => (22, false), 'v' => (47, false), 'w' => (17, false), 'x' => (45, false),
                'y' => (21, false), 'z' => (44, false),
                _ => return,
            },
        };

        // Smithay Keycode expects XKB keycode, which is evdev scancode + 8.
        // (X11/XKB keycodes start at 8, and Smithay subtracts 8 when emitting to Wayland client).
        let key = Keycode::new(evdev_code + 8);
        let shift_key = Keycode::new(42 + 8); // KEY_LEFTSHIFT = 42
        let keyboard = self.state.keyboard.clone();

        if need_shift {
            let shift_serial = SERIAL_COUNTER.next_serial();
            keyboard.input::<(), _>(
                &mut self.state,
                shift_key,
                KeyState::Pressed,
                shift_serial,
                time,
                |_, _, _| FilterResult::Forward,
            );
            time += 1;
        }

        // Send Press then Release
        keyboard.input::<(), _>(
            &mut self.state,
            key,
            KeyState::Pressed,
            serial,
            time,
            |_, _, _| FilterResult::Forward,
        );
        time += 1;
        let release_serial = SERIAL_COUNTER.next_serial();
        keyboard.input::<(), _>(
            &mut self.state,
            key,
            KeyState::Released,
            release_serial,
            time,
            |_, _, _| FilterResult::Forward,
        );
        time += 1;

        if need_shift {
            let shift_release_serial = SERIAL_COUNTER.next_serial();
            keyboard.input::<(), _>(
                &mut self.state,
                shift_key,
                KeyState::Released,
                shift_release_serial,
                time,
                |_, _, _| FilterResult::Forward,
            );
        }
    }
}
