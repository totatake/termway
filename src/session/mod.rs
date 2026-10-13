pub mod secret_bridge;

use crate::error::{Result, TermwayError};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};

/// Manages an isolated D-Bus session bus.
///
/// Spawning an independent D-Bus session ensures that single-instance applications
/// (e.g. `gnome-terminal`) do not connect to an existing host/remote desktop session,
/// preventing windows from appearing on the wrong display.
pub struct DbusSession {
    child: Child,
    address: String,
}

impl DbusSession {
    pub fn start() -> Result<Self> {
        let mut cmd = Command::new("dbus-daemon");
        cmd.arg("--session")
            .arg("--print-address")
            .arg("--nofork")
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        #[cfg(target_os = "linux")]
        unsafe {
            use std::os::unix::process::CommandExt;
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                Ok(())
            });
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| TermwayError::DbusSession(format!("Failed to spawn dbus-daemon: {}", e)))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| TermwayError::DbusSession("Failed to capture dbus-daemon stdout".to_string()))?;

        let mut reader = BufReader::new(stdout);
        let mut address = String::new();
        reader
            .read_line(&mut address)
            .map_err(|e| TermwayError::DbusSession(format!("Failed to read D-Bus address: {}", e)))?;

        let address = address.trim().to_string();
        if address.is_empty() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(TermwayError::DbusSession(
                "Empty D-Bus address returned from dbus-daemon".into(),
            ));
        }

        // Set environment variable for current process and future children
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &address);

        Ok(Self { child, address })
    }

    pub fn address(&self) -> &str {
        &self.address
    }
}

impl Drop for DbusSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Known paths for Polkit authentication agents in various Linux distributions.
/// Prioritizes official GNOME agent (policykit-1-gnome) when present, falling back to lxpolkit and others.
const KNOWN_POLKIT_AGENTS: &[&str] = &[
    "/usr/lib/policykit-1-gnome/polkit-gnome-authentication-agent-1",
    "/usr/lib/polkit-gnome/polkit-gnome-authentication-agent-1",
    "/usr/libexec/polkit-gnome-authentication-agent-1",
    "lxpolkit",
    "/usr/lib/mate-polkit/polkit-mate-authentication-agent-1",
    "/usr/libexec/polkit-mate-authentication-agent-1",
    "/usr/lib/x86_64-linux-gnu/polkit-mate/polkit-mate-authentication-agent-1",
    "/usr/bin/lxqt-policykit-agent",
    "/usr/lib/x86_64-linux-gnu/libexec/polkit-kde-authentication-agent-1",
    "/usr/lib/polkit-kde-authentication-agent-1",
];

/// Manages a background Polkit authentication agent.
///
/// Running an authentication agent allows applications requiring elevated privileges
/// (such as `gparted` or `pkexec`) to prompt for password dialogs inside Termway's display.
pub struct PolkitAgent {
    child: Option<Child>,
    agent_name: Option<String>,
}

impl PolkitAgent {
    pub fn start(
        wayland_display: Option<&str>,
        x11_display: Option<&str>,
        dbus_address: Option<&str>,
    ) -> Self {
        for &candidate in KNOWN_POLKIT_AGENTS {
            // Check if binary exists or is resolvable
            let is_available = if candidate.starts_with('/') {
                std::path::Path::new(candidate).is_file()
            } else {
                // Check PATH for command name
                if let Ok(path_var) = std::env::var("PATH") {
                    path_var.split(':').any(|dir| {
                        std::path::Path::new(dir).join(candidate).is_file()
                    })
                } else {
                    false
                }
            };

            if !is_available {
                continue;
            }

            let mut cmd = Command::new(candidate);
            if let Some(wdisp) = wayland_display {
                cmd.env("WAYLAND_DISPLAY", wdisp);
            }
            if let Some(xdisp) = x11_display {
                cmd.env("DISPLAY", xdisp);
            }
            if let Some(dbus) = dbus_address {
                cmd.env("DBUS_SESSION_BUS_ADDRESS", dbus);
            }
            if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
                cmd.env("XDG_RUNTIME_DIR", runtime_dir);
            }
            cmd.env("GDK_BACKEND", "x11,wayland");

            let polkit_log_file = if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
                let path = std::path::Path::new(&runtime_dir).join("termway_polkit.log");
                std::fs::OpenOptions::new().create(true).append(true).open(path).ok()
            } else {
                tempfile::Builder::new()
                    .prefix("termway_polkit_")
                    .suffix(".log")
                    .tempfile()
                    .ok()
                    .and_then(|t| t.reopen().ok())
            };

            if let Some(file) = polkit_log_file {
                if let Ok(err_file) = file.try_clone() {
                    cmd.stdout(Stdio::from(file));
                    cmd.stderr(Stdio::from(err_file));
                } else {
                    cmd.stdout(Stdio::null());
                    cmd.stderr(Stdio::null());
                }
            } else {
                cmd.stdout(Stdio::null());
                cmd.stderr(Stdio::null());
            }

            #[cfg(target_os = "linux")]
            unsafe {
                use std::os::unix::process::CommandExt;
                cmd.pre_exec(|| {
                    libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                    Ok(())
                });
            }

            match cmd.spawn() {
                Ok(mut child) => {
                    // Give the agent a moment to initialize and verify it didn't immediately crash
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    if let Ok(Some(status)) = child.try_wait() {
                        crate::compositor::log_line(format!(
                            "[termway] Warning: Polkit agent {} exited immediately with {}",
                            candidate, status
                        ));
                        continue;
                    }

                    return Self {
                        child: Some(child),
                        agent_name: Some(candidate.to_string()),
                    };
                }
                Err(_) => continue,
            }
        }

        Self {
            child: None,
            agent_name: None,
        }
    }

    pub fn agent_name(&self) -> Option<&str> {
        self.agent_name.as_deref()
    }

    pub fn is_running(&self) -> bool {
        self.child.is_some()
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(|c| c.id())
    }
}

impl Drop for PolkitAgent {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

const SNAP_BIN_DIR: &str = "/snap/bin";
const SNAP_DESKTOP_DATA_DIR: &str = "/var/lib/snapd/desktop";
const SHIM_DIR_PREFIX: &str = "termway-shims-";

/// Remove shim directories left behind by termway instances that were killed before cleanup.
fn remove_stale_shim_dirs(base: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|n| n.strip_prefix(SHIM_DIR_PREFIX))
            .and_then(|p| p.parse::<libc::pid_t>().ok())
        else {
            continue;
        };
        let alive = unsafe { libc::kill(pid, 0) } == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
        if !alive {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Adopt the host desktop's XDG_CURRENT_DESKTOP when termway runs outside the desktop
/// session (e.g. over SSH). Electron and Chromium only pick the libsecret keyring backend
/// for a recognized desktop and otherwise fall back to unencrypted "basic" storage.
fn import_host_current_desktop() {
    if std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|v| !v.is_empty()) {
        return;
    }
    let Ok(output) = Command::new("systemctl")
        .args(["--user", "show-environment"])
        .stderr(Stdio::null())
        .output()
    else {
        return;
    };
    let env = String::from_utf8_lossy(&output.stdout);
    if let Some(desktop) = env
        .lines()
        .find_map(|l| l.strip_prefix("XDG_CURRENT_DESKTOP="))
        .filter(|v| !v.is_empty())
    {
        std::env::set_var("XDG_CURRENT_DESKTOP", desktop);
    }
}

/// Make snap `.desktop` files discoverable when termway is started outside a login shell
/// (e.g. `ssh host termway -- code`), where /etc/profile.d has not extended XDG_DATA_DIRS.
/// Without it, GIO finds no handler for https and falls back to opening the page as text.
fn ensure_snap_desktop_data_dir() {
    if !std::path::Path::new(SNAP_DESKTOP_DATA_DIR).is_dir() {
        return;
    }
    let current = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|s| !s.is_empty())
        // Default from the XDG Base Directory specification
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_string());
    if !current.split(':').any(|d| d.trim_end_matches('/') == SNAP_DESKTOP_DATA_DIR) {
        std::env::set_var("XDG_DATA_DIRS", format!("{}:{}", current, SNAP_DESKTOP_DATA_DIR));
    }
}

/// URL openers that commonly hand off to a snap browser through an absolute path
/// (e.g. a `.desktop` Exec line of `/snap/bin/firefox`), which PATH shims for the
/// snap itself cannot intercept.
const URL_LAUNCHERS: &[&str] = &[
    "xdg-open",
    "sensible-browser",
    "x-www-browser",
    "gnome-www-browser",
    "www-browser",
];

/// A directory of wrapper scripts prepended to the clients' PATH.
///
/// Each wrapper re-points `DBUS_SESSION_BUS_ADDRESS` at the ambient (host) session bus
/// before exec'ing the real program: unconditionally for snap applications, and only for
/// non-file URLs for URL launchers, so files keep opening on the isolated bus.
pub struct LauncherShims {
    dir: std::path::PathBuf,
}

impl LauncherShims {
    pub fn create(
        dir: &std::path::Path,
        snap_bin: &std::path::Path,
        ambient_dbus: &str,
    ) -> std::io::Result<Self> {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

        // Resolve the real launchers before `dir` can shadow them on PATH
        let launchers: Vec<(&str, std::path::PathBuf)> = URL_LAUNCHERS
            .iter()
            .filter_map(|&name| find_executable(name).map(|p| (name, p)))
            .collect();

        // Fails if the path already exists, so a pre-planted directory is never reused
        std::fs::DirBuilder::new().mode(0o700).create(dir)?;
        let shims = Self { dir: dir.to_path_buf() };

        let bus = shell_quote(ambient_dbus);
        let write_shim = |name: &str, body: String| -> std::io::Result<()> {
            let path = shims.dir.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n# Generated by termway\n{}", body))?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        };

        for entry in std::fs::read_dir(snap_bin)?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            write_shim(
                name,
                format!(
                    "DBUS_SESSION_BUS_ADDRESS={}\nexport DBUS_SESSION_BUS_ADDRESS\nexec {} \"$@\"\n",
                    bus,
                    shell_quote(&path.to_string_lossy())
                ),
            )?;
        }

        for (name, real) in launchers {
            write_shim(
                name,
                format!(
                    "case \"$1\" in\n    file:*) ;;\n    *://*|mailto:*) DBUS_SESSION_BUS_ADDRESS={}; export DBUS_SESSION_BUS_ADDRESS ;;\nesac\nexec {} \"$@\"\n",
                    bus,
                    shell_quote(&real.to_string_lossy())
                ),
            )?;
        }

        Ok(shims)
    }

    pub fn dir(&self) -> &std::path::Path {
        &self.dir
    }
}

impl Drop for LauncherShims {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Check if a command is packaged as an Ubuntu/Linux Snap application.
/// Snap applications are strictly confined by `snap-confine` and require access to the
/// host's `systemd --user` session bus (`/run/user/<uid>/bus`) to register transient cgroup scopes.
pub fn is_snap_command(cmd_name: &str) -> bool {
    // Treat common URL openers and launchers as snap-dependent because they
    // frequently launch snap applications (like Firefox/Chromium) as default handlers.
    let launchers = ["xdg-open", "sensible-browser", "gio", "gnome-open"];
    if launchers.contains(&cmd_name) {
        return true;
    }

    if cmd_name == "snap" || cmd_name.starts_with("/snap/") {
        return true;
    }

    // Extract the base filename in case an absolute path like /usr/bin/firefox was passed
    if let Some(file_name) = std::path::Path::new(cmd_name).file_name().and_then(|n| n.to_str()) {
        if std::path::Path::new(&format!("/snap/bin/{}", file_name)).exists() {
            return true;
        }
    }

    let path_env = std::env::var("PATH").unwrap_or_default();
    let mut check_paths = vec![std::path::PathBuf::from(cmd_name)];
    for dir in path_env.split(':') {
        check_paths.push(std::path::Path::new(dir).join(cmd_name));
    }

    for full_path in check_paths {
        if full_path.exists() {
            let path_str = full_path.to_string_lossy();
            if path_str.contains("/snap/") {
                return true;
            }
            if let Ok(target) = std::fs::read_link(&full_path) {
                if target.to_string_lossy().contains("snap") {
                    return true;
                }
            }
            if let Ok(content) = std::fs::read_to_string(&full_path) {
                if content.contains("snap run") || content.contains("/usr/bin/snap") || content.contains("/snap/bin/") {
                    return true;
                }
            }
        }
    }
    false
}

/// Locate an executable binary in the system PATH or direct path.
/// Returns Some(PathBuf) if the file exists and has executable permissions, or None.
pub fn find_executable(cmd_name: &str) -> Option<std::path::PathBuf> {
    if cmd_name.is_empty() {
        return None;
    }
    let p = std::path::Path::new(cmd_name);
    if cmd_name.contains('/') {
        if p.is_file() {
            if let Ok(c_path) = std::ffi::CString::new(cmd_name) {
                if unsafe { libc::access(c_path.as_ptr(), libc::X_OK) } == 0 {
                    return Some(p.to_path_buf());
                }
            }
        }
        return None;
    }

    let path_env = std::env::var("PATH").unwrap_or_default();
    for dir in path_env.split(':') {
        let full = std::path::Path::new(dir).join(cmd_name);
        if full.is_file() {
            if let Ok(c_path) = std::ffi::CString::new(full.to_string_lossy().as_bytes()) {
                if unsafe { libc::access(c_path.as_ptr(), libc::X_OK) } == 0 {
                    return Some(full);
                }
            }
        }
    }
    None
}

/// Integrated session manager handling D-Bus sessions and Polkit authentication agent.
///
/// Termway maintains an isolated D-Bus session bus to guarantee that single-instance applications
/// like `gnome-terminal` spawn dedicated server processes on Termway's display, while retaining
/// access to the host's ambient `systemd --user` session bus for Snap applications like `firefox`.
pub struct SessionManager {
    isolated_dbus: Option<DbusSession>,
    ambient_dbus_address: Option<String>,
    polkit: Option<PolkitAgent>,
    launcher_shims: Option<LauncherShims>,
    secret_bridge: Option<secret_bridge::ServiceBridge>,
}

impl SessionManager {
    pub fn new() -> Self {
        // 1. Capture host's ambient systemd user session bus if available
        let uid = unsafe { libc::getuid() };
        let run_user_bus = format!("/run/user/{}/bus", uid);
        let ambient_dbus_address = std::env::var("DBUS_SESSION_BUS_ADDRESS")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| {
                if std::path::Path::new(&run_user_bus).exists() {
                    Some(format!("unix:path={}", run_user_bus))
                } else {
                    None
                }
            });

        // 2. Start an isolated D-Bus session for Termway
        // This ensures single-instance applications (e.g. gnome-terminal) do not attach to an
        // already-running background server on the host, which would fail to open windows on Termway.
        let isolated_dbus = match DbusSession::start() {
            Ok(s) => Some(s),
            Err(e) => {
                crate::compositor::log_line(format!(
                    "[termway] Notice: Isolated D-Bus session not started ({}). Using ambient session.",
                    e
                ));
                None
            }
        };

        // 3. Route snap launches from descendants back to the ambient bus.
        // Clients receive the isolated bus, so a snap started indirectly (e.g. VS Code ->
        // xdg-open -> snap Firefox for an OAuth login) cannot create its systemd cgroup scope
        // and snap-confine aborts. PATH shims restore the ambient bus for those launches only.
        let launcher_shims = match (&isolated_dbus, &ambient_dbus_address) {
            (Some(_), Some(ambient)) => Self::install_launcher_shims(ambient),
            _ => None,
        };
        ensure_snap_desktop_data_dir();

        // 4. Relay the host keyring (Secret Service) into the isolated bus so that apps
        // storing credentials through libsecret (VS Code, Chromium) can use the OS keyring.
        let secret_bridge = match (&isolated_dbus, &ambient_dbus_address) {
            (Some(isolated), Some(ambient)) => {
                match secret_bridge::ServiceBridge::start_secret_service(isolated.address(), ambient) {
                    Ok(bridge) => {
                        import_host_current_desktop();
                        Some(bridge)
                    }
                    Err(e) => {
                        crate::compositor::log_line(format!(
                            "[termway] Notice: Secret Service not bridged ({}).",
                            e
                        ));
                        None
                    }
                }
            }
            _ => None,
        };

        Self {
            isolated_dbus,
            ambient_dbus_address,
            polkit: None,
            launcher_shims,
            secret_bridge,
        }
    }

    /// Primary D-Bus address used by Termway and isolated clients (e.g. gnome-terminal, polkit).
    pub fn dbus_address(&self) -> Option<&str> {
        self.isolated_dbus
            .as_ref()
            .map(|d| d.address())
            .or(self.ambient_dbus_address.as_deref())
    }

    /// Ambient systemd user D-Bus address (used for Snap apps that require systemd cgroup interaction).
    fn install_launcher_shims(ambient: &str) -> Option<LauncherShims> {
        let snap_bin = std::path::Path::new(SNAP_BIN_DIR);
        if !snap_bin.is_dir() {
            return None;
        }
        let base = std::env::var("XDG_RUNTIME_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir());
        remove_stale_shim_dirs(&base);
        let dir = base.join(format!("{}{}", SHIM_DIR_PREFIX, std::process::id()));
        match LauncherShims::create(&dir, snap_bin, ambient) {
            Ok(shims) => {
                // Set for the current process so every spawned client and its descendants inherit it
                let path = std::env::var("PATH").unwrap_or_default();
                std::env::set_var("PATH", format!("{}:{}", shims.dir().display(), path));
                Some(shims)
            }
            Err(e) => {
                crate::compositor::log_line(format!(
                    "[termway] Notice: Snap launcher shims not installed ({}).",
                    e
                ));
                None
            }
        }
    }

    pub fn is_secret_service_bridged(&self) -> bool {
        self.secret_bridge.is_some()
    }

    pub fn launcher_shim_dir(&self) -> Option<&std::path::Path> {
        self.launcher_shims.as_ref().map(|s| s.dir())
    }

    pub fn ambient_dbus_address(&self) -> Option<&str> {
        self.ambient_dbus_address.as_deref()
    }

    /// Determine the appropriate D-Bus address for a client command.
    /// Snap applications require the ambient systemd user bus; all other apps use the isolated bus.
    pub fn dbus_address_for_client(&self, cmd_name: &str) -> Option<&str> {
        if is_snap_command(cmd_name) {
            if let Some(ambient) = self.ambient_dbus_address() {
                return Some(ambient);
            }
        }
        self.dbus_address()
    }

    pub fn start_polkit(&mut self, wayland_display: Option<&str>, x11_display: Option<&str>) {
        if self.polkit.is_some() {
            return;
        }
        let dbus_addr = self.dbus_address();
        let agent = PolkitAgent::start(wayland_display, x11_display, dbus_addr);
        if let Some(name) = agent.agent_name() {
            crate::compositor::log_line(format!(
                "[termway] Started Polkit Authentication Agent: {}",
                name
            ));
        } else {
            let msg = "[termway] Notice: No Polkit authentication agent found (policykit-1-gnome or lxpolkit).\n\
                       [termway]         To run privileged apps (e.g. gparted) without sudo, install one via:\n\
                       [termway]         sudo apt install -y policykit-1-gnome (or lxpolkit)\n\
                       [termway]         Alternatively, run directly with: sudo ./termway --wm <command>";
            crate::compositor::log_line(msg);
        }
        self.polkit = Some(agent);
    }

    pub fn is_polkit_running(&self) -> bool {
        self.polkit.as_ref().map(|p| p.is_running()).unwrap_or(false)
    }

    pub fn polkit_pid(&self) -> Option<u32> {
        self.polkit.as_ref().and_then(|p| p.pid())
    }

    pub fn update_activation_environment(
        &self,
        wayland_display: Option<&str>,
        x11_display: Option<&str>,
    ) {
        // Only update the isolated D-Bus session if present.
        // We MUST NOT overwrite the host user session (systemd user bus) with termway's
        // temporary DISPLAY / WAYLAND_DISPLAY, which would break gnome-terminal and other
        // host desktop applications after termway exits.
        if let Some(dbus) = self.isolated_dbus.as_ref().map(|d| d.address()) {
            Self::run_update_activation_env(dbus, false, wayland_display, x11_display);
        }
    }

    fn run_update_activation_env(
        dbus_addr: &str,
        use_systemd: bool,
        wayland_display: Option<&str>,
        x11_display: Option<&str>,
    ) {
        let mut cmd = Command::new("dbus-update-activation-environment");
        if use_systemd {
            cmd.arg("--systemd");
        }
        cmd.env("DBUS_SESSION_BUS_ADDRESS", dbus_addr);
        if let Some(wd) = wayland_display {
            cmd.env("WAYLAND_DISPLAY", wd);
            cmd.arg(format!("WAYLAND_DISPLAY={}", wd));
        }
        if let Some(xd) = x11_display {
            cmd.env("DISPLAY", xd);
            cmd.arg(format!("DISPLAY={}", xd));
        }
        if let Ok(rt) = std::env::var("XDG_RUNTIME_DIR") {
            cmd.env("XDG_RUNTIME_DIR", &rt);
            cmd.arg(format!("XDG_RUNTIME_DIR={}", rt));
        }
        cmd.arg(format!("DBUS_SESSION_BUS_ADDRESS={}", dbus_addr));
        cmd.arg("GDK_BACKEND=wayland,x11");
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());
        let _ = cmd.status();
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Verifies that /tmp/.X11-unix (or /tmp if the directory does not exist) is writable.
/// This prevents X11/XWayland servers and clients from failing or escaping to host displays.
pub fn check_x11_unix_writability() -> Result<()> {
    let x11_dir = std::path::Path::new("/tmp/.X11-unix");
    if x11_dir.exists() {
        if let Err(e) = tempfile::Builder::new().prefix(".termway_test_").tempfile_in(x11_dir) {
            return Err(TermwayError::X11SocketDirNotWritable(format!(
                "Cannot write to /tmp/.X11-unix ({}). X11/XWayland will not work.\n\
                 On WSL2, remount it as writable via: sudo mount -o remount,rw /tmp/.X11-unix",
                e
            )));
        }
    } else {
        let tmp_dir = std::path::Path::new("/tmp");
        if let Err(e) = tempfile::Builder::new().prefix(".termway_test_").tempfile_in(tmp_dir) {
            return Err(TermwayError::X11SocketDirNotWritable(format!(
                "Cannot write to /tmp ({}). X11/XWayland will not work: {}",
                e.kind(),
                e
            )));
        }
    }
    Ok(())
}

