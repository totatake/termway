use termway::session::{DbusSession, PolkitAgent, SessionManager};

#[test]
fn test_dbus_session_lifecycle() {
    // Attempt to start isolated D-Bus session if dbus-daemon is present in system
    if let Ok(session) = DbusSession::start() {
        let addr = session.address();
        assert!(!addr.is_empty());
        assert!(addr.starts_with("unix:"));
        assert!(std::env::var("DBUS_SESSION_BUS_ADDRESS").is_ok());
    }
}

#[test]
fn test_polkit_agent_lookup_and_lifecycle() {
    // Test creating polkit agent (gracefully handles environments without agent installed)
    let agent = PolkitAgent::start(Some("wayland-99"), Some(":99"), Some("unix:path=/tmp/test"));
    // If an agent binary exists on this machine, it should report running; otherwise None without panicking
    if agent.is_running() {
        assert!(agent.agent_name().is_some());
    }
}

#[test]
fn test_session_manager_integration() {
    let mut manager = SessionManager::new();
    let addr = manager.dbus_address();
    if addr.is_some() {
        assert!(addr.unwrap().starts_with("unix:"));
    }
    // Starting polkit should not panic
    manager.start_polkit(Some("wayland-1"), Some(":1"));
}

#[test]
fn test_snap_command_detection() {
    use termway::session::is_snap_command;
    assert!(is_snap_command("snap"));
    assert!(is_snap_command("/snap/bin/firefox"));
    assert!(!is_snap_command("nonexistent_binary_termway_xyz_123"));
}
