/// Path manipulation utilities for POSIX-like paths
/// Reduces duplication and centralizes path logic

/// Join a parent path and a name into a single path
pub fn join_path(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{}", name)
    } else {
        format!("{}/{}", parent, name)
    }
}

/// Get the parent directory path of a given path
pub fn parent_path(path: &str) -> String {
    if path == "/" {
        return "/".to_string();
    }
    match path.rfind('/') {
        Some(0) => "/".to_string(),
        Some(idx) => path[..idx].to_string(),
        None => "/".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_join_path() {
        assert_eq!(join_path("/", "file.txt"), "/file.txt");
        assert_eq!(join_path("/dir", "file.txt"), "/dir/file.txt");
        assert_eq!(join_path("/a/b", "c"), "/a/b/c");
    }

    #[test]
    fn test_parent_path() {
        assert_eq!(parent_path("/"), "/");
        assert_eq!(parent_path("/foo"), "/");
        assert_eq!(parent_path("/a/b/c"), "/a/b");
        assert_eq!(parent_path("file"), "/");
        assert_eq!(parent_path("/a/"), "/a");
    }
}
