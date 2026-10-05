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
            cmd.stdout(Stdio::null());
            cmd.stderr(Stdio::null());

            match cmd.spawn() {
                Ok(child) => {
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
}

impl Drop for PolkitAgent {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Integrated session manager handling both D-Bus isolation and Polkit authentication agent.
pub struct SessionManager {
    dbus: Option<DbusSession>,
    polkit: Option<PolkitAgent>,
}

impl SessionManager {
    pub fn new() -> Self {
        let dbus = match DbusSession::start() {
            Ok(s) => Some(s),
            Err(e) => {
                crate::compositor::log_line(format!(
                    "[termway] Notice: Isolated D-Bus session not started ({}). Using system/ambient session.",
                    e
                ));
                None
            }
        };

        Self {
            dbus,
            polkit: None,
        }
    }

    pub fn dbus_address(&self) -> Option<&str> {
        self.dbus.as_ref().map(|d| d.address())
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
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}
