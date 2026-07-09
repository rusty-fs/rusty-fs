# mounty – FUSE Remote File System Client Specification

## 1. Overview
mounty is a Rust-based FUSE client that mounts a remote file system, exposing it as a local directory. It provides transparent access to files and directories hosted on a remote server via a RESTful API.

## 2. Functional Requirements

### 2.1 Core Functionality
- Mount a virtual file system to a local path (e.g., `/mnt/remote-fs`).
- Display directories and files from the remote server.
- Read files from the remote server.
- Write files to the remote server.
- Support creation, deletion, and renaming of files and directories.
- Maintain file attributes (size, timestamps, permissions) as feasible.
- Operate as a long-running service process, suitable for management by
  platform service managers such as systemd on Linux and launchd on macOS.

### 2.2 Server Communication
- Interact with the remote server using the following RESTful API endpoints:
  - `GET /list/<path>` – List directory contents.
  - `GET /files/<path>` – Read file contents.
  - `PUT /files/<path>` – Write file contents.
  - `POST /mkdir/<path>` – Create directory.
  - `DELETE /files/<path>` – Delete file or directory.

### 2.3 Caching
- (Optional) Implement a local caching layer for performance.
- Support configurable cache invalidation (e.g., TTL or LRU).

## 3. Non-Functional Requirements

- Reasonable latency (<500ms for operations under normal network conditions).
- Full support for Linux using FUSE (`libfuse`, `fuser`, or `async-fuse`).
- Optional, best-effort support for macOS (macFUSE) and Windows (WinFSP/Dokany).
- Support for large files (100MB+) with streaming read/write.
- Graceful startup and shutdown procedures.
