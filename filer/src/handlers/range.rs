use axum::http::{HeaderValue, StatusCode};
use std::str::FromStr;

pub(super) fn parse_range_header(
    range_hdr: Option<&HeaderValue>,
) -> Result<Option<(u64, Option<u64>)>, StatusCode> {
    let hv = match range_hdr {
        Some(h) => h,
        None => return Ok(None),
    };
    let s = hv.to_str().map_err(|_| StatusCode::BAD_REQUEST)?;
    if !s.starts_with("bytes=") {
        return Err(StatusCode::BAD_REQUEST);
    }
    let spec = &s["bytes=".len()..];
    if spec.contains(',') {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut parts = spec.splitn(2, '-');
    let start_str = parts.next().unwrap_or("");
    let end_str = parts.next().unwrap_or("");
    if start_str.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let start = u64::from_str(start_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    let end_opt = if end_str.is_empty() {
        None
    } else {
        Some(u64::from_str(end_str).map_err(|_| StatusCode::BAD_REQUEST)?)
    };
    Ok(Some((start, end_opt)))
}

pub(super) fn parse_content_range(value: &str) -> Result<(u64, bool, Option<u64>), ()> {
    if !value.starts_with("bytes ") {
        return Err(());
    }

    let parts: Vec<&str> = value[6..].split('/').collect();
    if parts.len() != 2 {
        return Err(());
    }

    let range_parts: Vec<&str> = parts[0].split('-').collect();
    if range_parts.len() != 2 {
        return Err(());
    }

    let start: u64 = range_parts[0].parse().map_err(|_| ())?;
    let total_size: Option<u64> = if parts[1] == "*" {
        None
    } else {
        Some(parts[1].parse().map_err(|_| ())?)
    };
    Ok((start, false, total_size))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

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
}
