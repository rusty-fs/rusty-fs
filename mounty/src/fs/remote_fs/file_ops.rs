use crate::fs::http::FileEntry;
use crate::fs::http::HttpError;
use crate::fs::utils::path;
use crate::fs::utils::runtime;
use libc::{O_APPEND, O_EXCL, O_TRUNC, O_WRONLY};
use std::ffi::OsStr;
use std::time::Duration;
use tracing::{debug, error, trace};

use super::RemoteFileSystem;

impl RemoteFileSystem {
    /// Open a file (validate that it exists and allocate a file handle)
    pub fn open(&mut self, ino: u64, flags: i32) -> Result<u64, HttpError> {
        self.ensure_running()?;
        let path = self.get_path_for_inode(ino).ok_or(HttpError::NotFound)?;
        let client = self.http_client.clone();
        let path_clone = path.clone();

        // Validate file exists and get size
        let mut meta = runtime::runtime()
            .block_on(async move { client.get_file_metadata(&path_clone).await })?;

        let truncate = (flags & O_TRUNC) != 0;
        if truncate {
            let client2 = self.http_client.clone();
            let path2 = path.clone();
            runtime::runtime().block_on(async move {
                client2
                    .put_file_stream(&path2, reqwest::Body::from(Vec::<u8>::new()), None, None)
                    .await
            })?;
            meta.size = 0;
            self.metadata_cache_insert(&path, meta.clone());
        }

        // Allocate and return a file handle
        let fh = self.fh_manager.alloc_fh(ino, flags);
        if let Some(state) = self.fh_manager.get_fh_state(fh) {
            state.file_size = Some(meta.size);
        }
        Ok(fh)
    }

    /// Read bytes from a file by inode
    pub fn read_bytes(
        &self,
        ino: u64,
        fh: Option<u64>,
        offset: u64,
        size: usize,
    ) -> Result<Vec<u8>, HttpError> {
        let path = self.get_path_for_inode(ino).ok_or(HttpError::NotFound)?;

        // Use cached file size from FhState if available, avoiding a metadata HTTP call
        let file_size = fh.and_then(|fh| self.fh_manager.get_file_size(fh));

        let to_read = if let Some(fsize) = file_size {
            if offset >= fsize {
                return Ok(Vec::new());
            }
            std::cmp::min(size, (fsize - offset) as usize)
        } else {
            // Fallback: fetch metadata to determine file size
            let client = self.http_client.clone();
            let path_clone = path.clone();
            let meta = runtime::runtime()
                .block_on(async move { client.get_file_metadata(&path_clone).await })?;
            if offset >= meta.size {
                return Ok(Vec::new());
            }
            let remaining = (meta.size - offset) as usize;
            std::cmp::min(size, remaining)
        };

        // Fase 1: Read-ahead logic
        if let Some(fh) = fh {
            if let Some(state) = self.fh_manager.get_fh_state_ref(fh) {
                // Skip read buffer for write-only files
                let is_wronly = (state.open_flags & libc::O_ACCMODE) == O_WRONLY;
                if !is_wronly {
                    let cap = state.read_buf_cap;

                    // If requested read is larger than buffer, skip read-ahead
                    // (would need multiple fetches anyway)
                    if to_read <= cap {
                        // Check buffer hit: is [offset, offset+to_read) contained in [buf_offset, buf_offset+buf_len)?
                        let buf_offset = *state.read_buf_offset.borrow();
                        let buf_len = *state.read_buf_len.borrow();
                        let buf_end = buf_offset.saturating_add(buf_len as u64);
                        let req_end = offset.saturating_add(to_read as u64);

                        if buf_len > 0 && offset >= buf_offset && req_end <= buf_end {
                            // Cache hit: serve from buffer
                            let start_idx = (offset - buf_offset) as usize;
                            let data = state.read_buf.borrow();
                            let buf_ref = data.as_ref().unwrap();
                            // Safety check: ensure the range fits
                            if start_idx + to_read > buf_ref.len() {
                                // Fallback to direct HTTP read
                                drop(data);
                                let client = self.http_client.clone();
                                let path_clone = path.clone();
                                return runtime::runtime().block_on(async move {
                                    client.read_range(&path_clone, offset, to_read).await
                                });
                            }
                            let result = buf_ref[start_idx..start_idx + to_read].to_vec();
                            tracing::trace!(
                                "read-ahead HIT: offset={} size={} buf_offset={} buf_len={}",
                                offset,
                                to_read,
                                buf_offset,
                                buf_len
                            );
                            return Ok(result);
                        }

                        // Cache miss: fetch aligned chunk
                        let fetch_offset = (offset / cap as u64) * cap as u64;
                        let fetch_size = std::cmp::min(cap, {
                            if let Some(fsize) = file_size {
                                std::cmp::min(cap, (fsize.saturating_sub(fetch_offset)) as usize)
                            } else {
                                cap
                            }
                        });

                        tracing::trace!(
                            "read-ahead MISS: fetching offset={} size={} for request offset={} size={}",
                            fetch_offset, fetch_size, offset, to_read
                        );

                        let client = self.http_client.clone();
                        let path_clone = path.clone();
                        let data = runtime::runtime().block_on(async move {
                            client
                                .read_range(&path_clone, fetch_offset, fetch_size)
                                .await
                        })?;

                        // Update buffer
                        *state.read_buf.borrow_mut() = Some(data.clone());
                        *state.read_buf_offset.borrow_mut() = fetch_offset;
                        *state.read_buf_len.borrow_mut() = data.len();

                        // Serve from the freshly populated buffer
                        let start_idx = (offset - fetch_offset) as usize;
                        // If the requested range doesn't fit in the buffer, fall back to direct HTTP
                        if start_idx + to_read > data.len() {
                            tracing::trace!(
                                "read-ahead: request spans beyond buffer (start_idx={} + to_read={} > buf_len={}), falling back",
                                start_idx, to_read, data.len()
                            );
                            let client = self.http_client.clone();
                            let path_clone = path.clone();
                            return runtime::runtime().block_on(async move {
                                client.read_range(&path_clone, offset, to_read).await
                            });
                        }
                        let result = data[start_idx..start_idx + to_read].to_vec();
                        return Ok(result);
                    }
                }
            }
        }

        // Fallback: direct HTTP read (no file handle or write-only file)
        let client = self.http_client.clone();
        let path_clone = path.clone();
        runtime::runtime()
            .block_on(async move { client.read_range(&path_clone, offset, to_read).await })
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
        self.ensure_running()?;
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
                            .put_file_stream(
                                &path2,
                                reqwest::Body::from(Vec::<u8>::new()),
                                None,
                                None,
                            )
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
        let fh = self.fh_manager.alloc_fh(ino, flags);
        if let Some(state) = self.fh_manager.get_fh_state(fh) {
            // Set file size if known, None for new files
            state.file_size = file_exists;
            state.current_offset = 0;
        }

        // Update metadata cache if truncated or newly created
        if truncate || file_exists.is_none() {
            let mut new_meta = entry.clone();
            new_meta.size = 0;
            self.metadata_cache_insert(&full_path, new_meta);
        }

        Ok((self.config.ttl, attr, 0, fh, 0))
    }

    /// Write bytes to a file (Fase 2: buffered writes with synchronous PUT)
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
        self.ensure_running()?;
        let path = self.get_path_for_inode(ino).ok_or(HttpError::NotFound)?;
        let off = offset as u64;

        let state = self
            .fh_manager
            .get_fh_state(fh)
            .ok_or_else(|| HttpError::Other("invalid file handle".into()))?;

        // Check if we need to flush before appending
        let needs_flush = !state.write_buf.is_empty()
            && off != state.write_buf_offset + state.write_buf.len() as u64;
        let will_be_full = state.write_buf.len() + data.len() >= state.write_buf_cap;

        if needs_flush {
            // state borrow released by NLL before calling flush
            self.flush_write_buffer(fh, &path)?;
            // Re-borrow after flush
            let state = self.fh_manager.get_fh_state(fh).unwrap();
            if state.write_buf.is_empty() {
                state.write_buf_offset = off;
            }
            state.write_buf.extend_from_slice(data);
            state.dirty = true;

            if state.write_buf.len() >= state.write_buf_cap {
                // state borrow released by NLL
                self.flush_write_buffer(fh, &path)?;
            }
        } else {
            if state.write_buf.is_empty() {
                state.write_buf_offset = off;
            }
            state.write_buf.extend_from_slice(data);
            state.dirty = true;

            if will_be_full {
                // state borrow released by NLL
                self.flush_write_buffer(fh, &path)?;
            }
        }

        // Update file size
        let state = self.fh_manager.get_fh_state(fh).unwrap();
        let new_end = state.write_buf_offset + state.write_buf.len() as u64;
        if let Some(size) = state.file_size {
            if new_end > size {
                state.file_size = Some(new_end);
            }
        } else {
            state.file_size = Some(new_end);
        }

        Ok(data.len() as u32)
    }

    /// Flush the write buffer to the server synchronously
    pub(crate) fn flush_write_buffer(&mut self, fh: u64, path: &str) -> Result<(), HttpError> {
        let state = self
            .fh_manager
            .get_fh_state(fh)
            .ok_or_else(|| HttpError::Other("invalid file handle".into()))?;

        if state.write_buf.is_empty() {
            return Ok(());
        }

        let buf = std::mem::take(&mut state.write_buf);
        let offset = state.write_buf_offset;
        let buf_len = buf.len();

        tracing::info!(
            "[DIAG] flush_write_buffer: path={} offset={} size={}",
            path,
            offset,
            buf_len
        );

        let client = self.http_client.clone();
        let path_clone = path.to_string();
        let result = runtime::runtime().block_on(async move {
            client
                .put_file_stream(&path_clone, reqwest::Body::from(buf), Some(offset), None)
                .await
        });

        if let Err(e) = result {
            state
                .pending_write_errors
                .borrow_mut()
                .push(format!("PUT at offset {}: {}", offset, e));
            tracing::error!(
                "[DIAG] flush_write_buffer FAILED: path={} offset={} error={}",
                path,
                offset,
                e
            );
            return Err(e);
        }

        tracing::info!(
            "[DIAG] flush_write_buffer OK: path={} offset={} size={}",
            path,
            offset,
            buf_len
        );

        // Advance buffer offset by actual bytes written
        state.write_buf_offset += buf_len as u64;
        Ok(())
    }

    pub fn flush_and_finalize_handle(&mut self, ino: u64, fh: u64) -> Result<(), HttpError> {
        if let Some(state) = self.fh_manager.get_fh_state(fh) {
            *state.read_buf.borrow_mut() = None;
            *state.read_buf_len.borrow_mut() = 0;
        }

        let path = self.get_path_for_inode(ino).ok_or(HttpError::NotFound)?;
        self.flush_write_buffer(fh, &path)?;

        let (dirty, final_size) = {
            let state = self
                .fh_manager
                .get_fh_state(fh)
                .ok_or_else(|| HttpError::Other("invalid file handle".into()))?;
            (state.dirty, state.file_size)
        };

        if dirty {
            if let Some(final_size) = final_size {
                let client = self.http_client.clone();
                let path_clone = path.clone();
                runtime::runtime().block_on(async move {
                    client
                        .put_file_stream(
                            &path_clone,
                            reqwest::Body::from(Vec::<u8>::new()),
                            Some(0),
                            Some(final_size),
                        )
                        .await
                })?;
                trace!("finalized fh {} with size {}", fh, final_size);
            } else {
                return Err(HttpError::Other(format!(
                    "dirty file handle {} has no known final size",
                    fh
                )));
            }
        }

        if let Some(state) = self.fh_manager.get_fh_state(fh) {
            let errors = state.pending_write_errors.borrow();
            if !errors.is_empty() {
                return Err(HttpError::Other(format!(
                    "{} pending write errors for fh {}: {:?}",
                    errors.len(),
                    fh,
                    errors
                )));
            }
            state.dirty = false;
        }

        Ok(())
    }

    pub fn shutdown_flush_all(&mut self) -> Result<(), Vec<(u64, HttpError)>> {
        self.begin_shutdown();
        let handles = self.fh_manager.open_handles();
        let mut failures = Vec::new();

        for (fh, ino) in handles {
            match self.flush_and_finalize_handle(ino, fh) {
                Ok(()) => self.fh_manager.release_fh(fh),
                Err(err) => {
                    error!("shutdown: failed to flush/finalize fh {}: {}", fh, err);
                    failures.push((fh, err));
                }
            }
        }

        self.clear_caches();

        if failures.is_empty() {
            Ok(())
        } else {
            self.mark_shutdown_failed();
            Err(failures)
        }
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
        self.ensure_running()?;
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

            // Flush all open file handles for this inode before truncation
            let fhs = self.fh_manager.get_fhs_for_inode(ino);
            for fh in fhs {
                let _ = self.flush_write_buffer(fh, &path);
                // Also update the file size in the handle state
                if let Some(state) = self.fh_manager.get_fh_state(fh) {
                    state.file_size = Some(new_size);
                    state.dirty = false;
                }
            }

            let client = self.http_client.clone();
            let path_clone = path.clone();

            match runtime::runtime().block_on(async move {
                // Send with size hint so server knows this is a size-setting operation
                client
                    .put_file_stream(
                        &path_clone,
                        reqwest::Body::from(Vec::<u8>::new()),
                        Some(0),
                        Some(new_size),
                    )
                    .await
            }) {
                Ok(_) => {
                    // Update metadata cache immediately
                    let c2 = self.http_client.clone();
                    let p2 = path.clone();
                    if let Ok(entry) =
                        runtime::runtime().block_on(async move { c2.get_file_metadata(&p2).await })
                    {
                        self.metadata_cache_insert(&path, entry);
                    }
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
