use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use std::thread;
use std::time::Duration;

fn build_filer() -> std::path::PathBuf {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let filer_dir = manifest_dir.parent().unwrap().join("filer");
    
    let mut cmd = Command::new("cargo");
    cmd.arg("build").arg("--bin").arg("remote-fs-server");
    cmd.current_dir(&filer_dir);
    
    let status = cmd.status().expect("Failed to run cargo build for filer");
    assert!(status.success(), "Failed to build filer");
    
    filer_dir.join("target").join("debug").join("remote-fs-server")
}

struct TestEnv {
    filer: std::process::Child,
    mounty: std::process::Child,
    base_dir: String,
    mount_dir: String,
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        let _ = Command::new("fusermount").arg("-u").arg(&self.mount_dir).status();
        #[cfg(target_os = "macos")]
        let _ = Command::new("umount").arg(&self.mount_dir).status();
        
        self.mounty.kill().ok();
        self.filer.kill().ok();
        
        let _ = fs::remove_dir_all(&self.base_dir);
        let _ = fs::remove_dir_all(&self.mount_dir);
    }
}

fn setup_env(test_name: &str, port: u16) -> TestEnv {
    let base_dir = format!("/tmp/rusty-fs-e2e-base-{}", test_name);
    let mount_dir = format!("/tmp/rusty-fs-e2e-mount-{}", test_name);
    
    let _ = fs::remove_dir_all(&base_dir);
    let _ = fs::remove_dir_all(&mount_dir);
    fs::create_dir_all(&base_dir).unwrap();
    fs::create_dir_all(&mount_dir).unwrap();
    
    let filer_bin = build_filer();
    let mounty_bin = env!("CARGO_BIN_EXE_mounty");
    
    let filer = Command::new(filer_bin)
        .env("BASE_DIR", &base_dir)
        .arg("--port")
        .arg(port.to_string())
        .spawn()
        .expect("Failed to start filer");
        
    thread::sleep(Duration::from_millis(1500));
    
    let mounty = Command::new(mounty_bin)
        .arg(format!("http://127.0.0.1:{}", port))
        .arg(&mount_dir)
        .spawn()
        .expect("Failed to start mounty");
        
    thread::sleep(Duration::from_millis(2000));
    
    TestEnv { filer, mounty, base_dir, mount_dir }
}

#[test]
fn test_e2e_write_and_read() {
    let env = setup_env("write_read", 3055);
    
    let test_file = format!("{}/test.txt", env.mount_dir);
    let test_content = "Hello from FUSE E2E test!";
    
    assert!(fs::write(&test_file, test_content).is_ok(), "Write failed");
    
    let read_res = fs::read_to_string(&test_file).unwrap_or_default();
    assert_eq!(read_res, test_content, "Read content does not match");
}

#[test]
fn test_e2e_directories() {
    let env = setup_env("directories", 3056);
    
    let test_dir = format!("{}/my_folder", env.mount_dir);
    assert!(fs::create_dir(&test_dir).is_ok(), "mkdir failed");
    
    // Check it exists by getting metadata
    assert!(fs::metadata(&test_dir).unwrap().is_dir());
    
    assert!(fs::remove_dir(&test_dir).is_ok(), "rmdir failed");
    assert!(fs::metadata(&test_dir).is_err(), "Dir should not exist anymore");
}

#[test]
fn test_e2e_rename() {
    let env = setup_env("rename", 3057);
    
    let file1 = format!("{}/file1.txt", env.mount_dir);
    let file2 = format!("{}/file2.txt", env.mount_dir);
    
    fs::write(&file1, "rename me").unwrap();
    assert!(fs::rename(&file1, &file2).is_ok(), "rename failed");
    
    assert!(fs::metadata(&file1).is_err(), "Old file should not exist");
    assert!(fs::metadata(&file2).is_ok(), "New file should exist");
}

#[test]
fn test_e2e_permissions() {
    let env = setup_env("permissions", 3058);
    
    let test_file = format!("{}/chmod_test.txt", env.mount_dir);
    fs::write(&test_file, "secret").unwrap();
    
    let mut perms = fs::metadata(&test_file).unwrap().permissions();
    perms.set_mode(0o777);
    assert!(fs::set_permissions(&test_file, perms).is_ok(), "chmod failed");
    
    let new_perms = fs::metadata(&test_file).unwrap().permissions().mode();
    assert_eq!(new_perms & 0o777, 0o777, "Permissions were not updated");
}

#[test]
fn test_e2e_truncate() {
    let env = setup_env("truncate", 3059);
    
    let test_file = format!("{}/truncate_test.txt", env.mount_dir);
    fs::write(&test_file, "Hello World!").unwrap();
    
    // Truncate the file to 5 bytes
    let file = fs::OpenOptions::new().write(true).open(&test_file).unwrap();
    file.set_len(5).expect("truncate failed");
    
    let content = fs::read_to_string(&test_file).unwrap();
    assert_eq!(content, "Hello", "Content was not truncated correctly");
}

#[test]
fn test_e2e_copy() {
    let env = setup_env("copy", 3060);
    
    let src_file = format!("{}/src_file.txt", env.mount_dir);
    let dst_file = format!("{}/dst_file.txt", env.mount_dir);
    
    fs::write(&src_file, "Copy me").unwrap();
    
    assert!(fs::copy(&src_file, &dst_file).is_ok(), "copy failed");
    
    let content = fs::read_to_string(&dst_file).unwrap();
    assert_eq!(content, "Copy me", "Copied content does not match");
}
