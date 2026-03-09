// Layered module organization
pub mod http;        // HTTP backend layer
pub mod utils;       // Shared utilities layer
pub mod fuse;        // FUSE filesystem trait implementation
pub mod remote_fs;   // Core filesystem logic
pub mod config;      // Configuration

// Re-export commonly used types
pub use http::{HttpClient, HttpBackend, HttpError};
pub use remote_fs::RemoteFileSystem;
pub use config::FuseConfig;