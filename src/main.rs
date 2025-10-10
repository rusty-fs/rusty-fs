mod fs;

use fs::RemoteFileSystem;
use fs::HttpClient;

use fuser::MountOption;

fn main() -> anyhow::Result<()> { 
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: {} <server_url> <mountpoint>", args[0]);
        std::process::exit(1);
    }

    let server_url = &args[1];
    let mountpoint = &args[2];

    println!("Mounting remote filesystem from {} to {}", server_url, mountpoint);

    let fs = RemoteFileSystem::new(HttpClient::new(server_url.to_string()));
    
    let options = vec![
        MountOption::RO,               // Read-only
        MountOption::FSName("remote-fs".to_string()),
    ];

    fuser::mount2(fs, mountpoint, &options)?;
    
    Ok(())
}