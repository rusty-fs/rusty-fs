// Layered module organization
pub mod config;
pub mod fuse; // FUSE filesystem trait implementation
pub mod http; // HTTP backend layer
pub mod remote_fs; // Core filesystem logic
pub mod utils; // Shared utilities layer // Configuration

// Re-export commonly used types
pub use config::FuseConfig;
pub use http::{HttpBackend, HttpClient, HttpError};
pub use remote_fs::RemoteFileSystem;
