use std::os::unix::net::UnixStream;
use std::os::unix::io::AsRawFd;
use std::process::Command;

fn main() {
    let (s1, s2) = UnixStream::pair().unwrap();
    let mut cmd = Command::new("Xwayland");
    cmd.arg(":99");
    cmd.env("WAYLAND_SOCKET", s2.as_raw_fd().to_string());
    let mut child = cmd.spawn().unwrap();
    
    let res = child.wait().unwrap();
    println!("Exit status: {:?}", res);
}
