use std::collections::{HashMap, BTreeMap};

/// Represents the state of an open file handle
#[derive(Debug, Clone)]
pub struct FhState {
    /// Local buffer for accumulated writes (max 1MB per chunk)
    pub buf: Vec<u8>,
    /// Current offset within the buffer
    pub buf_offset: u64,
    /// Whether this file has been modified
    pub dirty: bool,
    /// File size known from metadata (None if new file)
    pub file_size: Option<u64>,
}

/// Manages file handles and their associated state
pub struct FhManager {
    next_fh: u64,
    fh_map: HashMap<u64, FhState>,
}

impl FhManager {
    /// Create a new file handle manager
    pub fn new() -> Self {
        Self {
            next_fh: 1,
            fh_map: HashMap::new(),
        }
    }

    /// Allocate a new file handle with empty state
    pub fn alloc_fh(&mut self) -> u64 {
        let fh = self.next_fh;
        self.next_fh += 1;
        self.fh_map.insert(
            fh,
            FhState {
                buf: Vec::with_capacity(1024 * 1024), // 1MB capacity
                buf_offset: 0,
                dirty: false,
                file_size: None,
            },
        );
        fh
    }

    /// Get mutable reference to file handle state
    pub fn get_fh_state(&mut self, fh: u64) -> Option<&mut FhState> {
        self.fh_map.get_mut(&fh)
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

    #[test]
    fn test_alloc_fh() {
        let mut manager = FhManager::new();
        let fh1 = manager.alloc_fh();
        let fh2 = manager.alloc_fh();
        assert_ne!(fh1, fh2);
        assert_eq!(fh1, 1);
        assert_eq!(fh2, 2);
    }

    #[test]
    fn test_get_fh_state() {
        let mut manager = FhManager::new();
        let fh = manager.alloc_fh();
        let state = manager.get_fh_state(fh);
        assert!(state.is_some());
        let state = state.unwrap();
        assert_eq!(state.buf.len(), 0);
        assert!(!state.dirty);
        assert_eq!(state.buf_offset, 0);
        assert!(state.file_size.is_none());
    }

    #[test]
    fn test_release_fh() {
        let mut manager = FhManager::new();
        let fh = manager.alloc_fh();
        assert!(manager.get_fh_state(fh).is_some());
        manager.release_fh(fh);
        assert!(manager.get_fh_state(fh).is_none());
    }
}
