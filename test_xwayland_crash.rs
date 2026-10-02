use std::process::Command;
fn main() {
    let status = Command::new("Xwayland").arg(":99").status().unwrap();
    println!("Status: {:?}", status);
}
