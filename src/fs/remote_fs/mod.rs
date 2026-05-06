use crate::fs::http::{HttpBackend, FileEntry};
use crate::fs::utils::inode_map::InodeMapper;
use crate::fs::config::FuseConfig;
use crate::fs::http::{FileEntry, HttpBackend, HttpError};
use crate::fs::utils::file_handle::FhManager;
use fuser::{FileAttr, FileType};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};
use std::collections::HashMap;
use std::sync::{RwLock, Mutex};
use std::time::Instant;
use indexmap::map::IndexMap;

pub mod dir_ops;
pub mod file_ops;
#[cfg(test)]
mod tests;

// metrics removed: runtime counters were deleted per user request

pub struct RemoteFileSystem {
    pub http_client: Arc<dyn HttpBackend>,
    inode_mapper: InodeMapper,
    pub fh_manager: FhManager,
    pub config: FuseConfig,
    /// When true, disable all client-side caches (strict nocache mode).
    disable_cache: bool,
    /// Simple negative lookup cache: path -> expiry Instant
    /// Stores recent ENOENT results to suppress repeated probes (short TTL)
    negative_lookup: RwLock<HashMap<String, Instant>>,
    /// Short-lived directory listing cache with simple LRU behavior.
    /// Uses an IndexMap stored inside a Mutex to allow reordering on access.
    listing_cache: Mutex<IndexMap<String, (Instant, Vec<crate::fs::http::FileEntry>)>>,
    /// Positive metadata cache: path -> (expiry, FileEntry)
    /// Stores recent getattr results to avoid repeated metadata RPCs.
    metadata_cache: Mutex<IndexMap<String, (Instant, crate::fs::http::FileEntry)>>,
}

impl RemoteFileSystem {
    pub fn new(client: Arc<dyn HttpBackend>) -> Self {
        Self::with_config(client, FuseConfig::default())
    }

    pub fn with_config(client: Arc<dyn HttpBackend>, config: FuseConfig) -> Self {
        // Allow an explicit runtime override to strictly disable caches.
        let disable_cache = std::env::var("MOUNTY_DISABLE_CACHE")
            .map(|v| {
                let v = v.to_lowercase();
                v == "1" || v == "true" || v == "yes"
            })
            .unwrap_or(false);

        Self {
            http_client: client,
            inode_mapper: InodeMapper::new(),
            fh_manager: FhManager::new(),
            config,
            disable_cache,
            negative_lookup: RwLock::new(HashMap::new()),
            listing_cache: Mutex::new(IndexMap::new()),
            metadata_cache: Mutex::new(IndexMap::new()),
        }
    }

    // metrics removed: no init_metrics method

    pub fn get_inode_for_path(&mut self, path: &str) -> u64 {
        self.inode_mapper.get_or_create_inode(path)
    }

    pub fn get_path_for_inode(&self, inode: u64) -> Option<String> {
        self.inode_mapper.path_for(inode)
    }

    pub fn file_entry_to_attr(&self, entry: &FileEntry, inode: u64) -> FileAttr {
        // Handle None modified timestamp safely
        // Use a safe default: Unix epoch (1970-01-01) or clamp invalid timestamps
        let modified = match entry.modified {
            Some(secs) => {
                // Only allow timestamps between 1970 (0) and year 2100 (~4102444800)
                if secs == 0 || secs > 4102444800 {
                    // Use a safe default timestamp instead of now()
                    // This prevents fuser from trying to calculate with extremely large/small values
                    UNIX_EPOCH + Duration::from_secs(1_000_000_000) // ~2001-09-09
                } else {
                    UNIX_EPOCH + Duration::from_secs(secs)
                }
            }
            None => {
                // Use a safe default instead of now() to avoid potential overflow
                UNIX_EPOCH + Duration::from_secs(1_000_000_000) // ~2001-09-09
            }
        };

        let perm = entry.permissions.unwrap_or(0o644) as u16;
        FileAttr {
            ino: inode,
            size: entry.size,
            blocks: (entry.size + 511) / 512,
            atime: modified,
            mtime: modified,
            ctime: modified,
            crtime: modified,
            kind: if entry.is_dir {
                FileType::Directory
            } else {
                FileType::RegularFile
            },
            perm,
            nlink: 1,
            uid: unsafe { libc::getuid() } as u32,
            gid: unsafe { libc::getgid() } as u32,
            rdev: 0,
            blksize: 512,
            flags: 0,
        }
    }

    // Negative lookup helpers -------------------------------------------------
    const NEGATIVE_LOOKUP_TTL_MS: u64 = 300;

    /// Check if a path is currently cached as a negative (not-found) result.
    pub fn negative_lookup_is_cached(&self, path: &str) -> bool {
        if self.disable_cache {
            return false;
        }
        let map = self.negative_lookup.read().unwrap();
        if let Some(&expiry) = map.get(path) {
            Instant::now() < expiry
        } else {
            false
        }
    }

    /// Insert a negative lookup entry for `path` with a short TTL.
    pub fn negative_lookup_insert(&self, path: &str) {
        let mut map = self.negative_lookup.write().unwrap();
        let expiry = Instant::now() + Duration::from_millis(Self::NEGATIVE_LOOKUP_TTL_MS);
        map.insert(path.to_string(), expiry);
        // Basic size defense: if map grows very large, clear expired entries.
        if map.len() > 10_000 {
            let now = Instant::now();
            map.retain(|_, &mut exp| exp > now);
            // if still large, clear entirely (last resort)
            if map.len() > 20_000 {
                map.clear();
            }
        }
    }

    /// Remove any negative cache entry for `path` (called after successful create/rename/delete)
    pub fn negative_lookup_remove(&self, path: &str) {
        let mut map = self.negative_lookup.write().unwrap();
        map.remove(path);
    }

    // Directory listing cache helpers -------------------------------------

    /// Try to get a cached listing for `dir`. Returns Some(cloned_vec) if cache is valid.
    pub fn listing_cache_get(&self, dir: &str) -> Option<Vec<crate::fs::http::FileEntry>> {
        if self.disable_cache {
            tracing::debug!("listing_cache disabled, skipping lookup for {}", dir);
            return None;
        }
        let mut map = self.listing_cache.lock().unwrap();
        if let Some(value) = map.get(dir) {
            let expiry = value.0.clone();
            let entries_clone = value.1.clone();
            if Instant::now() < expiry {
                // mark as recently used by removing and reinserting
                map.shift_remove(dir);
                map.insert(dir.to_string(), (expiry, entries_clone.clone()));
                tracing::debug!("listing_cache hit for {} (entries={})", dir, entries_clone.len());
                return Some(entries_clone);
            } else {
                // expired; remove it
                map.shift_remove(dir);
            }
        }
        tracing::debug!("listing_cache miss for {}", dir);
        None
    }

    /// Insert a listing into the cache with TTL.
    pub fn listing_cache_insert(&self, dir: &str, entries: Vec<crate::fs::http::FileEntry>) {
        if self.disable_cache {
            tracing::debug!("listing_cache disabled, skipping insert for {}", dir);
            return;
        }
        let mut map = self.listing_cache.lock().unwrap();
        let ttl_ms = self.config.listing_cache_ttl.as_millis() as u64;
        let expiry = Instant::now() + std::time::Duration::from_millis(ttl_ms);
        map.insert(dir.to_string(), (expiry, entries));
        tracing::debug!("listing_cache insert for {}", dir);
        // enforce capacity: evict oldest while over capacity
        while map.len() > self.config.listing_cache_capacity {
            // remove the oldest entry (front) to keep the map within capacity
            let _ = map.shift_remove_index(0);
        }
    }

    /// Remove cached listing for `dir`.
    pub fn listing_cache_remove(&self, dir: &str) {
        let mut map = self.listing_cache.lock().unwrap();
        if map.shift_remove(dir).is_some() {
            tracing::debug!("listing_cache removed for {}", dir);
        }
    }

    // Metadata cache helpers -----------------------------------------
    /// Try to get a cached metadata entry for `path`.
    pub fn metadata_cache_get(&self, path: &str) -> Option<crate::fs::http::FileEntry> {
        if self.disable_cache {
            tracing::debug!("metadata_cache disabled, skipping lookup for {}", path);
            return None;
        }
        let mut map = self.metadata_cache.lock().unwrap();
        if let Some(value) = map.get(path) {
            let expiry = value.0.clone();
            let entry_clone = value.1.clone();
            if Instant::now() < expiry {
                // mark as recently used
                map.shift_remove(path);
                map.insert(path.to_string(), (expiry, entry_clone.clone()));
                tracing::debug!("metadata_cache hit for {}", path);
                return Some(entry_clone);
            } else {
                map.shift_remove(path);
            }
        }
        tracing::debug!("metadata_cache miss for {}", path);
        None
    }

    /// Insert metadata into the cache with TTL based on `config.ttl`.
    pub fn metadata_cache_insert(&self, path: &str, entry: crate::fs::http::FileEntry) {
        if self.disable_cache {
            tracing::debug!("metadata_cache disabled, skipping insert for {}", path);
            return;
        }
        let mut map = self.metadata_cache.lock().unwrap();
        let ttl_ms = self.config.ttl.as_millis() as u64;
        let expiry = Instant::now() + std::time::Duration::from_millis(ttl_ms);
        map.insert(path.to_string(), (expiry, entry));
        tracing::debug!("metadata_cache insert for {}", path);
        // enforce capacity using same limit as listing cache
        while map.len() > self.config.listing_cache_capacity {
            let _ = map.shift_remove_index(0);
        }
    }

    /// Remove cached metadata for `path`.
    pub fn metadata_cache_remove(&self, path: &str) {
        let mut map = self.metadata_cache.lock().unwrap();
        if map.shift_remove(path).is_some() {
            tracing::debug!("metadata_cache removed for {}", path);
        }
    }
}
