use super::path::safe_join_path;
use super::range::parse_range_header;
use axum::{
    body::Body,
    extract::{Extension, Path},
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{ACCEPT_RANGES, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE},
    },
    response::{IntoResponse, Response},
};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs::File as TokioFile;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;
use tracing::debug;

async fn stream_file(
    full_path: PathBuf,
    range_header: Option<&HeaderValue>,
) -> Result<Response, StatusCode> {
    let meta = tokio::fs::metadata(&full_path)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    let file_size = meta.len();

    let file = TokioFile::open(&full_path)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;

    let range = parse_range_header(range_header)?;
    if range.is_none() {
        let stream = ReaderStream::new(file);
        let mut resp_headers = HeaderMap::new();
        resp_headers.insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
        resp_headers.insert(
            CONTENT_LENGTH,
            HeaderValue::from_str(&file_size.to_string())
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        );
        resp_headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        );

        // Return a concrete Response to avoid Rust 2024 impl Trait lifetime captures
        return Ok((StatusCode::OK, resp_headers, Body::from_stream(stream)).into_response());
    }

    let (start, end_opt) = range.unwrap();

    let max_end = file_size.saturating_sub(1);
    let mut end = end_opt.unwrap_or(max_end);

    if end > max_end {
        end = max_end;
    }

    if start >= file_size || start > end {
        return Err(StatusCode::RANGE_NOT_SATISFIABLE);
    }
    let read_len = end.saturating_sub(start).saturating_add(1);

    // Log range request for performance monitoring
    tracing::trace!(
        "Range request: {}-{}/{} ({} bytes)",
        start,
        end,
        file_size,
        read_len
    );

    let mut file = file;
    file.seek(std::io::SeekFrom::Start(start))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let reader = tokio::io::BufReader::with_capacity(8 * 1024 * 1024, file.take(read_len));
    let stream = ReaderStream::new(reader);

    let content_range = format!("bytes {}-{}/{}", start, end, file_size);
    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    resp_headers.insert(
        CONTENT_RANGE,
        HeaderValue::from_str(&content_range).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    );
    resp_headers.insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&read_len.to_string())
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    );
    resp_headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );

    Ok((
        StatusCode::PARTIAL_CONTENT,
        resp_headers,
        Body::from_stream(stream),
    )
        .into_response())
}

pub async fn read(
    file_path: Path<String>,
    Extension(base_dir): Extension<Arc<String>>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    let requested_raw = file_path.0;
    let full_path = safe_join_path(&base_dir, &requested_raw)?;
    let requested = requested_raw.trim_start_matches('/').to_string();

    debug!("Reading file: {} -> {:?}", requested, full_path);

    // Pass only the relevant header, not the whole map
    stream_file(full_path, headers.get("range")).await
}
