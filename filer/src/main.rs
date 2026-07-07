mod app;
mod handlers;

use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;

use crate::app::build_app;
use tracing::{debug, info};
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();

    let base_dir = base_dir_from_env()?;
    debug!("Base directory set to: {}", base_dir);
    let shared_base_dir = Arc::new(base_dir);

    let app = build_app(shared_base_dir);

    let port = parse_port(std::env::args())?;

    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("Listening on {}", addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    info!("Shutdown complete");
    Ok(())
}

fn base_dir_from_env() -> Result<String, Box<dyn Error>> {
    let base_dir =
        std::env::var("BASE_DIR").map_err(|_| "BASE_DIR environment variable is not set")?;
    let path = PathBuf::from(&base_dir);
    if !path.exists() {
        return Err(format!("BASE_DIR does not exist: {}", base_dir).into());
    }
    if !path.is_dir() {
        return Err(format!("BASE_DIR is not a directory: {}", base_dir).into());
    }
    Ok(base_dir)
}

fn parse_port<I>(args: I) -> Result<u16, Box<dyn Error>>
where
    I: IntoIterator<Item = String>,
{
    let args: Vec<String> = args.into_iter().collect();
    let mut port = 3000;

    if let Some(pos) = args.iter().position(|x| x == "--port") {
        let value = args.get(pos + 1).ok_or("--port requires a numeric value")?;
        port = value.parse::<u16>()?;
    }

    Ok(port)
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut terminate =
            signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");

        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                if let Err(err) = result {
                    tracing::error!("failed to listen for SIGINT: {}", err);
                }
                info!("Received SIGINT, starting graceful shutdown");
            }
            _ = terminate.recv() => {
                info!("Received SIGTERM, starting graceful shutdown");
            }
        }
    }

    #[cfg(not(unix))]
    {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::error!("failed to listen for Ctrl-C: {}", err);
        }
        info!("Received shutdown signal, starting graceful shutdown");
    }
}
