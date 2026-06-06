use axum::{
    Json,
    body::Body,
    extract::{Extension, Path},
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{ACCEPT_RANGES, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE},
    },
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use serde::Serialize;
use serde_json::json;
use std::fs;
use std::io::{Seek, SeekFrom, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::UNIX_EPOCH;
use tokio::fs::File as TokioFile;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio_util::io::ReaderStream;
use tracing::{debug, error, info, trace};

pub async fn list(
    requested_path: Option<Path<String>>,
    Extension(base_dir): Extension<Arc<String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let requested_path = requested_path.map(|p| p.0).unwrap_or_default();

    if requested_path.contains("..") {
        return Err(StatusCode::BAD_REQUEST);
    }

    let requested = requested_path.trim_start_matches('/').to_string();
    let full_path = if requested.is_empty() {
        PathBuf::from(base_dir.trim_end_matches('/'))
    } else {
        PathBuf::from(base_dir.trim_end_matches('/')).join(&requested)
    };

    let entries = fs::read_dir(&full_path)
        .map_err(|e| {
            error!("Failed to read directory {:?}: {}", full_path, e);
            StatusCode::INTERNAL_SERVER_ERROR
        })
        .ok();

    let mut file_list = Vec::new();
    if let Some(resolved) = entries {
        for entry in resolved {
            match entry {
                Ok(entry) => match entry.metadata() {
                    Ok(meta) => {
                        let modified_time = meta
                            .modified()
                            .ok()
                            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                            .map(|duration| duration.as_secs());

                        let file_type = meta.file_type();
                        let is_dir = file_type.is_dir();
                        let size = meta.len();
                        let permissions = meta.permissions();
                        file_list.push(FileEntry {
                            name: entry.file_name().to_string_lossy().into_owned(),
                            is_dir,
                            size,
                            modified: modified_time,
                            permissions: Some(permissions.mode()),
                        });
                    }
                    Err(e) => {
                        error!("Failed to get metadata for entry in {:?}: {}", full_path, e);
                    }
                },
                Err(e) => {
                    error!("Failed to read entry in {:?}: {}", full_path, e);
                    return Err(StatusCode::INTERNAL_SERVER_ERROR);
                }
            }
        }
    } else {
        return Err(StatusCode::NOT_FOUND);
    }

    Ok(Json(json!({
        "files": file_list,
        "message": "Files listed successfully"
    })))
}

pub async fn meta(
    file_path: Path<String>,
    Extension(base_dir): Extension<Arc<String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let requested_raw = file_path.0;
    if requested_raw.contains("..") {
        return Err(StatusCode::BAD_REQUEST);
    }

    let requested = requested_raw.trim_start_matches('/').to_string();
    let full_path = if requested.is_empty() {
        PathBuf::from(base_dir.trim_end_matches('/'))
    } else {
        PathBuf::from(base_dir.trim_end_matches('/')).join(&requested)
    };

    info!("Getting metadata for: {} -> {:?}", requested, full_path);

    match fs::metadata(&full_path) {
        Ok(meta) => {
            let modified_time = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs());
            let file_type = meta.file_type();
            let is_dir = file_type.is_dir();
            let size = meta.len();
            let permissions = meta.permissions();

            let entry = FileEntry {
                name: requested,
                is_dir,
                size,
                modified: modified_time,
                permissions: Some(permissions.mode()),
            };

            Ok(Json(serde_json::to_value(&entry).unwrap()))
        }
        Err(e) => {
            debug!("Failed to get metadata for {:?}: {}", full_path, e);
            Err(StatusCode::NOT_FOUND)
        }
    }
}

fn parse_range_header(
    range_hdr: Option<&HeaderValue>,
) -> Result<Option<(u64, Option<u64>)>, StatusCode> {
    let hv = match range_hdr {
        Some(h) => h,
        None => return Ok(None),
    };
    let s = hv.to_str().map_err(|_| StatusCode::BAD_REQUEST)?;
    if !s.starts_with("bytes=") {
        return Err(StatusCode::BAD_REQUEST);
    }
    let spec = &s["bytes=".len()..];
    if spec.contains(',') {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut parts = spec.splitn(2, '-');
    let start_str = parts.next().unwrap_or("");
    let end_str = parts.next().unwrap_or("");
    if start_str.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let start = u64::from_str(start_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    let end_opt = if end_str.is_empty() {
        None
    } else {
        Some(u64::from_str(end_str).map_err(|_| StatusCode::BAD_REQUEST)?)
    };
    Ok(Some((start, end_opt)))
}

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
    if requested_raw.contains("..") {
        return Err(StatusCode::BAD_REQUEST);
    }

    let requested = requested_raw.trim_start_matches('/').to_string();
    let full_path = if requested.is_empty() {
        PathBuf::from(base_dir.trim_end_matches('/'))
    } else {
        PathBuf::from(base_dir.trim_end_matches('/')).join(&requested)
    };

    debug!("Reading file: {} -> {:?}", requested, full_path);

    // Pass only the relevant header, not the whole map
    stream_file(full_path, headers.get("range")).await
}

pub async fn mkdir(
    file_path: Path<String>,
    Extension(base_dir): Extension<Arc<String>>,
) -> Result<StatusCode, StatusCode> {
    let requested_raw = file_path.0;
    if requested_raw.contains("..") {
        return Err(StatusCode::BAD_REQUEST);
    }

    let requested = requested_raw.trim_start_matches('/').to_string();
    let full_path = if requested.is_empty() {
        PathBuf::from(base_dir.trim_end_matches('/'))
    } else {
        PathBuf::from(base_dir.trim_end_matches('/')).join(&requested)
    };

    info!("Creating directory: {} -> {:?}", requested, full_path);

    match fs::create_dir_all(&full_path) {
        Ok(_) => Ok(StatusCode::CREATED),
        Err(e) => {
            error!("Failed to create directory {:?}: {}", full_path, e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

pub async fn delete_path(
    file_path: Path<String>,
    Extension(base_dir): Extension<Arc<String>>,
) -> Result<StatusCode, StatusCode> {
    let requested_raw = file_path.0;
    if requested_raw.contains("..") {
        return Err(StatusCode::BAD_REQUEST);
    }

    let requested = requested_raw.trim_start_matches('/').to_string();
    let full_path = if requested.is_empty() {
        PathBuf::from(base_dir.trim_end_matches('/'))
    } else {
        PathBuf::from(base_dir.trim_end_matches('/')).join(&requested)
    };

    info!("Deleting path: {} -> {:?}", requested, full_path);

    match fs::metadata(&full_path) {
        Ok(meta) => {
            if meta.is_dir() {
                match fs::remove_dir_all(&full_path) {
                    Ok(_) => Ok(StatusCode::NO_CONTENT),
                    Err(e) => {
                        error!("Failed to delete directory {:?}: {}", full_path, e);
                        Err(StatusCode::INTERNAL_SERVER_ERROR)
                    }
                }
            } else {
                match fs::remove_file(&full_path) {
                    Ok(_) => Ok(StatusCode::NO_CONTENT),
                    Err(e) => {
                        error!("Failed to delete file {:?}: {}", full_path, e);
                        Err(StatusCode::INTERNAL_SERVER_ERROR)
                    }
                }
            }
        }
        Err(e) => {
            error!("Failed to get metadata for {:?}: {}", full_path, e);
            Err(StatusCode::NOT_FOUND)
        }
    }
}

pub async fn put_file(
    file_path: Path<String>,
    Extension(base_dir): Extension<Arc<String>>,
    headers: HeaderMap,
    body: Body,
) -> Result<StatusCode, StatusCode> {
    let requested_raw = file_path.0;
    if requested_raw.contains("..") {
        return Err(StatusCode::BAD_REQUEST);
    }

    let requested = requested_raw.trim_start_matches('/').to_string();
    if requested.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let full_path = PathBuf::from(base_dir.trim_end_matches('/')).join(&requested);
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
        
        f.flush().await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let mut f = f.into_inner();
        
        // Only fsync if body is not empty (non-trivial write)
        if !is_empty_body {
            f.sync_all().await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        drop(f);

        tokio::fs::rename(&tmp_path, &full_path).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    } else {
        trace!("Partial write detected at offset {}, total_size {:?}", offset, total_size);
        let mut f = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(&full_path)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        // If total_size is known, update file length to match it
        if let Some(ts) = total_size {
            f.set_len(ts).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }

        f.seek(SeekFrom::Start(offset)).await
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
        
        f.flush().await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        f.into_inner().sync_all().await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }

    if existed {
        Ok(StatusCode::OK)
    } else {
        Ok(StatusCode::CREATED)
    }
}

fn parse_content_range(value: &str) -> Result<(u64, bool, Option<u64>), ()> {
    if !value.starts_with("bytes ") {
        return Err(());
    }

    let parts: Vec<&str> = value[6..].split('/').collect();
    if parts.len() != 2 {
        return Err(());
    }

    let range_parts: Vec<&str> = parts[0].split('-').collect();
    if range_parts.len() != 2 {
        return Err(());
    }

    let start: u64 = range_parts[0].parse().map_err(|_| ())?;
    let total_size: Option<u64> = if parts[1] == "*" {
        None
    } else {
        Some(parts[1].parse().map_err(|_| ())?)
    };
    Ok((start, false, total_size))
}

#[derive(Serialize)]
struct FileEntry {
    name: String,
    is_dir: bool,
    size: u64,
    modified: Option<u64>,
    permissions: Option<u32>,
}

#[derive(serde::Deserialize)]
pub struct RenameRequest {
    new_name: String,
}

pub async fn rename(
    file_path: Path<String>,
    Extension(base_dir): Extension<Arc<String>>,
    Json(payload): Json<RenameRequest>,
) -> Result<StatusCode, StatusCode> {
    let requested_raw = file_path.0;
    if requested_raw.contains("..") {
        return Err(StatusCode::BAD_REQUEST);
    }
    let requested = requested_raw.trim_start_matches('/').to_string();
    if requested.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let full_path = PathBuf::from(base_dir.trim_end_matches('/')).join(&requested);

    let dst_raw = payload.new_name;
    if dst_raw.contains("..") {
        return Err(StatusCode::BAD_REQUEST);
    }
    let dst_requested = dst_raw.trim_start_matches('/').to_string();
    if dst_requested.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let full_dst_path = PathBuf::from(base_dir.trim_end_matches('/')).join(&dst_requested);

    debug!("Renaming {} to {}", requested, dst_requested);

    if let Some(parent) = full_dst_path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    match tokio::fs::rename(&full_path, &full_dst_path).await {
        Ok(_) => Ok(StatusCode::NO_CONTENT),
        Err(e) => {
            error!("Failed to rename {:?} to {:?}: {}", full_path, full_dst_path, e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[cfg(test)]
mod tests;
