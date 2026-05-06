use crate::fs::http::FileEntry;
use crate::fs::http::HttpError;
use crate::fs::utils::path;
use crate::fs::utils::runtime;
use libc::{O_APPEND, O_EXCL, O_TRUNC};
use std::ffi::OsStr;
use std::time::{Duration};
use tracing::{debug, error};

use super::RemoteFileSystem;

impl RemoteFileSystem {
    /// Open a file (validate that it exists and allocate a file handle)
    pub fn open(&mut self, ino: u64) -> Result<u64, HttpError> {
        let path = self.get_path_for_inode(ino).ok_or(HttpError::NotFound)?;
        let client = self.http_client.clone();
        let path_clone = path.clone();

        // Validate file exists
        runtime::runtime().block_on(async move { client.get_file_metadata(&path_clone).await })?;

        // Allocate and return a file handle
        let fh = self.fh_manager.alloc_fh();
        Ok(fh)
    }

    /// Read bytes from a file by inode
    pub fn read_bytes(&self, ino: u64, offset: u64, size: usize) -> Result<Vec<u8>, HttpError> {
        let path = self.get_path_for_inode(ino).ok_or(HttpError::NotFound)?;
        let client = self.http_client.clone();
        let path_clone = path.clone();
        runtime::runtime()
            .block_on(async move { client.read_range(&path_clone, offset, size).await })
    }

    /// Create a file
    pub fn create_file(
        &mut self,
        parent: u64,
        name: &OsStr,
        mode: u32,
        umask: u32,
        flags: i32,
    ) -> Result<(Duration, super::FileAttr, u64, u64, u32), HttpError> {
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                return Err(HttpError::Other("invalid name".into()));
            }
        };
        let parent_path = match self.get_path_for_inode(parent) {
            Some(p) => p,
            None => {
                return Err(HttpError::NotFound);
            }
        };
        let full_path = path::join_path(&parent_path, name_str);

        // create a placeholder inode and attr
        let ino = self.get_inode_for_path(&full_path);
        let perm_bits = (mode & !umask) as u16;

        let entry = FileEntry {
            name: name_str.to_string(),
            is_dir: false,
            size: 0,
            permissions: Some(perm_bits as u32),
            modified: Some(1_000_000_000), // Safe default timestamp ~2001-09-09
        };

        if (flags & O_EXCL) != 0 {
            // if file exists, return EEXIST
            if let Ok((_, _)) = self.lookup(parent, name_str) {
                return Err(HttpError::AlreadyExists);
            }
        }

        let attr = self.file_entry_to_attr(&entry, ino);

        let truncate = (flags & O_TRUNC) != 0;
        let _append = (flags & O_APPEND) != 0;
        let client = self.http_client.clone();
        let path_clone = full_path.clone();

        // Check if file exists and get size
        let file_exists = match runtime::runtime()
            .block_on(async move { client.get_file_metadata(&path_clone).await })
        {
            Ok(meta) => {
                if truncate {
                    // Truncate on server: send empty content-range to set size to 0
                    let client2 = self.http_client.clone();
                    let path2 = full_path.clone();
                    if let Err(e) = runtime::runtime().block_on(async move {
                        client2
                            .put_file_stream(&path2, reqwest::Body::from(Vec::<u8>::new()), None, None)
                            .await
                    }) {
                        error!("create (truncate) failed: {}", e);
                        return Err(e);
                    }
                    Some(0)
                } else {
                    // File exists, don't prefetch - let writes handle it with ranges
                    Some(meta.size)
                }
            }
            Err(_) => {
                // File doesn't exist, create empty on server
                let client2 = self.http_client.clone();
                let path2 = full_path.clone();
                if let Err(e) = runtime::runtime().block_on(async move {
                    client2
                        .put_file_stream(&path2, reqwest::Body::from(Vec::<u8>::new()), None, None)
                        .await
                }) {
                    error!("create (new file) failed: {}", e);
                    return Err(e);
                }
                None
            }
        };

        // Allocate file handle without prefetching
        let fh = self.fh_manager.alloc_fh();
        if let Some(state) = self.fh_manager.get_fh_state(fh) {
            // Set file size if known, None for new files
            state.file_size = file_exists;
            state.current_offset = 0;
        }
        Ok((self.config.ttl, attr, 0, fh, 0))
    }

    /// Write bytes to a file
    pub fn write_bytes(
        &mut self,
        ino: u64,
        fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
    ) -> Result<u32, HttpError> {
        let path = self.get_path_for_inode(ino).ok_or(HttpError::NotFound)?;
        let off = offset as u64;

        let needs_new_stream = {
            if let Some(state) = self.fh_manager.get_fh_state(fh) {
                state.tx.is_none() || state.current_offset != off
            } else {
                return Err(HttpError::Other("invalid file handle".into()));
            }
        };

        if needs_new_stream {
            // Drop old sender if any
            if let Some(state) = self.fh_manager.get_fh_state(fh) {
                state.tx = None;
            }

            let (tx, rx) = tokio::sync::mpsc::channel(16);
            let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
            let body = reqwest::Body::wrap_stream(stream);

            let client = self.http_client.clone();
            let path_clone = path.clone();
            let start_offset = off;
            
            runtime::runtime().spawn(async move {
                if let Err(e) = client.put_file_stream(&path_clone, body, Some(start_offset), None).await {
                    tracing::error!("Streaming upload failed for {}: {}", path_clone, e);
                }
            });

            if let Some(state) = self.fh_manager.get_fh_state(fh) {
                state.tx = Some(tx);
                state.current_offset = off;
            }
        }

        // Send data
        {
            let state = self.fh_manager.get_fh_state(fh).unwrap();
            let tx = state.tx.as_ref().unwrap().clone();
            let data_bytes = bytes::Bytes::copy_from_slice(data);
            
            // If send fails, the background task died, probably network error
            if tx.blocking_send(Ok(data_bytes)).is_err() {
                state.tx = None; // force new stream next time
                return Err(HttpError::Network("Upload stream disconnected".into()));
            }

            state.current_offset += data.len() as u64;
            state.dirty = true;

            let new_end = state.current_offset;
            if let Some(size) = state.file_size {
                if new_end > size {
                    state.file_size = Some(new_end);
                }
            } else {
                state.file_size = Some(new_end);
            }
        }

        Ok(data.len() as u32)
    }


    /// Flush a buffer by splitting into multiple chunks and sending in parallel
    pub fn setattr(
        &mut self,
        ino: u64,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
    ) -> Result<super::FileAttr, HttpError> {
        // For now we just support size changes (truncate)
        let path = match self.get_path_for_inode(ino) {
            Some(p) => p,
            None => {
                error!("setattr failed: inode {} not found", ino);
                return Err(HttpError::NotFound);
            }
        };

        if let Some(new_size) = size {
            debug!("truncating file {} to size {}", path, new_size);

            // For truncate, we need to send the new size to the server
            // If new_size is 0, send empty file
            // If new_size > 0, we need to either pad with zeros or get the existing data
            let current_data = if new_size == 0 {
                Vec::new()
            } else {
                // Read what we need up to new_size
                // For large truncations, we don't want to load entire file
                // Instead, let server handle it by sending a special truncate marker
                // For now, we'll send an empty body with Content-Range to signal truncate
                Vec::new()
            };

            let client = self.http_client.clone();
            let path_clone = path.clone();
            let data_clone = current_data;

            match runtime::runtime().block_on(async move {
                // Send with size hint so server knows this is a size-setting operation
                client
                    .put_file_stream(&path_clone, reqwest::Body::from(data_clone), Some(0), Some(new_size))
                    .await
            }) {
                Ok(_) => {
                    // After truncation, get updated attributes
                    return self.getattr(ino);
                }
                Err(e) => {
                    error!("setattr failed during truncate upload: {}", e);
                    return Err(e);
                }
            }
        }
        Ok(self.getattr(ino)?)
    }
}
