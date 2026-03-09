use crate::fs::remote_fs::RemoteFileSystem;
use fuser::{Filesystem, ReplyAttr, ReplyDirectory, ReplyEntry, ReplyEmpty, Request};
use libc::ENOENT;
use std::ffi::OsStr;
use std::time::SystemTime;
use tracing::{debug, error};

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
        match self.lookup(parent, name_str) {
            Ok((_, attr)) => {
                reply.entry(&self.config.ttl, &attr, 0);
            }
            Err(e) => {
                error!("lookup failed: {:#?} {}", name, e);
                reply.error(e.to_errno());
            }
        }
    }

    // Use getattr helper
    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        // debug!("getattr: ino={}", ino);
        match self.getattr(ino) {
            Ok(attr) => {
                reply.attr(&self.config.ttl, &attr);
            }
            Err(e) => {
                error!("getattr failed: {}", e);
                reply.error(e.to_errno());
            }
        }
    }

    // Use readdir helper
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
        match self.open(ino) {
            Ok(fh) => {
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
        _flags: i32,
        _lock_owner: Option<u64>,
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
            Ok((_, attr)) => {
                reply.entry(&self.config.ttl, &attr, 0);
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
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<fuser::TimeOrNow>,
        _mtime: Option<fuser::TimeOrNow>,
        _ctime: Option<SystemTime>,
        _fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        debug!("setattr called for ino {} with size {:?}", ino, size);

        match self.setattr(ino, _mode, _uid, _gid, size) {
            Ok(attr) => {
                reply.attr(&self.config.ttl, &attr);
            }
            Err(e) => {
                error!("setattr failed: {}", e);
                reply.error(e.to_errno());
            }
        }
    }

    fn release(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        fh: u64,
        _flags: i32,
        _lock_owner: Option<u64>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        debug!("release called for fh {}", fh);
        
        // Extract data to flush before releasing the mutable borrow
        let data_to_flush = {
            if let Some(state) = self.fh_manager.get_fh_state(fh) {
                if state.dirty && !state.buf.is_empty() {
                    Some(state.buf.clone())
                } else {
                    None
                }
            } else {
                None
            }
        };
        
        // Now flush if we have data
        if let Some(data) = data_to_flush {
            if let Some(path) = self.get_path_for_inode(ino) {
                let client = self.http_client.clone();
                match crate::fs::runtime::runtime().block_on(async move {
                    client.put_file_stream(&path, data, None, None).await
                }) {
                    Ok(_) => {
                        debug!("release: flushed buffer for fh {}", fh);
                    }
                    Err(e) => {
                        error!("release: failed to flush buffer for fh {}: {}", fh, e);
                    }
                }
            }
        }
        
        // Release the file handle
        self.fh_manager.release_fh(fh);
        reply.ok();
    }
}
