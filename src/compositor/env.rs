use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

pub static CAPTURE_REQUESTED: AtomicBool = AtomicBool::new(false);
pub static STATE_DUMP_REQUESTED: AtomicBool = AtomicBool::new(false);

pub fn setup_test_signals() {
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

/// Helper to find Xwayland binary safely without invoking `which`
pub fn find_xwayland_binary() -> Option<std::path::PathBuf> {
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in path_var.split(':') {
            let candidate = std::path::Path::new(dir).join("Xwayland");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    for default_path in &["/usr/bin/Xwayland", "/usr/local/bin/Xwayland", "/usr/lib/xorg/Xwayland"] {
        let p = std::path::Path::new(default_path);
        if p.is_file() {
            return Some(p.to_path_buf());
        }
    }
    None
}

/// In raw mode, `\n` alone does not return to line beginning, so log with CRLF.
pub fn log_line(msg: impl AsRef<str>) {
    let log_path = if let Ok(path) = std::env::var("TERMWAY_DEBUG_LOG") {
        Some(std::path::PathBuf::from(path))
    } else if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        Some(std::path::Path::new(&runtime_dir).join("termway_debug.log"))
    } else {
        None
    };

    if let Some(path) = log_path {
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(file, "{}", msg.as_ref());
        }
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
        
        // Check if existing path is safe (not a symlink, owned by current user)
        let is_safe = if path.exists() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if let Ok(meta) = std::fs::symlink_metadata(path) {
                    meta.is_dir() && meta.uid() == uid && !meta.file_type().is_symlink()
                } else {
                    false
                }
            }
            #[cfg(not(unix))]
            {
                path.is_dir()
            }
        } else {
            std::fs::create_dir_all(path).is_ok()
        };

        if is_safe {
            // Secure permissions to 0700 (owner read/write/execute only, required by Wayland specs)
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
            std::env::set_var("XDG_RUNTIME_DIR", &fallback_dir);
            log_line(format!("[termway] Using fallback XDG_RUNTIME_DIR={}", fallback_dir));
        } else if let Ok(tmp_dir) = tempfile::Builder::new().prefix(&format!("termway-runtime-{}-", uid)).tempdir() {
            let safe_path = tmp_dir.keep();
            let _ = std::fs::set_permissions(&safe_path, std::fs::Permissions::from_mode(0o700));
            let path_str = safe_path.to_string_lossy().to_string();
            std::env::set_var("XDG_RUNTIME_DIR", &path_str);
            log_line(format!("[termway] Using secure fallback XDG_RUNTIME_DIR={}", path_str));
        }
    }

    // On WSL2 with systemd enabled, pam_systemd mounts a new tmpfs on /run/user/<uid>,
    // which hides WSLg's /mnt/wslg/runtime-dir/wayland-0 socket. Ensure the symlink
    // exists so native WSLg Wayland apps continue working outside termway.
    #[cfg(unix)]
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        let wslg_wayland = std::path::Path::new("/mnt/wslg/runtime-dir/wayland-0");
        if wslg_wayland.exists() {
            let target_wayland = std::path::Path::new(&dir).join("wayland-0");
            if std::fs::symlink_metadata(&target_wayland).is_err() {
                let _ = std::os::unix::fs::symlink(wslg_wayland, &target_wayland);
            }
            let wslg_lock = std::path::Path::new("/mnt/wslg/runtime-dir/wayland-0.lock");
            let target_lock = std::path::Path::new(&dir).join("wayland-0.lock");
            if wslg_lock.exists() && std::fs::symlink_metadata(&target_lock).is_err() {
                let _ = std::os::unix::fs::symlink(wslg_lock, &target_lock);
            }
        }
    }
}

/// Returns candidate X11 display numbers in `10..=64` that have neither a lock file
/// (`/tmp/.X{d}-lock`) nor an existing socket (`/tmp/.X11-unix/X{d}` or `/mnt/wslg/.X11-unix/X{d}`).
///
/// Never returns `0..=9` so host/WSLg X11 displays (such as `:0`) are never touched or unlinked,
/// even when the host X server's lock file lives in a different container namespace (e.g. WSLg).
pub fn free_x11_display_candidates() -> Vec<u32> {
    (10..=64)
        .filter(|&d| {
            let lock_path = format!("/tmp/.X{}-lock", d);
            let sock_path = format!("/tmp/.X11-unix/X{}", d);
            let wslg_sock_path = format!("/mnt/wslg/.X11-unix/X{}", d);
            std::fs::symlink_metadata(&lock_path).is_err()
                && std::fs::symlink_metadata(&sock_path).is_err()
                && std::fs::symlink_metadata(&wslg_sock_path).is_err()
        })
        .collect()
}

