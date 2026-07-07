use crate::fs::http::FileEntry;
use crate::fs::http::HttpError;
use crate::fs::utils::path;
use crate::fs::utils::runtime;
use fuser::FileType;
use std::ffi::OsStr;
use tracing::{debug, error};

use super::RemoteFileSystem;

impl RemoteFileSystem {
    /// Lookup a name under a parent inode and return (inode, FileAttr)
    pub fn lookup(&mut self, parent: u64, name: &str) -> Result<(u64, super::FileAttr), HttpError> {
        self.ensure_running()?;
        // increment total lookup counter
        // metrics removed: inc_lookup_total() removed
        let parent_path = self.get_path_for_inode(parent).ok_or(HttpError::NotFound)?;
        let full_path = path::join_path(&parent_path, name);
        // Fast path: consult negative lookup cache to suppress repeated ENOENT probes
        if self.negative_lookup_is_cached(&full_path) {
            debug!("lookup negative-cache hit for {}", full_path);
            return Err(HttpError::NotFound);
        }

        // Try to consult directory listing cache first to avoid network call.
        if let Some(entries) = self.listing_cache_get(&parent_path) {
            // If the cached listing doesn't contain the name, consider it authoritative
            // for a short TTL (avoids immediate network list calls). Insert a short
            // negative-lookup entry so subsequent probes are fast.
            if let Some(entry) = entries.into_iter().find(|e| e.name == name) {
                // successful lookup — remove any negative cache entry
                self.negative_lookup_remove(&full_path);
                let inode = self.get_inode_for_path(&full_path);
                let attr = self.file_entry_to_attr(&entry, inode);
                return Ok((inode, attr));
            } else {
                debug!(
                    "lookup: not found in listing cache for {} — returning NotFound (cached)",
                    full_path
                );
                // treat as NotFound and cache that result briefly
                self.negative_lookup_insert(&full_path);
                return Err(HttpError::NotFound);
            }
        }

        let client = self.http_client.clone();
        let parent_clone = parent_path.clone();
        let name_clone = name.to_string();
        // cache miss; record and perform the network call
        // cache miss; perform the network call
        let list_res = runtime::runtime().block_on(async move {
            match client.list_directory(&parent_clone).await {
                Ok(entries) => {
                    if let Some(entry) = entries.into_iter().find(|e| e.name == name_clone) {
                        Ok(entry)
                    } else {
                        Err(HttpError::NotFound)
                    }
                }
                Err(e) => Err(e),
            }
        });

        match list_res {
            Ok(result) => {
                // successful lookup — remove any negative cache entry
                self.negative_lookup_remove(&full_path);
                // Also insert the full listing into the short-lived cache so subsequent
                // lookups/readdir can use it without another network call.
                if let Ok(entries) =
                    runtime::runtime().block_on(self.http_client.list_directory(&parent_path))
                {
                    self.listing_cache_insert(&parent_path, entries);
                }
                let inode = self.get_inode_for_path(&full_path);
                let attr = self.file_entry_to_attr(&result, inode);
                Ok((inode, attr))
            }
            Err(e) => {
                // cache the not-found case for a short TTL to avoid repeated probes
                if let HttpError::NotFound = e {
                    self.negative_lookup_insert(&full_path);
                }
                Err(e)
            }
        }
    }

    /// Get file attributes by inode
    pub fn getattr(&mut self, ino: u64) -> Result<super::FileAttr, HttpError> {
        self.ensure_running()?;
        let path = self.get_path_for_inode(ino).ok_or(HttpError::NotFound)?;
        // root
        if path == "/" {
            // Use a safe default timestamp (~2001-09-09)
            let safe_timestamp = 1_000_000_000;
            let entry = FileEntry {
                name: "".to_string(),
                is_dir: true,
                size: 0,
                modified: Some(safe_timestamp),
                permissions: Some(0o755),
            };
            return Ok(self.file_entry_to_attr(&entry, ino));
        }

        let client = self.http_client.clone();
        let path_clone = path.clone();

        // Use precise size if we have it in write state
        let current_size = self.fh_manager.get_file_size_by_inode(ino);

        // Try metadata cache first
        if let Some(mut cached) = self.metadata_cache_get(&path) {
            if let Some(s) = current_size {
                cached.size = s;
            }
            return Ok(self.file_entry_to_attr(&cached, ino));
        }

        let res = runtime::runtime().block_on(async move {
            // try metadata first to avoid calling /list on regular files
            match client.get_file_metadata(&path_clone).await {
                Ok(meta) => Ok(meta),
                Err(_) => {
                    // fallback: try directory
                    if let Ok(_) = client.list_directory(&path_clone).await {
                        Ok(FileEntry {
                            name: path_clone.split('/').last().unwrap_or("").to_string(),
                            is_dir: true,
                            size: 0,
                            modified: Some(1_000_000_000),
                            permissions: Some(0o755),
                        })
                    } else {
                        Err(HttpError::NotFound)
                    }
                }
            }
        })?;

        // Insert into metadata cache for future getattr calls
        self.metadata_cache_insert(&path, res.clone());

        let mut attr = self.file_entry_to_attr(&res, ino);
        if let Some(s) = current_size {
            attr.size = s;
        }
        Ok(attr)
    }

    /// List directory contents
    pub fn readdir(&mut self, ino: u64) -> Result<Vec<(u64, FileEntry)>, HttpError> {
        self.ensure_running()?;
        let path = self.get_path_for_inode(ino).ok_or(HttpError::NotFound)?;
        let client = self.http_client.clone();
        let path_clone = path.clone();
        // record a readdir for metrics
        // metrics removed: inc_readdir_requests()

        // Try cache first
        if let Some(cached) = self.listing_cache_get(&path) {
            // Use cached listing; do not increment list_requests
            let mut out = Vec::new();
            for entry in cached.into_iter() {
                let entry_path = path::join_path(&path, &entry.name);
                let entry_inode = self.get_inode_for_path(&entry_path);
                out.push((entry_inode, entry));
            }
            return Ok(out);
        }

        // Cache miss: perform network list and record the list request
        // Cache miss: perform network list
        let entries =
            runtime::runtime().block_on(async move { client.list_directory(&path_clone).await })?;
        // insert into cache for a short TTL
        self.listing_cache_insert(&path, entries.clone());
        let mut out = Vec::new();
        for entry in entries.into_iter() {
            let entry_path = path::join_path(&path, &entry.name);
            let entry_inode = self.get_inode_for_path(&entry_path);
            out.push((entry_inode, entry));
        }
        Ok(out)
    }

    /// List directory entries with FUSE offset semantics
    pub fn readdir_entries(
        &mut self,
        ino: u64,
        offset: i64,
    ) -> Result<Vec<(u64, FileType, String)>, HttpError> {
        // get real entries (inode, FileEntry)
        let entries = self.readdir(ino)?;
        let mut out: Vec<(u64, FileType, String)> = Vec::new();

        // offset is the fuser offset: 0 means start
        // entry 0 -> "."  (dir ino)
        // entry 1 -> ".." (parent ino)
        // entry 2.. -> real entries

        // "."
        out.push((ino, FileType::Directory, ".".to_string()));

        // ".."
        let path = self.get_path_for_inode(ino).unwrap_or("/".to_string());
        let parent_path = path::parent_path(&path);
        let parent_ino = self.get_inode_for_path(&parent_path);
        out.push((parent_ino, FileType::Directory, "..".to_string()));

        // real entries
        for (entry_ino, entry) in entries.into_iter() {
            let ft = if entry.is_dir {
                FileType::Directory
            } else {
                FileType::RegularFile
            };
            out.push((entry_ino, ft, entry.name));
        }

        let start_idx = (offset.max(0)) as usize;
        let sliced = if start_idx < out.len() {
            out[start_idx..].to_vec()
        } else {
            Vec::new()
        };
        Ok(sliced)
    }

    /// Create a directory
    pub fn create_directory(
        &mut self,
        parent: u64,
        name: &OsStr,
        _mode: u32,
    ) -> Result<(u64, super::FileAttr), HttpError> {
        self.ensure_running()?;
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                error!("mkdir failed: invalid name {:?}", name);
                return Err(HttpError::Other("invalid name".into()));
            }
        };
        let parent_path = match self.get_path_for_inode(parent) {
            Some(p) => p,
            None => {
                error!("mkdir failed: parent inode {} not found", parent);
                return Err(HttpError::NotFound);
            }
        };
        let full_path = path::join_path(&parent_path, name_str);

        let client = self.http_client.clone();
        let path_clone = full_path.clone();
        match runtime::runtime().block_on(async move { client.create_directory(&path_clone).await })
        {
            Ok(_) => {
                // After creation, get attributes
                // Invalidate negative lookup for this path so subsequent probes fetch fresh data
                self.negative_lookup_remove(&full_path);
                // Invalidate the parent directory listing cache so future readdir/lookup fetches fresh listing
                self.listing_cache_remove(&parent_path);
                // Invalidate metadata cache for this path
                self.metadata_cache_remove(&full_path);
                let inode = self.get_inode_for_path(&full_path);
                match self.getattr(inode) {
                    Ok(attr) => Ok((inode, attr)),
                    Err(e) => {
                        error!("mkdir succeeded but getattr failed: {}", e);
                        return Err(e);
                    }
                }
            }
            Err(e) => {
                error!("mkdir failed: {}", e);
                return Err(e);
            }
        }
    }

    /// Delete a directory or file
    pub fn delete_directory(&mut self, parent: u64, name: &OsStr) -> Result<(), HttpError> {
        self.ensure_running()?;
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                error!("rmdir failed: invalid name {:?}", name);
                return Err(HttpError::Other("invalid name".into()));
            }
        };
        let parent_path = match self.get_path_for_inode(parent) {
            Some(p) => p,
            None => {
                error!("rmdir failed: parent inode {} not found", parent);
                return Err(HttpError::NotFound);
            }
        };
        let full_path = path::join_path(&parent_path, name_str);

        let client = self.http_client.clone();
        let path_clone = full_path.clone();
        let result =
            runtime::runtime().block_on(async move { client.delete_path(&path_clone).await });

        match result {
            Ok(_) => {
                // Remove from inode mapper (would need access to internal field)
                // Invalidate negative lookup cache for this path so subsequent
                // probes will fetch fresh data.
                self.negative_lookup_remove(&full_path);
                // Invalidate parent listing cache
                self.listing_cache_remove(&parent_path);
                // Invalidate metadata cache for deleted path
                self.metadata_cache_remove(&full_path);
                Ok(())
            }
            Err(e) => {
                error!("rmdir failed: {}", e);
                Err(e)
            }
        }
    }

    /// Rename or move a file: supports renaming within same directory or moving across directories
    pub fn rename(
        &mut self,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
    ) -> Result<(), HttpError> {
        self.ensure_running()?;
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                error!("rename failed: invalid name {:?}", name);
                return Err(HttpError::Other("invalid name".into()));
            }
        };
        let newname_str = match newname.to_str() {
            Some(s) => s,
            None => {
                error!("rename failed: invalid new name {:?}", newname);
                return Err(HttpError::Other("invalid new name".into()));
            }
        };

        let parent_path = self.get_path_for_inode(parent).ok_or(HttpError::NotFound)?;
        let full_path = path::join_path(&parent_path, name_str);
        let new_parent_path = self
            .get_path_for_inode(newparent)
            .ok_or(HttpError::NotFound)?;
        let new_full_path = path::join_path(&new_parent_path, newname_str);

        // Prevent directory traversal from client-supplied names
        if name_str.contains("..") || newname_str.contains("..") {
            return Err(HttpError::Other("invalid name".into()));
        }

        let client = self.http_client.clone();
        let src = full_path.clone();
        let dst = new_full_path.clone();

        // Call the HTTP client to perform server-side rename/move
        let call_src = src.clone();
        let call_dst = dst.clone();
        let res =
            runtime::runtime().block_on(async move { client.rename(&call_src, &call_dst).await });

        match res {
            Ok(_) => {
                // Update local inode mapping so subsequent lookups for the new
                // path succeed immediately and don't confuse user-space tools
                // (some file managers perform immediate lookups after rename).
                self.inode_mapper.rename(&src, &dst);
                // Invalidate negative lookup entries for source and destination
                self.negative_lookup_remove(&src);
                self.negative_lookup_remove(&dst);
                // Invalidate listings for source and destination parent directories
                let src_parent = path::parent_path(&src);
                let dst_parent = path::parent_path(&dst);
                self.listing_cache_remove(&src_parent);
                self.listing_cache_remove(&dst_parent);
                // Invalidate metadata cache for src and dst
                self.metadata_cache_remove(&src);
                self.metadata_cache_remove(&dst);
                Ok(())
            }
            Err(e) => {
                error!("rename failed for {} -> {}: {}", src, dst, e);
                Err(e)
            }
        }
    }
}
