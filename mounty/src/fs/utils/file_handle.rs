use bytes::Bytes;
use reqwest::Error as ReqwestError;
use std::cell::RefCell;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Represents the state of an open file handle
#[derive(Debug)]
pub struct FhState {
    /// Inode this handle belongs to
    pub ino: u64,
    /// Open flags (e.g. O_RDONLY, O_WRONLY, O_RDWR)
    pub open_flags: i32,
    /// File size known from metadata (None if new file)
    pub file_size: Option<u64>,

    // Fase 1 — Read-ahead buffer (RefCell for interior mutability since read_bytes takes &self)
    pub read_buf: RefCell<Option<Vec<u8>>>,
    pub read_buf_offset: RefCell<u64>,
    pub read_buf_len: RefCell<usize>,
    pub read_buf_cap: usize,

    // Fase 2 — Write batching buffer
    pub write_buf: Vec<u8>,
    pub write_buf_offset: u64,
    pub write_buf_cap: usize,
    /// Track errors from pending PUT operations
    pub pending_write_errors: RefCell<Vec<String>>,
    /// Whether any write has been attempted (for release flush)
    pub dirty: bool,

    // Legacy streaming field (kept for backward compat, unused in Fase 2)
    pub tx: Option<mpsc::Sender<Result<Bytes, ReqwestError>>>,
    pub current_offset: u64,
}

/// Manages file handles and their associated state
pub struct FhManager {
    next_fh: u64,
    fh_map: HashMap<u64, FhState>,
    read_buf_cap: usize,
    write_buf_cap: usize,
}

impl FhManager {
    /// Create a new file handle manager
    pub fn new() -> Self {
        Self {
            next_fh: 1,
            fh_map: HashMap::new(),
            read_buf_cap: 4 * 1024 * 1024,  // 4 MB default
            write_buf_cap: 1 * 1024 * 1024, // 1 MB default
        }
    }

    /// Create with custom buffer sizes
    pub fn with_buffer_sizes(read_buf_cap: usize, write_buf_cap: usize) -> Self {
        Self {
            next_fh: 1,
            fh_map: HashMap::new(),
            read_buf_cap,
            write_buf_cap,
        }
    }

    /// Allocate a new file handle with empty state
    pub fn alloc_fh(&mut self, ino: u64, open_flags: i32) -> u64 {
        let fh = self.next_fh;
        self.next_fh += 1;
        self.fh_map.insert(
            fh,
            FhState {
                ino,
                open_flags,
                file_size: None,
                read_buf: RefCell::new(None),
                read_buf_offset: RefCell::new(0),
                read_buf_len: RefCell::new(0),
                read_buf_cap: self.read_buf_cap,
                write_buf: Vec::new(),
                write_buf_offset: 0,
                write_buf_cap: self.write_buf_cap,
                pending_write_errors: RefCell::new(Vec::new()),
                dirty: false,
                tx: None,
                current_offset: 0,
            },
        );
        fh
    }

    /// Get mutable reference to file handle state
    pub fn get_fh_state(&mut self, fh: u64) -> Option<&mut FhState> {
        self.fh_map.get_mut(&fh)
    }

    /// Read-only access to file handle state (for read_bytes which takes &self)
    pub fn get_fh_state_ref(&self, fh: u64) -> Option<&FhState> {
        self.fh_map.get(&fh)
    }

    /// Get all file handles for a given inode
    pub fn get_fhs_for_inode(&self, ino: u64) -> Vec<u64> {
        self.fh_map
            .iter()
            .filter(|(_, state)| state.ino == ino)
            .map(|(&fh, _)| fh)
            .collect()
    }

    /// Get the latest file size across all handles for an inode
    pub fn get_file_size_by_inode(&self, ino: u64) -> Option<u64> {
        self.fh_map
            .values()
            .filter(|state| state.ino == ino)
            .filter_map(|state| state.file_size)
            .max()
    }

    /// Read-only access to the file size for a handle
    pub fn get_file_size(&self, fh: u64) -> Option<u64> {
        self.fh_map.get(&fh).and_then(|s| s.file_size)
    }

    /// Release a file handle
    pub fn release_fh(&mut self, fh: u64) {
        self.fh_map.remove(&fh);
    }
}

impl Default for FhManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn test_alloc_fh() {
        let mut manager = FhManager::new();
        let fh1 = manager.alloc_fh(0, 0);
        let fh2 = manager.alloc_fh(0, 0);
        assert_ne!(fh1, fh2);
        assert_eq!(fh1, 1);
        assert_eq!(fh2, 2);
    }

    #[test]
    fn test_get_fh_state() {
        let mut manager = FhManager::new();
        let fh = manager.alloc_fh(0, 0);
        let state = manager.get_fh_state(fh);
        assert!(state.is_some());
        let state = state.unwrap();
        assert!(!state.dirty);
        assert!(state.file_size.is_none());
        assert!(state.write_buf.is_empty());
    }

    #[test]
    fn test_release_fh() {
        let mut manager = FhManager::new();
        let fh = manager.alloc_fh(0, 0);
        assert!(manager.get_fh_state(fh).is_some());
        manager.release_fh(fh);
        assert!(manager.get_fh_state(fh).is_none());
    }

    proptest! {
        #[test]
        fn prop_alloc_and_release(ino in any::<u64>(), flags in any::<i32>()) {
            let mut manager = FhManager::new();
            let fh = manager.alloc_fh(ino, flags);
            let state = manager.get_fh_state_ref(fh).unwrap();
            assert_eq!(state.ino, ino);
            assert_eq!(state.open_flags, flags);
            manager.release_fh(fh);
            assert!(manager.get_fh_state_ref(fh).is_none());
        }

        #[test]
        fn prop_multiple_fhs_for_inode(ino in any::<u64>(), count in 1..50usize) {
            let mut manager = FhManager::new();
            let mut fhs = Vec::new();
            for _ in 0..count {
                fhs.push(manager.alloc_fh(ino, 0));
            }
            let retrieved_fhs = manager.get_fhs_for_inode(ino);
            assert_eq!(retrieved_fhs.len(), count);
            for fh in fhs {
                assert!(retrieved_fhs.contains(&fh));
            }
        }

        #[test]
        fn prop_file_size_tracking(ino in any::<u64>(), sizes in proptest::collection::vec(any::<u64>(), 1..10)) {
            let mut manager = FhManager::new();
            for (i, &size) in sizes.iter().enumerate() {
                let fh = manager.alloc_fh(ino, 0);
                if i % 2 == 0 {
                    manager.get_fh_state(fh).unwrap().file_size = Some(size);
                }
            }
            let max_size = sizes.iter().enumerate().filter(|(i, _)| i % 2 == 0).map(|(_, &s)| s).max();
            assert_eq!(manager.get_file_size_by_inode(ino), max_size);
        }
    }
}
