use crate::fs::http_client::HttpBackend;
use crate::fs::types::FileEntry;
use fuser::{FUSE_ROOT_ID, FileAttr, FileType};
use fuser::{Filesystem, ReplyAttr, ReplyDirectory, ReplyEntry, Request};
use libc::{EIO, ENOENT};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::runtime::Runtime;
use crate::fs::http_client::HttpError;
use tracing::{debug, error, warn};

const TTL: Duration = Duration::from_secs(1);

pub struct RemoteFileSystem {
    http_client: Arc<dyn HttpBackend>,
    runtime: Runtime,
    inode_to_path: HashMap<u64, String>,
    path_to_inode: HashMap<String, u64>,
    next_inode: u64,
}

impl RemoteFileSystem {
    pub fn new(client: Arc<dyn HttpBackend>) -> Self {
        let mut fs = Self {
            http_client: client,
            runtime: Runtime::new().unwrap(),
            inode_to_path: HashMap::new(),
            path_to_inode: HashMap::new(),
            next_inode: 2,
        };
        fs.inode_to_path.insert(FUSE_ROOT_ID, "/".to_string());
        fs.path_to_inode.insert("/".to_string(), FUSE_ROOT_ID);
        fs
    }

    pub fn get_inode_for_path(&mut self, path: &str) -> u64 {
        if let Some(&ino) = self.path_to_inode.get(path) {
            return ino;
        }
        let ino = self.next_inode;
        self.next_inode += 1;
        self.inode_to_path.insert(ino, path.to_string());
        self.path_to_inode.insert(path.to_string(), ino);
        ino
    }

    fn get_path_for_inode(&self, inode: u64) -> Option<String> {
        self.inode_to_path.get(&inode).cloned()
    }

    // helper per ottenere il parent path di un path posix-like
    fn parent_path(path: &str) -> String {
        if path == "/" {
            return "/".to_string();
        }
        match path.rfind('/') {
            Some(0) => "/".to_string(),
            Some(idx) => path[..idx].to_string(),
            None => "/".to_string(),
        }
    }

    pub fn file_entry_to_attr(&self, entry: &FileEntry, inode: u64) -> FileAttr {
        let modified = UNIX_EPOCH + Duration::from_secs(entry.modified.unwrap_or(0));
        let perm = entry
            .permissions
            .unwrap_or(if entry.is_dir { 0o755 } else { 0o644 }) as u16;
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
            uid: 1000,
            gid: 1000,
            rdev: 0,
            blksize: 512,
            flags: 0,
        }
    }
}

impl Filesystem for RemoteFileSystem {
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        debug!("lookup: parent={}, name={:?}", parent, name);

        let parent_path = match self.get_path_for_inode(parent) {
            Some(path) => path,
            None => {
                error!("lookup failed: parent inode {} not found", parent);
                reply.error(ENOENT);
                return;
            }
        };

        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                error!("lookup failed: invalid name {:?}", name);
                reply.error(ENOENT);
                return;
            }
        };

        // costruisci full_path (per inode) ma chiedi la lista del parent
        let full_path = if parent_path == "/" {
            format!("/{}", name_str)
        } else {
            format!("{}/{}", parent_path, name_str)
        };

        let client = self.http_client.clone();
        let parent_clone = parent_path.clone();
        let name_clone = name_str.to_string();
        let result = self.runtime.block_on(async move {
            // list_directory sul parent e cerca il nome
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

        match result {
            Ok(entry) => {
                let inode = self.get_inode_for_path(&full_path);
                let attr = self.file_entry_to_attr(&entry, inode);
                reply.entry(&TTL, &attr, 0);
            }
            Err(e) => {
                error!("lookup failed: {}", e);
                reply.error(e.to_errno());
            }
        }
    }

    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        debug!("getattr: ino={}", ino);

        let path = match self.get_path_for_inode(ino) {
            Some(path) => path,
            None => {
                error!("getattr failed: inode {} not found", ino);
                reply.error(ENOENT);
                return;
            }
        };

        let client = self.http_client.clone();
        let result = self.runtime.block_on(async move {
            // Per la root directory
            if path == "/" {
                return Ok(FileEntry {
                    name: "".to_string(),
                    is_dir: true,
                    size: 0,
                    modified: Some(
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap()
                            .as_secs(),
                    ),
                    permissions: None,
                });
            }

            // Prova come directory
            if let Ok(_) = client.list_directory(&path).await {
                return Ok(FileEntry {
                    name: path.split('/').last().unwrap_or("").to_string(),
                    is_dir: true,
                    size: 0,
                    modified: Some(
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap()
                            .as_secs(),
                    ),
                    permissions: None,
                });
            }

            // Fallback per file (se implementi get_file_metadata)
            Err(anyhow::anyhow!("Not found"))
        });

        match result {
            Ok(entry) => {
                let attr = self.file_entry_to_attr(&entry, ino);
                reply.attr(&TTL, &attr);
            }
            Err(_) => {
                error!("getattr failed: path not found");
                reply.error(ENOENT);
            }
        }
    }

    fn readdir(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        debug!("readdir: ino={}, offset={}", ino, offset);

        let path = match self.get_path_for_inode(ino) {
            Some(path) => path,
            None => {
                error!("readdir failed: inode {} not found", ino);
                reply.error(ENOENT);
                return;
            }
        };

        let path_clone = path.clone();
        let client = self.http_client.clone();
        let result = self
            .runtime
            .block_on(async move { client.list_directory(&path_clone).await });

        match result {
            Ok(entries) => {
                let mut current_offset = 1i64;

                // "."
                if offset < current_offset {
                    if reply.add(ino, current_offset, FileType::Directory, ".") {
                        reply.ok();
                        return;
                    }
                }
                current_offset += 1;

                // ".." -> calcola parent path reale
                if offset < current_offset {
                    let parent_path = Self::parent_path(&path);
                    let parent_ino = self.get_inode_for_path(&parent_path);
                    if reply.add(parent_ino, current_offset, FileType::Directory, "..") {
                        reply.ok();
                        return;
                    }
                }
                current_offset += 1;

                // entries reali
                for entry in entries.iter().skip((offset - 2).max(0) as usize) {
                    let entry_path = if path == "/" {
                        format!("/{}", entry.name)
                    } else {
                        format!("{}/{}", path, entry.name)
                    };

                    let entry_inode = self.get_inode_for_path(&entry_path);
                    let file_type = if entry.is_dir {
                        FileType::Directory
                    } else {
                        FileType::RegularFile
                    };

                    if reply.add(entry_inode, current_offset, file_type, &entry.name) {
                        break;
                    }
                    current_offset += 1;
                }
                reply.ok();
            }
            Err(e) => {
                error!("readdir failed: {}", e);
                reply.error(e.to_errno());
            }
        }
    }

    fn open(&mut self, _req: &Request<'_>, _ino: u64, _flags: i32, reply: fuser::ReplyOpen) {
        todo!("Implement open if needed");
    }

    fn read(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        fh: u64,
        offset: i64,
        size: u32,
        flags: i32,
        lock_owner: Option<u64>,
        reply: fuser::ReplyData,
    ) {
        todo!("Implement read if needed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::http_client::{HttpBackend, HttpError};
    use crate::fs::types::FileEntry;
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::os::unix::raw::mode_t;
    use std::sync::Arc;

    #[derive(Clone)]
    struct FakeBackend {
        listing: HashMap<String, Vec<FileEntry>>,
    }

    impl FakeBackend {
        fn new() -> Self {
            let mut listing = HashMap::new();
            listing.insert(
                "/".to_string(),
                vec![
                    FileEntry {
                        name: "f.txt".to_string(),
                        is_dir: false,
                        size: 10,
                        modified: Some(1),
                        permissions: Some(0o644 as mode_t),
                    },
                    FileEntry {
                        name: "dir".to_string(),
                        is_dir: true,
                        size: 0,
                        modified: Some(2),
                        permissions: Some(0o755 as mode_t),
                    },
                ],
            );
            listing.insert(
                "/dir".to_string(),
                vec![FileEntry {
                    name: "inner.txt".to_string(),
                    is_dir: false,
                    size: 5,
                    modified: Some(3),
                    permissions: Some(0o600 as mode_t),
                }],
            );
            Self { listing }
        }
    }

    #[async_trait]
    impl HttpBackend for FakeBackend {
        async fn list_directory(&self, path: &str) -> Result<Vec<FileEntry>, HttpError> {
            if let Some(vec) = self.listing.get(path) {
                Ok(vec.clone())
            } else {
                Err(HttpError::NotFound)
            }
        }

        async fn get_file_metadata(&self, _path: &str) -> Result<FileEntry, HttpError> {
            Err(HttpError::Other(("not implemented").into()))
        }

        async fn head(&self, _path: &str) -> Result<(u64, Option<String>), HttpError> {
            Err(HttpError::Other(("not implemented").into()))
        }

        async fn read_all(&self, _path: &str) -> Result<Vec<u8>, HttpError> {
            Err(HttpError::Other(("not implemented").into()))
        }

        async fn read_range(
            &self,
            _path: &str,
            _offset: u64,
            _length: usize,
        ) -> Result<Vec<u8>, HttpError> {
            Err(HttpError::Other(("not implemented").into()))
        }
    }

    #[test]
    fn test_parent_path_cases() {
        assert_eq!(RemoteFileSystem::parent_path("/"), "/");
        assert_eq!(RemoteFileSystem::parent_path("/foo"), "/");
        assert_eq!(RemoteFileSystem::parent_path("/a/b/c"), "/a/b");
        assert_eq!(RemoteFileSystem::parent_path("file"), "/");
        assert_eq!(RemoteFileSystem::parent_path("/a/"), "/a");
    }

    #[test]
    fn test_get_inode_for_path_and_root_mapping() {
        let backend = Arc::new(FakeBackend::new());
        let mut fs = RemoteFileSystem::new(backend);
        // root should be mapped to FUSE_ROOT_ID
        let root_ino = fs.get_inode_for_path("/");
        assert_eq!(root_ino, FUSE_ROOT_ID);

        // new path gets new inode, subsequent call returns same inode
        let p = "/foo/bar";
        let ino1 = fs.get_inode_for_path(p);
        let ino2 = fs.get_inode_for_path(p);
        assert_eq!(ino1, ino2);
        assert!(ino1 != FUSE_ROOT_ID);
    }

    #[test]
    fn test_file_entry_to_attr_values() {
        let backend = Arc::new(FakeBackend::new());
        let fs = RemoteFileSystem::new(backend);

        let entry_file = FileEntry {
            name: "f.txt".into(),
            is_dir: false,
            size: 1234,
            modified: Some(100),
            permissions: Some(0o644 as mode_t),
        };
        let attr = fs.file_entry_to_attr(&entry_file, 42);
        assert_eq!(attr.ino, 42);
        assert_eq!(attr.size, 1234);
        assert_eq!(attr.kind, FileType::RegularFile);
        assert_eq!(attr.perm, 0o644);

        let entry_dir = FileEntry {
            name: "d".into(),
            is_dir: true,
            size: 0,
            modified: Some(200),
            permissions: Some(0o755 as mode_t),
        };
        let attrd = fs.file_entry_to_attr(&entry_dir, 43);
        assert_eq!(attrd.kind, FileType::Directory);
        assert_eq!(attrd.perm, 0o755);
        // blocks calculation: (size + 511) / 512
        assert_eq!(attr.blocks, (1234 + 511) / 512);
    }
}
