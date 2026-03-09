use crate::fs::http_client::HttpError;
use crate::fs::types::FileEntry;
use crate::fs::path_utils;
use crate::fs::runtime;
use fuser::FileType;
use std::ffi::OsStr;
use tracing::error;

use super::RemoteFileSystem;

impl RemoteFileSystem {
    /// Lookup a name under a parent inode and return (inode, FileAttr)
    pub fn lookup(&mut self, parent: u64, name: &str) -> Result<(u64, super::FileAttr), HttpError> {
        let parent_path = self.get_path_for_inode(parent).ok_or(HttpError::NotFound)?;
        let full_path = path_utils::join_path(&parent_path, name);

        let client = self.http_client.clone();
        let parent_clone = parent_path.clone();
        let name_clone = name.to_string();
        let result = runtime::runtime().block_on(async move {
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
        })?;

        let inode = self.get_inode_for_path(&full_path);
        let attr = self.file_entry_to_attr(&result, inode);
        Ok((inode, attr))
    }

    /// Get file attributes by inode
    pub fn getattr(&mut self, ino: u64) -> Result<super::FileAttr, HttpError> {
        let path = self.get_path_for_inode(ino).ok_or(HttpError::NotFound)?;
        // root
        if path == "/" {
            let entry = FileEntry {
                name: "".to_string(),
                is_dir: true,
                size: 0,
                modified: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
                permissions: 0o755,
            };
            return Ok(self.file_entry_to_attr(&entry, ino));
        }

        let client = self.http_client.clone();
        let path_clone = path.clone();
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
                            modified: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap()
                                .as_secs(),
                            permissions: 0o755,
                        })
                    } else {
                        Err(HttpError::NotFound)
                    }
                }
            }
        })?;

        Ok(self.file_entry_to_attr(&res, ino))
    }

    /// List directory contents
    pub fn readdir(&mut self, ino: u64) -> Result<Vec<(u64, FileEntry)>, HttpError> {
        let path = self.get_path_for_inode(ino).ok_or(HttpError::NotFound)?;
        let client = self.http_client.clone();
        let path_clone = path.clone();
        let entries = runtime::runtime()
            .block_on(async move { client.list_directory(&path_clone).await })?;
        let mut out = Vec::new();
        for entry in entries.into_iter() {
            let entry_path = path_utils::join_path(&path, &entry.name);
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
        let parent_path = path_utils::parent_path(&path);
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
        let full_path = path_utils::join_path(&parent_path, name_str);

        let client = self.http_client.clone();
        let path_clone = full_path.clone();
        match runtime::runtime()
            .block_on(async move { client.create_directory(&path_clone).await })
        {
            Ok(_) => {
                // After creation, get attributes
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
        let full_path = path_utils::join_path(&parent_path, name_str);

        let client = self.http_client.clone();
        let path_clone = full_path.clone();
        let result = runtime::runtime()
            .block_on(async move { client.delete_path(&path_clone).await });

        match result {
            Ok(_) => {
                // Remove from inode mapper (would need access to internal field)
                Ok(())
            }
            Err(e) => {
                error!("rmdir failed: {}", e);
                Err(e)
            }
        }
    }
}
