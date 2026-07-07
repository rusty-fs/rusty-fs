use super::types::{DirectoryListing, FileEntry};
use async_trait::async_trait;
use libc::{EACCES, EIO, ENOENT};
use reqwest::header::RANGE;
use reqwest::StatusCode;
use thiserror::Error;
use tracing::{debug, trace};

#[derive(Debug, Error)]
pub enum HttpError {
    #[error("not found")]
    NotFound,
    #[error("permission denied")]
    PermissionDenied,
    #[error("already exists")]
    AlreadyExists,
    #[error("filesystem is shutting down")]
    ShuttingDown,
    #[error("network error: {0}")]
    Network(String),
    #[error("other: {0}")]
    Other(String),
}

impl HttpError {
    pub fn to_errno(&self) -> i32 {
        match self {
            HttpError::NotFound => ENOENT,
            HttpError::PermissionDenied => EACCES,
            HttpError::AlreadyExists => libc::EEXIST,
            HttpError::ShuttingDown => libc::ESHUTDOWN,
            HttpError::Network(_) | HttpError::Other(_) => EIO,
        }
    }

    pub fn from_status(status: StatusCode, text: &str) -> Self {
        match status {
            StatusCode::NOT_FOUND => HttpError::NotFound,
            StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED => HttpError::PermissionDenied,
            StatusCode::CONFLICT => HttpError::AlreadyExists,
            _ => HttpError::Other(format!("HTTP {}: {}", status, text)),
        }
    }
}

impl From<reqwest::Error> for HttpError {
    fn from(e: reqwest::Error) -> Self {
        HttpError::Network(e.to_string())
    }
}

impl From<tokio::task::JoinError> for HttpError {
    fn from(e: tokio::task::JoinError) -> Self {
        HttpError::Other(e.to_string())
    }
}

#[async_trait]
pub trait HttpBackend: Send + Sync {
    async fn list_directory(&self, path: &str) -> Result<Vec<FileEntry>, HttpError>;
    async fn get_file_metadata(&self, path: &str) -> Result<FileEntry, HttpError>;
    async fn read_range(
        &self,
        path: &str,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>, HttpError>;
    async fn create_directory(&self, path: &str) -> Result<(), HttpError>;
    async fn delete_path(&self, path: &str) -> Result<(), HttpError>;
    async fn put_file_stream(
        &self,
        path: &str,
        body: reqwest::Body,
        offset: Option<u64>,
        total_size: Option<u64>,
    ) -> Result<(), HttpError>;
    async fn rename(&self, path: &str, dst: &str) -> Result<(), HttpError>;
    async fn update_meta(&self, path: &str, body: serde_json::Value) -> Result<(), HttpError>;
}

#[derive(Clone)]
pub struct HttpClient {
    client: reqwest::Client,
    base_url: String,
}

impl HttpClient {
    pub fn new(base_url: String) -> Self {
        let base_url = if base_url.starts_with("http://") || base_url.starts_with("https://") {
            base_url
        } else {
            format!("http://{}", base_url)
        };

        Self {
            client: reqwest::Client::new(),
            base_url,
        }
    }
}

#[async_trait]
impl HttpBackend for HttpClient {
    async fn list_directory(&self, path: &str) -> Result<Vec<FileEntry>, HttpError> {
        let url = format!("{}/list{}", self.base_url, path);
        debug!("Listing directory at URL: {}", url);

        let response = self.client.get(&url).send().await;

        match response {
            Ok(response) => {
                let response_text = response.text().await?;

                if response_text.trim().is_empty() {
                    return Err(HttpError::Other("Empty response from server".into()));
                }

                let listing: DirectoryListing =
                    serde_json::from_str(&response_text).map_err(|e| {
                        HttpError::Other(
                            format!("JSON parse error: {} - Response: {}", e, response_text).into(),
                        )
                    })?;

                Ok(listing.files)
            }
            Err(e) => {
                return Err(HttpError::Other(
                    format!("Failed to send request: {}", e),
                ));
            }
        }
    }

    async fn get_file_metadata(&self, path: &str) -> Result<FileEntry, HttpError> {
        let url = format!("{}/meta{}", self.base_url, path);
        let resp = self.client.get(&url).send().await?;
        let status = resp.status();
        if let Some(len) = resp.content_length() {
            debug!(
                "get_file_metadata response: {} content-length={}",
                status, len
            );
        } else {
            debug!("get_file_metadata response: {} (no content-length)", status);
        }
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(HttpError::from_status(status, &text));
        }
        let entry: FileEntry = serde_json::from_str(&text).map_err(|e| {
            HttpError::Other(
                format!("JSON parse error for metadata: {} - Response: {}", e, text),
            )
        })?;
        Ok(entry)
    }

    async fn read_range(
        &self,
        path: &str,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>, HttpError> {
        let end = offset.saturating_add(length as u64).saturating_sub(1);

        trace!(
            "Sending range request: {}-{} ({} bytes) for file {}",
            offset,
            end,
            length,
            path
        );

        let url = format!("{}/files{}", self.base_url, path);
        let range_header = format!("bytes={}-{}", offset, end);
        let resp = self
            .client
            .get(&url)
            .header(RANGE, range_header)
            .send()
            .await?;
        let status = resp.status();
        if !(status.is_success() || status == StatusCode::PARTIAL_CONTENT) {
            let text = resp.text().await.unwrap_or_default();
            return Err(HttpError::from_status(status, &text));
        }
        let bytes = resp.bytes().await?;
        Ok(bytes.to_vec())
    }

    async fn create_directory(&self, path: &str) -> Result<(), HttpError> {
        let url = format!("{}/mkdir{}", self.base_url, path);
        let resp = self.client.post(&url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(HttpError::from_status(status, &text));
        }
        Ok(())
    }

    async fn delete_path(&self, path: &str) -> Result<(), HttpError> {
        let url = format!("{}/files{}", self.base_url, path);
        let resp = self.client.delete(&url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(HttpError::from_status(status, &text));
        }
        Ok(())
    }

    async fn put_file_stream(
        &self,
        path: &str,
        body: reqwest::Body,
        offset: Option<u64>,
        total_size: Option<u64>,
    ) -> Result<(), HttpError> {
        let url = format!("{}/files{}", self.base_url, path);

        let mut request = self.client.put(&url).body(body);

        if let Some(start) = offset {
            let range_value = if let Some(size) = total_size {
                format!("bytes {}-*/{}", start, size)
            } else {
                format!("bytes {}-*/*", start)
            };
            request = request.header("Content-Range", range_value);
        }

        let resp = request.send().await?;
        let status = resp.status();

        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(HttpError::from_status(status, &text));
        }

        Ok(())
    }

    async fn rename(&self, path: &str, dst: &str) -> Result<(), HttpError> {
        let body = serde_json::json!({ "new_name": dst });
        self.update_meta(path, body).await
    }

    async fn update_meta(&self, path: &str, body: serde_json::Value) -> Result<(), HttpError> {
        let url = format!("{}/meta{}", self.base_url, path);
        debug!("update_meta URL: {}", url);
        let resp = self
            .client
            .patch(&url)
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(HttpError::from_status(status, &text));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::Method::{GET, POST, DELETE, PUT, PATCH};
    use httpmock::MockServer;
    use serde_json::json;
    use tokio;

    fn make_client(base_url: &str) -> HttpClient {
        HttpClient::new(base_url.to_string())
    }

    #[tokio::test]
    async fn test_list_directory_success() {
        let server = MockServer::start_async().await;
        let files = vec![
            json!({"name": "foo.txt", "is_dir": false, "size": 123, "modified": Some(1), "permissions": Some(0o644)}),
            json!({"name": "bar", "is_dir": true, "size": 0, "modified": Some(2), "permissions": Some(0o755)}),
        ];
        let body = json!({"files": files, "message": "ok"}).to_string();

        let _mock = server
            .mock_async(|when, then| {
                when.method(GET).path("/list/test");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(body.clone());
            })
            .await;

        let client = make_client(&server.base_url());
        let result = client.list_directory("/test").await.unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].name, "foo.txt");
        assert!(result[1].is_dir);
    }

    #[tokio::test]
    async fn test_get_file_metadata_success() {
        let server = MockServer::start_async().await;
        let entry = json!({"name": "foo.txt", "is_dir": false, "size": 123, "modified": Some(1), "permissions": Some(0o644)});
        let _mock = server
            .mock_async(|when, then| {
                when.method(GET).path("/meta/foo.txt");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(entry.to_string());
            })
            .await;

        let client = make_client(&server.base_url());
        let result = client.get_file_metadata("/foo.txt").await.unwrap();
        assert_eq!(result.name, "foo.txt");
        assert!(!result.is_dir);
    }

    #[tokio::test]
    async fn test_read_range_success() {
        let server = MockServer::start_async().await;
        let data = b"abcdef";
        let _mock = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/files/foo.txt")
                    .header("range", "bytes=2-4");
                then.status(206).body(&data[2..=4]);
            })
            .await;

        let client = make_client(&server.base_url());
        let result = client.read_range("/foo.txt", 2, 3).await.unwrap();
        assert_eq!(result, b"cde");
    }
    #[test]
    fn test_error_mapping() {
        let err = HttpError::from_status(reqwest::StatusCode::NOT_FOUND, "not found text");
        assert!(matches!(err, HttpError::NotFound));
        assert_eq!(err.to_errno(), libc::ENOENT);

        let err = HttpError::from_status(reqwest::StatusCode::FORBIDDEN, "forbidden text");
        assert!(matches!(err, HttpError::PermissionDenied));
        assert_eq!(err.to_errno(), libc::EACCES);

        let err = HttpError::from_status(reqwest::StatusCode::CONFLICT, "conflict text");
        assert!(matches!(err, HttpError::AlreadyExists));
        assert_eq!(err.to_errno(), libc::EEXIST);

        let err = HttpError::from_status(reqwest::StatusCode::INTERNAL_SERVER_ERROR, "error text");
        assert!(matches!(err, HttpError::Other(_)));
        assert_eq!(err.to_errno(), libc::EIO);
    }

    #[tokio::test]
    async fn test_create_directory_success() {
        let server = MockServer::start_async().await;
        let _mock = server.mock_async(|when, then| {
            when.method(POST).path("/mkdir/new_dir");
            then.status(200);
        }).await;

        let client = make_client(&server.base_url());
        let result = client.create_directory("/new_dir").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_delete_path_success() {
        let server = MockServer::start_async().await;
        let _mock = server.mock_async(|when, then| {
            when.method(DELETE).path("/files/old_dir");
            then.status(200);
        }).await;

        let client = make_client(&server.base_url());
        let result = client.delete_path("/old_dir").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_update_meta_success() {
        let server = MockServer::start_async().await;
        let _mock = server.mock_async(|when, then| {
            when.method(PATCH).path("/meta/file.txt");
            then.status(200);
        }).await;

        let client = make_client(&server.base_url());
        let body = json!({"permissions": 0o777});
        let result = client.update_meta("/file.txt", body).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_rename_success() {
        let server = MockServer::start_async().await;
        let _mock = server.mock_async(|when, then| {
            when.method(PATCH).path("/meta/old.txt");
            then.status(200);
        }).await;

        let client = make_client(&server.base_url());
        let result = client.rename("/old.txt", "new.txt").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_put_file_stream_success() {
        let server = MockServer::start_async().await;
        let _mock = server.mock_async(|when, then| {
            when.method(PUT).path("/files/file.txt")
                .header("Content-Range", "bytes 5-*/10");
            then.status(200);
        }).await;

        let client = make_client(&server.base_url());
        let body = reqwest::Body::from(vec![1, 2, 3]);
        let result = client.put_file_stream("/file.txt", body, Some(5), Some(10)).await;
        assert!(result.is_ok());
    }
}
