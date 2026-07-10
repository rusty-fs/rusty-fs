use super::path::safe_join_path;
use super::types::FileEntry;
use axum::{
    Json,
    extract::{Extension, Path},
    http::StatusCode,
};
use serde_json::json;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::UNIX_EPOCH;
use tracing::{debug, error};

pub async fn list(
    requested_path: Option<Path<String>>,
    Extension(base_dir): Extension<Arc<String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let requested_path = requested_path.map(|p| p.0).unwrap_or_default();

    let full_path = safe_join_path(&base_dir, &requested_path)?;
    let _requested = requested_path.trim_start_matches('/').to_string();

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

pub async fn mkdir(
    file_path: Path<String>,
    Extension(base_dir): Extension<Arc<String>>,
) -> Result<StatusCode, StatusCode> {
    let requested_raw = file_path.0;
    let full_path = safe_join_path(&base_dir, &requested_raw)?;
    let requested = requested_raw.trim_start_matches('/').to_string();

    debug!("Creating directory: {} -> {:?}", requested, full_path);

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
    let full_path = safe_join_path(&base_dir, &requested_raw)?;
    let requested = requested_raw.trim_start_matches('/').to_string();

    debug!("Deleting path: {} -> {:?}", requested, full_path);

    match fs::metadata(&full_path) {
        Ok(meta) => {
            if meta.is_dir() {
                match tokio::fs::remove_dir(&full_path).await {
                    Ok(_) => Ok(StatusCode::NO_CONTENT),
                    Err(e) => {
                        error!("Failed to delete directory {:?}: {}", full_path, e);
                        if e.kind() == std::io::ErrorKind::DirectoryNotEmpty {
                            Err(StatusCode::UNPROCESSABLE_ENTITY)
                        } else {
                            Err(StatusCode::INTERNAL_SERVER_ERROR)
                        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::fs as tfs;

    #[tokio::test]
    async fn test_mkdir_and_delete() {
        let base_path = std::env::temp_dir().join(format!("filer_test_{}", std::process::id()));
        let _ = tfs::remove_dir_all(&base_path).await; // Clean up before test
        tfs::create_dir_all(&base_path).await.unwrap();

        let base_dir = Arc::new(base_path.to_string_lossy().to_string());

        // Create a directory
        let dir_name = "test_dir".to_string();
        let res = mkdir(Path(dir_name.clone()), Extension(base_dir.clone())).await;
        assert!(res.is_ok());
        assert!(base_path.join(&dir_name).exists());

        // Delete the directory
        let res = delete_path(Path(dir_name.clone()), Extension(base_dir.clone())).await;
        assert!(res.is_ok());
        assert!(!base_path.join(&dir_name).exists());

        // Clean up
        let _ = tfs::remove_dir_all(&base_path).await;
    }
}
