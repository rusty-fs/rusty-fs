use axum::http::HeaderMap;
use axum::{Extension, Json, extract::Path, http::StatusCode};
use serde::Serialize;
use serde_json::json;
use std::fs;
use std::io::{Seek, SeekFrom, Write};
use std::time::UNIX_EPOCH;
use std::{
    os::unix::{fs::PermissionsExt, raw::mode_t},
    path::PathBuf,
    sync::Arc,
};
use tracing::{error, info};

pub async fn list(
    requested_path: Option<Path<String>>,
    Extension(base_dir): Extension<Arc<String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let requested_path = requested_path.map(|p| p.0).unwrap_or_default();

    // Reject obvious traversal attempts
    if requested_path.contains("..") {
        return Err(StatusCode::BAD_REQUEST);
    }

    let requested = requested_path.trim_start_matches('/').to_string();
    let full_path = if requested.is_empty() {
        PathBuf::from(base_dir.trim_end_matches('/'))
    } else {
        PathBuf::from(base_dir.trim_end_matches('/')).join(&requested)
    };

    // info!("Listing files in: {} -> {:?}", requested, full_path);

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
                            permissions: Some(permissions.mode() as mode_t),
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
                permissions: Some(permissions.mode() as mode_t),
            };

            Ok(Json(serde_json::to_value(&entry).unwrap()))
        }
        Err(e) => {
            error!("Failed to get metadata for {:?}: {}", full_path, e);
            Err(StatusCode::NOT_FOUND)
        }
    }
}

pub async fn read(
    file_path: Path<String>,
    Extension(base_dir): Extension<Arc<String>>,
) -> Result<(StatusCode, Vec<u8>), StatusCode> {
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

    info!("Reading file: {} -> {:?}", requested, full_path);

    match fs::read(&full_path) {
        Ok(content) => Ok((StatusCode::OK, content)),
        Err(e) => {
            error!("Failed to read file {:?}: {}", full_path, e);
            Err(StatusCode::NOT_FOUND)
        }
    }
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
    body: axum::body::Bytes,
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
    info!("Writing file: {} -> {:?}", requested, full_path);

    tracing::debug!("Headers: {:?}", headers);
    tracing::debug!("Content-Range header: {:?}", headers.get("content-range"));
    tracing::debug!("Body length: {}", body.len());

    if let Some(parent) = full_path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            error!("Failed to create parent dirs {:?}: {}", parent, e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    }

    let existed = full_path.exists();

    let (offset, should_truncate) = if let Some(range) = headers.get("content-range") {
        parse_content_range(range.to_str().map_err(|_| StatusCode::BAD_REQUEST)?)
            .map_err(|_| StatusCode::BAD_REQUEST)?
    } else {
        (0, true)
    };

    if should_truncate {
        info!("Full write detected, truncating file if it exists");
        // Full replacement: use temporary file for atomicity
        let mut tmp_path = full_path.clone();
        tmp_path.set_extension(format!(
            "{}.filer_tmp",
            tmp_path.extension().and_then(|s| s.to_str()).unwrap_or("")
        ));

        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp_path)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        f.write_all(&body)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        f.sync_all()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        drop(f);

        std::fs::rename(&tmp_path, &full_path)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    } else {
        info!("Partial write detected at offset {}", offset);
        // Partial write: write directly to target file
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(&full_path)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        f.seek(SeekFrom::Start(offset))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        f.write_all(&body)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        f.sync_all()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }

    if existed {
        Ok(StatusCode::OK)
    } else {
        Ok(StatusCode::CREATED)
    }
}

fn parse_content_range(value: &str) -> Result<(u64, bool), ()> {
    // "bytes 1024-2047/5000" -> offset=1024, truncate=false
    // Assenza header -> offset=0, truncate=true

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
    Ok((start, false))
}

#[derive(Serialize)]
struct FileEntry {
    name: String,
    is_dir: bool,
    size: u64,
    modified: Option<u64>,       // Unix timestamp in seconds
    permissions: Option<mode_t>, // File permissions
}
