use crate::fs::http_client::HttpBackend;
use crate::fs::http_client::HttpError;
use crate::fs::types::FileEntry;
use fuser::{FUSE_ROOT_ID, FileAttr, FileType};
use fuser::{Filesystem, ReplyAttr, ReplyDirectory, ReplyEntry, Request};
use libc::size_t;
use libc::{EIO, ENOENT};
use libc::{O_APPEND, O_EXCL, O_TRUNC};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::runtime::Runtime;
use tracing::info;
use tracing::{debug, error, warn};

const TTL: Duration = Duration::from_secs(1);

struct FhState {
    buf: Vec<u8>,
    append: bool,
}

struct FhManager {
    next_fh: u64,
    fh_map: HashMap<u64, FhState>,
}

impl FhManager {
    fn new() -> Self {
        Self {
            next_fh: 1,
            fh_map: HashMap::new(),
        }
    }

    fn alloc_fh(&mut self, append: bool) -> u64 {
        let fh = self.next_fh;
        self.next_fh += 1;
        self.fh_map.insert(
            fh,
            FhState {
                buf: Vec::new(),
                append,
            },
        );
        fh
    }

    fn get_fh_state(&mut self, fh: u64) -> Option<&mut FhState> {
        self.fh_map.get_mut(&fh)
    }

    fn release_fh(&mut self, fh: u64) {
        self.fh_map.remove(&fh);
    }
}

pub struct RemoteFileSystem {
    http_client: Arc<dyn HttpBackend>,
    runtime: Runtime,
    inode_to_path: HashMap<u64, String>,
    path_to_inode: HashMap<String, u64>,
    next_inode: u64,
    fh_manager: FhManager,
}

impl RemoteFileSystem {
    pub fn new(client: Arc<dyn HttpBackend>) -> Self {
        let mut fs = Self {
            http_client: client,
            runtime: Runtime::new().unwrap(),
            inode_to_path: HashMap::new(),
            path_to_inode: HashMap::new(),
            next_inode: 2,
            fh_manager: FhManager::new(),
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
        let modified = UNIX_EPOCH + Duration::from_secs(entry.modified);
        let perm = entry.permissions as u16;
        // .unwrap_or(if entry.is_dir { 0o755 } else { 0o644 }) as u16;
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

    // Testable helper: lookup a name under a parent inode and return (inode, FileAttr)
    pub fn lookup_path(&mut self, parent: u64, name: &str) -> Result<(u64, FileAttr), HttpError> {
        let parent_path = self.get_path_for_inode(parent).ok_or(HttpError::NotFound)?;
        let full_path = if parent_path == "/" {
            format!("/{}", name)
        } else {
            format!("{}/{}", parent_path, name)
        };

        let client = self.http_client.clone();
        let parent_clone = parent_path.clone();
        let name_clone = name.to_string();
        let result = self.runtime.block_on(async move {
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

    // Testable helper: getattr by inode
    pub fn getattr_path(&mut self, ino: u64) -> Result<FileAttr, HttpError> {
        let path = self.get_path_for_inode(ino).ok_or(HttpError::NotFound)?;
        // root
        if path == "/" {
            let entry = FileEntry {
                name: "".to_string(),
                is_dir: true,
                size: 0,
                modified: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
                permissions: 0o755,
            };
            return Ok(self.file_entry_to_attr(&entry, ino));
        }

        let client = self.http_client.clone();
        let path_clone = path.clone();
        let res = self.runtime.block_on(async move {
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
                            modified: SystemTime::now()
                                .duration_since(UNIX_EPOCH)
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

    // Testable helper: readdir returns (inode, FileEntry) vector
    pub fn readdir_path(&mut self, ino: u64) -> Result<Vec<(u64, FileEntry)>, HttpError> {
        let path = self.get_path_for_inode(ino).ok_or(HttpError::NotFound)?;
        let client = self.http_client.clone();
        let path_clone = path.clone();
        let entries = self
            .runtime
            .block_on(async move { client.list_directory(&path_clone).await })?;
        let mut out = Vec::new();
        for entry in entries.into_iter() {
            let entry_path = if path == "/" {
                format!("/{}", entry.name)
            } else {
                format!("{}/{}", path, entry.name)
            };
            let entry_inode = self.get_inode_for_path(&entry_path);
            out.push((entry_inode, entry));
        }
        Ok(out)
    }

    pub fn readdir_entries(
        &mut self,
        ino: u64,
        offset: i64,
    ) -> Result<Vec<(u64, FileType, String)>, HttpError> {
        // get real entries (inode, FileEntry)
        let entries = self.readdir_path(ino)?;
        let mut out: Vec<(u64, FileType, String)> = Vec::new();

        // offset is the fuser offset: 0 means start; this module used current_offset=1.. so adapt
        // we will build full list then return slice starting from offset
        // entry 0 -> "."  (dir ino)
        // entry 1 -> ".." (parent ino)
        // entry 2.. -> real entries

        // "."
        out.push((ino, FileType::Directory, ".".to_string()));

        // ".."
        let path = self.get_path_for_inode(ino).unwrap_or("/".to_string());
        let parent_path = Self::parent_path(&path);
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

        // apply offset semantics as in the FUSE callback: the callback used current_offset starting at 1
        // if caller offset < 1 then start at 1; here we treat offset as number already used previously
        let start_idx = (offset.max(0)) as usize; // adapt if needed to match previous logic
        let sliced = if start_idx < out.len() {
            out[start_idx..].to_vec()
        } else {
            Vec::new()
        };
        Ok(sliced)
    }

    // Testable helper: open (validate file exists)
    pub fn open_path(&mut self, ino: u64) -> Result<(), HttpError> {
        let path = self.get_path_for_inode(ino).ok_or(HttpError::NotFound)?;
        let client = self.http_client.clone();
        let path_clone = path.clone();
        self.runtime
            .block_on(async move { client.get_file_metadata(&path_clone).await })
            .map(|_| ())
    }

    // Testable helper: read bytes by inode
    pub fn read_bytes(&self, ino: u64, offset: u64, size: usize) -> Result<Vec<u8>, HttpError> {
        let path = self.get_path_for_inode(ino).ok_or(HttpError::NotFound)?;
        let client = self.http_client.clone();
        let path_clone = path.clone();
        self.runtime
            .block_on(async move { client.read_range(&path_clone, offset, size).await })
    }

    pub fn create_directory(
        &mut self,
        parent: u64,
        name: &OsStr,
        mode: u32,
    ) -> Result<(u64, FileAttr), HttpError> {
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
        let full_path = if parent_path == "/" {
            format!("/{}", name_str)
        } else {
            format!("{}/{}", parent_path, name_str)
        };

        let client = self.http_client.clone();
        let path_clone = full_path.clone();
        match self
            .runtime
            .block_on(async move { client.create_directory(&path_clone).await })
        {
            Ok(_) => {
                // After creation, get attributes
                let inode = self.get_inode_for_path(&full_path);
                match self.getattr_path(inode) {
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
        let full_path = if parent_path == "/" {
            format!("/{}", name_str)
        } else {
            format!("{}/{}", parent_path, name_str)
        };

        let client = self.http_client.clone();
        let path_clone = full_path.clone();
        let result = self
            .runtime
            .block_on(async move { client.delete_path(&path_clone).await });

        match result {
            Ok(_) => {
                // Optionally remove from inode/path maps
                if let Some(ino) = self.path_to_inode.remove(&full_path) {
                    self.inode_to_path.remove(&ino);
                }
                Ok(())
            }
            Err(e) => {
                error!("rmdir failed: {}", e);
                Err(e)
            }
        }
    }
    pub fn create_file(
        &mut self,
        parent: u64,
        name: &OsStr,
        mode: u32,
        umask: u32,
        flags: i32,
    ) -> Result<(Duration, FileAttr, u64, u64, u32), HttpError> {
        // allocate fh and empty buffer, return attr + fh
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
        let full_path = if parent_path == "/" {
            format!("/{}", name_str)
        } else {
            format!("{}/{}", parent_path, name_str)
        };

        // create a placeholder inode and attr (you may want to call server metadata later)
        let ino = self.get_inode_for_path(&full_path);
        let perm_bits = (mode & !umask) as u16;

        let entry = FileEntry {
            name: name_str.to_string(),
            is_dir: false,
            size: 0,
            permissions: perm_bits.into(),
            modified: 0, // will be updated on write
        };

        if (flags & O_EXCL) != 0 {
            // if file exists, return EEXIST
            if let Ok((_, _)) = self.lookup_path(parent, name_str) {
                return Err(HttpError::AlreadyExists);
            }
        }

        let attr = self.file_entry_to_attr(&entry, ino);

        // Decide creation/truncation/prefetch behavior
        let truncate = (flags & O_TRUNC) != 0;
        let append = (flags & O_APPEND) != 0;
        let client = self.http_client.clone();
        let path_clone = full_path.clone();

        // If file exists and O_TRUNC not set -> prefetch existing contents into the fh buffer
        // If O_TRUNC set -> truncate immediately (upload empty)
        // If file doesn't exist -> create empty file on server (could be lazy instead)
        match self
            .runtime
            .block_on(async move { client.get_file_metadata(&path_clone).await })
        {
            Ok(meta) => {
                if truncate {
                    // truncate on server now
                    let client2 = self.http_client.clone();
                    let path2 = full_path.clone();
                    if let Err(e) = self
                        .runtime
                        .block_on(async move { client2.put_file_stream(&path2, Vec::new()).await })
                    {
                        error!("create (truncate) failed: {}", e);
                        return Err(e);
                    }
                } else {
                    // Prefetch whole file so writes can patch existing data
                    let client2 = self.http_client.clone();
                    let path2 = full_path.clone();
                    let size = meta.size as usize;
                    match self
                        .runtime
                        .block_on(async move { client2.read_range(&path2, 0, size).await })
                    {
                        Ok(data) => {
                            let fh = self.fh_manager.alloc_fh(append);
                            if let Some(state) = self.fh_manager.get_fh_state(fh) {
                                state.buf = data;
                            }
                            // reply below after setting fh
                            return Ok((TTL, attr, 0, fh, 0));
                        }
                        Err(e) => {
                            error!("create prefetch failed: {}", e);
                            return Err(e);
                        }
                    }
                }
            }
            Err(_) => {
                // file doesn't exist -> create empty file on server now (or leave lazy)
                let client2 = self.http_client.clone();
                let path2 = full_path.clone();
                if let Err(e) = self
                    .runtime
                    .block_on(async move { client2.put_file_stream(&path2, Vec::new()).await })
                {
                    error!("create (new file) failed: {}", e);
                    return Err(e);
                }
            }
        }

        // default path: alloc fh with empty buffer (already handled above in some branches)
        let fh = self.fh_manager.alloc_fh(append);
        return Ok((TTL, attr, 0, fh, 0));
    }

    pub fn write_bytes(
        &mut self,
        ino: u64,
        fh: u64,
        offset: i64,
        data: &[u8],
        write_flags: u32,
        flags: i32,
        lock_owner: Option<u64>,
    ) -> Result<(u32), HttpError> {
        let path = match self.get_path_for_inode(ino) {
            Some(p) => p,
            None => {
                error!("write failed: inode {} not found", ino);
                return Err(HttpError::NotFound);
            }
        };

        match self.fh_manager.get_fh_state(fh) {
            Some(state) => {
                let off = offset as usize;
                // Ensure buffer is large enough
                if state.buf.len() < off + data.len() {
                    state.buf.resize(off + data.len(), 0);
                }
                // Write data into buffer
                state.buf[off..off + data.len()].copy_from_slice(data);
                let client = self.http_client.clone();
                let path_clone = path.clone();
                let data_clone = state.buf.clone();
                let result = self
                    .runtime
                    .block_on(async move { client.put_file_stream(&path_clone, data_clone).await });
                match result {
                    Ok(_) => {
                        Ok(data.len() as u32)
                    }
                    Err(e) => {
                        error!("write failed during upload: {}", e);
                        return Err(e);
                    }
                }
            }
            None => {
                return Err(HttpError::Other("invalid file handle".into()));
            }
        }
    }
}

impl Filesystem for RemoteFileSystem {
    // Use lookup_path helper
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        debug!("lookup: parent={}, name={:?}", parent, name);
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                error!("lookup failed: invalid name {:?}", name);
                reply.error(ENOENT);
                return;
            }
        };
        match self.lookup_path(parent, name_str) {
            Ok((inode, attr)) => {
                reply.entry(&TTL, &attr, 0);
            }
            Err(e) => {
                error!("lookup failed: {}", e);
                reply.error(e.to_errno());
            }
        }
    }

    // Use getattr_path helper
    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        debug!("getattr: ino={}", ino);
        match self.getattr_path(ino) {
            Ok(attr) => {
                reply.attr(&TTL, &attr);
            }
            Err(e) => {
                error!("getattr failed: {}", e);
                reply.error(e.to_errno());
            }
        }
    }

    // Use readdir_path helper
    fn readdir(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        match self.readdir_entries(ino, offset) {
            Ok(items) => {
                for (i, (entry_ino, file_type, name)) in items.into_iter().enumerate() {
                    // compute an offset value for reply.add — keep same semantics as before
                    let entry_offset = offset + (i as i64) + 1; // tune to match previous current_offset
                    if reply.add(entry_ino, entry_offset, file_type, name) {
                        reply.ok();
                        return;
                    }
                }
                reply.ok();
            }
            Err(e) => {
                error!("readdir failed: {}", e);
                reply.error(e.to_errno());
            }
        }
    }

    // Use open_path helper
    fn open(&mut self, _req: &Request<'_>, ino: u64, _flags: i32, reply: fuser::ReplyOpen) {
        debug!("open called for ino {}", ino);
        match self.open_path(ino) {
            Ok(_) => {
                let fh = 0; // file handle, not used here
                reply.opened(fh, 0);
            }
            Err(e) => {
                error!("open failed: {}", e);
                reply.error(e.to_errno());
            }
        }
    }

    // Use read_bytes helper
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
        debug!(
            "read called for ino {}, fh {}, offset {}, size {}",
            ino, fh, offset, size
        );
        match self.read_bytes(ino, offset as u64, size as usize) {
            Ok(data) => {
                reply.data(&data);
            }
            Err(e) => {
                error!("read failed: {}", e);
                reply.error(e.to_errno());
            }
        }
    }

    // Use create_directory helper
    fn mkdir(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        mode: u32,
        umask: u32,
        reply: fuser::ReplyEntry,
    ) {
        debug!("mkdir called for parent {}, name {:?}", parent, name);
        // Calculate actual permissions
        let actual_mode = mode & !umask;
        match self.create_directory(parent, name, actual_mode) {
            Ok((_inode, attr)) => {
                reply.entry(&TTL, &attr, 0);
            }
            Err(e) => {
                error!("mkdir failed: {}", e);
                reply.error(e.to_errno());
            }
        }
    }

    fn rmdir(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: fuser::ReplyEmpty) {
        debug!("rmdir called for parent {}, name {:?}", parent, name);

        match self.delete_directory(parent, name) {
            Ok(_) => {
                reply.ok();
            }
            Err(e) => {
                error!("rmdir failed: {}", e);
                reply.error(e.to_errno());
            }
        }
    }

    fn unlink(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: fuser::ReplyEmpty) {
        debug!("unlink called for parent {}, name {:?}", parent, name);

        match self.delete_directory(parent, name) {
            Ok(_) => {
                reply.ok();
            }
            Err(e) => {
                error!("unlink failed: {}", e);
                reply.error(e.to_errno());
            }
        }
    }

    fn create(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        mode: u32,
        umask: u32,
        flags: i32,
        reply: fuser::ReplyCreate,
    ) {
        debug!(
            "create called for parent {}, name {:?}, mode {:o}, flags {:o}",
            parent, name, mode, flags
        );
        match self.create_file(parent, name, mode, umask, flags) {
            Ok((ttl, attr, rdev, fh, write_flags)) => {
                reply.created(&ttl, &attr, rdev, fh, write_flags);
            }
            Err(e) => {
                error!("create failed: {}", e);
                reply.error(e.to_errno());
            }
        }
    }
    fn write(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        fh: u64,
        offset: i64,
        data: &[u8],
        write_flags: u32,
        flags: i32,
        lock_owner: Option<u64>,
        reply: fuser::ReplyWrite,
    ) {
        debug!(
            "write called for ino {}, fh {}, offset {}, size {}",
            ino,
            fh,
            offset,
            data.len()
        );
        match self.write_bytes(ino, fh, offset, data, write_flags, flags, lock_owner) {
            Ok(written) => {
                reply.written(written);
            }
            Err(e) => {
                error!("write failed: {}", e);
                reply.error(e.to_errno());
            }
        }
    }

    fn setattr(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<fuser::TimeOrNow>,
        _mtime: Option<fuser::TimeOrNow>,
        _ctime: Option<SystemTime>,
        fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        debug!(
            "setattr called for ino {} with mode {:?}, uid {:?}, gid {:?}, size {:?}, flags {:?}",
            ino, mode, uid, gid, size, flags
        );
        // Return error for unsupported operations
        reply.error(libc::ENOSYS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::http_client::{HttpBackend, HttpError};
    use crate::fs::types::FileEntry;
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use std::time::SystemTime;

    struct FakeBackend {
        listing: Mutex<HashMap<String, Vec<FileEntry>>>,
        metadata: Mutex<HashMap<String, FileEntry>>,
        contents: Mutex<HashMap<String, Vec<u8>>>,
    }

    impl FakeBackend {
        fn new() -> Self {
            let mut listing_map = HashMap::new();
            let root_children = vec![
                FileEntry {
                    name: "f.txt".to_string(),
                    is_dir: false,
                    size: 10,
                    modified: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                    permissions: 0o644,
                },
                FileEntry {
                    name: "dir".to_string(),
                    is_dir: true,
                    size: 0,
                    modified: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                    permissions: 0o755,
                },
            ];
            listing_map.insert("/".to_string(), root_children.clone());

            listing_map.insert(
                "/dir".to_string(),
                vec![FileEntry {
                    name: "inner.txt".to_string(),
                    is_dir: false,
                    size: 5,
                    modified: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                    permissions: 0o600,
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
                    modified: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                    permissions: 0o644,
                },
            );
            metadata_map.insert(
                "/dir".to_string(),
                FileEntry {
                    name: "dir".to_string(),
                    is_dir: true,
                    size: 0,
                    modified: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                    permissions: 0o755,
                },
            );
            metadata_map.insert(
                "/dir/inner.txt".to_string(),
                FileEntry {
                    name: "inner.txt".to_string(),
                    is_dir: false,
                    size: 5,
                    modified: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                    permissions: 0o600,
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

        async fn put_file_stream(&self, path: &str, data: Vec<u8>) -> Result<(), HttpError> {
            let mut contents = self.contents.lock().unwrap();
            contents.insert(path.to_string(), data.clone());
            let mut metadata = self.metadata.lock().unwrap();
            metadata.insert(
                path.to_string(),
                FileEntry {
                    name: path.split('/').last().unwrap_or("").to_string(),
                    is_dir: false,
                    size: data.len() as u64,
                    modified: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                    permissions: 0o644,
                },
            );
            Ok(())
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
            modified: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            permissions: 0o644,
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
            modified: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            permissions: 0o755,
        };
        let attrd = fs.file_entry_to_attr(&entry_dir, 43);
        assert_eq!(attrd.kind, FileType::Directory);
        assert_eq!(attrd.perm, 0o755);
        // blocks calculation: (size + 511) / 512
        assert_eq!(attr.blocks, (1234 + 511) / 512);
    }

    #[test]
    fn test_readdir_and_inode_mapping() {
        let backend = Arc::new(FakeBackend::new());
        let mut fs = RemoteFileSystem::new(backend.clone());

        // root readdir
        let entries = fs
            .readdir_path(FUSE_ROOT_ID)
            .expect("readdir should succeed");
        assert_eq!(entries.len(), 2);
        let names: Vec<String> = entries.iter().map(|(_, e)| e.name.clone()).collect();
        assert!(names.contains(&"f.txt".to_string()));
        assert!(names.contains(&"dir".to_string()));
    }

    #[test]
    fn test_open_and_read_flow() {
        let backend = Arc::new(FakeBackend::new());
        let mut fs = RemoteFileSystem::new(backend.clone());

        // open existing file
        let ino = fs.get_inode_for_path("/f.txt");
        assert!(fs.open_path(ino).is_ok());
        // open non-existing file
        let fake_ino = fs.get_inode_for_path("/nonexistent.txt");
        assert!(matches!(fs.open_path(fake_ino), Err(HttpError::NotFound)));
        // read existing file
        let data = fs.read_bytes(ino, 0, 10).expect("read should succeed");
        assert_eq!(data, b"0123456789".to_vec());
        // read with offset and length
        let part = fs.read_bytes(ino, 2, 5).expect("read should succeed");
        assert_eq!(part, b"23456".to_vec());
        // read beyond EOF
        let beyond = fs.read_bytes(ino, 15, 5).expect("read should succeed");
        assert_eq!(beyond.len(), 0);
        // read non-existing file
        assert!(matches!(
            fs.read_bytes(fake_ino, 0, 10),
            Err(HttpError::NotFound)
        ));
    }

    #[test]
    fn test_readdir_entries_offsets() {
        let backend = Arc::new(FakeBackend::new());
        let mut fs = RemoteFileSystem::new(backend.clone());

        let ino = FUSE_ROOT_ID;
        // offset 0: should return all entries
        let all = fs
            .readdir_entries(ino, 0)
            .expect("readdir_entries should succeed");
        assert_eq!(all.len(), 4); // ., .., f.txt, dir

        // offset 1: should skip "."
        let skip_dot = fs
            .readdir_entries(ino, 1)
            .expect("readdir_entries should succeed");
        assert_eq!(skip_dot.len(), 3); // .., f.txt, dir

        // offset 2: should skip ".", ".."
        let skip_dot_dot = fs
            .readdir_entries(ino, 2)
            .expect("readdir_entries should succeed");
        assert_eq!(skip_dot_dot.len(), 2); // f.txt, dir

        // offset beyond: should return empty
        let beyond = fs
            .readdir_entries(ino, 10)
            .expect("readdir_entries should succeed");
        assert_eq!(beyond.len(), 0);
    }

    #[test]
    fn test_lookup() {
        let backend = Arc::new(FakeBackend::new());
        let mut fs = RemoteFileSystem::new(backend.clone());

        // lookup existing file
        let (ino, attr) = fs
            .lookup_path(FUSE_ROOT_ID, "f.txt")
            .expect("lookup should succeed");
        assert_eq!(attr.kind, FileType::RegularFile);
        assert_eq!(attr.size, 10);
        assert_eq!(fs.get_path_for_inode(ino).unwrap(), "/f.txt");

        // lookup existing directory
        let (d_ino, d_attr) = fs
            .lookup_path(FUSE_ROOT_ID, "dir")
            .expect("lookup should succeed");
        assert_eq!(d_attr.kind, FileType::Directory);
        assert_eq!(fs.get_path_for_inode(d_ino).unwrap(), "/dir");

        // lookup non-existing entry
        assert!(matches!(
            fs.lookup_path(FUSE_ROOT_ID, "nonexistent"),
            Err(HttpError::NotFound)
        ));
    }

    #[test]
    fn test_getattr() {
        let backend = Arc::new(FakeBackend::new());
        let mut fs = RemoteFileSystem::new(backend.clone());

        // getattr root
        let root_attr = fs
            .getattr_path(FUSE_ROOT_ID)
            .expect("getattr should succeed");
        assert_eq!(root_attr.kind, FileType::Directory);

        // getattr existing file
        let f_ino = fs.get_inode_for_path("/f.txt");
        let f_attr = fs.getattr_path(f_ino).expect("getattr should succeed");
        assert_eq!(f_attr.kind, FileType::RegularFile);
        assert_eq!(f_attr.size, 10);

        // getattr existing directory
        let d_ino = fs.get_inode_for_path("/dir");
        let d_attr = fs.getattr_path(d_ino).expect("getattr should succeed");
        assert_eq!(d_attr.kind, FileType::Directory);

        // getattr non-existing inode
        let fake_ino = 9999;
        assert!(matches!(
            fs.getattr_path(fake_ino),
            Err(HttpError::NotFound)
        ));
    }
}
