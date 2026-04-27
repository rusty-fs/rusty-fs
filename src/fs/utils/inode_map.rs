use fuser::FUSE_ROOT_ID;
use std::collections::HashMap;

/// Manages the bidirectional mapping between inodes and file paths
/// Centralizes all inode allocation and path lookup logic
#[derive(Debug)]
pub struct InodeMapper {
    inode_to_path: HashMap<u64, String>,
    path_to_inode: HashMap<String, u64>,
    next_inode: u64,
}

impl InodeMapper {
    /// Create a new InodeMapper with the root directory initialized
    pub fn new() -> Self {
        let mut mapper = Self {
            inode_to_path: HashMap::new(),
            path_to_inode: HashMap::new(),
            next_inode: 2,
        };
        mapper.inode_to_path.insert(FUSE_ROOT_ID, "/".to_string());
        mapper.path_to_inode.insert("/".to_string(), FUSE_ROOT_ID);
        mapper
    }

    /// Get or create an inode for the given path
    pub fn get_or_create_inode(&mut self, path: &str) -> u64 {
        if let Some(&ino) = self.path_to_inode.get(path) {
            return ino;
        }
        let ino = self.next_inode;
        self.next_inode += 1;
        self.inode_to_path.insert(ino, path.to_string());
        self.path_to_inode.insert(path.to_string(), ino);
        ino
    }

    /// Get the path for the given inode
    pub fn path_for(&self, inode: u64) -> Option<String> {
        self.inode_to_path.get(&inode).cloned()
    }

    /// Remove an inode and its associated path
    pub fn remove(&mut self, path: &str) {
        if let Some(ino) = self.path_to_inode.remove(path) {
            self.inode_to_path.remove(&ino);
        }
    }

    /// Rename a mapped path to a new path. If the old path exists in the mapper,
    /// preserve its inode and move the mapping to the new path. If the old path
    /// is not known, ensure the new path has a mapping (create one).
    pub fn rename(&mut self, old_path: &str, new_path: &str) {
        if let Some(ino) = self.path_to_inode.remove(old_path) {
            // update inode -> path and path -> inode
            self.inode_to_path.insert(ino, new_path.to_string());
            self.path_to_inode.insert(new_path.to_string(), ino);
        } else {
            // if old path not present, just ensure new_path has an inode
            let _ = self.get_or_create_inode(new_path);
        }
    }

    /// Check if a path exists in the mapper
    pub fn contains(&self, path: &str) -> bool {
        self.path_to_inode.contains_key(path)
    }

    /// Clear all mappings except root
    pub fn clear_except_root(&mut self) {
        let root_ino = FUSE_ROOT_ID;
        self.inode_to_path.clear();
        self.path_to_inode.clear();
        self.inode_to_path.insert(root_ino, "/".to_string());
        self.path_to_inode.insert("/".to_string(), root_ino);
        self.next_inode = 2;
    }
}

impl Default for InodeMapper {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_mapper_has_root() {
        let mapper = InodeMapper::new();
        assert_eq!(mapper.path_for(FUSE_ROOT_ID), Some("/".to_string()));
    }

    #[test]
    fn test_get_or_create_inode() {
        let mut mapper = InodeMapper::new();
        let ino1 = mapper.get_or_create_inode("/file.txt");
        let ino2 = mapper.get_or_create_inode("/file.txt");
        assert_eq!(ino1, ino2);
        assert!(ino1 != FUSE_ROOT_ID);
    }

    #[test]
    fn test_path_for() {
        let mut mapper = InodeMapper::new();
        let ino = mapper.get_or_create_inode("/test.txt");
        assert_eq!(mapper.path_for(ino), Some("/test.txt".to_string()));
    }

    #[test]
    fn test_remove() {
        let mut mapper = InodeMapper::new();
        let ino = mapper.get_or_create_inode("/temp.txt");
        assert!(mapper.contains("/temp.txt"));
        mapper.remove("/temp.txt");
        assert!(!mapper.contains("/temp.txt"));
        assert_eq!(mapper.path_for(ino), None);
    }

    #[test]
    fn test_clear_except_root() {
        let mut mapper = InodeMapper::new();
        mapper.get_or_create_inode("/file1.txt");
        mapper.get_or_create_inode("/file2.txt");
        mapper.clear_except_root();
        assert_eq!(mapper.path_for(FUSE_ROOT_ID), Some("/".to_string()));
        assert!(!mapper.contains("/file1.txt"));
        assert!(!mapper.contains("/file2.txt"));
    }
}
