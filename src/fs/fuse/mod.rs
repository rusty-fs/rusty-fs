// FUSE trait implementation for RemoteFileSystem

use crate::fs::remote_fs::RemoteFileSystem;
use fuser::{
    Filesystem, KernelConfig, ReplyAttr, ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyStatfs,
    ReplyXattr, Request,
};
use libc::{ENOENT, EOPNOTSUPP};
use std::ffi::OsStr;
use std::time::SystemTime;
use tracing::{debug, error, info, trace};

impl Filesystem for RemoteFileSystem {
    fn init(&mut self, _req: &Request<'_>, config: &mut KernelConfig) -> Result<(), libc::c_int> {
        let desired_write = self.config.max_buffer_size.min(16 * 1024 * 1024) as u32;
        let desired_readahead = self.config.chunk_size.min(16 * 1024 * 1024) as u32;

        let negotiated_write = config
            .set_max_write(desired_write)
            .unwrap_or_else(|limit| limit);
        let negotiated_readahead = config
            .set_max_readahead(desired_readahead)
            .unwrap_or_else(|limit| limit);

        info!(
            "FUSE init: requested max_write={} readahead={}, negotiated max_write={} readahead={}",
            desired_write, desired_readahead, negotiated_write, negotiated_readahead
        );
        Ok(())
    }

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

    fn open(&mut self, _req: &Request<'_>, ino: u64, flags: i32, reply: fuser::ReplyOpen) {
        debug!("open called for ino {}", ino);
        match self.open(ino) {
            Ok(fh) => {
                // Use FOPEN_DIRECT_IO (1 << 0) to bypass page cache so writes are not lost
                let open_flags = fuser::consts::FOPEN_DIRECT_IO;
                reply.opened(fh, open_flags);
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
        trace!(
            "read called for ino {}, fh {}, offset {}, size {}",
            ino,
            fh,
            offset,
            size
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

    fn rename(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
        _flags: u32,
        reply: fuser::ReplyEmpty,
    ) {
        debug!("rename called: parent={}, name={:?}, newparent={}, newname={:?}", parent, name, newparent, newname);

        match self.rename(parent, name, newparent, newname) {
            Ok(_) => reply.ok(),
            Err(e) => {
                error!("rename failed: {}", e);
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
            Ok((ttl, attr, rdev, fh, mut write_flags)) => {
                // Use FOPEN_DIRECT_IO to bypass page cache so writes are not lost
                write_flags |= fuser::consts::FOPEN_DIRECT_IO;
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
        trace!(
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
            Ok(mut attr) => {
                if let Some(m) = _mode { attr.perm = m as u16; }
                if let Some(u) = _uid { attr.uid = u; }
                if let Some(g) = _gid { attr.gid = g; }
                if let Some(f) = _flags { attr.flags = f; }

                if let Some(t) = _atime {
                    attr.atime = match t {
                        fuser::TimeOrNow::SpecificTime(st) => st,
                        fuser::TimeOrNow::Now => std::time::SystemTime::now(),
                    };
                }
                if let Some(t) = _mtime {
                    attr.mtime = match t {
                        fuser::TimeOrNow::SpecificTime(st) => st,
                        fuser::TimeOrNow::Now => std::time::SystemTime::now(),
                    };
                }
                if let Some(t) = _ctime { attr.ctime = t; }
                if let Some(t) = _crtime { attr.crtime = t; }

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
        trace!("flush called for ino {} fh {}", ino, fh);

        let finalization_info = {
            if let Some(state) = self.fh_manager.get_fh_state(fh) {
                state.tx = None;
                if state.dirty {
                    state.dirty = false;
                    Some(state.file_size)
                } else {
                    None
                }
            } else {
                None
            }
        };

        if let Some(final_file_size) = finalization_info {
            if let Some(path) = self.get_path_for_inode(ino) {
                let client = self.http_client.clone();
                match crate::fs::utils::runtime().block_on(async move {
                    client
                        .put_file_stream(&path, reqwest::Body::from(Vec::<u8>::new()), Some(0), final_file_size)
                        .await
                }) {
                    Ok(_) => {
                        trace!("flush: finalized buffer for fh {} with size {:?}", fh, final_file_size);
                        reply.ok();
                    }
                    Err(e) => {
                        error!("flush: failed to finalize buffer for fh {}: {}", fh, e);
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


    fn fallocate(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _fh: u64,
        _offset: i64,
        _length: i64,
        _mode: i32,
        reply: ReplyEmpty,
    ) {
        reply.ok();
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
        trace!("release called for fh {}", fh);

        let finalization_info = {
            if let Some(state) = self.fh_manager.get_fh_state(fh) {
                // Drop the sender to close the HTTP stream immediately
                state.tx = None;
                if state.dirty {
                    Some(state.file_size)
                } else {
                    None
                }
            } else {
                None
            }
        };

        if let Some(final_file_size) = finalization_info {
            if let Some(path) = self.get_path_for_inode(ino) {
                let client = self.http_client.clone();
                match crate::fs::utils::runtime().block_on(async move {
                    client
                        .put_file_stream(&path, reqwest::Body::from(Vec::<u8>::new()), Some(0), final_file_size)
                        .await
                }) {
                    Ok(_) => trace!("release: finalized fh {} with size {:?}", fh, final_file_size),
                    Err(e) => error!("release: failed to finalize fh {}: {}", fh, e),
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
        reply.error(libc::ENOTSUP);
    }

    fn getxattr(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _name: &OsStr,
        _size: u32,
        reply: ReplyXattr,
    ) {
        reply.error(libc::ENOTSUP);
    }

    fn listxattr(&mut self, _req: &Request<'_>, _ino: u64, _size: u32, reply: ReplyXattr) {
        reply.error(libc::ENOTSUP);
    }

    fn removexattr(&mut self, _req: &Request<'_>, _ino: u64, _name: &OsStr, reply: ReplyEmpty) {
        reply.error(libc::ENOTSUP);
    }

    fn access(&mut self, _req: &Request<'_>, _ino: u64, _mask: i32, reply: ReplyEmpty) {
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
            1_000_000, // blocks
            900_000,   // bfree (free blocks)
            900_000,   // bavail (available blocks)
            1_000_000, // files
            900_000,   // ffree (free files)
            4096,      // block size (common value)
            255,       // max filename length
            4096,      // fragment size
        );
    }
}
