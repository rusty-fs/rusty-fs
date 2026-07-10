#![allow(dead_code)]
mod fs;

use fs::http::HttpBackend;
use fs::HttpClient;
use fs::RemoteFileSystem;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use fuser::{MountOption, Session, SessionUnmounter};

use tracing::{debug, error, info, warn};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
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
    let forced_shutdown = Arc::new(AtomicBool::new(false));
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
        options.push(MountOption::CUSTOM(format!("iosize={iosize}")));
    }

    let mut session = Session::new(fs, &args.mountpoint, &options)?;
    install_shutdown_unmounter(
        session.unmount_callable(),
        args.mountpoint.clone(),
        forced_shutdown.clone(),
    );
    info!("Mounted remote filesystem at {}", args.mountpoint.display());

    let run_result = session.run();
    drop(session);

    if forced_shutdown.load(Ordering::SeqCst) {
        std::process::exit(130);
    }

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

fn install_shutdown_unmounter(
    mut unmounter: SessionUnmounter,
    mountpoint: PathBuf,
    forced_shutdown: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        fs::utils::runtime::runtime().block_on(async move {
            let signal = wait_for_shutdown_signal().await;
            info!("Received {}, starting graceful filesystem unmount", signal);

            let additional_shutdown_signals = Arc::new(AtomicUsize::new(0));
            let shutdown_notify = Arc::new(tokio::sync::Notify::new());
            {
                let additional_shutdown_signals = additional_shutdown_signals.clone();
                let shutdown_notify = shutdown_notify.clone();
                tokio::spawn(async move {
                    loop {
                        let signal = wait_for_shutdown_signal().await;
                        additional_shutdown_signals.fetch_add(1, Ordering::SeqCst);
                        warn!(
                            "Received {} during graceful shutdown; forcing unmount at next safe point",
                            signal
                        );
                        shutdown_notify.notify_waiters();
                    }
                });
            }

            let mut attempts = 0usize;

            loop {
                force_if_additional_shutdown_signal(
                    &additional_shutdown_signals,
                    &forced_shutdown,
                    &mountpoint,
                );

                attempts += 1;
                match unmounter.unmount() {
                    Ok(()) => {
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        force_if_additional_shutdown_signal(
                            &additional_shutdown_signals,
                            &forced_shutdown,
                            &mountpoint,
                        );
                        if !is_mount_active(&mountpoint) {
                            info!("Graceful filesystem unmount completed");
                            return;
                        }
                    }
                    Err(err) => {
                        if attempts == 1 || attempts % 10 == 0 {
                            warn!(
                                "Graceful filesystem unmount attempt {} failed: {}. Retrying; press Ctrl-C again to force unmount and exit",
                                attempts,
                                err
                            );
                        }
                    }
                }

                request_platform_unmount(&mountpoint);
                force_if_additional_shutdown_signal(
                    &additional_shutdown_signals,
                    &forced_shutdown,
                    &mountpoint,
                );
                tokio::time::sleep(Duration::from_millis(200)).await;
                force_if_additional_shutdown_signal(
                    &additional_shutdown_signals,
                    &forced_shutdown,
                    &mountpoint,
                );
                if !is_mount_active(&mountpoint) {
                    info!("Graceful filesystem unmount completed");
                    return;
                }

                if attempts == 1 || attempts % 10 == 0 {
                    warn!(
                        "Graceful filesystem unmount attempt {} left {} mounted. Retrying; press Ctrl-C again to force unmount and exit",
                        attempts,
                        mountpoint.display()
                    );
                }

                tokio::select! {
                    _ = shutdown_notify.notified() => {}
                    _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                }
            }
        });
    });
}

fn force_if_additional_shutdown_signal(
    additional_shutdown_signals: &AtomicUsize,
    forced_shutdown: &AtomicBool,
    mountpoint: &Path,
) {
    if additional_shutdown_signals.load(Ordering::SeqCst) == 0 {
        return;
    }

    warn!(
        "Forcing unmount of {} after additional shutdown signal",
        mountpoint.display()
    );
    forced_shutdown.store(true, Ordering::SeqCst);
    force_unmount(mountpoint);
    std::process::exit(130);
}

fn request_platform_unmount(mountpoint: &Path) {
    #[cfg(target_os = "macos")]
    {
        if run_unmount_command("umount", &[], mountpoint, "Graceful") {
            return;
        }
        run_unmount_command("diskutil", &["unmount"], mountpoint, "Graceful");
    }

    #[cfg(target_os = "linux")]
    {
        if run_unmount_command("fusermount3", &["-u"], mountpoint, "Graceful") {
            return;
        }
        if run_unmount_command("fusermount", &["-u"], mountpoint, "Graceful") {
            return;
        }
        run_unmount_command("umount", &[], mountpoint, "Graceful");
    }
}

fn is_mount_active(mountpoint: &Path) -> bool {
    let mountpoint = std::fs::canonicalize(mountpoint).unwrap_or_else(|_| mountpoint.to_path_buf());
    let mountpoint = mountpoint.to_string_lossy();

    let output = match Command::new("mount").output() {
        Ok(output) => output,
        Err(err) => {
            error!("Failed to inspect active mounts: {}", err);
            return false;
        }
    };

    let mounts = String::from_utf8_lossy(&output.stdout);
    mounts
        .lines()
        .any(|line| line.contains(&format!(" on {} ", mountpoint)))
}

fn force_unmount(mountpoint: &Path) {
    #[cfg(target_os = "macos")]
    {
        if run_unmount_command("umount", &["-f"], mountpoint, "Forced") {
            return;
        }
        run_unmount_command("diskutil", &["unmount", "force"], mountpoint, "Forced");
    }

    #[cfg(target_os = "linux")]
    {
        if run_unmount_command("fusermount3", &["-uz"], mountpoint, "Forced") {
            return;
        }
        if run_unmount_command("fusermount", &["-uz"], mountpoint, "Forced") {
            return;
        }
        run_unmount_command("umount", &["-l"], mountpoint, "Forced");
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        error!(
            "Forced unmount is not implemented for this platform: {}",
            mountpoint.display()
        );
    }
}

fn run_unmount_command(command: &str, args: &[&str], mountpoint: &Path, label: &str) -> bool {
    match Command::new(command).args(args).arg(mountpoint).status() {
        Ok(status) if status.success() => {
            debug!(
                "{} unmount command succeeded: {} {} {}",
                label,
                command,
                args.join(" "),
                mountpoint.display()
            );
            true
        }
        Ok(status) => {
            if label == "Graceful" {
                warn!(
                    "{} unmount command failed with status {}: {} {} {}",
                    label,
                    status,
                    command,
                    args.join(" "),
                    mountpoint.display()
                );
            } else {
                error!(
                    "{} unmount command failed with status {}: {} {} {}",
                    label,
                    status,
                    command,
                    args.join(" "),
                    mountpoint.display()
                );
            }
            false
        }
        Err(err) => {
            if label == "Graceful" {
                warn!(
                    "Failed to run {} unmount command {} for {}: {}",
                    label.to_lowercase(),
                    command,
                    mountpoint.display(),
                    err
                );
            } else {
                error!(
                    "Failed to run {} unmount command {} for {}: {}",
                    label.to_lowercase(),
                    command,
                    mountpoint.display(),
                    err
                );
            }
            false
        }
    }
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
