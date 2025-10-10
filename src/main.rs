// # Test che devi poter fare:
// curl http://localhost:8080/list/
// curl http://localhost:8080/files/file1.txt
// curl -X PUT -d "Hello World" http://localhost:8080/files/new.txt
// curl -X DELETE http://localhost:8080/files/new.txt

use std::{fs::FileType, os::unix::{fs::PermissionsExt, raw::mode_t}, sync::Arc};

use axum::{
    extract::Path, http::{request, StatusCode}, routing::{delete, get, put}, Extension, Json, Router
};
use serde::{Serialize, Serializer};
use serde::ser::SerializeStruct;
use serde_json::json;
use std::fs;
use std::io;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, error, info, warn};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() {
    // initialize tracing
    tracing_subscriber::registry().with(fmt::layer()).init();

    // read base directory from environment
    let base_dir: String = if let Ok(val) = std::env::var("BASE_DIR") {
        debug!("Base directory set to: {}", val);
        val
    } else {
        // return an error if BASE_DIR is not set
        error!("BASE_DIR environment variable is not set.");
        std::process::exit(1);
    };
    let shared_base_dir = Arc::new(base_dir);

    // build our application with a route
    let app = Router::new()
        .route("/list", get(list)) 
        .route("/list/", get(list))
        .route("/list/{*path}", get(list))
        // .route("/list", get(list))
        .route("/meta/{*file_path}", get(meta))
        .layer(Extension(shared_base_dir));
        // .route("/files/:filename", get(get_file));
    // .route("/files/:filename", put(create_file))
    // .route("/files/:filename", delete(delete_file));

    // run our app with hyper, listening globally on port 3000
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

// handler to list files
// #[axum::debug_handler]
// async fn list(Extension(base_dir): Extension<Arc<String>>) -> Json<serde_json::Value> {
//     // logic to list files
//     info!("Listing files in the directory: {}", base_dir);
//     // List files logic

//     let entries = fs::read_dir(base_dir.as_str())
//         .map_err(|e| {
//             error!("Failed to read directory: {}", e);
//             StatusCode::INTERNAL_SERVER_ERROR
//         })
//         .ok();

//     let mut file_list = Vec::new();
//     if let Some(resolved) = entries {
//         for entry in resolved {
//             match entry {
//                 Ok(entry) => {
//                     let metadata = entry.metadata();
//                     match metadata {
//                         Ok(meta) => {
//                             let modified_time = meta
//                                 .modified()
//                                 .ok()
//                                 .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
//                                 .map(|duration| duration.as_secs());


//                             let file_type = meta.file_type();
//                             let is_dir = file_type.is_dir();
//                             let size = meta.len();
//                             let permissions = meta.permissions();
//                             file_list.push(FileEntry {
//                                 name: entry.file_name().to_string_lossy().into_owned(),
//                                 is_dir: is_dir,
//                                 size: size,
//                                 modified: modified_time,
//                                 permissions: Some(permissions.mode() as mode_t),     
//                             });
//                         }
//                         Err(e) => {
//                             error!("Failed to get metadata for entry: {}", e);
//                         }
//                     }
//                 }
//                 Err(e) => {
//                     error!("Failed to read entry: {}", e);
//                     return Json(json!({
//                         "error": "Failed to read directory entries"
//                     }));
//                 }
//             }
//         }
//     }

//     // Encode the response as a json
//     Json(json!({
//         "files": file_list,
//         "message": "Files listed successfully"
//     }))
// }


async fn list(requested_path: Option<Path<String>>, base_dir: Extension<Arc<String>>) -> Json<serde_json::Value> {
    let requested_path = requested_path.map(|p| p.0).unwrap_or_default();

    let requested = requested_path.trim_start_matches('/').to_string();
    let full_path = if requested.is_empty() {
        base_dir.trim_end_matches('/').to_string()
    } else {
        format!("{}/{}", base_dir.trim_end_matches('/'), requested)
    };

    info!("Listing files in: {} -> {}", requested, full_path);

    let entries = fs::read_dir(&full_path)
        .map_err(|e| {
            error!("Failed to read directory {}: {}", full_path, e);
            StatusCode::INTERNAL_SERVER_ERROR
        })
        .ok();

    let mut file_list = Vec::new();
    if let Some(resolved) = entries {
        for entry in resolved {
            match entry {
                Ok(entry) => {
                    let metadata = entry.metadata();
                    match metadata {
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
                            error!("Failed to get metadata for entry in {}: {}", full_path, e);
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to read entry in {}: {}", full_path, e);
                    return Json(json!({
                        "error": "Failed to read directory entries"
                    }));
                }
            }
        }
    } else {
        return Json(json!({
            "error": "Failed to read directory"
        }));
    }

    // Encode the response as a json
    Json(json!({
        "files": file_list,
        "message": "Files listed successfully"
    }))
}



async fn meta(file_path: Path<String>, base_dir: Extension<Arc<String>>) -> Json<serde_json::Value> {
    let requested = file_path.trim_start_matches('/').to_string();
    let full_path = if requested.is_empty() {
        base_dir.trim_end_matches('/').to_string()
    } else {
        format!("{}/{}", base_dir.trim_end_matches('/'), requested)
    };

    info!("Getting metadata for: {} -> {}", requested, full_path);

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

            Json(serde_json::to_value(&entry).unwrap())
        }
        Err(e) => {
            error!("Failed to get metadata for {}: {}", full_path, e);
            Json(json!({ "error": "Failed to get file metadata" }))
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
    StatusCode::NO_CONTENT
}

async fn create_file() -> StatusCode {
    // logic to put a file
    StatusCode::CREATED
}

#[derive(Serialize)]
struct FileEntry {
    name: String,
    is_dir: bool,
    size: u64,
    modified: Option<u64>, // Unix timestamp in seconds
    permissions: Option<mode_t>, // File permissions
}
