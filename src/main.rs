mod fs;

use fs::RemoteFileSystem;
use fs::HttpClient;
use std::sync::Arc;

use fuser::MountOption;

use tracing::{debug, error, warn, info};

fn main() -> anyhow::Result<()> { 
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        info!("Usage: {} <server_url> <mountpoint>", args[0]);
        std::process::exit(1);
    }

    let server_url = &args[1];
    let mountpoint = &args[2];

    debug!("Mounting remote filesystem from {} to {}", server_url, mountpoint);

    let http_client =  Arc::new(HttpClient::new(server_url.to_string()));
    let fs = RemoteFileSystem::new(http_client.clone());
    
    let options = vec![
        MountOption::RW,               // Read-write
        MountOption::FSName("remote-fs".to_string()),
    ];

    fuser::mount2(fs, mountpoint, &options)?;
    
    Ok(())
}