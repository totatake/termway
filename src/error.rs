use std::fmt;

/// Unified, type-safe error enumeration for `termway`.
#[derive(Debug)]
pub enum TermwayError {
    /// Standard I/O or OS-level error.
    Io(std::io::Error),
    /// Could not bind any free `wayland-1`..`wayland-32` socket in `XDG_RUNTIME_DIR`.
    NoFreeWaylandSocket,
    /// `/tmp/.X11-unix` or `/tmp` is not writable, preventing safe XWayland startup.
    X11SocketDirNotWritable(String),
    /// XWayland server failed to initialize or encountered a fatal startup error.
    XWayland(String),
    /// Isolated D-Bus session daemon failed to start or report its bus address.
    DbusSession(String),
    /// Offscreen Pixman renderer initialization or frame compositing error.
    Renderer(String),
    /// Failed to spawn the requested client GUI command.
    ClientSpawn {
        command: String,
        source: std::io::Error,
    },
    /// Client GUI process exited prematurely or failed before opening any window.
    ClientFailed(String),
    /// Invalid numeric argument passed on the CLI.
    ConfigParse(std::num::ParseIntError),
}

/// Type alias for `Result<T, TermwayError>`.
pub type Result<T> = std::result::Result<T, TermwayError>;

impl fmt::Display for TermwayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{}", err),
            Self::NoFreeWaylandSocket => write!(f, "Failed to find free wayland socket name"),
            Self::X11SocketDirNotWritable(msg) => write!(f, "{}", msg),
            Self::XWayland(msg) => write!(f, "XWayland failed to start: {}", msg),
            Self::DbusSession(msg) => write!(f, "{}", msg),
            Self::Renderer(msg) => write!(f, "Renderer error: {}", msg),
            Self::ClientSpawn { command, source } => {
                write!(f, "Failed to execute client command '{}': {}", command, source)
            }
            Self::ClientFailed(msg) => write!(f, "{}", msg),
            Self::ConfigParse(err) => write!(f, "Invalid numeric argument: {}", err),
        }
    }
}

impl std::error::Error for TermwayError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::ClientSpawn { source, .. } => Some(source),
            Self::ConfigParse(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for TermwayError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<std::num::ParseIntError> for TermwayError {
    fn from(err: std::num::ParseIntError) -> Self {
        Self::ConfigParse(err)
    }
}

impl From<smithay::backend::renderer::pixman::PixmanError> for TermwayError {
    fn from(err: smithay::backend::renderer::pixman::PixmanError) -> Self {
        Self::Renderer(err.to_string())
    }
}

impl From<smithay::reexports::calloop::Error> for TermwayError {
    fn from(err: smithay::reexports::calloop::Error) -> Self {
        Self::Io(std::io::Error::other(err))
    }
}

impl From<smithay::reexports::wayland_server::backend::InitError> for TermwayError {
    fn from(err: smithay::reexports::wayland_server::backend::InitError) -> Self {
        Self::Io(std::io::Error::other(err))
    }
}

