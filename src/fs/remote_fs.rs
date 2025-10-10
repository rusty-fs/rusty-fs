use super::http_client::HttpClient;
use super::types::FileEntry;
use fuser::{FUSE_ROOT_ID, FileAttr, FileType};
use fuser::{Filesystem, ReplyAttr, ReplyDirectory, ReplyEntry, Request};
use libc::{ENOENT, EIO};
use tracing::warn;
use std::collections::HashMap;
use std::time::{Duration,SystemTime, UNIX_EPOCH};
use tokio::runtime::Runtime;
use std::ffi::OsStr;

const TTL: Duration = Duration::from_secs(1);

pub struct RemoteFileSystem {
    http_client: HttpClient,
    runtime: Runtime,
    inode_to_path: HashMap<u64, String>,
    path_to_inode: HashMap<String, u64>,
    next_inode: u64,
}

impl RemoteFileSystem {
    pub fn new(client: HttpClient) -> Self {
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
        let perm = entry.permissions.unwrap_or(if entry.is_dir { 0o755 } else { 0o644 }) as u16;
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
        println!("lookup: parent={}, name={:?}", parent, name);

        let parent_path = match self.get_path_for_inode(parent) {
            Some(path) => path,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
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
                        Err(anyhow::anyhow!("Not found"))
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
                warn!("lookup failed: {}", e);
                reply.error(ENOENT);
            }
        }
    }

    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        println!("getattr: ino={}", ino);

        let path = match self.get_path_for_inode(ino) {
            Some(path) => path,
            None => {
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
        println!("readdir: ino={}, offset={}", ino, offset);

        let path = match self.get_path_for_inode(ino) {
            Some(path) => path,
            None => {
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
                println!("readdir failed: {}", e);
                // mappare gli errori di rete su EIO se necessario
                reply.error(ENOENT);
            }
        }
    }
}
