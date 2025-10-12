use super::types::{DirectoryListing, FileEntry};
use reqwest::header::{RANGE, CONTENT_LENGTH, LAST_MODIFIED};
use reqwest::StatusCode;
use thiserror::Error;
use async_trait::async_trait;

use libc::{EACCES, EIO, ENOENT};

#[derive(Debug, Error)]
pub enum HttpError {
    #[error("not found")]
    NotFound,
    #[error("permission denied")]
    PermissionDenied,
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
    async fn head(&self, path: &str) -> Result<(u64, Option<String>), HttpError>;
    async fn read_all(&self, path: &str) -> Result<Vec<u8>, HttpError>;
    async fn read_range(&self, path: &str, offset: u64, length: usize) -> Result<Vec<u8>, HttpError>;
}

#[derive(Clone)]
pub struct HttpClient {
    client: reqwest::Client,
    base_url: String,
}

impl HttpClient {
    pub fn new(base_url: String) -> Self {
        // Ensure the base_url has the protocol
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

    /// List directory contents at the given path.
    /// Calls GET /list{path} which returns a DirectoryListing JSON object
    /// Example: GET /list/some/directory
    /// Returns a Vec<FileEntry>
    async fn list_directory(&self, path: &str) -> Result<Vec<FileEntry>, HttpError> {
        let url = format!("{}/list{}", self.base_url, path);
        println!("GET {}", url);
         
        let response = self.client.get(&url).send().await;

        
        match response {
            Ok(response) => {
                // Get the response text first to debug
                let response_text = response.text().await?;
                
                // Check if response is empty
                if response_text.trim().is_empty() {
                    return Err(HttpError::Other("Empty response from server".into()));
                }
                
                // Parse the JSON as DirectoryListing and extract the files array
                let listing: DirectoryListing = serde_json::from_str(&response_text)
                    .map_err(|e| HttpError::Other(format!("JSON parse error: {} - Response: {}", e, response_text).into()))?;
                println!("Response body: {:?}", listing.files);
  
                Ok(listing.files)
            }            
            Err(e) => {
                return Err(HttpError::Other(format!("Failed to send request: {}", e).into()));
            }
        }
    }

    /// Get metadata for a single file or directory
    /// Calls GET /meta{path} which returns a FileEntry JSON object
    /// Example: GET /meta/some/file.txt    
    /// Returns a FileEntry
    async fn get_file_metadata(&self, path: &str) -> Result<FileEntry, HttpError> {
        let url = format!("{}/meta{}", self.base_url, path);
        let resp = self.client.get(&url).send().await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(HttpError::Other(format!("meta request failed: {} - {}", status, text).into()));
        }
        let entry: FileEntry = serde_json::from_str(&text)
            .map_err(|e| HttpError::Other(format!("JSON parse error for metadata: {} - Response: {}", e, text).into()))?;
        Ok(entry)
    }

    /// Retrieve file size and optional last modified time.
    /// Returns (size in bytes, optional last modified string).
    /// Server must support HEAD /file{path} returning Content-Length and Last-Modified headers
    async fn head(&self, path: &str) -> Result<(u64, Option<String>), HttpError> {
        let url = format!("{}/file{}", self.base_url, path);
        let resp = self.client.head(&url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(HttpError::Other(format!("head failed: {}", status).into()));
        }
        let size = resp
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        let lm = resp
            .headers()
            .get(LAST_MODIFIED)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        Ok((size, lm))
    }

    /// Read the entire file into a Vec<u8>.
    /// This is not efficient for large files, prefer read_range.
    /// Calls GET /file{path}
    /// Example: GET /file/some/file.txt
    /// Returns a Vec<u8> with the file data
    /// Server must support full file download.
    async fn read_all(&self, path: &str) -> Result<Vec<u8>, HttpError> {
        let url = format!("{}/file{}", self.base_url, path);
        let resp = self.client.get(&url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(HttpError::Other(format!("read_all failed: {} - {}", status, text).into()));
        }
        let bytes = resp.bytes().await?;
        Ok(bytes.to_vec())
    }

    /// Read a byte range of the file. Offset is u64, length is usize.
    /// Returns a Vec<u8> with the data.
    /// Calls GET /file{path} with Range header.
    /// Example: Range: bytes=0-1023 to read first 1024 bytes
    /// Server must support Range requests.
    async fn read_range(&self, path: &str, offset: u64, length: usize) -> Result<Vec<u8>, HttpError> {
        let url = format!("{}/file{}", self.base_url, path);
        // Range: bytes=START-END (END inclusive)
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
            return Err(HttpError::Other(format!("read_range failed: {} - {}", status, text).into()));
        }
        let bytes = resp.bytes().await?;
        Ok(bytes.to_vec())
    }
    
}
