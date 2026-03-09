use crate::fs::http_client::HttpBackend;
use crate::fs::http_client::HttpError;
use crate::fs::types::FileEntry;
use crate::fs::path_utils;
use crate::fs::inode_map::InodeMapper;
use crate::fs::runtime;
use crate::fs::config::FuseConfig;
use crate::fs::file_handle::FhManager;
use fuser::{FUSE_ROOT_ID, FileAttr, FileType};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{debug, error};

pub mod dir_ops;
pub mod file_ops;
#[cfg(test)]
mod tests;

pub use dir_ops::*;
pub use file_ops::*;

pub struct RemoteFileSystem {
    pub http_client: Arc<dyn HttpBackend>,
    inode_mapper: InodeMapper,
    pub fh_manager: FhManager,
    pub config: FuseConfig,
}

impl RemoteFileSystem {
    pub fn new(client: Arc<dyn HttpBackend>) -> Self {
        Self::with_config(client, FuseConfig::default())
    }

    pub fn with_config(client: Arc<dyn HttpBackend>, config: FuseConfig) -> Self {
        Self {
            http_client: client,
            inode_mapper: InodeMapper::new(),
            fh_manager: FhManager::new(),
            config,
        }
    }

    pub fn get_inode_for_path(&mut self, path: &str) -> u64 {
        self.inode_mapper.get_or_create_inode(path)
    }

    pub fn get_path_for_inode(&self, inode: u64) -> Option<String> {
        self.inode_mapper.path_for(inode)
    }

    pub fn file_entry_to_attr(&self, entry: &FileEntry, inode: u64) -> FileAttr {
        let modified = UNIX_EPOCH + Duration::from_secs(entry.modified);
        let perm = entry.permissions as u16;
        FileAttr {
            ino: inode,
            size: entry.size,
            blocks: (entry.size + 511) / 512,
            atime: modified,
            mtime: modified,
            ctime: modified,
            crtime: modified,
            kind: if entry.is_dir {
                FileType::Directory
            } else {
                FileType::RegularFile
            },
            perm,
            nlink: 1,
            uid: unsafe { libc::getuid() } as u32,
            gid: unsafe { libc::getgid() } as u32,
            rdev: 0,
            blksize: 512,
            flags: 0,
        }
    }
}
