#![allow(dead_code)]
mod fs;

use fs::http::HttpBackend;
use fs::HttpClient;
use fs::RemoteFileSystem;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use fuser::{MountOption, Session, SessionUnmounter};

use tracing::{debug, error, info};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();

    info!("Starting mounty...");

    let args = parse_args(std::env::args())?;
    validate_mountpoint(&args.mountpoint)?;

    debug!(
        "Mounting remote filesystem from {} to {}",
        args.server_url,
        args.mountpoint.display()
    );

    let http_client = Arc::new(HttpClient::new(args.server_url));
    fs::utils::runtime::runtime()
        .block_on(async { http_client.list_directory("/").await.map(|_| ()) })?;
    info!("Backend probe succeeded");

    // load configuration from env (with sensible defaults)
    let config = fs::config::FuseConfig::from_env();
    debug!("using FuseConfig: {:?}", config);
    let fs = RemoteFileSystem::with_config(http_client.clone(), config.clone());
    let shutdown_failed = fs.shutdown_failed_flag();
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

    let mut session = Session::new(fs, &args.mountpoint, &options)?;
    install_shutdown_unmounter(session.unmount_callable());
    info!("Mounted remote filesystem at {}", args.mountpoint.display());

    let run_result = session.run();
    drop(session);

    if shutdown_failed.load(Ordering::SeqCst) {
        anyhow::bail!("shutdown drain failed; see logs for failed file handles");
    }

    run_result?;
    info!("FUSE session ended");

    Ok(())
}

struct Args {
    server_url: String,
    mountpoint: PathBuf,
}

fn parse_args<I>(args: I) -> anyhow::Result<Args>
where
    I: IntoIterator<Item = String>,
{
    let args: Vec<String> = args.into_iter().collect();
    if args.len() != 3 {
        anyhow::bail!("Usage: {} <server_url> <mountpoint>", args[0]);
    }

    Ok(Args {
        server_url: args[1].clone(),
        mountpoint: PathBuf::from(&args[2]),
    })
}

fn validate_mountpoint(mountpoint: &Path) -> anyhow::Result<()> {
    if !mountpoint.exists() {
        anyhow::bail!("mountpoint does not exist: {}", mountpoint.display());
    }
    if !mountpoint.is_dir() {
        anyhow::bail!("mountpoint is not a directory: {}", mountpoint.display());
    }
    Ok(())
}

fn install_shutdown_unmounter(mut unmounter: SessionUnmounter) {
    std::thread::spawn(move || {
        let signal = fs::utils::runtime::runtime().block_on(wait_for_shutdown_signal());
        info!("Received {}, unmounting filesystem", signal);
        if let Err(err) = unmounter.unmount() {
            error!("Failed to unmount filesystem during shutdown: {}", err);
        }
    });
}

async fn wait_for_shutdown_signal() -> &'static str {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        let mut terminate =
            signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");

        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                if let Err(err) = result {
                    error!("failed to listen for SIGINT: {}", err);
                }
                "SIGINT"
            }
            _ = terminate.recv() => "SIGTERM",
        }
    }

    #[cfg(not(unix))]
    {
        if let Err(err) = tokio::signal::ctrl_c().await {
            error!("failed to listen for Ctrl-C: {}", err);
        }
        "shutdown signal"
    }
}
