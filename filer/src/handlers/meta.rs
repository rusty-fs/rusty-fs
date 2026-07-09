use super::path::safe_join_path;
use super::types::FileEntry;
use axum::{
    Json,
    extract::{Extension, Path},
    http::StatusCode,
};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::UNIX_EPOCH;
use tracing::{debug, error, info};

pub async fn meta(
    file_path: Path<String>,
    Extension(base_dir): Extension<Arc<String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let requested_raw = file_path.0;
    let full_path = safe_join_path(&base_dir, &requested_raw)?;
    let requested = requested_raw.trim_start_matches('/').to_string();

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

#[derive(serde::Deserialize)]
pub struct UpdateMetaRequest {
    new_name: Option<String>,
    mode: Option<u32>,
    mtime: Option<u64>,
    atime: Option<u64>,
}

pub async fn update_meta(
    file_path: Path<String>,
    Extension(base_dir): Extension<Arc<String>>,
    Json(payload): Json<UpdateMetaRequest>,
) -> Result<StatusCode, StatusCode> {
    let requested_raw = file_path.0;
    let full_path = safe_join_path(&base_dir, &requested_raw)?;
    let requested = requested_raw.trim_start_matches('/').to_string();
    if requested.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    if !full_path.exists() {
        return Err(StatusCode::NOT_FOUND);
    }

    // Process mode updates
    if let Some(mode) = payload.mode {
        debug!("Setting permissions {:o} for {:?}", mode, full_path);
        if let Ok(mut perms) = fs::metadata(&full_path).map(|m| m.permissions()) {
            perms.set_mode(mode);
            if let Err(e) = fs::set_permissions(&full_path, perms) {
                error!("Failed to set permissions on {:?}: {}", full_path, e);
            }
        }
    }

    // Process timestamp updates
    if payload.atime.is_some() || payload.mtime.is_some() {
        if let Ok(meta) = fs::metadata(&full_path) {
            let atime = payload
                .atime
                .map(|t| filetime::FileTime::from_unix_time(t as i64, 0))
                .unwrap_or_else(|| filetime::FileTime::from_last_access_time(&meta));
            let mtime = payload
                .mtime
                .map(|t| filetime::FileTime::from_unix_time(t as i64, 0))
                .unwrap_or_else(|| filetime::FileTime::from_last_modification_time(&meta));

            debug!(
                "Setting timestamps (atime: {:?}, mtime: {:?}) for {:?}",
                atime, mtime, full_path
            );
            if let Err(e) = filetime::set_file_times(&full_path, atime, mtime) {
                error!("Failed to set timestamps on {:?}: {}", full_path, e);
            }
        }
    }

    // Process rename if new_name is provided
    if let Some(dst_raw) = payload.new_name {
        let full_dst_path = safe_join_path(&base_dir, &dst_raw)?;
        let dst_requested = dst_raw.trim_start_matches('/').to_string();
        if dst_requested.is_empty() {
            return Err(StatusCode::BAD_REQUEST);
        }

        debug!("Renaming {} to {}", requested, dst_requested);

        if let Some(parent) = full_dst_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        return match tokio::fs::rename(&full_path, &full_dst_path).await {
            Ok(_) => Ok(StatusCode::NO_CONTENT),
            Err(e) => {
                error!(
                    "Failed to rename {:?} to {:?}: {}",
                    full_path, full_dst_path, e
                );
                Err(StatusCode::INTERNAL_SERVER_ERROR)
            }
        };
    }

    Ok(StatusCode::OK)
}
