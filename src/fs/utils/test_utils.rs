/// Test utilities for FUSE filesystem testing
use crate::fs::http::{FileEntry, HttpBackend, HttpError};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// A fake HTTP backend for testing that stores data in-memory
pub struct FakeBackend {
    pub listing: Mutex<HashMap<String, Vec<FileEntry>>>,
    pub metadata: Mutex<HashMap<String, FileEntry>>,
    pub contents: Mutex<HashMap<String, Vec<u8>>>,
}

impl FakeBackend {
    /// Create a new fake backend with predefined test data
    pub fn new() -> Self {
        let mut listing_map = HashMap::new();
        let root_children = vec![
            FileEntry {
                name: "f.txt".to_string(),
                is_dir: false,
                size: 10,
                modified: Some(
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                ),
                permissions: Some(0o644),
            },
            FileEntry {
                name: "dir".to_string(),
                is_dir: true,
                size: 0,
                modified: Some(
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                ),
                permissions: Some(0o755),
            },
        ];
        listing_map.insert("/".to_string(), root_children.clone());

        listing_map.insert(
            "/dir".to_string(),
            vec![FileEntry {
                name: "inner.txt".to_string(),
                is_dir: false,
                size: 5,
                modified: Some(
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                ),
                permissions: Some(0o600),
            }],
        );

        // metadata map for direct lookup
        let mut metadata_map = HashMap::new();
        metadata_map.insert(
            "/f.txt".to_string(),
            FileEntry {
                name: "f.txt".to_string(),
                is_dir: false,
                size: 10,
                modified: Some(
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                ),
                permissions: Some(0o644),
            },
        );
        metadata_map.insert(
            "/dir".to_string(),
            FileEntry {
                name: "dir".to_string(),
                is_dir: true,
                size: 0,
                modified: Some(
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                ),
                permissions: Some(0o755),
            },
        );
        metadata_map.insert(
            "/dir/inner.txt".to_string(),
            FileEntry {
                name: "inner.txt".to_string(),
                is_dir: false,
                size: 5,
                modified: Some(
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                ),
                permissions: Some(0o600),
            },
        );

        // contents map for read_range
        let mut contents_map = HashMap::new();
        contents_map.insert("/f.txt".to_string(), b"0123456789".to_vec()); // 10 bytes
        contents_map.insert("/dir/inner.txt".to_string(), b"abcde".to_vec()); // 5 bytes

        Self {
            listing: Mutex::new(listing_map),
            metadata: Mutex::new(metadata_map),
            contents: Mutex::new(contents_map),
        }
    }
}

impl Default for FakeBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HttpBackend for FakeBackend {
    async fn list_directory(&self, path: &str) -> Result<Vec<FileEntry>, HttpError> {
        let listing = self.listing.lock().unwrap();
        if let Some(vec) = listing.get(path) {
            Ok(vec.clone())
        } else {
            Err(HttpError::NotFound)
        }
    }

    async fn get_file_metadata(&self, path: &str) -> Result<FileEntry, HttpError> {
        let metadata = self.metadata.lock().unwrap();
        if let Some(entry) = metadata.get(path) {
            Ok(entry.clone())
        } else {
            Err(HttpError::NotFound)
        }
    }

    async fn read_range(
        &self,
        path: &str,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>, HttpError> {
        let contents = self.contents.lock().unwrap();
        if let Some(data) = contents.get(path) {
            let off = offset as usize;
            if off >= data.len() {
                return Ok(vec![]);
            }
            let end = std::cmp::min(off + length, data.len());
            Ok(data[off..end].to_vec())
        } else {
            Err(HttpError::NotFound)
        }
    }

    async fn create_directory(&self, path: &str) -> Result<(), HttpError> {
        // Create directory in listing map
        let mut listing = self.listing.lock().unwrap();
        listing.insert(path.to_string(), Vec::new());
        Ok(())
    }

    async fn delete_path(&self, path: &str) -> Result<(), HttpError> {
        // Remove from listing, metadata, contents
        let mut listing = self.listing.lock().unwrap();
        let mut metadata = self.metadata.lock().unwrap();
        let mut contents = self.contents.lock().unwrap();
        listing.remove(path);
        metadata.remove(path);
        contents.remove(path);
        Ok(())
    }

    async fn put_file_stream(
        &self,
        path: &str,
        data: Vec<u8>,
        _offset: Option<u64>,
        _total_size: Option<u64>,
    ) -> Result<(), HttpError> {
        let mut contents = self.contents.lock().unwrap();
        contents.insert(path.to_string(), data.clone());
        let mut metadata = self.metadata.lock().unwrap();
        metadata.insert(
            path.to_string(),
            FileEntry {
                name: path.split('/').last().unwrap_or("").to_string(),
                is_dir: false,
                size: data.len() as u64,
                modified: Some(
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                ),
                permissions: Some(0o644),
            },
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fake_backend_creation() {
        let backend = FakeBackend::new();
        let listing = backend.listing.lock().unwrap();
        assert!(listing.contains_key("/"));
        assert!(listing.contains_key("/dir"));
    }
}
