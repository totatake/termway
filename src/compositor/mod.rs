pub mod state;
pub mod renderer;
pub mod xwayland;
pub mod window;
pub mod bitmap_font;
pub mod decoration;

use std::io::{self, Read};
use std::process::{Child, Command};
use std::sync::Arc;
use crate::terminal::winsize::WINCH_RECEIVED;

use smithay::{
    backend::input::{ButtonState, KeyState, Keycode},
    input::{
        keyboard::FilterResult,
        pointer::{ButtonEvent, MotionEvent},
    },
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{Display, ListeningSocket},
    },
    utils::{Logical, Point, SERIAL_COUNTER},
};

use crate::backend::TerminalBackend;
use crate::engine::EngineConfig;
use crate::protocol::{KeyCode, KeyboardParser, MouseAction, MouseButton, SgrMouseParser};
use crate::terminal::{RawModeGuard, TerminalSize};

use self::renderer::TermwayRenderer;
use self::state::{send_frames_surface_tree, ClientState, TermwayState};
use self::window::{WindowDragState, WindowSurface, BORDER_WIDTH, TITLEBAR_HEIGHT};

// Raw mode では `\n` だけだと行頭に戻らず表示が崩れるため、CRLF で出力する。
pub(crate) fn log_line(msg: impl AsRef<str>) {
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/termway_debug.log") {
        let _ = writeln!(file, "{}", msg.as_ref());
    }
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
    has_opened_windows: bool,
    spawn_time: Option<std::time::Instant>,
    xwayland_source: Option<smithay::xwayland::XWayland>,
    _xwayland_client: Option<smithay::reexports::wayland_server::Client>,
    session: crate::session::SessionManager,
    cell_size_queried: bool,
}

impl TermwayCompositor {
    pub fn new(config: EngineConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let mut term_size = TerminalSize::query().unwrap_or_else(|_| TerminalSize::fallback(80, 24));
        let cell_size_queried = config.cell_size.is_some();
        if let Some((cw, ch)) = config.cell_size {
            term_size.cell_width_px = cw;
            term_size.cell_height_px = ch;
            term_size.width_px = term_size.cols * cw;
            term_size.height_px = term_size.rows * ch;
        }
        let max_grid_w = term_size.cols as u32 * term_size.cell_width_px as u32;
        let effective_w = if max_grid_w > 0 {
            (term_size.width_px as u32).min(max_grid_w)
        } else {
            term_size.width_px as u32
        };
        let width = config.width.unwrap_or(effective_w).max(320);
        let height = if let Some(h) = config.height {
            h.max(240)
        } else if config.use_sixel {
            // Sixel 出力時は最下行（ステータス表示行）を空け、高さを 6 の倍数に切り捨てて端末スクロールを完全に防止
            let avail_rows = term_size.rows.saturating_sub(1).max(1);
            let raw_h = avail_rows as u32 * term_size.cell_height_px as u32;
            (raw_h / 6 * 6).max(240)
        } else {
            let max_grid_h = term_size.rows as u32 * term_size.cell_height_px as u32;
            let effective_h = if max_grid_h > 0 {
                (term_size.height_px as u32).min(max_grid_h)
            } else {
                term_size.height_px as u32
            };
            effective_h.max(240)
        };

        Self::ensure_xdg_runtime_dir();

        let session = crate::session::SessionManager::new();

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
        let state = TermwayState::new(&dh, width, height, config.enable_wm);

        // 3. Initialize Pixman Offscreen Renderer
        let renderer = TermwayRenderer::new(width, height)?;

        // 4. Initialize Terminal Output Backend
        let backend = TerminalBackend::new(term_size, config.use_sixel, true);

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
            "#!/bin/bash\nunset WAYLAND_SOCKET\nexport WAYLAND_DISPLAY={}\nARGS=()\nfor arg in \"$@\"; do\n    if [ \"$arg\" != \"-terminate\" ]; then\n        ARGS+=(\"$arg\")\n    fi\ndone\nexec {} \"${{ARGS[@]}}\" -ac\n",
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
            has_opened_windows: false,
            spawn_time: None,
            pending_command,
            xwayland_source,
            _xwayland_client,
            socket_name,
            x11_display,
            session,
            cell_size_queried,
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
                        display_number,
                    } => {
                        let disp_str = format!(":{}", display_number);
                        log_line(format!("[termway] XWayland ready on {}, starting XWM...", disp_str));
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
        let mut all_windows_closed_time: Option<std::time::Instant> = None;
        while self.state.running {
            loop_count += 1;
            
            // Dispatch calloop (processes XWM and other registered sources) with 0 timeout
            let _ = event_loop.dispatch(Some(std::time::Duration::ZERO), &mut self.state);
            
            if WINCH_RECEIVED.swap(false, std::sync::atomic::Ordering::Relaxed) {
                if let Ok(mut new_size) = TerminalSize::query() {
                    let old_size = self.backend.term_size();
                    if self.cell_size_queried {
                        new_size.cell_width_px = old_size.cell_width_px;
                        new_size.cell_height_px = old_size.cell_height_px;
                        new_size.width_px = new_size.cols * old_size.cell_width_px;
                        new_size.height_px = new_size.rows * old_size.cell_height_px;
                    }
                    self.apply_terminal_size(new_size);
                }
            }
            
            // Spawn pending command after Wayland globals are advertised and XWayland (if present) is ready
            let xwayland_is_ready = self.state.xwayland_ready || self.x11_display.is_none();
            if loop_count >= 5 && xwayland_is_ready {
                // Start Polkit Authentication Agent once Wayland and XWayland are fully ready
                if !self.session.is_polkit_running() {
                    self.session.update_activation_environment(Some(&self.socket_name), self.x11_display.as_deref());
                    self.session.start_polkit(Some(&self.socket_name), self.x11_display.as_deref());
                    // Allow the agent to register with D-Bus and polkitd before launching the client
                    std::thread::sleep(std::time::Duration::from_millis(300));
                }

                if let Some(cmd_args) = self.pending_command.take() {
                    let mut cmd = Command::new(&cmd_args[0]);
                    if cmd_args.len() > 1 {
                        cmd.args(&cmd_args[1..]);
                    }
                    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
                        cmd.env("XDG_RUNTIME_DIR", runtime_dir);
                    }
                    cmd.env("WAYLAND_DISPLAY", &self.socket_name);
                    cmd.env("GDK_BACKEND", "wayland,x11");
                    cmd.env("QT_QPA_PLATFORM", "wayland;xcb");
                    cmd.env("CLUTTER_BACKEND", "wayland");
                    cmd.env("SDL_VIDEODRIVER", "wayland,x11");
                    if self.state.enable_wm {
                        cmd.env("GTK_CSD", "0");
                        cmd.env("QT_WAYLAND_DISABLE_WINDOWDECORATION", "1");
                    }
                    if let Some(ref disp) = self.x11_display {
                        cmd.env("DISPLAY", disp);
                    }
                    if let Some(dbus) = self.session.dbus_address_for_client(&cmd_args[0]) {
                        cmd.env("DBUS_SESSION_BUS_ADDRESS", dbus);
                    }
                    if let Ok(file) = std::fs::OpenOptions::new()
                        .create(true)
                        .truncate(true)
                        .write(true)
                        .open("/tmp/termway_client.log")
                    {
                        if let Ok(err_file) = file.try_clone() {
                            cmd.stdout(file);
                            cmd.stderr(err_file);
                        }
                    }
                    match cmd.spawn() {
                        Ok(child) => {
                            log_line(format!("[termway] Spawned client: {:?}", cmd_args));
                            self.client_child = Some(child);
                            self.spawn_time = Some(std::time::Instant::now());
                        }
                        Err(e) => {
                            log_line(format!("[termway] Failed to spawn client {:?}: {}", cmd_args, e));
                            return Err(format!("Failed to execute client command '{}': {}", cmd_args[0], e).into());
                        }
                    }
                }
            }

            // Clean up destroyed windows/surfaces
            self.state.windows.retain(|w| smithay::reexports::wayland_server::Resource::is_alive(w.wl_surface()));
            self.state.managed_windows.retain(|w| {
                w.wl_surface().as_ref().map(|s| smithay::reexports::wayland_server::Resource::is_alive(s)).unwrap_or(true)
            });

            let polkit_pid = self.session.polkit_pid();
            let is_x11_polkit = |xwin: &smithay::xwayland::xwm::X11Surface| -> bool {
                if let (Some(ppid), Some(wpid)) = (polkit_pid, xwin.pid()) {
                    if ppid == wpid {
                        return true;
                    }
                }
                let title = xwin.title().to_lowercase();
                let class = xwin.class().to_lowercase();
                title.contains("authentication")
                    || title.contains("polkit")
                    || class.contains("polkit")
                    || class.contains("policykit")
            };

            let is_wayland_polkit = |surface: &smithay::wayland::shell::xdg::ToplevelSurface| -> bool {
                let title = smithay::wayland::compositor::with_states(surface.wl_surface(), |states| {
                    states
                        .data_map
                        .get::<smithay::wayland::shell::xdg::XdgToplevelSurfaceData>()
                        .and_then(|d| d.lock().ok())
                        .and_then(|attrs| attrs.title.clone())
                }).unwrap_or_default().to_lowercase();
                title.contains("authentication") || title.contains("polkit") || title.contains("policykit")
            };

            let active_window_count = self.state.managed_windows.len()
                + self.state.windows.len()
                + self.state.x11_windows.len();

            let client_window_count = self.state.managed_windows.iter().filter(|w| !w.is_polkit(polkit_pid)).count()
                + self.state.windows.iter().filter(|w| !is_wayland_polkit(w)).count()
                + self.state.x11_windows.iter().filter(|w| !is_x11_polkit(w)).count();

            if client_window_count > 0 {
                self.has_opened_windows = true;
            }

            // Check if client child process is still running
            let is_child_alive = if let Some(ref mut child) = self.client_child {
                match child.try_wait() {
                    Ok(None) => true,
                    _ => false,
                }
            } else {
                false
            };

            // A. Check exit condition based on window lifecycle
            if self.has_opened_windows {
                if active_window_count == 0 {
                    if is_child_alive {
                        // While the child process is still running, do not exit immediately on 0 windows;
                        // wait up to 3 seconds grace period in case a new window is about to open (e.g. after dialog or splash).
                        if let Some(t) = all_windows_closed_time {
                            if t.elapsed() > std::time::Duration::from_millis(3000) {
                                log_line("[termway] All windows closed and grace period expired while client running, exiting compositor.");
                                break;
                            }
                        } else {
                            all_windows_closed_time = Some(std::time::Instant::now());
                        }
                    } else {
                        // Client child has finished and all windows are closed
                        log_line("[termway] All windows closed and client exited, exiting compositor.");
                        break;
                    }
                } else {
                    all_windows_closed_time = None;
                }
            } else if let Some(ref mut child) = self.client_child {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        if !status.success() {
                            let err_msg = std::fs::read_to_string("/tmp/termway_client.log").unwrap_or_default();
                            let preview = err_msg.lines().take(5).collect::<Vec<_>>().join("\n  ");
                            log_line(format!(
                                "[termway] Client process exited with error ({:?}) before opening any window: {}",
                                status,
                                if preview.is_empty() { "(no output)" } else { &preview }
                            ));
                            let msg = if preview.is_empty() {
                                format!("Client process exited with error ({}) before opening any window.", status)
                            } else {
                                format!("Client process exited with error ({}) before opening any window:\n  {}", status, preview)
                            };
                            return Err(msg.into());
                        }
                        // If client exited with success (e.g. gvim fork or gnome-terminal launcher), wait up to 6s grace period for window to appear
                        if let Some(st) = self.spawn_time {
                            if st.elapsed() > std::time::Duration::from_secs(6) {
                                log_line("[termway] Client exited and grace period expired with no window opened.");
                                let err_msg = std::fs::read_to_string("/tmp/termway_client.log").unwrap_or_default();
                                let preview = err_msg.lines().take(5).collect::<Vec<_>>().join("\n  ");
                                let msg = if preview.is_empty() {
                                    "Client process exited and grace period expired with no window opened.".to_string()
                                } else {
                                    format!("Client process exited and no window was opened:\n  {}", preview)
                                };
                                return Err(msg.into());
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(e) => return Err(format!("Error waiting for client process: {}", e).into()),
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
                    for window in &self.state.managed_windows {
                        if let Some(surface) = self.state.surface_for_managed_window(window) {
                            send_frames_surface_tree(&surface, time);
                        }
                    }
                    for window in &self.state.windows {
                        send_frames_surface_tree(window.wl_surface(), time);
                    }
                    for window in &self.state.x11_windows {
                        if let Some(surface) = self.state.surface_for_x11(window) {
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

    fn apply_terminal_size(&mut self, mut new_size: TerminalSize) {
        let max_grid_w = new_size.cols as u32 * new_size.cell_width_px as u32;
        if max_grid_w > 0 {
            new_size.width_px = (new_size.width_px as u32).min(max_grid_w).max(320) as u16;
        }
        if self.backend.is_sixel() {
            let avail_rows = new_size.rows.saturating_sub(1).max(1);
            let raw_h = avail_rows as u32 * new_size.cell_height_px as u32;
            new_size.height_px = ((raw_h / 6 * 6).max(240)) as u16;
        } else {
            let max_grid_h = new_size.rows as u32 * new_size.cell_height_px as u32;
            if max_grid_h > 0 {
                new_size.height_px = (new_size.height_px as u32).min(max_grid_h).max(240) as u16;
            }
        }

        if self.term_cols == new_size.cols
            && self.term_rows == new_size.rows
            && self.state.width == new_size.width_px as u32
            && self.state.height == new_size.height_px as u32
        {
            return;
        }

        self.term_cols = new_size.cols;
        self.term_rows = new_size.rows;
        self.state.width = new_size.width_px as u32;
        self.state.height = new_size.height_px as u32;

        let _ = self.renderer.resize(self.state.width, self.state.height);
        self.backend.resize(new_size);

        // 端末サイズ変更時は画面をクリアしてリサイズ直後の残骸とカーソル位置をリセット (Sixelのみ、Kittyは画像を上書きするため不要かつ消去を防ぐ)
        if self.backend.is_sixel() {
            let mut stdout = std::io::stdout();
            let _ = std::io::Write::write_all(&mut stdout, b"\x1b[2J\x1b[H");
            let _ = std::io::Write::flush(&mut stdout);
        }

        let new_mode = smithay::output::Mode {
            size: (self.state.width as i32, self.state.height as i32).into(),
            refresh: 60000,
        };
        self.state.output.change_current_state(Some(new_mode), None, None, None);

        if self.state.enable_wm {
            for m_win in &mut self.state.managed_windows {
                let top_margin = if m_win.has_ssd { TITLEBAR_HEIGHT + BORDER_WIDTH * 2 } else { 0 };
                let side_margin = if m_win.has_ssd { BORDER_WIDTH * 2 } else { 0 };
                let max_w = (self.state.width as i32 - side_margin - 10).max(160);
                let max_h = (self.state.height as i32 - top_margin - 10).max(100);
                let new_w = m_win.size.w.min(max_w);
                let new_h = m_win.size.h.min(max_h);

                let min_x = if m_win.has_ssd { BORDER_WIDTH } else { 0 };
                let max_x = (self.state.width as i32 - new_w - min_x).max(min_x);
                let min_y = if m_win.has_ssd { TITLEBAR_HEIGHT + BORDER_WIDTH } else { 0 };
                let max_y = (self.state.height as i32 - new_h - min_x).max(min_y);
                m_win.loc.x = m_win.loc.x.clamp(min_x, max_x);
                m_win.loc.y = m_win.loc.y.clamp(min_y, max_y);

                if new_w != m_win.size.w || new_h != m_win.size.h {
                    m_win.configure_size(new_w, new_h);
                }
            }
        } else {
            for window in &self.state.windows {
                window.with_pending_state(|state| {
                    state.states.set(xdg_toplevel::State::Activated);
                    state.states.set(xdg_toplevel::State::Maximized);
                    state.size = Some((self.state.width as i32, self.state.height as i32).into());
                });
                window.send_configure();
            }
            for xwin in &self.state.x11_windows {
                let rect = smithay::utils::Rectangle::new(
                    (0, 0).into(),
                    (self.state.width as i32, self.state.height as i32).into(),
                );
                let _ = xwin.configure(rect);
            }
            for m_win in &mut self.state.managed_windows {
                m_win.loc = (0, 0).into();
                m_win.size = (self.state.width as i32, self.state.height as i32).into();
            }
        }

        self.state.dirty = true;
    }

    fn process_terminal_input(&mut self) {
        while !self.input_buffer.is_empty() {
            // 1. Try parsing cell size response (\x1b[6;height;widtht from \x1b[16t)
            if let Ok(input_str) = std::str::from_utf8(&self.input_buffer) {
                if let Some(start) = input_str.find("\x1b[6;") {
                    if let Some(end_offset) = input_str[start..].find('t') {
                        let inner = &input_str[start + 4..start + end_offset];
                        let parts: Vec<&str> = inner.split(';').collect();
                        if parts.len() == 2 {
                            if let (Ok(h), Ok(w)) = (parts[0].parse::<u16>(), parts[1].parse::<u16>()) {
                                if w > 0 && h > 0 {
                                    log_line(format!("[termway] Terminal reported cell size: {}x{}px", w, h));
                                    self.cell_size_queried = true;
                                    let mut new_size = self.backend.term_size();
                                    new_size.cell_width_px = w;
                                    new_size.cell_height_px = h;
                                    new_size.width_px = new_size.cols * w;
                                    new_size.height_px = new_size.rows * h;
                                    self.apply_terminal_size(new_size);
                                }
                            }
                        }
                        self.input_buffer.drain(start..start + end_offset + 1);
                        continue;
                    }
                }
            }

            // 2. Try parsing window pixel size response (\x1b[4;height;widtht from \x1b[14t)
            if let Ok(input_str) = std::str::from_utf8(&self.input_buffer) {
                if let Some(start) = input_str.find("\x1b[4;") {
                    if let Some(end_offset) = input_str[start..].find('t') {
                        let inner = &input_str[start + 4..start + end_offset];
                        let parts: Vec<&str> = inner.split(';').collect();
                        if parts.len() == 2 {
                            if let (Ok(h), Ok(w)) = (parts[0].parse::<u16>(), parts[1].parse::<u16>()) {
                                if w > 0 && h > 0 {
                                    log_line(format!("[termway] Terminal reported window size: {}x{}px", w, h));
                                    if !self.cell_size_queried {
                                        let mut new_size = self.backend.term_size();
                                        if new_size.cols > 0 {
                                            new_size.cell_width_px = (w / new_size.cols).max(1);
                                        }
                                        if new_size.rows > 0 {
                                            new_size.cell_height_px = (h / new_size.rows).max(1);
                                        }
                                        new_size.width_px = new_size.cols * new_size.cell_width_px;
                                        new_size.height_px = new_size.rows * new_size.cell_height_px;
                                        self.apply_terminal_size(new_size);
                                    }
                                }
                            }
                        }
                        self.input_buffer.drain(start..start + end_offset + 1);
                        continue;
                    }
                }
            }

            // 3. Try parsing SGR mouse sequence
            if let Ok(input_str) = std::str::from_utf8(&self.input_buffer) {
                if let Some((mouse_ev, consumed)) = SgrMouseParser::parse(input_str) {
                    self.dispatch_mouse_event(&mouse_ev);
                    self.input_buffer.drain(..consumed);
                    continue;
                }
            }

            // 4. Try parsing keyboard sequence
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

        let avail_rows = if self.backend.is_sixel() {
            self.term_rows.saturating_sub(1).max(1)
        } else {
            self.term_rows.max(1)
        };
        let cell_w = self.renderer.width() as f64 / self.term_cols as f64;
        let cell_h = self.renderer.height() as f64 / avail_rows as f64;
        let px = (ev.col.saturating_sub(1)) as f64 * cell_w + (cell_w * 0.5);
        let py = (ev.row.saturating_sub(1)) as f64 * cell_h + (cell_h * 0.5);
        let loc: Point<f64, Logical> = Point::from((px, py));
        let loc_i32 = Point::<i32, Logical>::from((px as i32, py as i32));

        let pointer = self.state.pointer.clone();
        let keyboard = self.state.keyboard.clone();

        match ev.action {
            MouseAction::Move | MouseAction::Drag => {
                // 1. Process active window dragging (Moving or Resizing)
                match self.state.drag_state {
                    WindowDragState::Moving { start_cursor, start_window_pos } => {
                        let dx = (px - start_cursor.0).round() as i32;
                        let dy = (py - start_cursor.1).round() as i32;
                        if let Some(top_win) = self.state.managed_windows.last_mut() {
                            let min_x = if top_win.has_ssd { BORDER_WIDTH } else { 0 };
                            let max_x = (self.renderer.width() as i32 - top_win.size.w - min_x).max(min_x);
                            let min_y = if top_win.has_ssd { TITLEBAR_HEIGHT + BORDER_WIDTH } else { 0 };
                            let max_y = (self.renderer.height() as i32 - top_win.size.h - min_x).max(min_y);

                            let new_x = (start_window_pos.0 + dx).clamp(min_x, max_x);
                            let new_y = (start_window_pos.1 + dy).clamp(min_y, max_y);
                            top_win.set_loc(Point::from((new_x, new_y)));
                            self.state.dirty = true;
                        }
                        return;
                    }
                    WindowDragState::Resizing { start_cursor, start_window_geo } => {
                        let dw = (px - start_cursor.0).round() as i32;
                        let dh = (py - start_cursor.1).round() as i32;
                        if let Some(top_win) = self.state.managed_windows.last_mut() {
                            let max_w = (self.renderer.width() as i32 - top_win.loc.x - BORDER_WIDTH).max(100);
                            let max_h = (self.renderer.height() as i32 - top_win.loc.y - BORDER_WIDTH).max(60);

                            let new_w = (start_window_geo.size.w + dw).clamp(100, max_w);
                            let new_h = (start_window_geo.size.h + dh).clamp(60, max_h);
                            top_win.configure_size(new_w, new_h);
                            self.state.dirty = true;
                        }
                        return;
                    }
                    WindowDragState::None => {}
                }

                // 2. Normal pointer motion: hit test managed windows (Top to Bottom)
                let mut focus = None;
                for win in self.state.managed_windows.iter().rev() {
                    let content_geo = win.content_geometry();
                    if content_geo.contains(loc_i32) {
                        if let Some(surface) = self.state.surface_for_managed_window(win) {
                            let win_origin = Point::from((win.loc.x as f64, win.loc.y as f64));
                            focus = Some((surface, win_origin));
                            break;
                        }
                    }
                }
                // Check unmanaged X11 popup/override-redirect surfaces
                if focus.is_none() {
                    for xwin in self.state.x11_windows.iter().rev() {
                        let is_managed = self.state.managed_windows.iter().any(|mw| matches!(&mw.surface, WindowSurface::X11(w) if w == xwin));
                        if !is_managed {
                            let geo = xwin.geometry();
                            if geo.contains(loc_i32) {
                                if let Some(s) = self.state.surface_for_x11(xwin) {
                                    focus = Some((s, Point::from((geo.loc.x as f64, geo.loc.y as f64))));
                                    break;
                                }
                            }
                        }
                    }
                }
                if focus.is_none() {
                    if let Some(surface) = self.state.unassociated_surfaces.last() {
                        focus = Some((surface.clone(), Point::from((0.0, 0.0))));
                    }
                }

                let motion = MotionEvent { location: loc, serial, time };
                pointer.motion(&mut self.state, focus, &motion);
                pointer.frame(&mut self.state);
            }

            MouseAction::Press => {
                let is_left = matches!(ev.button, MouseButton::Left);

                // A. Hit test managed windows from front to back
                let mut hit_window_id = None;
                let mut clicked_close = false;
                let mut clicked_titlebar = false;
                let mut clicked_resize = false;
                let mut clicked_content = false;
                let mut win_loc = Point::from((0, 0));
                let mut win_geo = smithay::utils::Rectangle::new((0, 0).into(), (0, 0).into());

                for win in self.state.managed_windows.iter().rev() {
                    if self.state.enable_wm && win.has_ssd {
                        if is_left && win.close_button_hit_geometry().contains(loc_i32) {
                            hit_window_id = Some(win.id);
                            clicked_close = true;
                            break;
                        }
                        if is_left && win.resize_handle_hit_geometry().contains(loc_i32) {
                            hit_window_id = Some(win.id);
                            clicked_resize = true;
                            win_geo = win.content_geometry();
                            break;
                        }
                        if win.titlebar_geometry().contains(loc_i32) {
                            hit_window_id = Some(win.id);
                            clicked_titlebar = true;
                            win_loc = win.loc;
                            break;
                        }
                    }
                    if win.content_geometry().contains(loc_i32) {
                        hit_window_id = Some(win.id);
                        clicked_content = true;
                        win_loc = win.loc;
                        break;
                    }
                }

                if let Some(id) = hit_window_id {
                    if clicked_close {
                        // Close target window
                        if let Some(pos) = self.state.managed_windows.iter().position(|w| w.id == id) {
                            let win = self.state.managed_windows.remove(pos);
                            win.close();

                            // If this was an X11 surface, send SIGTERM to its process if PID is available
                            if let WindowSurface::X11(ref xwin) = win.surface {
                                if let Ok(pid) = xwin.get_client_pid() {
                                    if pid > 0 {
                                        let _ = nix::sys::signal::kill(
                                            nix::unistd::Pid::from_raw(pid as i32),
                                            nix::sys::signal::Signal::SIGTERM,
                                        );
                                    }
                                }
                            }

                            // If all managed windows closed, terminate compositor or client process
                            if self.state.managed_windows.is_empty() {
                                if let Some(ref mut child) = self.client_child {
                                    let _ = child.kill();
                                }
                                self.state.running = false;
                            } else {
                                if let Some(top) = self.state.managed_windows.last_mut() {
                                    top.active = true;
                                }
                                if let Some(top) = self.state.managed_windows.last() {
                                    if let Some(s) = self.state.surface_for_managed_window(top) {
                                        keyboard.set_focus(&mut self.state, Some(s), serial);
                                    }
                                }
                            }
                            self.state.dirty = true;
                        }
                        return;
                    }

                    // Raise clicked window to top of Z-order & activate
                    self.state.raise_window(id);

                    if clicked_titlebar && is_left {
                        // Begin window drag/move
                        self.state.drag_state = WindowDragState::Moving {
                            start_cursor: (px, py),
                            start_window_pos: (win_loc.x, win_loc.y),
                        };
                        self.state.dirty = true;
                        return;
                    }

                    if clicked_resize && is_left {
                        // Begin window resize
                        self.state.drag_state = WindowDragState::Resizing {
                            start_cursor: (px, py),
                            start_window_geo: win_geo,
                        };
                        self.state.dirty = true;
                        return;
                    }

                    if clicked_content {
                        // Forward press to client surface
                        let win = self.state.managed_windows.last().unwrap();
                        let focus = self.state.surface_for_managed_window(win).map(|s| (s, Point::from((win.loc.x as f64, win.loc.y as f64))));

                        let motion = MotionEvent { location: loc, serial, time };
                        pointer.motion(&mut self.state, focus.clone(), &motion);

                        let btn = match ev.button {
                            MouseButton::Left => 0x110,
                            MouseButton::Middle => 0x112,
                            MouseButton::Right => 0x111,
                            _ => 0x110,
                        };
                        let btn_ev = ButtonEvent { serial, time, button: btn, state: ButtonState::Pressed };
                        pointer.button(&mut self.state, &btn_ev);
                        pointer.frame(&mut self.state);

                        if let Some((s, _)) = focus {
                            keyboard.set_focus(&mut self.state, Some(s), serial);
                        }
                        return;
                    }
                }

                // B. Fallback: outside managed windows (unmanaged X11 popups or desktop)
                let mut focus = None;
                for xwin in self.state.x11_windows.iter().rev() {
                    let is_managed = self.state.managed_windows.iter().any(|mw| matches!(&mw.surface, WindowSurface::X11(w) if w == xwin));
                    if !is_managed {
                        let geo = xwin.geometry();
                        if geo.contains(loc_i32) {
                            if let Some(s) = self.state.surface_for_x11(xwin) {
                                focus = Some((s, Point::from((geo.loc.x as f64, geo.loc.y as f64))));
                                break;
                            }
                        }
                    }
                }
                if focus.is_none() {
                    if let Some(surface) = self.state.unassociated_surfaces.last() {
                        focus = Some((surface.clone(), Point::from((0.0, 0.0))));
                    }
                }

                let motion = MotionEvent { location: loc, serial, time };
                pointer.motion(&mut self.state, focus.clone(), &motion);
                let btn = match ev.button {
                    MouseButton::Left => 0x110,
                    MouseButton::Middle => 0x112,
                    MouseButton::Right => 0x111,
                    _ => 0x110,
                };
                let btn_ev = ButtonEvent { serial, time, button: btn, state: ButtonState::Pressed };
                pointer.button(&mut self.state, &btn_ev);
                pointer.frame(&mut self.state);

                if let Some((s, _)) = focus {
                    keyboard.set_focus(&mut self.state, Some(s), serial);
                }
            }

            MouseAction::Release => {
                let was_dragging = !matches!(self.state.drag_state, WindowDragState::None);
                self.state.drag_state = WindowDragState::None;

                if was_dragging {
                    self.state.dirty = true;
                    return;
                }

                // Forward normal release event
                let mut focus = None;
                for win in self.state.managed_windows.iter().rev() {
                    if win.content_geometry().contains(loc_i32) {
                        if let Some(surface) = self.state.surface_for_managed_window(win) {
                            focus = Some((surface, Point::from((win.loc.x as f64, win.loc.y as f64))));
                            break;
                        }
                    }
                }
                if focus.is_none() {
                    for xwin in self.state.x11_windows.iter().rev() {
                        let is_managed = self.state.managed_windows.iter().any(|mw| matches!(&mw.surface, WindowSurface::X11(w) if w == xwin));
                        if !is_managed {
                            let geo = xwin.geometry();
                            if geo.contains(loc_i32) {
                                if let Some(s) = self.state.surface_for_x11(xwin) {
                                    focus = Some((s, Point::from((geo.loc.x as f64, geo.loc.y as f64))));
                                    break;
                                }
                            }
                        }
                    }
                }
                if focus.is_none() {
                    if let Some(surface) = self.state.unassociated_surfaces.last() {
                        focus = Some((surface.clone(), Point::from((0.0, 0.0))));
                    }
                }

                let motion = MotionEvent { location: loc, serial, time };
                pointer.motion(&mut self.state, focus, &motion);
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
        // Also determine if Shift or Ctrl modifier is required.
        let (evdev_code, need_shift, need_ctrl): (u32, bool, bool) = match code {
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
                    (ev, false, false)
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
                    (ev, true, false)
                }
                '1' => (2, false, false), '2' => (3, false, false), '3' => (4, false, false),
                '4' => (5, false, false), '5' => (6, false, false), '6' => (7, false, false),
                '7' => (8, false, false), '8' => (9, false, false), '9' => (10, false, false),
                '0' => (11, false, false),
                '!' => (2, true, false), '@' => (3, true, false), '#' => (4, true, false),
                '$' => (5, true, false), '%' => (6, true, false), '^' => (7, true, false),
                '&' => (8, true, false), '*' => (9, true, false), '(' => (10, true, false),
                ')' => (11, true, false),
                '-' => (12, false, false), '_' => (12, true, false),
                '=' => (13, false, false), '+' => (13, true, false),
                '[' => (26, false, false), '{' => (26, true, false),
                ']' => (27, false, false), '}' => (27, true, false),
                '\\' => (43, false, false), '|' => (43, true, false),
                ';' => (39, false, false), ':' => (39, true, false),
                '\'' => (40, false, false), '"' => (40, true, false),
                '`' => (41, false, false), '~' => (41, true, false),
                ',' => (51, false, false), '<' => (51, true, false),
                '.' => (52, false, false), '>' => (52, true, false),
                '/' => (53, false, false), '?' => (53, true, false),
                ' ' => (57, false, false),
                '\n' | '\r' => (28, false, false),
                '\t' => (15, false, false),
                _ => return,
            },
            KeyCode::Enter => (28, false, false),
            KeyCode::Backspace => (14, false, false),
            KeyCode::Tab => (15, false, false),
            KeyCode::Escape => (1, false, false),
            KeyCode::Up => (103, false, false),
            KeyCode::Down => (108, false, false),
            KeyCode::Left => (105, false, false),
            KeyCode::Right => (106, false, false),
            KeyCode::Home => (102, false, false),
            KeyCode::End => (107, false, false),
            KeyCode::PageUp => (104, false, false),
            KeyCode::PageDown => (109, false, false),
            KeyCode::Delete => (111, false, false),
            KeyCode::Ctrl(c) => match c.to_ascii_lowercase() {
                'a' => (30, false, true), 'b' => (48, false, true), 'c' => (46, false, true), 'd' => (32, false, true),
                'e' => (18, false, true), 'f' => (33, false, true), 'g' => (34, false, true), 'h' => (35, false, true),
                'i' => (23, false, true), 'j' => (36, false, true), 'k' => (37, false, true), 'l' => (38, false, true),
                'm' => (50, false, true), 'n' => (49, false, true), 'o' => (24, false, true), 'p' => (25, false, true),
                'q' => (16, false, true), 'r' => (19, false, true), 's' => (31, false, true), 't' => (20, false, true),
                'u' => (22, false, true), 'v' => (47, false, true), 'w' => (17, false, true), 'x' => (45, false, true),
                'y' => (21, false, true), 'z' => (44, false, true),
                _ => return,
            },
        };

        // Smithay Keycode expects XKB keycode, which is evdev scancode + 8.
        // (X11/XKB keycodes start at 8, and Smithay subtracts 8 when emitting to Wayland client).
        let key = Keycode::new(evdev_code + 8);
        let shift_key = Keycode::new(42 + 8); // KEY_LEFTSHIFT = 42
        let ctrl_key = Keycode::new(29 + 8);  // KEY_LEFTCTRL = 29
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

        if need_ctrl {
            let ctrl_serial = SERIAL_COUNTER.next_serial();
            keyboard.input::<(), _>(
                &mut self.state,
                ctrl_key,
                KeyState::Pressed,
                ctrl_serial,
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

        if need_ctrl {
            let ctrl_release_serial = SERIAL_COUNTER.next_serial();
            keyboard.input::<(), _>(
                &mut self.state,
                ctrl_key,
                KeyState::Released,
                ctrl_release_serial,
                time,
                |_, _, _| FilterResult::Forward,
            );
            time += 1;
        }

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

    /// Ensures that `XDG_RUNTIME_DIR` is set to an existing, writable directory with 0700 permissions.
    /// When running under `sudo` or minimal container environments, `XDG_RUNTIME_DIR` is often unset or invalid.
    pub fn ensure_xdg_runtime_dir() {
        use std::os::unix::fs::PermissionsExt;

        let needs_fallback = match std::env::var("XDG_RUNTIME_DIR") {
            Ok(dir) => {
                let path = std::path::Path::new(&dir);
                if !path.is_dir() {
                    true
                } else {
                    // Check if writable by creating/checking a test file
                    let test_file = path.join(".termway_runtime_test");
                    match std::fs::OpenOptions::new().create(true).write(true).open(&test_file) {
                        Ok(_) => {
                            let _ = std::fs::remove_file(test_file);
                            false
                        }
                        Err(_) => true,
                    }
                }
            }
            Err(_) => true,
        };

        if needs_fallback {
            let uid = unsafe { libc::getuid() };
            let fallback_dir = format!("/tmp/termway-runtime-{}", uid);
            let path = std::path::Path::new(&fallback_dir);
            if !path.exists() {
                if let Err(e) = std::fs::create_dir_all(path) {
                    log_line(format!("[termway] Warning: Failed to create fallback XDG_RUNTIME_DIR {}: {}", fallback_dir, e));
                }
            }
            // Secure permissions to 0700 (owner read/write/execute only, required by Wayland specs)
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
            std::env::set_var("XDG_RUNTIME_DIR", &fallback_dir);
            log_line(format!("[termway] Using fallback XDG_RUNTIME_DIR={}", fallback_dir));
        }
    }
}
