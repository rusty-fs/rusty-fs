# rusty-fs

A remote filesystem in Rust: a REST server exposes a local directory over
HTTP, and a FUSE client mounts it as a normal local directory.

- **[`filer/`](filer/README.md)** — the REST server that exposes files over HTTP.
- **[`mounty/`](mounty/README.md)** — the FUSE client that mounts `filer` locally.
- **[`test/`](test/README.md)** — end-to-end performance tests for the two together.
