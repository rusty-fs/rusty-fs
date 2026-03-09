mod fs;

use fs::RemoteFileSystem;
use fs::HttpClient;
use std::sync::Arc;

use fuser::MountOption;

use tracing::{debug, error, warn, info};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt};
use tracing_subscriber::filter::EnvFilter;

fn main() -> anyhow::Result<()> { 
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();

    info!("Starting mounty...");

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
    
    let mut options = vec![
        MountOption::RW,               // Read-write
        MountOption::FSName("remote-fs".to_string()),
    ];

    if cfg!(target_os = "macos") {
        options.push(MountOption::CUSTOM("noappledouble".to_string()));
        options.push(MountOption::CUSTOM("noapplexattr".to_string()));
    }
    fuser::mount2(fs, mountpoint, &options)?;
    
    Ok(())
}