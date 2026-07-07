use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use std::process::Command;
use std::time::Duration;
use tempfile::tempdir;

#[test]
fn test_startup_without_base_dir_fails() {
    let output = Command::new(env!("CARGO_BIN_EXE_remote-fs-server"))
        .env_remove("BASE_DIR")
        .output()
        .expect("Failed to execute command");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("BASE_DIR environment variable is not set"));
}

#[test]
fn test_startup_with_nonexistent_base_dir_fails() {
    let output = Command::new(env!("CARGO_BIN_EXE_remote-fs-server"))
        .env("BASE_DIR", "/does/not/exist/we/hope")
        .output()
        .expect("Failed to execute command");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("BASE_DIR does not exist"));
}

#[test]
fn test_graceful_shutdown_with_sigterm() {
    let dir = tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_remote-fs-server"))
        .env("BASE_DIR", dir.path())
        .arg("--port")
        .arg("3050") // Use different port
        .spawn()
        .expect("Failed to spawn process");

    // Give it a moment to start
    std::thread::sleep(Duration::from_millis(500));

    // Send SIGTERM
    let pid = Pid::from_raw(child.id() as i32);
    signal::kill(pid, Signal::SIGTERM).expect("Failed to send SIGTERM");

    // Wait for the child to exit gracefully
    let exit_status = child.wait().expect("Failed to wait on child");
    assert!(exit_status.success() || exit_status.code() == Some(0));
}
