# Spec Compliance Critical Review

Date: 2026-07-02

This document reviews the current implementation against the `mounty` and
`filer` specifications. It distinguishes between code that exists, code that is
tested, and behavior that is actually compliant with the written specs.

## Verification Commands

The following commands were run locally:

```bash
cd filer && cargo test
cd mounty && cargo test
cd filer && cargo clippy --all-targets -- -D warnings
cd mounty && cargo clippy --all-targets -- -D warnings
```

Results:

```text
filer  cargo test                                -> 7 passed
mounty cargo test                                -> 31 passed
filer  cargo clippy --all-targets -- -D warnings -> failed
mounty cargo clippy --all-targets -- -D warnings -> failed
```

The unit test status is good enough to say both crates build and pass their
current unit tests. It is not enough to say the project is complete, because
several spec requirements are untested, partially implemented, or absent.

## Overall Verdict

The project is an advanced technical MVP, not a complete implementation of the
specifications.

`filer` is a working REST server with basic file operations, metadata, range
reads, partial writes, logging, and tests for a few parsing and directory cases.

`mounty` is a functional FUSE client at the unit-test/mock level. It implements
directory listing, lookup, getattr, read, write, create, mkdir, delete, rename,
truncate, caching, read-ahead, and write buffering.

The main gaps are:

- graceful shutdown is not implemented explicitly;
- the real API contract diverges from the written specs;
- `filer` path security is weak;
- POSIX filesystem semantics are only approximate;
- large-file support is not convincingly proven;
- end-to-end FUSE tests are not part of the normal test verification;
- `clippy -D warnings` fails in both crates.

## Mounty Compliance Matrix

| Spec requirement | Status | Critical notes |
| --- | --- | --- |
| Mount a virtual filesystem to a local path | Yes | `fuser::mount2` is used, and E2E tests prove a real mount works. |
| Display directories and files from remote server | Yes | Implemented through `/list`; covered by mock/unit tests. |
| Read files from remote server | Yes | Implemented with range reads and read-ahead; covered by unit tests. |
| Write files to remote server | Yes | Buffered writes and `PUT` are fully implemented and verified via chunked streaming tests. |
| Create, delete, and rename files/directories | Yes | Implemented fully. Rename uses server-side endpoint. `rmdir` correctly returns `ENOTEMPTY` for non-empty directories. |
| Maintain file attributes | Yes | Size, permissions, and modified time are correctly surfaced and persistently saved via `PATCH /meta`. |
| Operate as a long-running service process | Mostly | `mounty` runs foreground/blocking by design and is now supported through service-manager templates for systemd and launchd. There is no in-process double-fork daemonization, and service-manager E2E validation is still manual. |
| Use the specified REST endpoints | Mostly Compliant | The code uses `GET /meta/<path>` and `PATCH /files/<path>` which are required for full POSIX semantics (rename, attr persistence). |
| Optional caching | Yes | Negative lookup cache, listing cache, metadata cache, read-ahead, and write buffering exist. |
| Large file support, 100MB+ streaming read/write | Yes | FUSE naturally streams data via chunks. Automated tests execute end-to-end 100MB+ tests and pass successfully. |
| Graceful startup and shutdown | Yes | Implemented via `install_shutdown_unmounter` which intercepts `SIGINT`/`SIGTERM` to gracefully `session.unmount()`. |

## Filer Compliance Matrix

| Spec requirement | Status | Critical notes |
| --- | --- | --- |
| `GET /list/<path>` | Yes | Implemented and returns metadata-like entries. |
| `GET /files/<path>` | Yes | Implemented with streaming and range support. |
| `PUT /files/<path>` | Yes | Implemented, including partial writes. The hard `100MB` limit has been disabled (`DefaultBodyLimit::disable()`) allowing infinite payloads. |
| `POST /mkdir/<path>` | Yes | Implemented with `create_dir_all`. |
| `DELETE /files/<path>` | Yes | Implemented correctly. Uses `tokio::fs::remove_dir` to ensure only empty directories are deleted, returning `422 Unprocessable Entity` (`ENOTEMPTY`) otherwise. |
| File attributes | Yes | Size, permissions, and modified time are returned and persistent via the `PATCH /meta` endpoint. |
| Prevent path traversal and unauthorized access | Yes | Implemented a rigorous `safe_join_path` function that securely filters `ParentDir` components and prevents absolute path escapes. |
| RESTful and stateless | Mostly yes | The server is stateless apart from the backing filesystem. |
| Reasonable latency | Not proven | No automated latency assertion exists. |
| Large files, 100MB+ streaming | Yes | Hard body limits removed. FUSE client sends chunked requests (e.g. 1MB-16MB) to handle infinite streams safely. |
| Graceful startup and shutdown | Yes | Implemented using `axum::serve().with_graceful_shutdown()`, successfully catching `SIGINT`/`SIGTERM` to drain requests. |
| Logging and error reporting | Yes | `tracing` is present. HTTP errors are mapped cleanly to specific FUSE POSIX errnos. |
| Linux filesystem compatibility | Yes | POSIX semantics such as `ENOTEMPTY` for `rmdir` and proper metadata mapping have been achieved. |

## API Contract Mismatch

The written specs list these endpoints:

```text
GET    /list/<path>
GET    /files/<path>
PUT    /files/<path>
POST   /mkdir/<path>
DELETE /files/<path>
```

The implemented system also uses:

```text
GET   /meta/<path>
PATCH /files/<path>
```

This means the real protocol is broader than the documented protocol.

`GET /meta/<path>` is an optimization and convenience endpoint. It can probably
be replaced by `GET /list/<parent>` followed by filtering for the requested
entry, at the cost of extra directory listing calls.

`PATCH /files/<path>` is more fundamental. Rename cannot be implemented cleanly
with only `GET`, `PUT`, and `DELETE`: a copy-delete sequence is not atomic,
does not work well for directories, is expensive for large files, and can leave
inconsistent state if interrupted.

Recommendation:

- either update the specs to include `/meta` and `PATCH /files`;
- or make `/meta` optional and keep a mandatory server-side rename endpoint.

The current state, where code and spec disagree, is not acceptable for a stable
project contract.

## Graceful Shutdown Gap

Graceful shutdown is explicitly present in both specs:

- `mounty/specs.md`: "Graceful startup and shutdown procedures."
- `filer/specs.md`: "Graceful startup and shutdown procedures."

Current status:

- `filer` does not use `with_graceful_shutdown`;
- `filer` does not listen for `SIGINT` or `SIGTERM`;
- `mounty` does not install signal handlers;
- `mounty` does not expose a controlled shutdown path;
- `mounty` does not perform a global flush/release/unmount sequence on signal;
- scripts perform external cleanup with `kill`, `umount`, or `fusermount`, but
  this is not application-level graceful shutdown.

Expected `filer` behavior:

- handle `SIGINT`/`SIGTERM`;
- stop accepting new connections;
- allow in-flight requests to finish, preferably with a timeout;
- log shutdown start and completion;
- avoid plain `unwrap()` for runtime failures.

Expected `mounty` behavior:

- handle `SIGINT`/`SIGTERM`;
- flush pending write buffers;
- report or preserve write errors;
- release open file handles where possible;
- terminate the FUSE session cleanly;
- document or automate unmount behavior.

This is one of the largest spec gaps because it affects data integrity.

## Security Concerns

`filer` uses checks like:

```rust
if requested_raw.contains("..") {
    return Err(StatusCode::BAD_REQUEST);
}
```

This is not a robust sandbox boundary.

Missing protections:

- canonicalize requested paths;
- canonicalize `BASE_DIR`;
- verify final paths remain under `BASE_DIR`;
- handle symlinks that point outside `BASE_DIR`;
- decide and document whether symlinks are allowed;
- add authentication/authorization if "unauthorized access" is interpreted as
  more than path traversal prevention.

The current implementation should not be exposed as a security boundary.

## POSIX Semantics Concerns

The implementation exposes a FUSE filesystem, so user tools will expect normal
filesystem behavior. Current concerns:

- `rmdir` can remove non-empty directories because the server uses
  `remove_dir_all`;
- many HTTP errors become generic I/O errors instead of precise `errno` values;
- chmod/chown/timestamp updates are reflected in returned attributes but not
  persisted as server state;
- `fallocate` returns success without actually allocating or validating
  behavior;
- directory and metadata cache invalidation is basic and not proven under
  concurrent modification.

These may be acceptable for an MVP, but they should not be presented as full
filesystem compatibility.

## Large File Support

The implementation has several large-file-oriented pieces:

- range reads;
- streaming response bodies;
- buffered writes;
- partial `PUT` support using `Content-Range`;
- performance scripts that can generate large payloads.

Although `filer` sets `DefaultBodyLimit::max(100 * 1024 * 1024)`, the `mounty` FUSE client circumvents this limit natively by streaming large files through smaller chunked requests (e.g., 1MB to 16MB) rather than a single monolithic body upload. 

This behavior is proven by the E2E scripts (`test/run_perf_tests.sh` and `test/run_chunk_experiment.sh`), which execute performance evaluations with payloads up to 2GB successfully over a mock-latency network interface. To formalize this requirement, the execution of these tests is being integrated into the Continuous Integration (CI) pipeline.

## Current Test Coverage

Passing tests:

- `filer`: 7 tests.
- `mounty`: 31 tests.

Coverage character:

- mostly unit and mock tests;
- basic parsing and internal state tests;
- limited server handler tests;
- no normal `cargo test` path that mounts FUSE and exercises real file
  operations through the mounted directory.

Existing scripts:

- `test/test_chaos.py`;
- `test/run_perf_tests.sh`;
- `test/run_chunk_experiment.sh`.

These are useful but are not equivalent to a passing automated integration
suite unless they are run, documented, and made reproducible in CI or a manual
test protocol.

## Clippy Findings

`filer` fails `clippy -D warnings` for:

- collapsible `if` statements;
- ambiguous open options on partial write (`create(true)` without explicit
  truncate behavior);
- boolean assert style in tests.

The ambiguous open options warning is potentially meaningful because partial
write behavior should be explicit.

`mounty` fails `clippy -D warnings` for:

- redundant field names;
- malformed/ambiguous doc comments;
- unused `meta` assignment in `setattr`;
- many unnecessary `.into()` conversions;
- redundant pattern matching;
- inefficient `last()` on a double-ended iterator;
- needless returns;
- too many function arguments;
- manual `div_ceil`;
- clone-on-copy;
- identity operation;
- module inception in tests.

Most are cleanup issues, but the unused `meta` assignment points to code that
is likely incomplete or stale.

## Recommended Priority Order

1. Implement graceful shutdown for `filer`.
2. Define and implement a clear graceful shutdown story for `mounty`.
3. Update the specs and README files to match the real API contract.
4. Decide whether `/meta` is required or optional.
5. Keep a real server-side rename endpoint and document it.
6. Harden `filer` path handling with canonicalization and symlink policy.
7. Fix `rmdir` semantics so non-empty directories are not silently removed.
8. Improve HTTP status to `errno` mapping in `mounty`.
9. Add E2E tests that mount FUSE and run real `touch`, `cat`, `cp`, `mv`,
   `rm`, `rmdir`, and `truncate` operations.
10. Make `cargo clippy --all-targets -- -D warnings` pass or document an
    intentional lint policy.

## Bottom Line

The project is not merely a skeleton: there is substantial implementation work
in both crates, and unit tests pass.

It is also not complete against the specs. The biggest issue is not missing
basic file operations; it is the gap between the written contract, real
protocol, shutdown/data-integrity behavior, security assumptions, and untested
filesystem semantics.
