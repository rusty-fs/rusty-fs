use axum::http::StatusCode;
use std::path::PathBuf;

pub(super) fn safe_join_path(base_dir: &str, requested: &str) -> Result<PathBuf, StatusCode> {
    let mut full_path = PathBuf::from(base_dir.trim_end_matches('/'));
    for component in std::path::Path::new(requested).components() {
        match component {
            std::path::Component::Normal(c) => full_path.push(c),
            std::path::Component::ParentDir => return Err(StatusCode::BAD_REQUEST),
            _ => {} // Ignore RootDir, CurDir, Prefix to prevent absolute path escapes
        }
    }
    Ok(full_path)
}
