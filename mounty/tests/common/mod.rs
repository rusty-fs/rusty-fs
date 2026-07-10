use std::fs;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::thread;
use std::time::{Duration, Instant};

use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use tempfile::TempDir;

pub fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind free port")
        .local_addr()
        .expect("read local addr")
        .port()
}

pub fn wait_for_tcp(port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return true;
        }
        thread::sleep(Duration::from_millis(50));
    }
    false
}

pub fn wait_for_mount(mountpoint: &Path, mounted: bool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if is_mount_active(mountpoint) == mounted {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    false
}

pub fn wait_for_exit(child: &mut Child, timeout: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().expect("poll child") {
            return Some(status);
        }
        thread::sleep(Duration::from_millis(50));
    }
    None
}

pub fn send_sigterm(child: &Child) {
    let pid = Pid::from_raw(child.id() as i32);
    signal::kill(pid, Signal::SIGTERM).expect("send SIGTERM");
}

pub fn build_filer() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let filer_dir = manifest_dir.parent().expect("workspace parent").join("filer");

    let status = Command::new("cargo")
        .arg("build")
        .arg("--bin")
        .arg("remote-fs-server")
        .current_dir(&filer_dir)
        .status()
        .expect("build filer");
    assert!(status.success(), "failed to build filer");

    filer_dir
        .join("target")
        .join("debug")
        .join("remote-fs-server")
}

pub struct FilerProcess {
    pub child: Child,
    pub base_dir: TempDir,
    pub port: u16,
}

impl FilerProcess {
    pub fn start() -> Self {
        let port = free_port();
        let base_dir = TempDir::new().expect("create filer base dir");
        let child = Command::new(build_filer())
            .env("BASE_DIR", base_dir.path())
            .arg("--port")
            .arg(port.to_string())
            .spawn()
            .expect("start filer");

        assert!(wait_for_tcp(port, Duration::from_secs(5)), "filer did not start");

        Self {
            child,
            base_dir,
            port,
        }
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

impl Drop for FilerProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct MountyProcess {
    pub child: Child,
    pub mount_dir: TempDir,
}

impl MountyProcess {
    pub fn start(server_url: &str) -> Self {
        let mount_dir = TempDir::new().expect("create mount dir");
        let mut child = Command::new(env!("CARGO_BIN_EXE_mounty"))
            .arg(server_url)
            .arg(mount_dir.path())
            .spawn()
            .expect("start mounty");

        if !wait_for_mount(mount_dir.path(), true, Duration::from_secs(10)) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("mounty did not mount {}", mount_dir.path().display());
        }

        Self { child, mount_dir }
    }
}

impl Drop for MountyProcess {
    fn drop(&mut self) {
        let _ = unmount(self.mount_dir.path());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn is_mount_active(mountpoint: &Path) -> bool {
    let mountpoint = fs::canonicalize(mountpoint).unwrap_or_else(|_| mountpoint.to_path_buf());
    let needle = format!(" on {} ", mountpoint.display());
    let output = Command::new("mount").output().expect("run mount");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .any(|line| line.contains(&needle))
}

pub fn unmount(mountpoint: &Path) -> bool {
    #[cfg(target_os = "linux")]
    {
        if Command::new("fusermount3")
            .arg("-u")
            .arg(mountpoint)
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
        {
            return true;
        }
        if Command::new("fusermount")
            .arg("-u")
            .arg(mountpoint)
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
        {
            return true;
        }
        Command::new("umount")
            .arg(mountpoint)
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    #[cfg(target_os = "macos")]
    {
        if Command::new("umount")
            .arg(mountpoint)
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
        {
            return true;
        }
        Command::new("diskutil")
            .arg("unmount")
            .arg(mountpoint)
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = mountpoint;
        false
    }
}
