mod common;

use std::fs;
use std::io::{Seek, SeekFrom, Write};
use std::time::Duration;

#[test]
#[ignore = "requires FUSE/macFUSE and local mount privileges"]
fn sigterm_unmounts_and_persists_file_data() {
    let mut filer = common::FilerProcess::start();
    let mut mounty = common::MountyProcess::start(&filer.url());

    let mounted_path = mounty.mount_dir.path().join("open-write.txt");
    let mut file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&mounted_path)
        .expect("open file through mount");
    file.write_all(b"hello before shutdown")
        .expect("write before shutdown");
    file.seek(SeekFrom::Start(6)).expect("seek before shutdown");
    file.write_all(b"after!").expect("overwrite before shutdown");
    drop(file);

    common::send_sigterm(&mounty.child);
    let status = common::wait_for_exit(&mut mounty.child, Duration::from_secs(15))
        .expect("mounty should exit after SIGTERM");
    assert!(status.success(), "mounty exited with {status}");
    assert!(
        common::wait_for_mount(mounty.mount_dir.path(), false, Duration::from_secs(5)),
        "mountpoint should be unmounted after SIGTERM"
    );

    let persisted = fs::read(filer.base_dir.path().join("open-write.txt"))
        .expect("read persisted server file");
    assert_eq!(persisted, b"hello after! shutdown");

    common::send_sigterm(&filer.child);
    let _ = common::wait_for_exit(&mut filer.child, Duration::from_secs(5));
}

#[test]
#[ignore = "requires FUSE/macFUSE and local mount privileges"]
fn external_unmount_makes_mounty_exit_and_persist_data() {
    let mut filer = common::FilerProcess::start();
    let mut mounty = common::MountyProcess::start(&filer.url());

    let mounted_path = mounty.mount_dir.path().join("external-unmount.txt");
    fs::write(&mounted_path, b"persist me").expect("write through mount");

    assert!(common::unmount(mounty.mount_dir.path()), "external unmount failed");
    let status = common::wait_for_exit(&mut mounty.child, Duration::from_secs(15))
        .expect("mounty should exit after external unmount");
    assert!(status.success(), "mounty exited with {status}");

    let persisted = fs::read(filer.base_dir.path().join("external-unmount.txt"))
        .expect("read persisted server file");
    assert_eq!(persisted, b"persist me");

    common::send_sigterm(&filer.child);
    let _ = common::wait_for_exit(&mut filer.child, Duration::from_secs(5));
}
