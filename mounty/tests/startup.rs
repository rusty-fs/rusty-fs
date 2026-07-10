mod common;

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::thread;
use std::time::Duration;

use tempfile::TempDir;

fn mounty_command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_mounty"))
}

#[test]
fn startup_without_required_args_fails() {
    let output = mounty_command().output().expect("run mounty");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Usage:"), "stderr was: {stderr}");
}

#[test]
fn startup_with_nonexistent_mountpoint_fails_before_mount() {
    let missing = std::env::temp_dir().join(format!(
        "rusty-fs-missing-mountpoint-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&missing);

    let output = mounty_command()
        .arg("http://127.0.0.1:9")
        .arg(&missing)
        .output()
        .expect("run mounty");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("mountpoint does not exist"),
        "stderr was: {stderr}"
    );
}

#[test]
fn startup_with_file_mountpoint_fails_before_mount() {
    let dir = TempDir::new().expect("temp dir");
    let mountpoint = dir.path().join("not-a-directory");
    fs::write(&mountpoint, b"file").expect("write mountpoint file");

    let output = mounty_command()
        .arg("http://127.0.0.1:9")
        .arg(&mountpoint)
        .output()
        .expect("run mounty");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("mountpoint is not a directory"),
        "stderr was: {stderr}"
    );
}

#[test]
fn startup_with_backend_down_fails_during_probe() {
    let mountpoint = TempDir::new().expect("mountpoint");
    let port = common::free_port();

    let output = mounty_command()
        .arg(format!("http://127.0.0.1:{port}"))
        .arg(mountpoint.path())
        .output()
        .expect("run mounty");

    assert!(!output.status.success());
}

#[test]
fn startup_with_incompatible_backend_fails_during_probe() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind fake backend");
    let port = listener.local_addr().expect("local addr").port();
    let server = thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0_u8; 1024];
            let _ = stream.read(&mut buf);
            let body = b"not json";
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\ncontent-type: text/plain\r\n\r\n{}",
                body.len(),
                String::from_utf8_lossy(body)
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });

    let mountpoint = TempDir::new().expect("mountpoint");
    let output = mounty_command()
        .arg(format!("http://127.0.0.1:{port}"))
        .arg(mountpoint.path())
        .output()
        .expect("run mounty");

    let _ = server.join();
    assert!(!output.status.success());
    assert!(
        common::wait_for_mount(mountpoint.path(), false, Duration::from_millis(100)),
        "incompatible backend should not leave a mount active"
    );
}
