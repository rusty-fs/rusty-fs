mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;

struct TestEnv {
    _filer: common::FilerProcess,
    mounty: common::MountyProcess,
}

fn setup_env() -> TestEnv {
    let filer = common::FilerProcess::start();
    let mounty = common::MountyProcess::start(&filer.url());
    TestEnv {
        _filer: filer,
        mounty,
    }
}

#[test]
#[ignore = "requires FUSE/macFUSE and local mount privileges"]
fn test_e2e_write_and_read() {
    let env = setup_env();

    let test_file = env.mounty.mount_dir.path().join("test.txt");
    let test_content = "Hello from FUSE E2E test!";

    assert!(fs::write(&test_file, test_content).is_ok(), "Write failed");

    let read_res = fs::read_to_string(&test_file).unwrap_or_default();
    assert_eq!(read_res, test_content, "Read content does not match");
}

#[test]
#[ignore = "requires FUSE/macFUSE and local mount privileges"]
fn test_e2e_directories() {
    let env = setup_env();

    let test_dir = env.mounty.mount_dir.path().join("my_folder");
    assert!(fs::create_dir(&test_dir).is_ok(), "mkdir failed");

    // Check it exists by getting metadata
    assert!(fs::metadata(&test_dir).unwrap().is_dir());

    assert!(fs::remove_dir(&test_dir).is_ok(), "rmdir failed");
    assert!(
        fs::metadata(&test_dir).is_err(),
        "Dir should not exist anymore"
    );
}

#[test]
#[ignore = "requires FUSE/macFUSE and local mount privileges"]
fn test_e2e_rename() {
    let env = setup_env();

    let file1 = env.mounty.mount_dir.path().join("file1.txt");
    let file2 = env.mounty.mount_dir.path().join("file2.txt");

    fs::write(&file1, "rename me").unwrap();
    assert!(fs::rename(&file1, &file2).is_ok(), "rename failed");

    assert!(fs::metadata(&file1).is_err(), "Old file should not exist");
    assert!(fs::metadata(&file2).is_ok(), "New file should exist");
}

#[test]
#[ignore = "requires FUSE/macFUSE and local mount privileges"]
fn test_e2e_permissions() {
    let env = setup_env();

    let test_file = env.mounty.mount_dir.path().join("chmod_test.txt");
    fs::write(&test_file, "secret").unwrap();

    let mut perms = fs::metadata(&test_file).unwrap().permissions();
    perms.set_mode(0o777);
    assert!(
        fs::set_permissions(&test_file, perms).is_ok(),
        "chmod failed"
    );

    let new_perms = fs::metadata(&test_file).unwrap().permissions().mode();
    assert_eq!(new_perms & 0o777, 0o777, "Permissions were not updated");
}

#[test]
#[ignore = "requires FUSE/macFUSE and local mount privileges"]
fn test_e2e_truncate() {
    let env = setup_env();

    let test_file = env.mounty.mount_dir.path().join("truncate_test.txt");
    fs::write(&test_file, "Hello World!").unwrap();

    // Truncate the file to 5 bytes
    let file = fs::OpenOptions::new().write(true).open(&test_file).unwrap();
    file.set_len(5).expect("truncate failed");

    let content = fs::read_to_string(&test_file).unwrap();
    assert_eq!(content, "Hello", "Content was not truncated correctly");
}

#[test]
#[ignore = "requires FUSE/macFUSE and local mount privileges"]
fn test_e2e_copy() {
    let env = setup_env();

    let src_file = env.mounty.mount_dir.path().join("src_file.txt");
    let dst_file = env.mounty.mount_dir.path().join("dst_file.txt");

    fs::write(&src_file, "Copy me").unwrap();
    let src_content = fs::read(&src_file).expect("read source failed");
    fs::write(&dst_file, src_content).expect("write destination failed");

    let content = fs::read_to_string(&dst_file).unwrap();
    assert_eq!(content, "Copy me", "Copied content does not match");
}
