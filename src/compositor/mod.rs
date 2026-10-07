pub mod state;
pub mod renderer;
pub mod xwayland;
pub mod window;
pub mod bitmap_font;
pub mod decoration;
pub mod input;

use std::io::{self, Read};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use crate::terminal::winsize::WINCH_RECEIVED;

pub static CAPTURE_REQUESTED: AtomicBool = AtomicBool::new(false);
pub static STATE_DUMP_REQUESTED: AtomicBool = AtomicBool::new(false);

fn setup_test_signals() {
    unsafe {
        libc::signal(libc::SIGUSR1, handle_sigusr1 as *const () as usize);
        libc::signal(libc::SIGUSR2, handle_sigusr2 as *const () as usize);
    }
}

extern "C" fn handle_sigusr1(_: libc::c_int) {
    CAPTURE_REQUESTED.store(true, Ordering::Relaxed);
}

extern "C" fn handle_sigusr2(_: libc::c_int) {
    STATE_DUMP_REQUESTED.store(true, Ordering::Relaxed);
}

use smithay::reexports::{
    wayland_protocols::xdg::shell::server::xdg_toplevel,
    wayland_server::{Display, ListeningSocket},
};

use crate::backend::TerminalBackend;
use crate::engine::EngineConfig;
use crate::protocol::{KeyCode, KeyboardParser, SgrMouseParser};
use crate::terminal::{RawModeGuard, TerminalSize};

use self::renderer::TermwayRenderer;
use self::state::{send_frames_surface_tree, ClientState, TermwayState};
use self::window::{WindowSurface, BORDER_WIDTH, TITLEBAR_HEIGHT};

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
    wrap_dir: Option<String>,
}

impl TermwayCompositor {
    pub fn new(config: EngineConfig) -> Result<Self, Box<dyn std::error::Error>> {
        setup_test_signals();
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

        // Check writability of /tmp/.X11-unix so X11 apps don't fail mysteriously or fall back
        crate::session::check_x11_unix_writability()?;

        // 5. Try starting XWayland for legacy X11 compatibility
        // Workaround for environments where WAYLAND_SOCKET FD gets closed (e.g. by security wrappers):
        // We create a wrapper script that unsets WAYLAND_SOCKET and uses WAYLAND_DISPLAY instead.
        let xwayland_real = String::from_utf8(
            std::process::Command::new("which").arg("Xwayland").output().unwrap().stdout
        ).unwrap();
        let xwayland_real = xwayland_real.trim();
        let uid = unsafe { libc::getuid() };
        let pid = std::process::id();
        let wrap_dir = format!("/tmp/termway_wrap_{}_{}", uid, pid);
        let _ = std::fs::create_dir_all(&wrap_dir);
        let wrapper_path = format!("{}/Xwayland", wrap_dir);
        let wrapper_script = format!(
            "#!/bin/bash\nunset WAYLAND_SOCKET\nexport WAYLAND_DISPLAY={}\nARGS=()\nfor arg in \"$@\"; do\n    if [ \"$arg\" != \"-terminate\" ]; then\n        ARGS+=(\"$arg\")\n    fi\ndone\nexec {} \"${{ARGS[@]}}\" -ac\n",
            socket_name, xwayland_real
        );
        std::fs::write(&wrapper_path, wrapper_script).unwrap();
        std::fs::set_permissions(&wrapper_path, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        
        let old_path = std::env::var("PATH").unwrap_or_default();
        std::env::set_var("PATH", format!("{}:{}", wrap_dir, old_path));

        let mut state = state;
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
                state.xwayland_error = Some(format!("XWayland server could not be started: {}", e));
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
            wrap_dir: Some(wrap_dir),
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
                        state.xwayland_ready = false;
                        state.xwayland_error = Some("XWayland failed to start (check permissions on /tmp/.X11-unix)".to_string());
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
            
            if let Some(ref err) = self.state.xwayland_error {
                return Err(format!("XWayland failed to start: {}", err).into());
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
                    } else {
                        cmd.env_remove("DISPLAY");
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
                w.wl_surface().as_ref().map(smithay::reexports::wayland_server::Resource::is_alive).unwrap_or(true)
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

            let is_unmanaged_real_x11 = |w: &smithay::xwayland::X11Surface| -> bool {
                let is_managed = self.state.managed_windows.iter().any(|mw| matches!(&mw.surface, WindowSurface::X11(x) if x == w));
                let geo = w.geometry();
                !is_managed && geo.size.w > 1 && geo.size.h > 1
            };

            let active_window_count = self.state.managed_windows.len()
                + self.state.windows.len()
                + self.state.x11_windows.iter().filter(|w| is_unmanaged_real_x11(w)).count();

            let client_window_count = self.state.managed_windows.iter().filter(|w| !w.is_polkit(polkit_pid)).count()
                + self.state.windows.iter().filter(|w| !is_wayland_polkit(w)).count()
                + self.state.x11_windows.iter().filter(|w| !is_x11_polkit(w) && is_unmanaged_real_x11(w)).count();

            if client_window_count > 0 {
                self.has_opened_windows = true;
            }

            // Check if client child process is still running
            let is_child_alive = self
                .client_child
                .as_mut()
                .map(|c| matches!(c.try_wait(), Ok(None)))
                .unwrap_or(false);

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
            self.state.popups.cleanup();

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
                        if let WindowSurface::Wayland(ref toplevel) = window.surface {
                            for (popup, _) in smithay::desktop::PopupManager::popups_for_surface(toplevel.wl_surface()) {
                                send_frames_surface_tree(popup.wl_surface(), time);
                            }
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

            // G. Process test capture / inspection requests
            let dump_requested = CAPTURE_REQUESTED.swap(false, Ordering::Relaxed);
            let state_requested = STATE_DUMP_REQUESTED.swap(false, Ordering::Relaxed);

            if dump_requested || std::env::var("TERMWAY_TEST_CAPTURE_NOW").is_ok() {
                if std::env::var("TERMWAY_TEST_CAPTURE_NOW").is_ok() {
                    std::env::remove_var("TERMWAY_TEST_CAPTURE_NOW");
                }
                let dump_dir = std::env::var("TERMWAY_TEST_DUMP_DIR").unwrap_or_else(|_| "/tmp".into());
                let _ = std::fs::create_dir_all(&dump_dir);
                let w = self.renderer.width();
                let h = self.renderer.height();
                if let Ok(pixels) = self.renderer.render(&self.state) {
                    let ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis();
                    let path = format!("{}/termway_capture_{}.png", dump_dir, ts);
                    if let Some(img) = image::RgbaImage::from_raw(w, h, pixels.to_vec()) {
                        let _ = img.save(&path);
                        log_line(format!("[test] Saved screenshot to {}", path));
                    }
                    // Also maintain latest link/copy
                    let latest_path = format!("{}/termway_capture_latest.png", dump_dir);
                    if let Some(img) = image::RgbaImage::from_raw(w, h, pixels.to_vec()) {
                        let _ = img.save(&latest_path);
                    }
                }
            }

            if state_requested || std::env::var("TERMWAY_TEST_STATE_NOW").is_ok() {
                if std::env::var("TERMWAY_TEST_STATE_NOW").is_ok() {
                    std::env::remove_var("TERMWAY_TEST_STATE_NOW");
                }
                let state_file = std::env::var("TERMWAY_TEST_STATE_FILE")
                    .unwrap_or_else(|_| "/tmp/termway_gui_state.json".into());
                let json = self.state.dump_state_json();
                let _ = std::fs::write(&state_file, json);
                log_line(format!("[test] Dumped GUI state to {}", state_file));
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
                                        if let Some(res) = w.checked_div(new_size.cols) {
                                            new_size.cell_width_px = res.max(1);
                                        }
                                        if let Some(res) = h.checked_div(new_size.rows) {
                                            new_size.cell_height_px = res.max(1);
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
        let avail_rows = if self.backend.is_sixel() {
            self.term_rows.saturating_sub(1).max(1)
        } else {
            self.term_rows.max(1)
        };
        self::input::dispatch_mouse_event(
            &mut self.state,
            self.renderer.width(),
            self.renderer.height(),
            self.term_cols,
            avail_rows,
            ev,
            &mut self.client_child,
        );
    }

    fn dispatch_key_event(&mut self, code: KeyCode) {
        self::input::dispatch_key_event(&mut self.state, code);
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
                    match std::fs::OpenOptions::new().create(true).truncate(true).write(true).open(&test_file) {
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

impl Drop for TermwayCompositor {
    fn drop(&mut self) {
        if let Some(ref dir) = self.wrap_dir {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}
