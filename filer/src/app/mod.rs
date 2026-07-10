use crate::handlers::{delete_path, list, meta, mkdir, put_file, read, update_meta};
use axum::extract::DefaultBodyLimit;
use axum::{
    Extension, Router,
    routing::{delete, get, patch, post, put},
};
use std::sync::Arc;
use tower_http::trace::{self, TraceLayer};
use tracing::Level;

pub fn build_app(shared_base_dir: Arc<String>) -> Router {
    Router::new()
        .route("/list", get(list))
        .route("/list/", get(list))
        .route("/list/{*path}", get(list))
        .route("/meta/{*file_path}", get(meta))
        .route("/meta/{*file_path}", patch(update_meta))
        .route("/files/{*file_path}", get(read))
        .route("/mkdir/{*file_path}", post(mkdir))
        .route("/files/{*file_path}", delete(delete_path))
        .route("/files/{*file_path}", put(put_file))
        .layer(Extension(shared_base_dir))
        .layer(DefaultBodyLimit::disable())
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(trace::DefaultMakeSpan::new().level(Level::TRACE))
                .on_request(trace::DefaultOnRequest::new().level(Level::TRACE))
                .on_response(trace::DefaultOnResponse::new().level(Level::TRACE)),
        )
}
