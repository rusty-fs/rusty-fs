# Filer

A simple HTTP file server built with Rust and Axum that provides REST API endpoints for file operations.

## Features

- List files and directories with metadata (size, permissions, modification time)
- RESTful API design
- Environment-based configuration
- Structured logging with tracing

## Setup

1. Set the base directory environment variable:
   ```bash
   export BASE_DIR=/path/to/your/files
   ```

2. Run the server:
   ```bash
   BASE_DIR=./test RUST_LOG=debug cargo run 
   ```

The server will start on `http://0.0.0.0:3000`

## API Endpoints

### List Files
```bash
curl http://localhost:3000/list/
```

Returns JSON with file listing including:
- File/directory names
- Type (file or directory)
- Size in bytes
- Unix permissions
- Last modified timestamp

### Planned Endpoints
- `GET /files/:filename` - Download a file
- `PUT /files/:filename` - Upload/create a file
- `DELETE /files/:filename` - Delete a file

## Environment Variables

- `BASE_DIR` - Required. The root directory to serve files from.

## Service Managers

For background operation, run `filer` as a foreground process managed by the
platform service manager. See `docs/service_manager_setup.md` for systemd and
launchd templates.

## License
