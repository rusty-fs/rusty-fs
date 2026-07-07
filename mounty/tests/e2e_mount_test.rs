use std::fs;
use std::process::Command;
use std::thread;
use std::time::Duration;

fn build_filer() -> std::path::PathBuf {
    // env!("CARGO_MANIFEST_DIR") ci dà il percorso assoluto a `rusty-fs/mounty`
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let filer_dir = manifest_dir.parent().unwrap().join("filer");
    
    let mut cmd = Command::new("cargo");
    cmd.arg("build").arg("--bin").arg("remote-fs-server");
    cmd.current_dir(&filer_dir);
    
    let status = cmd.status().expect("Failed to run cargo build for filer");
    assert!(status.success(), "Failed to build filer");
    
    // Il binario sarà in `rusty-fs/filer/target/debug/remote-fs-server`
    filer_dir.join("target").join("debug").join("remote-fs-server")
}

#[test]
fn test_e2e_mount_and_write() {
    let base_dir = "/tmp/rusty-fs-e2e-base";
    let mount_dir = "/tmp/rusty-fs-e2e-mount";
    
    let _ = fs::remove_dir_all(base_dir);
    let _ = fs::remove_dir_all(mount_dir);
    fs::create_dir_all(base_dir).unwrap();
    fs::create_dir_all(mount_dir).unwrap();
    
    // Assicuriamoci che il server sia compilato
    let filer_bin = build_filer();
    // CARGO_BIN_EXE_<name> è reso disponibile automaticamente da cargo test
    let mounty_bin = env!("CARGO_BIN_EXE_mounty");
    
    // 1. Avvia il server
    let mut filer = Command::new(filer_bin)
        .env("BASE_DIR", base_dir)
        .arg("--port")
        .arg("3055")
        .spawn()
        .expect("Failed to start filer");
        
    thread::sleep(Duration::from_secs(1)); // Attendi avvio server
    
    // 2. Avvia il client FUSE
    let mut mounty = Command::new(mounty_bin)
        .arg("http://127.0.0.1:3055")
        .arg(mount_dir)
        .spawn()
        .expect("Failed to start mounty");
        
    thread::sleep(Duration::from_secs(2)); // Attendi il mounting
    
    // 3. Verifica operazioni file reali (Questo prova sia il Mount che il Write)
    let test_file = format!("{}/test.txt", mount_dir);
    let test_content = "Hello from FUSE E2E test!";
    
    // Test Scrittura (risolve "Write files to remote server" non provato)
    let write_res = fs::write(&test_file, test_content);
    
    // Test Lettura
    let read_res = fs::read_to_string(&test_file).unwrap_or_default();
    
    // 4. Pulizia (Smonta e uccidi processi)
    #[cfg(target_os = "linux")]
    let _ = Command::new("fusermount").arg("-u").arg(mount_dir).status();
    #[cfg(target_os = "macos")]
    let _ = Command::new("umount").arg(mount_dir).status();
    
    mounty.kill().ok();
    filer.kill().ok();
    
    let _ = fs::remove_dir_all(base_dir);
    let _ = fs::remove_dir_all(mount_dir);

    // Asserzioni finali (fatte alla fine per garantire la pulizia)
    assert!(write_res.is_ok(), "Failed to write to mounted FUSE");
    assert_eq!(read_res, test_content, "Content read does not match content written");
}
