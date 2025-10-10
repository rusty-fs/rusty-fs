use axum::{Extension, Router, routing::get};
use crate::handlers::{list, meta};
use std::sync::Arc;

pub fn build_app(shared_base_dir: Arc<String>) -> Router {
    Router::new()
        .route("/list", get(list))
        .route("/list/", get(list))
        .route("/list/{*path}", get(list))
        .route("/meta/{*file_path}", get(meta))
        .layer(Extension(shared_base_dir))
}
