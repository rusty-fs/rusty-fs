# mounty

A FUSE-based remote filesystem client that mounts a remote directory over HTTP as a local filesystem.

## Features

- Mount remote directories as local filesystems using FUSE
- Read-only access to remote files and directories
- HTTP-based communication with remote server
- Async/await support with Tokio runtime

## Usage

```bash
cargo run <server_url> <mountpoint>
```

### Example

```bash
# Mount remote filesystem from server to local directory
cargo run http://localhost:3000 /mnt/remote

# Unmount (from another terminal)
fusermount -u /mnt/remote
```

## Dependencies

- `fuser` - FUSE bindings for Rust
- `tokio` - Async runtime
- `reqwest` - HTTP client
- `serde` - Serialization/deserialization

## Build

```bash
cargo build --release
```

## License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for details.