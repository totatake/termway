use std::os::unix::net::UnixStream;
use std::os::unix::io::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::Command;

fn main() {
    let (s1, s2) = UnixStream::pair().unwrap();
    let mut cmd = Command::new("Xwayland");
    cmd.arg(":99");
    cmd.env("WAYLAND_SOCKET", s2.as_raw_fd().to_string());
    unsafe {
        cmd.pre_exec(move || {
            // Unset CLOEXEC
            let flags = libc::fcntl(s2.as_raw_fd(), libc::F_GETFD);
            libc::fcntl(s2.as_raw_fd(), libc::F_SETFD, flags & !libc::FD_CLOEXEC);
            Ok(())
        });
    }
    let mut child = cmd.spawn().unwrap();
    
    // Parent keeps s1 open.
    // Let's read from s1 to see if Xwayland writes Wayland greeting!
    use std::io::Read;
    let mut buf = [0u8; 1024];
    s1.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
    match s1.read(&mut buf) {
        Ok(n) => println!("Read {} bytes from Xwayland Wayland socket!", n),
        Err(e) => println!("Read error: {}", e),
    }
    child.wait().unwrap();
}
