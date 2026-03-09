use serde::{Deserialize, Serialize};
use std::os::unix::raw::mode_t;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: u64,
    pub permissions: mode_t,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryListing {
    pub files: Vec<FileEntry>,
    pub message: String,
}
