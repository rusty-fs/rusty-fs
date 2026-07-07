#![allow(dead_code)]
mod fs;

use fs::HttpClient;
use fs::RemoteFileSystem;
use std::sync::Arc;

use fuser::MountOption;

use tracing::{debug, info};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

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

    debug!(
        "Mounting remote filesystem from {} to {}",
        server_url, mountpoint
    );

    let http_client = Arc::new(HttpClient::new(server_url.to_string()));
    // load configuration from env (with sensible defaults)
    let config = fs::config::FuseConfig::from_env();
    debug!("using FuseConfig: {:?}", config);
    let fs = RemoteFileSystem::with_config(http_client.clone(), config.clone());
    // metrics removed: no background metrics reporter

    let mut options = vec![
        MountOption::RW, // Read-write
        MountOption::FSName("remote-fs".to_string()),
    ];

    if cfg!(target_os = "macos") {
        let iosize = std::env::var("MOUNTY_IOSIZE")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "1048576".to_string());

        options.push(MountOption::CUSTOM("noappledouble".to_string()));
        options.push(MountOption::CUSTOM("noapplexattr".to_string()));
        options.push(MountOption::CUSTOM(format!("iosize={iosize}")));
    }
    fuser::mount2(fs, mountpoint, &options)?;

    Ok(())
}
