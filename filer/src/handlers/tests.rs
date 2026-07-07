use super::*;
use axum::http::HeaderValue;
use proptest::prelude::*;
use std::sync::Arc;
use tokio::fs as tfs;

proptest! {
    #[test]
    fn test_parse_content_range_valid(start in 0u64..1000, end in 1000u64..2000) {
        let input = format!("bytes {}-{}/{}", start, end, end + 1);
        let res = parse_content_range(&input);
        assert!(res.is_ok());
        let (s, trunc, total) = res.unwrap();
        assert_eq!(s, start);
        assert!(!trunc);
        assert_eq!(total, Some(end + 1));
    }

    #[test]
    fn test_parse_content_range_unknown_total(start in 0u64..1000, end in 1000u64..2000) {
        let input = format!("bytes {}-{}/*", start, end);
        let res = parse_content_range(&input);
        assert!(res.is_ok());
        let (s, trunc, total) = res.unwrap();
        assert_eq!(s, start);
        assert!(!trunc);
        assert_eq!(total, None);
    }

    #[test]
    fn test_parse_range_header_valid(start in 0u64..1000, end in 1000u64..2000) {
        let input = format!("bytes={}-{}", start, end);
        let hv = HeaderValue::from_str(&input).unwrap();
        let res = parse_range_header(Some(&hv));
        assert!(res.is_ok());
        let parsed = res.unwrap();
        assert!(parsed.is_some());
        let (s, e) = parsed.unwrap();
        assert_eq!(s, start);
        assert_eq!(e, Some(end));
    }

    #[test]
    fn test_parse_range_header_no_end(start in 0u64..1000) {
        let input = format!("bytes={}-", start);
        let hv = HeaderValue::from_str(&input).unwrap();
        let res = parse_range_header(Some(&hv));
        assert!(res.is_ok());
        let parsed = res.unwrap();
        assert!(parsed.is_some());
        let (s, e) = parsed.unwrap();
        assert_eq!(s, start);
        assert_eq!(e, None);
    }
}

#[test]
fn test_parse_content_range_invalid() {
    assert!(parse_content_range("invalid").is_err());
    assert!(parse_content_range("bytes 10-20").is_err());
    assert!(parse_content_range("bytes 10/20").is_err());
}

#[test]
fn test_parse_range_header_invalid() {
    let hv = HeaderValue::from_static("invalid");
    assert!(parse_range_header(Some(&hv)).is_err());

    let hv = HeaderValue::from_static("bytes=10-20,30-40");
    assert!(parse_range_header(Some(&hv)).is_err());

    let hv = HeaderValue::from_static("bytes=-20");
    assert!(parse_range_header(Some(&hv)).is_err());
}

#[tokio::test]
async fn test_mkdir_and_delete() {
    let base_path = std::env::temp_dir().join(format!("filer_test_{}", std::process::id()));
    let _ = tfs::remove_dir_all(&base_path).await; // Clean up before test
    tfs::create_dir_all(&base_path).await.unwrap();

    let base_dir = Arc::new(base_path.to_string_lossy().to_string());

    // Create a directory
    let dir_name = "test_dir".to_string();
    let res = mkdir(Path(dir_name.clone()), Extension(base_dir.clone())).await;
    assert!(res.is_ok());
    assert!(base_path.join(&dir_name).exists());

    // Delete the directory
    let res = delete_path(Path(dir_name.clone()), Extension(base_dir.clone())).await;
    assert!(res.is_ok());
    assert!(!base_path.join(&dir_name).exists());

    // Clean up
    let _ = tfs::remove_dir_all(&base_path).await;
}
