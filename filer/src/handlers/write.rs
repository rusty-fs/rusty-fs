use super::path::safe_join_path;
use super::range::parse_content_range;
use axum::{
    body::Body,
    extract::{Extension, Path},
    http::{HeaderMap, StatusCode},
};
use std::fs;
use std::io::SeekFrom;
use std::sync::Arc;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tracing::{debug, error, trace};

pub async fn put_file(
    file_path: Path<String>,
    Extension(base_dir): Extension<Arc<String>>,
    headers: HeaderMap,
    body: Body,
) -> Result<StatusCode, StatusCode> {
    let requested_raw = file_path.0;
    let full_path = safe_join_path(&base_dir, &requested_raw)?;
    let requested = requested_raw.trim_start_matches('/').to_string();
    if requested.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    debug!("Writing file: {} -> {:?}", requested, full_path);

    trace!("Headers: {:?}", headers);
    trace!("Content-Range header: {:?}", headers.get("content-range"));

    if let Some(parent) = full_path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            error!("Failed to create parent dirs {:?}: {}", parent, e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    }

    let existed = full_path.exists();

    let (offset, should_truncate, total_size) = if let Some(range) = headers.get("content-range") {
        parse_content_range(range.to_str().map_err(|_| StatusCode::BAD_REQUEST)?)
            .map_err(|_| StatusCode::BAD_REQUEST)?
    } else {
        (0, true, None)
    };

    let content_length = headers
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());
    let is_empty_body = content_length == Some(0);

    if should_truncate {
        debug!("Full write detected, truncating file if it exists");
        let mut tmp_path = full_path.clone();
        tmp_path.set_extension(format!(
            "{}.filer_tmp",
            tmp_path.extension().and_then(|s| s.to_str()).unwrap_or("")
        ));

        let f = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp_path)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        let mut f = tokio::io::BufWriter::with_capacity(8 * 1024 * 1024, f); // 8MB buffer

        let mut stream = body.into_data_stream();
        use futures::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| {
                error!("Failed to read stream chunk: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
            f.write_all(&chunk).await.map_err(|e| {
                error!("Failed to write chunk: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        }

        f.flush()
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let f = f.into_inner();

        // Only fsync if body is not empty (non-trivial write)
        if !is_empty_body {
            f.sync_all()
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        drop(f);

        tokio::fs::rename(&tmp_path, &full_path)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    } else {
        trace!(
            "Partial write detected at offset {}, total_size {:?}",
            offset, total_size
        );
        let mut f = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&full_path)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        // If total_size is known, update file length to match it
        if let Some(ts) = total_size {
            f.set_len(ts)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }

        f.seek(SeekFrom::Start(offset))
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        let mut f = tokio::io::BufWriter::with_capacity(8 * 1024 * 1024, f); // 8MB buffer
        let mut stream = body.into_data_stream();
        use futures::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| {
                error!("Failed to read stream chunk: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
            f.write_all(&chunk).await.map_err(|e| {
                error!("Failed to write chunk: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        }

        f.flush()
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        f.into_inner()
            .sync_all()
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }

    if existed {
        Ok(StatusCode::OK)
    } else {
        Ok(StatusCode::CREATED)
    }
}
