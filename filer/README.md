# filer

A stateless REST server, built with Rust and Axum, that exposes a local
directory over HTTP so it can be mounted remotely (see [`mounty`](../mounty/README.md)).

## Features

- List, read, write, delete files and directories over HTTP
- Range reads and partial (offset-based) writes for large files
- File metadata (size, permissions, mtime/atime) readable and writable
- Server-side atomic rename via `PATCH`
- Graceful shutdown on `SIGINT`/`SIGTERM` (drains in-flight requests before exiting)
- Structured logging via `tracing`

## Quick Start

```bash
export BASE_DIR=/path/to/your/files
cargo run -- --port 3000
```

| Variable / flag | Required | Default | Description |
| --- | --- | --- | --- |
| `BASE_DIR` | yes | — | Root directory served. Must exist and be a directory. |
| `--port` | no | `3000` | TCP port to listen on. |

## API

All paths are relative to `BASE_DIR`.

| Method | Path | Description |
| --- | --- | --- |
| `GET` | `/list/<path>` | List directory contents (name, type, size, permissions, mtime). |
| `GET` | `/meta/<path>` | Get metadata for a single file or directory. |
| `PATCH` | `/meta/<path>` | Update mode, atime/mtime, or rename (`new_name`) an entry. |
| `GET` | `/files/<path>` | Read a file. Supports `Range` header for partial reads. |
| `PUT` | `/files/<path>` | Write a file. Supports `Content-Range` for partial/chunked writes. |
| `POST` | `/mkdir/<path>` | Create a directory (and parents). |
| `DELETE` | `/files/<path>` | Delete a file, or an empty directory (`422` if not empty). |

## Running as a Service

The quickest path is the interactive installer:

```bash
./deploy/install.sh            # builds release binaries, installs + starts the service
./deploy/install.sh --dry-run  # preview without touching the system
```

For manual installation and the systemd/launchd templates, see
[`docs/service_manager_setup.md`](../docs/service_manager_setup.md).

## Testing

```bash
cargo test
```

## Layout

```text
src/
  main.rs              startup, arg/env parsing, graceful shutdown signal
  app/mod.rs           route table
  handlers/
    mod.rs             submodule declarations + re-exports
    path.rs            safe_join_path (path traversal protection)
    range.rs           Range / Content-Range header parsing
    types.rs           FileEntry (shared response type)
    dir.rs             list, mkdir, delete_path
    meta.rs            meta, update_meta
    read.rs            file read + range streaming
    write.rs           file write
```
