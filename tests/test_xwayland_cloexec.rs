use std::os::unix::net::UnixStream;
use std::os::unix::io::{AsRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::Command;

fn unset_cloexec(fd: RawFd) {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
    }
}

#[test]
fn test_xwayland_cloexec() {
    let (_s1, s2) = UnixStream::pair().unwrap();
    let mut cmd = Command::new("Xwayland");
    cmd.arg(":99");
    cmd.env("WAYLAND_SOCKET", s2.as_raw_fd().to_string());
    unsafe {
        let fd = s2.as_raw_fd();
        cmd.pre_exec(move || {
            unset_cloexec(fd);
            Ok(())
        });
    }
    let mut child = cmd.spawn().unwrap();
    
    // wait for it
    let res = child.wait().unwrap();
    println!("Exit status: {:?}", res);
}
