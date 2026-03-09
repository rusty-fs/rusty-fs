use crate::fs::http::HttpError;
use crate::fs::http::FileEntry;
use crate::fs::utils::path;
use crate::fs::utils::runtime;
use libc::{O_APPEND, O_EXCL, O_TRUNC};
use std::ffi::OsStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::error;

use super::RemoteFileSystem;

impl RemoteFileSystem {
    /// Open a file (validate that it exists and allocate a file handle)
    pub fn open(&mut self, ino: u64) -> Result<u64, HttpError> {
        let path = self.get_path_for_inode(ino).ok_or(HttpError::NotFound)?;
        let client = self.http_client.clone();
        let path_clone = path.clone();
        
        // Validate file exists
        runtime::runtime()
            .block_on(async move { client.get_file_metadata(&path_clone).await })?;
        
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
            permissions: perm_bits.into(),
            modified: 0,
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

        match runtime::runtime()
            .block_on(async move { client.get_file_metadata(&path_clone).await })
        {
            Ok(meta) => {
                if truncate {
                    // truncate on server now
                    let client2 = self.http_client.clone();
                    let path2 = full_path.clone();
                    if let Err(e) = runtime::runtime().block_on(async move {
                        client2
                            .put_file_stream(&path2, Vec::new(), None, None)
                            .await
                    }) {
                        error!("create (truncate) failed: {}", e);
                        return Err(e);
                    }
                } else {
                    // Prefetch whole file so writes can patch existing data
                    let client2 = self.http_client.clone();
                    let path2 = full_path.clone();
                    let size = meta.size as usize;
                    match runtime::runtime()
                        .block_on(async move { client2.read_range(&path2, 0, size).await })
                    {
                        Ok(data) => {
                            let fh = self.fh_manager.alloc_fh();
                            if let Some(state) = self.fh_manager.get_fh_state(fh) {
                                state.buf = data;
                            }
                            return Ok((self.config.ttl, attr, 0, fh, 0));
                        }
                        Err(e) => {
                            error!("create prefetch failed: {}", e);
                            return Err(e);
                        }
                    }
                }
            }
            Err(_) => {
                // file doesn't exist -> create empty file on server now
                let client2 = self.http_client.clone();
                let path2 = full_path.clone();
                if let Err(e) = runtime::runtime().block_on(async move {
                    client2
                        .put_file_stream(&path2, Vec::new(), None, None)
                        .await
                }) {
                    error!("create (new file) failed: {}", e);
                    return Err(e);
                }
            }
        }

        let fh = self.fh_manager.alloc_fh();
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
        match self.fh_manager.get_fh_state(fh) {
            Some(state) => {
                let off = offset as usize;

                // Expand buffer if needed
                if state.buf.len() < off + data.len() {
                    state.buf.resize(off + data.len(), 0);
                }

                // Write to local buffer
                state.buf[off..off + data.len()].copy_from_slice(data);

                // Mark as dirty
                state.dirty = true;
                state.last_write_offset = Some(off);

                if state.buf.len() > self.config.max_buffer_size {
                    let path = match self.get_path_for_inode(ino) {
                        Some(p) => p,
                        None => {
                            error!("write failed: inode {} not found", ino);
                            return Err(HttpError::NotFound);
                        }
                    };

                    let data_to_flush = {
                        let state_ref = self.fh_manager.get_fh_state(fh).unwrap();
                        let taken = std::mem::take(&mut state_ref.buf);
                        state_ref.dirty = false;
                        state_ref.last_write_offset = None;
                        taken
                    };
                 
                    self.flush_buffer_owned(path, data_to_flush)?;
                }
                Ok(data.len() as u32)
            }
            None => Err(HttpError::Other("invalid file handle".into())),
        }
    }

    fn flush_buffer_owned(&self, path: String, data: Vec<u8>) -> Result<(), HttpError> {
        if data.is_empty() {
            return Ok(());
        }

        let client = self.http_client.clone();
        let path_clone = path.clone();
        let total_size = Some(data.len() as u64);

        let result = runtime::runtime()
            .block_on(async move {
                client
                    .put_file_stream(&path_clone, data, None, total_size)
                    .await
            });

        match result {
            Ok(_) => Ok(()),
            Err(e) => {
                error!("flush failed during upload: {}", e);
                Err(e)
            }
        }
    }

    /// Set file attributes (e.g., truncate by size)
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
            error!("truncating file {} to size {}", path, new_size);

            let current_data;
            if new_size == 0 {
                current_data = Vec::new();
            } else {
                // Read existing data up to new_size
                current_data = self.read_bytes(ino, 0, new_size as usize)?;
            }

            let client = self.http_client.clone();
            let path_clone = path.clone();
            let data_clone = current_data;

            match runtime::runtime().block_on(async move {
                client
                    .put_file_stream(&path_clone, data_clone, None, None)
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
