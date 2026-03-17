// FUSE trait implementation for RemoteFileSystem

use crate::fs::remote_fs::RemoteFileSystem;
use fuser::{Filesystem, ReplyAttr, ReplyDirectory, ReplyEntry, ReplyEmpty, ReplyXattr, ReplyStatfs, Request};
use libc::{ENOENT, EOPNOTSUPP};
use std::ffi::OsStr;
use std::time::SystemTime;
use tracing::{debug, error};

impl Filesystem for RemoteFileSystem {
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

    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
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
                    let entry_offset = offset + (i as i64) + 1;
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
            ino, fh, offset, data.len()
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

    fn flush(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        fh: u64,
        _lock_owner: u64,
        reply: ReplyEmpty,
    ) {
        debug!("flush called for ino {} fh {}", ino, fh);
        
        // Flush any buffered data for this file handle
        let data_to_flush = {
            if let Some(state) = self.fh_manager.get_fh_state(fh) {
                if state.dirty && !state.buf.is_empty() {
                    Some((state.buf.clone(), state.buf_offset, state.file_size))
                } else {
                    None
                }
            } else {
                None
            }
        };
        
        if let Some((data, offset, file_size)) = data_to_flush {
            if let Some(path) = self.get_path_for_inode(ino) {
                let client = self.http_client.clone();
                match crate::fs::utils::runtime().block_on(async move {
                    client.put_file_stream(&path, data, Some(offset), file_size).await
                }) {
                    Ok(_) => {
                        debug!("flush: flushed buffer for fh {} at offset {}", fh, offset);
                        // Clear the buffer after successful flush
                        if let Some(state) = self.fh_manager.get_fh_state(fh) {
                            state.buf.clear();
                            state.dirty = false;
                        }
                        reply.ok();
                    }
                    Err(e) => {
                        error!("flush: failed to flush buffer for fh {}: {}", fh, e);
                        reply.error(e.to_errno());
                    }
                }
            } else {
                reply.ok(); // No path found, silently succeed
            }
        } else {
            // Nothing to flush, just succeed
            reply.ok();
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
        
        let (data_to_flush, final_file_size) = {
            if let Some(state) = self.fh_manager.get_fh_state(fh) {
                // Flush any remaining buffered data
                let data = if !state.buf.is_empty() {
                    Some(state.buf.clone())
                } else {
                    None
                };
                let offset = state.buf_offset;
                let file_size = state.file_size;
                (data.map(|d| (d, offset)), file_size)
            } else {
                (None, None)
            }
        };
        
        if let Some((data, offset)) = data_to_flush {
            if let Some(path) = self.get_path_for_inode(ino) {
                let client = self.http_client.clone();
                // Final flush: include the file_size to tell server the final size
                match crate::fs::utils::runtime().block_on(async move {
                    client.put_file_stream(&path, data, Some(offset), final_file_size).await
                }) {
                    Ok(_) => {
                        debug!("release: flushed buffer for fh {} at offset {}", fh, offset);
                    }
                    Err(e) => {
                        error!("release: failed to flush buffer for fh {}: {}", fh, e);
                    }
                }
            }
        }
        
        self.fh_manager.release_fh(fh);
        reply.ok();
    }

    fn setxattr(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _name: &OsStr,
        _value: &[u8],
        _flags: i32,
        _position: u32,
        reply: ReplyEmpty,
    ) {
        // Report that extended attributes are not supported
        // This tells macOS/cp to skip xattr operations rather than fail
        debug!("setxattr called (not supported)");
        reply.error(EOPNOTSUPP);
    }

    fn getxattr(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _name: &OsStr,
        _size: u32,
        reply: ReplyXattr,
    ) {
        // Return empty extended attributes
        debug!("getxattr called (returning empty)");
        reply.data(&[]);
    }

    fn listxattr(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _size: u32,
        reply: ReplyXattr,
    ) {
        // Return empty list of extended attributes
        debug!("listxattr called (returning empty)");
        reply.data(&[]);
    }

    fn removexattr(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _name: &OsStr,
        reply: ReplyEmpty,
    ) {
        // Accept removal requests without error
        debug!("removexattr called (silently accepted)");
        reply.ok();
    }

    fn access(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _mask: i32,
        reply: ReplyEmpty,
    ) {
        // Grant all access permissions
        // This prevents "operation not permitted" errors on permission checks
        debug!("access called (granting all permissions)");
        reply.ok();
    }

    fn statfs(&mut self, _req: &Request<'_>, _ino: u64, reply: fuser::ReplyStatfs) {
        // Return basic filesystem stats to satisfy statfs calls
        // These values don't need to be accurate for a remote filesystem
        debug!("statfs called");
        reply.statfs(
            1_000_000,  // blocks
            900_000,    // bfree (free blocks)
            900_000,    // bavail (available blocks)
            1_000_000,  // files
            900_000,    // ffree (free files)
            4096,       // block size (common value)
            255,        // max filename length
            4096,       // fragment size
        );
    }
}
