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
    if let Some(addr) = manager.dbus_address() {
        assert!(addr.starts_with("unix:"));
    }
    // Starting polkit should not panic
    manager.start_polkit(Some("wayland-1"), Some(":1"));
}

#[test]
fn test_check_x11_unix_writability() {
    use termway::session::check_x11_unix_writability;
    // Calling writability check in standard environment should succeed or return Err gracefully without panicking
    let _ = check_x11_unix_writability();
}

#[test]
fn test_snap_command_detection() {
    use termway::session::is_snap_command;
    assert!(is_snap_command("snap"));
    assert!(is_snap_command("/snap/bin/firefox"));
    assert!(is_snap_command("xdg-open"));
    assert!(!is_snap_command("nonexistent_binary_termway_xyz_123"));
}

#[test]
fn test_find_executable() {
    use termway::session::find_executable;
    // Standard system binary in PATH
    assert!(find_executable("ls").is_some());
    assert!(find_executable("sh").is_some());
    // Direct absolute path
    assert!(find_executable("/bin/sh").is_some());
    // Non-existent command
    assert!(find_executable("nonexistent_binary_xyz_123").is_none());
    assert!(find_executable("/bin/nonexistent_binary_xyz_123").is_none());
    // Empty command
    assert!(find_executable("").is_none());
}

#[test]
fn test_launcher_shims_route_snaps_to_ambient_bus() {
    use std::process::Command;
    use termway::session::LauncherShims;

    let root = std::env::temp_dir().join(format!("termway-shim-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let snap_bin = root.join("snap-bin");
    std::fs::create_dir_all(&snap_bin).unwrap();
    // Fake snap app that reports the bus it was launched with
    let fake = snap_bin.join("fakebrowser");
    std::fs::write(&fake, "#!/bin/sh\necho \"$DBUS_SESSION_BUS_ADDRESS $1\"\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let ambient = "unix:path=/run/user/0/it's-ambient";
    let shims = LauncherShims::create(&root.join("shims"), &snap_bin, ambient).unwrap();
    let out = Command::new(shims.dir().join("fakebrowser"))
        .arg("https://example.com")
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/tmp/isolated")
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        format!("{} https://example.com", ambient)
    );

    // Creating over an existing directory must fail rather than reuse it
    assert!(LauncherShims::create(&root.join("shims"), &snap_bin, ambient).is_err());

    let dir = shims.dir().to_path_buf();
    drop(shims);
    assert!(!dir.exists());
    let _ = std::fs::remove_dir_all(&root);
}
