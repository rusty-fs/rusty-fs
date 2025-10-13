use axum::{Extension, Router, routing::get};
use crate::handlers::{list, meta, read};
use std::sync::Arc;

pub fn build_app(shared_base_dir: Arc<String>) -> Router {
    Router::new()
        .route("/list", get(list))
        .route("/list/", get(list))
        .route("/list/{*path}", get(list))
        .route("/meta/{*file_path}", get(meta))
        .route("/files/{*file_path}", get(read))

        .layer(Extension(shared_base_dir))
}
