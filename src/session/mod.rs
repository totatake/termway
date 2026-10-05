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

/// Integrated session manager handling both D-Bus session and Polkit authentication agent.
pub struct SessionManager {
    dbus: Option<DbusSession>,
    ambient_dbus_address: Option<String>,
    polkit: Option<PolkitAgent>,
}

impl SessionManager {
    pub fn new() -> Self {
        // Check if an existing system or user session bus is already available.
        // On modern Linux (systemd-based distributions like Ubuntu), the user session bus
        // at `/run/user/<uid>/bus` is integrated with systemd --user and snapd.
        // Overwriting this with an isolated dbus-daemon breaks Snap confinement (e.g. Snap Firefox)
        // because snap-confine cannot register transient cgroups with systemd.
        let uid = unsafe { libc::getuid() };
        let run_user_bus = format!("/run/user/{}/bus", uid);
        let existing_bus = std::env::var("DBUS_SESSION_BUS_ADDRESS")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| {
                if std::path::Path::new(&run_user_bus).exists() {
                    Some(format!("unix:path={}", run_user_bus))
                } else {
                    None
                }
            });

        let (dbus, ambient_dbus_address) = if let Some(addr) = existing_bus {
            std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &addr);
            (None, Some(addr))
        } else {
            let s = match DbusSession::start() {
                Ok(s) => Some(s),
                Err(e) => {
                    crate::compositor::log_line(format!(
                        "[termway] Notice: Isolated D-Bus session not started ({}). Using system/ambient session.",
                        e
                    ));
                    None
                }
            };
            (s, None)
        };

        Self {
            dbus,
            ambient_dbus_address,
            polkit: None,
        }
    }

    pub fn dbus_address(&self) -> Option<&str> {
        self.dbus
            .as_ref()
            .map(|d| d.address())
            .or(self.ambient_dbus_address.as_deref())
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
        if let Some(dbus) = self.dbus_address() {
            let mut cmd = Command::new("dbus-update-activation-environment");
            if std::path::Path::new("/run/systemd/system").exists() {
                cmd.arg("--systemd");
            }
            cmd.env("DBUS_SESSION_BUS_ADDRESS", dbus);
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
            cmd.arg(format!("DBUS_SESSION_BUS_ADDRESS={}", dbus));
            cmd.arg("GDK_BACKEND=wayland,x11");
            cmd.stdout(Stdio::null());
            cmd.stderr(Stdio::null());
            let _ = cmd.status();
        }
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}
