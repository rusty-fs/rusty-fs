use std::time::Duration;

/// Configuration for the FUSE filesystem
#[derive(Debug, Clone)]
pub struct FuseConfig {
    /// TTL for attribute cache
    pub ttl: Duration,
    /// Read-ahead buffer size (in bytes)
    pub max_buffer_size: usize,
    /// Write chunk size sent to the server (in bytes)
    pub chunk_size: usize,
    /// Directory listing cache TTL
    pub listing_cache_ttl: Duration,
    /// Directory listing cache capacity (LRU)
    pub listing_cache_capacity: usize,
    /// UID exposed in FUSE file attributes.
    pub owner_uid: u32,
    /// GID exposed in FUSE file attributes.
    pub owner_gid: u32,
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
            .unwrap_or(8 * 1024 * 1024); // 8 MB

        let chunk_size = std::env::var("MOUNTY_CHUNK_SIZE")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(4 * 1024 * 1024); // 4 MB

        let listing_cache_ttl_ms = std::env::var("MOUNTY_LISTING_CACHE_TTL_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(500);

        let listing_cache_capacity = std::env::var("MOUNTY_LISTING_CACHE_CAPACITY")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(1024);

        let owner_uid = std::env::var("MOUNTY_UID")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or_else(current_uid);

        let owner_gid = std::env::var("MOUNTY_GID")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or_else(current_gid);

        Self {
            ttl: Duration::from_secs(ttl_secs),
            max_buffer_size,
            chunk_size,
            listing_cache_ttl: Duration::from_millis(listing_cache_ttl_ms),
            listing_cache_capacity,
            owner_uid,
            owner_gid,
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

    /// Set listing cache TTL in milliseconds
    pub fn with_listing_cache_ttl_ms(mut self, ms: u64) -> Self {
        self.listing_cache_ttl = Duration::from_millis(ms);
        self
    }

    /// Set listing cache capacity
    pub fn with_listing_cache_capacity(mut self, cap: usize) -> Self {
        self.listing_cache_capacity = cap;
        self
    }

    /// Set the UID/GID exposed by FUSE attributes.
    pub fn with_owner(mut self, uid: u32, gid: u32) -> Self {
        self.owner_uid = uid;
        self.owner_gid = gid;
        self
    }
}

impl Default for FuseConfig {
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(1),
            max_buffer_size: 8 * 1024 * 1024, // 8 MB
            chunk_size: 4 * 1024 * 1024,      // 4 MB
            listing_cache_ttl: Duration::from_millis(500),
            listing_cache_capacity: 1024,
            owner_uid: current_uid(),
            owner_gid: current_gid(),
        }
    }
}

fn current_uid() -> u32 {
    unsafe { libc::getuid() as u32 }
}

fn current_gid() -> u32 {
    unsafe { libc::getgid() as u32 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = FuseConfig::default();
        assert_eq!(config.ttl, Duration::from_secs(1));
        assert_eq!(config.max_buffer_size, 8 * 1024 * 1024);
        assert_eq!(config.chunk_size, 4 * 1024 * 1024);
        assert_eq!(config.owner_uid, current_uid());
        assert_eq!(config.owner_gid, current_gid());
    }

    #[test]
    fn test_builder_pattern() {
        let config = FuseConfig::new()
            .with_ttl(Duration::from_secs(5))
            .with_max_buffer_size(2 * 1024 * 1024);

        assert_eq!(config.ttl, Duration::from_secs(5));
        assert_eq!(config.max_buffer_size, 2 * 1024 * 1024);
        assert_eq!(config.chunk_size, 4 * 1024 * 1024); // unchanged
    }

    #[test]
    fn test_owner_builder() {
        let config = FuseConfig::new().with_owner(123, 456);

        assert_eq!(config.owner_uid, 123);
        assert_eq!(config.owner_gid, 456);
    }
}
