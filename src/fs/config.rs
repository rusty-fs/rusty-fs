use std::time::Duration;

/// Configuration for the FUSE filesystem
#[derive(Debug, Clone)]
pub struct FuseConfig {
    /// TTL for attribute cache
    pub ttl: Duration,
    /// Maximum buffer size before flushing to server (in bytes)
    pub max_buffer_size: usize,
    /// Read chunk size for partial reads (in bytes)
    pub chunk_size: usize,
}

impl FuseConfig {
    /// Create a new config with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new config from environment variables with fallback to defaults
    pub fn from_env() -> Self {
        let ttl_secs = std::env::var("MOUNTY_TTL_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(1);

        let max_buffer_size = std::env::var("MOUNTY_MAX_BUFFER_SIZE")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(4 * 1024 * 1024); // 4 MB

        let chunk_size = std::env::var("MOUNTY_CHUNK_SIZE")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(1024 * 1024); // 1 MB

        Self {
            ttl: Duration::from_secs(ttl_secs),
            max_buffer_size,
            chunk_size,
        }
    }

    /// Set TTL duration
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// Set maximum buffer size
    pub fn with_max_buffer_size(mut self, size: usize) -> Self {
        self.max_buffer_size = size;
        self
    }

    /// Set chunk size
    pub fn with_chunk_size(mut self, size: usize) -> Self {
        self.chunk_size = size;
        self
    }
}

impl Default for FuseConfig {
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(1),
            max_buffer_size: 4 * 1024 * 1024, // 4 MB
            chunk_size: 1024 * 1024, // 1 MB
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = FuseConfig::default();
        assert_eq!(config.ttl, Duration::from_secs(1));
        assert_eq!(config.max_buffer_size, 4 * 1024 * 1024);
        assert_eq!(config.chunk_size, 1024 * 1024);
    }

    #[test]
    fn test_builder_pattern() {
        let config = FuseConfig::new()
            .with_ttl(Duration::from_secs(5))
            .with_max_buffer_size(2 * 1024 * 1024);
        
        assert_eq!(config.ttl, Duration::from_secs(5));
        assert_eq!(config.max_buffer_size, 2 * 1024 * 1024);
        assert_eq!(config.chunk_size, 1024 * 1024); // unchanged
    }
}
