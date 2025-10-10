use super::types::{DirectoryListing, FileEntry};

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

    pub async fn list_directory(&self, path: &str) -> anyhow::Result<Vec<FileEntry>> {
        let url = format!("{}/list{}", self.base_url, path);
        println!("GET {}", url);
         
        let response = self.client.get(&url).send().await;

        
        match response {
            Ok(response) => {
                // Get the response text first to debug
                let response_text = response.text().await?;
                
                // Check if response is empty
                if response_text.trim().is_empty() {
                    return Err(anyhow::anyhow!("Empty response from server"));
                }
                
                // Parse the JSON as DirectoryListing and extract the files array
                let listing: DirectoryListing = serde_json::from_str(&response_text)
                    .map_err(|e| anyhow::anyhow!("JSON parse error: {} - Response: {}", e, response_text))?;
                println!("Response body: {:?}", listing.files);
  
                Ok(listing.files)
            }            
            Err(e) => {
                return Err(anyhow::anyhow!("Failed to send request: {}", e));
            }
        }
        
    }
}
