use super::types::{DirectoryListing, FileEntry};
use async_trait::async_trait;
use libc::{EACCES, EIO, ENOENT};
use reqwest::StatusCode;
use reqwest::header::RANGE;
use thiserror::Error;
use tracing::{debug, error};

#[derive(Debug, Error)]
pub enum HttpError {
    #[error("not found")]
    NotFound,
    #[error("permission denied")]
    PermissionDenied,
    #[error("already exists")]
    AlreadyExists,
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
            HttpError::Network(_) | HttpError::Other(_) => EIO,
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
        data: Vec<u8>,
        offset: Option<u64>,
        total_size: Option<u64>,
    ) -> Result<(), HttpError>;
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
                    format!("Failed to send request: {}", e).into(),
                ));
            }
        }
    }

    async fn get_file_metadata(&self, path: &str) -> Result<FileEntry, HttpError> {
        let url = format!("{}/meta{}", self.base_url, path);
        let resp = self.client.get(&url).send().await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(HttpError::Other(
                format!("meta request failed: {} - {}", status, text).into(),
            ));
        }
        let entry: FileEntry = serde_json::from_str(&text).map_err(|e| {
            HttpError::Other(
                format!("JSON parse error for metadata: {} - Response: {}", e, text).into(),
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
        debug!(
            "Reading range {}-{} from file {}",
            offset,
            offset.saturating_add(length as u64).saturating_sub(1),
            path
        );
        let url = format!("{}/files{}", self.base_url, path);
        let end = offset.saturating_add(length as u64).saturating_sub(1);
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
            return Err(HttpError::Other(
                format!("read_range failed: {} - {}", status, text).into(),
            ));
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
            return Err(HttpError::Other(
                format!("create_directory failed: {} - {}", status, text).into(),
            ));
        }
        Ok(())
    }

    async fn delete_path(&self, path: &str) -> Result<(), HttpError> {
        let url = format!("{}/files{}", self.base_url, path);
        let resp = self.client.delete(&url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(HttpError::Other(
                format!("delete_path failed: {} - {}", status, text).into(),
            ));
        }
        Ok(())
    }

    async fn put_file_stream(
        &self,
        path: &str,
        data: Vec<u8>,
        offset: Option<u64>,
        total_size: Option<u64>,
    ) -> Result<(), HttpError> {
        let url = format!("{}/files{}", self.base_url, path);

        let mut request = self.client.put(&url).body(data.clone());

        if let Some(start) = offset {
            // Use saturating arithmetic to prevent underflow
            let end = start.saturating_add(data.len() as u64).saturating_sub(1);
            let range_value = if let Some(size) = total_size {
                format!("bytes {}-{}/{}", start, end, size)
            } else {
                format!("bytes {}-{}/*", start, end)
            };

            request = request.header("Content-Range", range_value);
        }

        let resp = request.send().await?;
        let status = resp.status();

        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(HttpError::Other(
                format!("put_file_stream failed: {} - {}", status, text).into(),
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::Method::GET;
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
}
