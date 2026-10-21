pub mod state;
pub mod window_manager;
mod handlers;
pub mod renderer;
pub mod xwayland;
pub mod window;
pub mod bitmap_font;
pub mod decoration;
pub mod input;
pub mod env;
pub mod taskbar;
pub mod profile;
pub mod screenshot;

pub use self::env::{
    ensure_xdg_runtime_dir, free_x11_display_candidates, log_line, setup_test_signals,
    CAPTURE_REQUESTED, STATE_DUMP_REQUESTED,
};
pub use self::profile::CompositorMetrics;
pub use self::taskbar::{Taskbar, TaskbarChip};

use std::io::{self, Read};
use std::process::{Child, Command};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use crate::terminal::winsize::WINCH_RECEIVED;

use smithay::reexports::{
    wayland_protocols::xdg::shell::server::xdg_toplevel,
    wayland_server::{Display, ListeningSocket},
};

use crate::backend::TerminalBackend;
use crate::error::{Result, TermwayError};
use crate::protocol::{KeyCode, KeyboardParser, SgrMouseParser};
use crate::terminal::{RawModeGuard, TerminalSize};

use self::env::find_xwayland_binary;
use self::renderer::TermwayRenderer;
use self::state::{send_frames_surface_tree, ClientState, TermwayState};
use self::window::WindowSurface;

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub cell_size: Option<(u16, u16)>,
    pub fps: u32,
    pub scale: f64,
    pub enable_wm: bool,
    pub use_sixel: bool,
    pub hud: bool,
    pub profile: bool,
    pub no_splash: bool,
    pub command: Vec<String>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            width: None,
            height: None,
            cell_size: None,
            fps: 30,
            scale: 1.0,
            enable_wm: true,
            use_sixel: false,
            hud: false,
            profile: false,
            no_splash: false,
            command: Vec::new(),
        }
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
    taskbar: Taskbar,
    input_buffer: Vec<u8>,
    pending_command: Option<Vec<String>>,
    client_child: Option<Child>,
    client_log_file: Option<tempfile::NamedTempFile>,
    has_opened_windows: bool,
    spawn_time: Option<std::time::Instant>,
    xwayland_source: Option<smithay::xwayland::XWayland>,
    _xwayland_client: Option<smithay::reexports::wayland_server::Client>,
    session: crate::session::SessionManager,
    cell_size_queried: bool,
    wrap_dir: Option<tempfile::TempDir>,
    damage_tracker: crate::display::DamageTracker,
    prev_taskbar: Option<String>,
    target_frame_duration: std::time::Duration,
    profile_enabled: bool,
    metrics: CompositorMetrics,
}

impl TermwayCompositor {
    pub fn new(config: EngineConfig) -> Result<Self> {
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
        let avail_rows = term_size.rows.saturating_sub(1).max(1);
        let raw_h = avail_rows as u32 * term_size.cell_height_px as u32;
        let height = if let Some(h) = config.height {
            h.max(240)
        } else if config.use_sixel {
            // Sixel 出力時は最下行（ステータス表示行）を空け、高さを 6 の倍数に切り捨てて端末スクロールを完全に防止
            (raw_h / 6 * 6).max(240)
        } else {
            // Kitty 出力時も最下行（タスクバー・ステータス表示行）を空けてウィンドウとの重なりを防止
            raw_h.max(240)
        };

        Self::ensure_xdg_runtime_dir();

        let session = crate::session::SessionManager::new();

        let display = Display::new()?;
        let dh = display.handle();

        // 1. Create Wayland listening socket (never touch wayland-0 or active sockets)
        let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_default();
        let (listener, socket_name) = {
            let mut id = 1;
            loop {
                let name = format!("wayland-{}", id);
                if !runtime_dir.is_empty() {
                    let sock_path = std::path::Path::new(&runtime_dir).join(&name);
                    if let Ok(meta) = std::fs::symlink_metadata(&sock_path) {
                        if meta.file_type().is_symlink() || std::os::unix::net::UnixStream::connect(&sock_path).is_ok() {
                            id += 1;
                            if id > 32 {
                                return Err(TermwayError::NoFreeWaylandSocket);
                            }
                            continue;
                        }
                    }
                }
                if let Ok(socket) = ListeningSocket::bind(&name) {
                    break (socket, name);
                }
                id += 1;
                if id > 32 {
                    return Err(TermwayError::NoFreeWaylandSocket);
                }
            }
        };

        log_line("[termway] Headless Wayland Compositor (Smithay)");
        log_line(format!("[termway] Socket: WAYLAND_DISPLAY={}", socket_name));
        log_line(format!("[termway] Resolution: {}x{} (@{:.2}x)", width, height, config.scale));

        // 2. Initialize Compositor State
        let state = TermwayState::with_scale(&dh, width, height, config.enable_wm, config.scale);
        let refresh_mhz = (config.fps.clamp(1, 240) * 1000) as i32;
        let mode = smithay::output::Mode {
            size: (width as i32, height as i32).into(),
            refresh: refresh_mhz,
        };
        state.output.change_current_state(Some(mode), None, None, None);
        state.output.set_preferred(mode);

        // 3. Initialize Pixman Offscreen Renderer
        let renderer = TermwayRenderer::new(width, height)?;

        // 4. Initialize Terminal Output Backend
        let mut backend = TerminalBackend::new(term_size, config.use_sixel);
        if config.hud {
            backend.set_hud_enabled(true);
        }

        // Check writability of /tmp/.X11-unix so X11 apps don't fail mysteriously or fall back
        crate::session::check_x11_unix_writability()?;

        // 5. Try starting XWayland for legacy X11 compatibility
        // Workaround for environments where WAYLAND_SOCKET FD gets closed (e.g. by security wrappers):
        // We create a wrapper script that unsets WAYLAND_SOCKET and uses WAYLAND_DISPLAY instead.
        let mut wrap_dir_guard = None;
        if let Some(xwayland_real) = find_xwayland_binary() {
            if let Ok(temp_dir) = tempfile::Builder::new().prefix("termway_wrap_").tempdir() {
                let wrapper_path = temp_dir.path().join("Xwayland");
                let wrapper_script = format!(
                    "#!/bin/bash\nunset WAYLAND_SOCKET\nexport WAYLAND_DISPLAY={}\nARGS=()\nfor arg in \"$@\"; do\n    if [ \"$arg\" != \"-terminate\" ]; then\n        ARGS+=(\"$arg\")\n    fi\ndone\nexec {} \"${{ARGS[@]}}\" -ac\n",
                    socket_name, xwayland_real.display()
                );
                if std::fs::write(&wrapper_path, wrapper_script).is_ok() {
                    let _ = std::fs::set_permissions(&wrapper_path, std::os::unix::fs::PermissionsExt::from_mode(0o700));
                    let old_path = std::env::var("PATH").unwrap_or_default();
                    std::env::set_var("PATH", format!("{}:{}", temp_dir.path().display(), old_path));
                    wrap_dir_guard = Some(temp_dir);
                }
            }
        }

        let mut state = state;
        // Allocate an X11 display number >= 10 and verify neither the lockfile nor the socket
        // exists on disk. Passing `None` to Smithay starts at `:0` and only checks `/tmp/.X0-lock`,
        // which on WSL2 (WSLg) unlinks `/tmp/.X11-unix/X0` because WSLg's lockfile lives in the
        // system container rather than the distro's `/tmp`.
        let mut xwayland_res = Err(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            "No free X11 display slot in 10..=64",
        ));
        for disp_candidate in Self::free_x11_display_candidates() {
            match smithay::xwayland::XWayland::spawn(
                &dh,
                Some(disp_candidate),
                std::iter::empty::<(std::ffi::OsString, std::ffi::OsString)>(),
                true,
                std::process::Stdio::null(),
                std::process::Stdio::null(),
                |_| (),
            ) {
                Ok(res) => {
                    xwayland_res = Ok(res);
                    break;
                }
                Err(e) => {
                    xwayland_res = Err(e);
                }
            }
        }

        let (xwayland, x11_display) = match xwayland_res {
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

        state.show_splash = !config.no_splash;
        let proto_name = if config.use_sixel { "SIXEL" } else { "KITTY-ZLIB" };
        let x11_str = x11_display.as_deref().unwrap_or("off");
        state.splash_info = format!("{} | {} | X11{} | {}x{}", proto_name, socket_name, x11_str, width, height);
        state.splash_command = if let Some(first) = config.command.first() {
            if first == "python3" && config.command.len() > 2 {
                String::from("termway-demo")
            } else {
                std::path::Path::new(first)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(first)
                    .to_string()
            }
        } else {
            String::from("gui-session")
        };

        let (xwayland_source, _xwayland_client) = match xwayland {
            Some((source, client)) => (Some(source), Some(client)),
            None => (None, None),
        };

            let damage_tracker = crate::display::DamageTracker::new(renderer.width(), renderer.height(), 32);

            Ok(Self {
                display,
                listener,
                state,
                renderer,
                backend,
                client_child: None,
                client_log_file: None,
                has_opened_windows: false,
                spawn_time: None,
                pending_command,
                xwayland_source,
                _xwayland_client,
                socket_name,
                x11_display,
                session,
                cell_size_queried,
                wrap_dir: wrap_dir_guard,
                term_cols: term_size.cols.max(1),
                term_rows: term_size.rows.max(1),
                taskbar: Taskbar::new(),
                input_buffer: Vec::with_capacity(1024),
                damage_tracker,
                prev_taskbar: None,
                target_frame_duration: std::time::Duration::from_nanos(
                    1_000_000_000 / (config.fps.clamp(1, 240) as u64),
                ),
                profile_enabled: config.profile || std::env::var("TERMWAY_PROFILE").is_ok(),
                metrics: CompositorMetrics::new(),
            })
    }

    pub fn socket_name(&self) -> &str {
        &self.socket_name
    }

    pub fn x11_display(&self) -> Option<&str> {
        self.x11_display.as_deref()
    }

    pub fn run(&mut self) -> Result<()> {
        // Raw mode guard ensures terminal is restored upon exit
        let _guard = RawModeGuard::enter()?;

        let mut chunk = [0u8; 512];
        let stdin = io::stdin();

        let mut event_loop = smithay::reexports::calloop::EventLoop::<TermwayState>::try_new()?;
        let loop_handle = event_loop.handle();

        struct CalloopTokenGuard<'a, D> {
            handle: smithay::reexports::calloop::LoopHandle<'a, D>,
            token: Option<smithay::reexports::calloop::RegistrationToken>,
        }
        impl<'a, D> Drop for CalloopTokenGuard<'a, D> {
            fn drop(&mut self) {
                if let Some(token) = self.token.take() {
                    self.handle.remove(token);
                }
            }
        }

        let mut _xwayland_token_guard = CalloopTokenGuard {
            handle: loop_handle.clone(),
            token: None,
        };

        if let (Some(xwayland), Some(client)) = (self.xwayland_source.take(), self._xwayland_client.clone()) {
            let handle = loop_handle.clone();
            if let Ok(token) = loop_handle.insert_source(xwayland, move |event, _, state| {
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
                            match x11rb::rust_connection::RustConnection::connect(Some(&disp_str)) {
                                Ok((conn, _)) => state.x11_shape_conn = Some(conn),
                                Err(e) => log_line(format!("[termway] X11 shape connection failed: {}", e)),
                            }
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
            }) {
                _xwayland_token_guard.token = Some(token);
            }
        } else {
            self.state.xwayland_ready = true;
        }

        let mut loop_count = 0;
        let mut all_windows_closed_time: Option<std::time::Instant> = None;
        let mut last_render_time = std::time::Instant::now()
            .checked_sub(self.target_frame_duration)
            .unwrap_or_else(std::time::Instant::now);
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
                return Err(TermwayError::XWayland(err.clone()));
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
                    cmd.env("MOZ_ENABLE_WAYLAND", "1");
                    cmd.env_remove("GTK_IM_MODULE");
                    cmd.env_remove("QT_IM_MODULE");
                    cmd.env_remove("SDL_IM_MODULE");
                    cmd.env_remove("XMODIFIERS");
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
                    let log_temp = tempfile::Builder::new()
                        .prefix("termway_client_")
                        .suffix(".log")
                        .tempfile()
                        .ok();
                    if let Some(ref file) = log_temp {
                        if let Ok(out_file) = file.reopen() {
                            if let Ok(err_file) = out_file.try_clone() {
                                cmd.stdout(out_file);
                                cmd.stderr(err_file);
                            }
                        }
                    }
                    self.client_log_file = log_temp;
                    match cmd.spawn() {
                        Ok(child) => {
                            log_line(format!("[termway] Spawned client: {:?}", cmd_args));
                            self.client_child = Some(child);
                            self.spawn_time = Some(std::time::Instant::now());
                        }
                        Err(e) => {
                            log_line(format!("[termway] Failed to spawn client {:?}: {}", cmd_args, e));
                            return Err(TermwayError::ClientSpawn {
                                command: cmd_args[0].clone(),
                                source: e,
                            });
                        }
                    }
                }
            }

            // Clean up destroyed windows/surfaces
            self.state.toplevels.retain(|w| w.alive() && smithay::reexports::wayland_server::Resource::is_alive(w.wl_surface()));
            let prev_count = self.state.wm.windows.len();
            self.state.wm.windows.retain(|w| {
                match &w.surface {
                    WindowSurface::Wayland(t) => {
                        t.alive() && smithay::reexports::wayland_server::Resource::is_alive(t.wl_surface())
                    }
                    WindowSurface::X11(x) => x.alive(),
                    WindowSurface::Dummy => true,
                }
            });
            if self.state.wm.windows.len() != prev_count {
                if let Some(top) = self.state.wm.windows.last_mut() {
                    top.active = true;
                }
                self.state.dirty = true;
            }

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
                let is_managed = self.state.wm.windows.iter().any(|mw| matches!(&mw.surface, WindowSurface::X11(x) if x == w));
                let geo = w.geometry();
                !is_managed && geo.size.w > 1 && geo.size.h > 1
            };

            let active_window_count = self.state.wm.windows.len()
                + self.state.toplevels.len()
                + self.state.x11_windows.iter().filter(|w| is_unmanaged_real_x11(w)).count();

            let client_window_count = self.state.wm.windows.iter().filter(|w| !w.is_polkit(polkit_pid)).count()
                + self.state.toplevels.iter().filter(|w| !is_wayland_polkit(w)).count()
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
                            let err_msg = self
                                .client_log_file
                                .as_ref()
                                .and_then(|f| std::fs::read_to_string(f.path()).ok())
                                .unwrap_or_default();
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
                            return Err(TermwayError::ClientFailed(msg));
                        }
                        // If client exited with success (e.g. gvim fork or gnome-terminal launcher), wait up to 6s grace period for window to appear
                        if let Some(st) = self.spawn_time {
                            if st.elapsed() > std::time::Duration::from_secs(6) {
                                log_line("[termway] Client exited and grace period expired with no window opened.");
                                let err_msg = self
                                    .client_log_file
                                    .as_ref()
                                    .and_then(|f| std::fs::read_to_string(f.path()).ok())
                                    .unwrap_or_default();
                                let preview = err_msg.lines().take(5).collect::<Vec<_>>().join("\n  ");
                                let msg = if preview.is_empty() {
                                    "Client process exited and grace period expired with no window opened.".to_string()
                                } else {
                                    format!("Client process exited and no window was opened:\n  {}", preview)
                                };
                                return Err(TermwayError::ClientFailed(msg));
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        return Err(TermwayError::ClientFailed(format!(
                            "Error waiting for client process: {}",
                            e
                        )))
                    }
                }
            }

            // B. Accept new Wayland clients
            while let Ok(Some(stream)) = self.listener.accept() {
                let _ = self
                    .display
                    .handle()
                    .insert_client(stream, Arc::new(ClientState::default()));
            }

            // C. Unified polling: terminal stdin, Wayland listener, and Wayland client display FD
            let mut poll_fds = [
                nix::poll::PollFd::new(
                    std::os::fd::AsFd::as_fd(&stdin),
                    nix::poll::PollFlags::POLLIN,
                ),
                nix::poll::PollFd::new(
                    std::os::fd::AsFd::as_fd(&self.listener),
                    nix::poll::PollFlags::POLLIN,
                ),
                nix::poll::PollFd::new(
                    self.display.backend().poll_fd(),
                    nix::poll::PollFlags::POLLIN,
                ),
            ];

            // While splash screen is displayed, trigger dirty for animated spinner/pulse
            if self.state.show_splash {
                if self.state.start_time.elapsed() > std::time::Duration::from_secs(4) {
                    self.state.show_splash = false;
                    self.state.needs_clear = true;
                    self.state.dirty = true;
                } else {
                    self.state.dirty = true;
                }
            }

            let elapsed_since_render = last_render_time.elapsed();
            let poll_timeout = if self.state.dirty {
                if elapsed_since_render >= self.target_frame_duration
                    || self.state.needs_clear
                    || self.backend.needs_clear
                {
                    nix::poll::PollTimeout::ZERO
                } else {
                    let remaining = self.target_frame_duration - elapsed_since_render;
                    let ms = (remaining.as_millis() as u16).max(1);
                    nix::poll::PollTimeout::from(ms)
                }
            } else {
                nix::poll::PollTimeout::from(50u16)
            };

            if let Ok(res) = nix::poll::poll(&mut poll_fds, poll_timeout) {
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
                    if let Some(revents) = poll_fds[1].revents() {
                        if revents.contains(nix::poll::PollFlags::POLLIN) {
                            while let Ok(Some(stream)) = self.listener.accept() {
                                let _ = self
                                    .display
                                    .handle()
                                    .insert_client(stream, Arc::new(ClientState::default()));
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

            // F. Render if damaged and frame pacing interval has elapsed
            let should_render = self.state.dirty
                && (last_render_time.elapsed() >= self.target_frame_duration
                    || self.state.needs_clear
                    || self.backend.needs_clear);

            if should_render {
                last_render_time = std::time::Instant::now();
                if self.state.needs_clear {
                    self.state.needs_clear = false;
                    self.backend.needs_clear = true;
                    self.damage_tracker.reset();
                }
                let w = self.renderer.width();
                let h = self.renderer.height();
                let taskbar = self.taskbar.build(
                    &self.state.wm.windows,
                    self.state.enable_wm,
                    self.backend.name(),
                    self.term_cols,
                );
                let t_r0 = std::time::Instant::now();
                let render_res = self.renderer.render(&self.state);
                let r_dur = t_r0.elapsed();

                if let Ok(pixels) = render_res {
                    let r_us = r_dur.as_micros() as u64;
                    let t_d0 = std::time::Instant::now();
                    let damage = self.damage_tracker.track_damage(pixels);
                    let d_dur = t_d0.elapsed();
                    let d_us = d_dur.as_micros() as u64;
                    self.metrics.record_render(r_us, d_us);

                    let taskbar_changed = taskbar != self.prev_taskbar;
                    self.prev_taskbar = taskbar.clone();

                    if !damage.is_empty() || taskbar_changed || self.backend.needs_clear {
                        let damage_arg = if self.backend.needs_clear {
                            None
                        } else {
                            Some(damage)
                        };
                        let _ = self.backend.render_pixels_with_damage(pixels, w, h, taskbar.as_deref(), damage_arg);
                    } else {
                        self.metrics.record_clean_skip();
                    }

                    let time = self.state.start_time.elapsed().as_millis() as u32;
                    for window in &self.state.wm.windows {
                        if let Some(surface) = self.state.surface_for_managed_window(window) {
                            send_frames_surface_tree(&surface, time);
                        }
                        if let WindowSurface::Wayland(ref toplevel) = window.surface {
                            for (popup, _) in smithay::desktop::PopupManager::popups_for_surface(toplevel.wl_surface()) {
                                send_frames_surface_tree(popup.wl_surface(), time);
                            }
                        }
                    }
                    for window in &self.state.toplevels {
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
                screenshot::save_screenshot(&mut self.renderer, &self.state);
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

        if self.profile_enabled {
            self.metrics.print_summary(&self.backend, self.state.start_time);
        }

        Ok(())
    }

    fn apply_terminal_size(&mut self, mut new_size: TerminalSize) {
        let max_grid_w = new_size.cols as u32 * new_size.cell_width_px as u32;
        if max_grid_w > 0 {
            new_size.width_px = (new_size.width_px as u32).min(max_grid_w).max(320) as u16;
        }
        let avail_rows = new_size.rows.saturating_sub(1).max(1);
        let raw_h = avail_rows as u32 * new_size.cell_height_px as u32;
        if self.backend.is_sixel() {
            new_size.height_px = ((raw_h / 6 * 6).max(240)) as u16;
        } else {
            new_size.height_px = (raw_h.max(240)) as u16;
        }

        let phys_w = new_size.width_px as u32;
        let phys_h = new_size.height_px as u32;

        if self.term_cols == new_size.cols
            && self.term_rows == new_size.rows
            && self.state.physical_width == phys_w
            && self.state.physical_height == phys_h
        {
            return;
        }

        self.term_cols = new_size.cols;
        self.term_rows = new_size.rows;
        self.state.physical_width = phys_w;
        self.state.physical_height = phys_h;
        self.state.width = ((phys_w as f64 / self.state.scale).round() as u32).max(1);
        self.state.height = ((phys_h as f64 / self.state.scale).round() as u32).max(1);

        let _ = self.renderer.resize(phys_w, phys_h);
        self.damage_tracker.resize(phys_w, phys_h);
        self.backend.resize(new_size);

        let refresh_mhz = (1_000_000_000u64 / self.target_frame_duration.as_nanos().max(1) as u64 * 1000) as i32;
        let new_mode = smithay::output::Mode {
            size: (phys_w as i32, phys_h as i32).into(),
            refresh: refresh_mhz,
        };
        self.state.output.change_current_state(Some(new_mode), None, None, None);
        self.state.output.set_preferred(new_mode);

        if self.state.enable_wm {
            self.state.wm.update_layout_for_resize(self.state.width, self.state.height);
        } else {
            for window in &self.state.toplevels {
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
            for m_win in &mut self.state.wm.windows {
                m_win.loc = (0, 0).into();
                m_win.size = (self.state.width as i32, self.state.height as i32).into();
            }
        }

        self.state.dirty = true;
    }

    fn process_terminal_input(&mut self) {
        while !self.input_buffer.is_empty() {
            // 0. Try parsing bracketed paste (\x1b[200~ ... \x1b[201~)
            if let Ok(input_str) = std::str::from_utf8(&self.input_buffer) {
                if let Some(start) = input_str.find("\x1b[200~") {
                    if let Some(end_offset) = input_str[start..].find("\x1b[201~") {
                        let pasted_text = input_str[start + 6..start + end_offset].to_string();
                        let total_consumed = start + end_offset + 6;
                        self.input_buffer.drain(..total_consumed);
                        self.handle_bracketed_paste(pasted_text);
                        continue;
                    } else {
                        // Incomplete bracketed paste; wait for remainder to arrive
                        break;
                    }
                }
            }

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
                if key_ev.code == KeyCode::F12 {
                    screenshot::save_screenshot(&mut self.renderer, &self.state);
                    self.input_buffer.drain(..consumed);
                    continue;
                }
                if let KeyCode::Char(first_ch) = key_ev.code {
                    if !first_ch.is_ascii() {
                        let mut utf8_batch = String::new();
                        utf8_batch.push(first_ch);
                        self.input_buffer.drain(..consumed);
                        while let Some((next_ev, next_consumed)) = KeyboardParser::parse(&self.input_buffer) {
                            if let KeyCode::Char(next_ch) = next_ev.code {
                                if !next_ch.is_ascii() {
                                    utf8_batch.push(next_ch);
                                    self.input_buffer.drain(..next_consumed);
                                    continue;
                                }
                            }
                            break;
                        }
                        self::input::dispatch_utf8_text(&mut self.state, &utf8_batch);
                        continue;
                    }
                }
                self.dispatch_key_event(key_ev.code);
                self.input_buffer.drain(..consumed);
                continue;
            }

            if self.input_buffer.windows(6).any(|w| w == b"\x1b[200~") {
                break;
            }

            if self.input_buffer.len() > 32 {
                self.input_buffer.remove(0);
            } else {
                break;
            }
        }
    }

    fn handle_bracketed_paste(&mut self, text: String) {
        log_line(format!("[termway] Bracketed paste received ({} bytes)", text.len()));
        let bytes = text.into_bytes();
        self.state.clipboard_text = bytes.clone();
        self.state.primary_selection_text = bytes.clone();
        smithay::wayland::selection::data_device::set_data_device_selection(
            &self.state.dh,
            &self.state.seat,
            vec![
                "text/plain;charset=utf-8".to_string(),
                "text/plain".to_string(),
                "UTF8_STRING".to_string(),
                "STRING".to_string(),
            ],
            bytes.clone(),
        );
        smithay::wayland::selection::primary_selection::set_primary_selection(
            &self.state.dh,
            &self.state.seat,
            vec![
                "text/plain;charset=utf-8".to_string(),
                "text/plain".to_string(),
                "UTF8_STRING".to_string(),
                "STRING".to_string(),
            ],
            bytes,
        );
        if let Some(ref mut xwm) = self.state.xwm {
            let _ = xwm.new_selection(
                smithay::wayland::selection::SelectionTarget::Clipboard,
                Some(vec![
                    "text/plain;charset=utf-8".to_string(),
                    "text/plain".to_string(),
                    "UTF8_STRING".to_string(),
                    "TEXT".to_string(),
                    "STRING".to_string(),
                ]),
            );
            let _ = xwm.new_selection(
                smithay::wayland::selection::SelectionTarget::Primary,
                Some(vec![
                    "text/plain;charset=utf-8".to_string(),
                    "text/plain".to_string(),
                    "UTF8_STRING".to_string(),
                    "TEXT".to_string(),
                    "STRING".to_string(),
                ]),
            );
            let _ = xwm.get_randr_primary_output();
        }
        self.dispatch_key_event(KeyCode::Ctrl('v'));
    }

    fn dispatch_mouse_event(&mut self, ev: &crate::protocol::MouseEvent) {
        // Check taskbar click on the bottom row
        if ev.action == crate::protocol::MouseAction::Press
            && matches!(ev.button, crate::protocol::MouseButton::Left)
            && ev.row == self.term_rows
        {
            if let Some(chip_id) = self.taskbar.find_chip_at_col(ev.col) {
                if let Some(win) = self.state.wm.windows.iter().find(|w| w.id == chip_id) {
                    if win.is_minimized {
                        self.state.restore_window(chip_id);
                    } else if win.active {
                        self.state.minimize_window(chip_id);
                    } else {
                        self.state.raise_window(chip_id);
                    }
                }
                self.state.dirty = true;
                return;
            }
        }

        let avail_rows = self.term_rows.saturating_sub(1).max(1);
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
    pub fn ensure_xdg_runtime_dir() {
        env::ensure_xdg_runtime_dir();
    }

    /// Returns candidate X11 display numbers in `10..=64` that have neither a lock file
    /// nor an existing socket.
    pub fn free_x11_display_candidates() -> Vec<u32> {
        env::free_x11_display_candidates()
    }
}

impl Drop for TermwayCompositor {
    fn drop(&mut self) {
        if let Some(mut child) = self.client_child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.state.x11_shape_conn.take();
        self.state.xwm.take();
        self.xwayland_source.take();
        self._xwayland_client.take();

        if let Some(ref disp_str) = self.x11_display {
            if let Some(num_str) = disp_str.strip_prefix(':') {
                if let Ok(d) = num_str.parse::<u32>() {
                    if d >= 10 {
                        let _ = std::fs::remove_file(format!("/tmp/.X11-unix/X{}", d));
                        let _ = std::fs::remove_file(format!("/tmp/.X{}-lock", d));
                    }
                }
            }
        }

        if self.socket_name != "wayland-0" {
            if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
                let sock_path = std::path::Path::new(&runtime_dir).join(&self.socket_name);
                let lock_path = std::path::Path::new(&runtime_dir).join(format!("{}.lock", self.socket_name));
                let _ = std::fs::remove_file(sock_path);
                let _ = std::fs::remove_file(lock_path);
            }
        }

        // TempDir is automatically deleted on drop
        self.wrap_dir.take();
    }
}
