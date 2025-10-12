# filer – RESTful Remote File System Server Specification

## 1. Overview
filer is a stateless RESTful server that exposes a local file system to remote clients. It provides endpoints for standard file and directory operations, enabling remote access and manipulation.

## 2. Functional Requirements

### 2.1 API Endpoints
- `GET /list/<path>` – Return a JSON listing of directory contents, including file attributes (name, type, size, permissions, timestamps).
- `GET /files/<path>` – Return the contents of a file as a byte stream.
- `PUT /files/<path>` – Write or overwrite file contents with data provided by the client.
- `POST /mkdir/<path>` – Create a new directory at the specified path.
- `DELETE /files/<path>` – Delete a file or directory at the specified path.

### 2.2 File Attributes
- Provide file size, timestamps, and permissions where possible.

### 2.3 Security
- Prevent path traversal and unauthorized access.
- Be RESTful and stateless.

## 3. Non-Functional Requirements

- Reasonable latency (<500ms for operations under normal network conditions).
- Support for large files (100MB+) with streaming.
- Graceful startup and shutdown procedures.
- Logging and error reporting.
- Full compatibility with Linux file systems.