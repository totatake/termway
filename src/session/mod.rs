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
    pub fn start() -> Result<Self, String> {
        let mut child = Command::new("dbus-daemon")
            .arg("--session")
            .arg("--print-address")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Failed to spawn dbus-daemon: {}", e))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Failed to capture dbus-daemon stdout".to_string())?;

        let mut reader = BufReader::new(stdout);
        let mut address = String::new();
        reader
            .read_line(&mut address)
            .map_err(|e| format!("Failed to read D-Bus address: {}", e))?;

        let address = address.trim().to_string();
        if address.is_empty() {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Empty D-Bus address returned from dbus-daemon".into());
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

            if let Ok(file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/termway_polkit.log")
            {
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

/// Check if a command is packaged as an Ubuntu/Linux Snap application.
/// Snap applications are strictly confined by `snap-confine` and require access to the
/// host's `systemd --user` session bus (`/run/user/<uid>/bus`) to register transient cgroup scopes.
pub fn is_snap_command(cmd_name: &str) -> bool {
    if cmd_name == "snap" || cmd_name.starts_with("/snap/") {
        return true;
    }
    if std::path::Path::new(&format!("/snap/bin/{}", cmd_name)).exists() {
        return true;
    }
    let path_env = std::env::var("PATH").unwrap_or_default();
    for dir in path_env.split(':') {
        let full_path = std::path::Path::new(dir).join(cmd_name);
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
                if content.contains("snap run") || content.contains("/usr/bin/snap") {
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

        Self {
            isolated_dbus,
            ambient_dbus_address,
            polkit: None,
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
            eprintln!("{}", msg);
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
pub fn check_x11_unix_writability() -> Result<(), String> {
    let x11_dir = std::path::Path::new("/tmp/.X11-unix");
    if x11_dir.exists() {
        let test_file = format!("/tmp/.X11-unix/.termway_test_{}", std::process::id());
        if let Err(e) = std::fs::OpenOptions::new().write(true).create_new(true).open(&test_file) {
            return Err(format!("Cannot write to /tmp/.X11-unix ({}). X11/XWayland will not work: {}", e.kind(), e));
        } else {
            let _ = std::fs::remove_file(&test_file);
        }
    } else {
        let test_file = format!("/tmp/.termway_test_{}", std::process::id());
        if let Err(e) = std::fs::OpenOptions::new().write(true).create_new(true).open(&test_file) {
            return Err(format!("Cannot write to /tmp ({}). X11/XWayland will not work: {}", e.kind(), e));
        } else {
            let _ = std::fs::remove_file(&test_file);
        }
    }
    Ok(())
}

