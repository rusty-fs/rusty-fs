#[cfg(test)]
mod tests {
    use super::super::RemoteFileSystem;
    use crate::fs::http_client::HttpError;
    use crate::fs::path_utils;
    use crate::fs::test_utils::FakeBackend;
    use fuser::FUSE_ROOT_ID;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn test_parent_path_cases() {
        assert_eq!(path_utils::parent_path("/"), "/");
        assert_eq!(path_utils::parent_path("/foo"), "/");
        assert_eq!(path_utils::parent_path("/a/b/c"), "/a/b");
        assert_eq!(path_utils::parent_path("file"), "/");
        assert_eq!(path_utils::parent_path("/a/"), "/a");
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
        use crate::fs::types::FileEntry;
        use fuser::FileType;

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
            .readdir(FUSE_ROOT_ID)
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
        assert!(fs.open(ino).is_ok());
        // open non-existing file
        let fake_ino = fs.get_inode_for_path("/nonexistent.txt");
        assert!(matches!(fs.open(fake_ino), Err(HttpError::NotFound)));
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
        use fuser::FileType;

        let backend = Arc::new(FakeBackend::new());
        let mut fs = RemoteFileSystem::new(backend.clone());

        // lookup existing file
        let (ino, attr) = fs
            .lookup(FUSE_ROOT_ID, "f.txt")
            .expect("lookup should succeed");
        assert_eq!(attr.kind, FileType::RegularFile);
        assert_eq!(attr.size, 10);
        assert_eq!(fs.get_path_for_inode(ino).unwrap(), "/f.txt");

        // lookup existing directory
        let (d_ino, d_attr) = fs
            .lookup(FUSE_ROOT_ID, "dir")
            .expect("lookup should succeed");
        assert_eq!(d_attr.kind, FileType::Directory);
        assert_eq!(fs.get_path_for_inode(d_ino).unwrap(), "/dir");

        // lookup non-existing entry
        assert!(matches!(
            fs.lookup(FUSE_ROOT_ID, "nonexistent"),
            Err(HttpError::NotFound)
        ));
    }

    #[test]
    fn test_getattr() {
        use fuser::FileType;

        let backend = Arc::new(FakeBackend::new());
        let mut fs = RemoteFileSystem::new(backend.clone());

        // getattr root
        let root_attr = fs
            .getattr(FUSE_ROOT_ID)
            .expect("getattr should succeed");
        assert_eq!(root_attr.kind, FileType::Directory);

        // getattr existing file
        let f_ino = fs.get_inode_for_path("/f.txt");
        let f_attr = fs.getattr(f_ino).expect("getattr should succeed");
        assert_eq!(f_attr.kind, FileType::RegularFile);
        assert_eq!(f_attr.size, 10);

        // getattr existing directory
        let d_ino = fs.get_inode_for_path("/dir");
        let d_attr = fs.getattr(d_ino).expect("getattr should succeed");
        assert_eq!(d_attr.kind, FileType::Directory);

        // getattr non-existing inode
        let fake_ino = 9999;
        assert!(matches!(
            fs.getattr(fake_ino),
            Err(HttpError::NotFound)
        ));
    }
}
