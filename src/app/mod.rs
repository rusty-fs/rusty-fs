use axum::{routing::{delete, get, post, put}, Extension, Router};
use crate::handlers::{list, meta, read, mkdir, delete_path, put_file};
use std::sync::Arc;

pub fn build_app(shared_base_dir: Arc<String>) -> Router {
    Router::new()
        .route("/list", get(list))
        .route("/list/", get(list))
        .route("/list/{*path}", get(list))
        .route("/meta/{*file_path}", get(meta))
        .route("/files/{*file_path}", get(read))
        .route("/mkdir/{*file_path}", post(mkdir))
        .route("/files/{*file_path}", delete(delete_path))
        .route("/files/{*file_path}", put(put_file))
        .layer(Extension(shared_base_dir))
}
