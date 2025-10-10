use std::{
    os::unix::{fs::PermissionsExt, raw::mode_t},
    sync::Arc,
    path::PathBuf,
};

use axum::{Extension, Json, extract::Path, http::StatusCode};
use serde::Serialize;
use serde_json::json;
use std::fs;
use std::time::UNIX_EPOCH;
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

    info!("Listing files in: {} -> {:?}", requested, full_path);

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
                Ok(entry) => {
                    match entry.metadata() {
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
                    }
                }
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

// basic handler that responds with a static string
// #[axum::debug_handler]
// async fn get_file() -> Json<serde_json::Value> {
//     // logic to get a file
//     info!("Getting file");

// }

async fn delete_file() -> StatusCode {
    // logic to delete a file
    // StatusCode::NO_CONTENT
    todo!()
}

async fn create_file() -> StatusCode {
    // logic to put a file
    // StatusCode::CREATED
    todo!()
}

#[derive(Serialize)]
struct FileEntry {
    name: String,
    is_dir: bool,
    size: u64,
    modified: Option<u64>,       // Unix timestamp in seconds
    permissions: Option<mode_t>, // File permissions
}
