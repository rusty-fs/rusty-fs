mod app;
mod handlers;

use std::sync::Arc;

use crate::app::build_app;
use tracing::{debug, error};
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() {
    // initialize tracing
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();

    // read base directory from environment
    let base_dir: String = if let Ok(val) = std::env::var("BASE_DIR") {
        debug!("Base directory set to: {}", val);
        val
    } else {
        // return an error if BASE_DIR is not set
        error!("BASE_DIR environment variable is not set.");
        std::process::exit(1);
    };
    let shared_base_dir = Arc::new(base_dir);

    // build our application with a route
    let app = build_app(shared_base_dir);

    // run our app with hyper, listening globally on port 3000
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
