# Known Gaps

Date: 2026-07-09

This document lists gaps that were verified directly against the current
codebase (not just inferred from `spec_compliance_critical_review.md`, parts
of which are stale — see that document's narrative sections on graceful
shutdown and path security, which no longer match the implementation or the
compliance matrices in the same file).

Each item below was confirmed by reading the current source or by running the
relevant command.

## 1. `fallocate` is a no-op in `mounty`

File: `mounty/src/fs/fuse/mod.rs:384-393`

```rust
fn fallocate(
    &mut self,
    _req: &Request<'_>,
    _ino: u64,
    _fh: u64,
    _offset: i64,
    _length: i64,
    _mode: i32,
    reply: ReplyEmpty,
) {
    reply.ok();
}
```

The handler acknowledges the FUSE `fallocate` request without allocating
space or validating the request in any way. Callers that rely on `fallocate`
to pre-reserve disk space get a false "ok".

## 2. No symlink handling or path canonicalization in `filer`

File: `filer/src/handlers/path.rs`

`safe_join_path` rejects `..` components structurally (via
`Component::ParentDir`), which correctly blocks traversal through the
requested path string. It does not canonicalize the resolved path or check
whether it stays under `BASE_DIR` after symlink resolution. If a symlink
exists inside `BASE_DIR` pointing outside of it, following that symlink
(e.g. via `fs::read_dir`, `fs::metadata`, `TokioFile::open`) escapes the
sandbox. There is no decision recorded on whether symlinks should be
followed, rejected, or sandboxed.

Confirmed by `grep -rn "canonicalize\|symlink" filer/src/` returning no
results.

## 3. No E2E test for `mounty` graceful shutdown under a real FUSE mount

`mounty/tests/e2e_mount_test.rs` covers 6 scenarios: `test_e2e_write_and_read`,
`test_e2e_directories`, `test_e2e_rename`, `test_e2e_permissions`,
`test_e2e_truncate`, `test_e2e_copy`. None of them send `SIGTERM`/`SIGINT` to
`mounty` while a write buffer is dirty and then verify that the file on the
server ends up correct and the mountpoint is cleanly unmounted.

This is the exact residual risk called out in
`docs/graceful_startup_shutdown_implementation_notes.md` ("Remaining Risks
→ No real FUSE E2E yet"), and it is still open.

By contrast, `filer/tests/startup_shutdown.rs` does have
`test_graceful_shutdown_with_sigterm`, but it only covers the HTTP server
process, not the client-side buffered-write drain path.

## 4. Latency requirement (<500ms) is not tested

Both specs state: "Reasonable latency (<500ms for operations under normal
network conditions)." No test asserts on operation latency.

The two `Duration::from_millis(500)` occurrences in the test suite
(`filer/tests/http_api.rs:22`, `filer/tests/startup_shutdown.rs:42`) are
startup sleeps used to wait for the server process to come up — not latency
assertions.

## 5. `cargo clippy --all-targets -- -D warnings` fails in both crates

Re-run on 2026-07-09; still fails for both `filer` and `mounty`. Representative
failures:

- `filer`: `needless_borrows_for_generic_args` across multiple assertions in
  `filer/tests/http_api.rs`.
- `mounty`: `too_many_arguments` in `mounty/src/fs/remote_fs/file_ops.rs`
  (`setattr`, 8/7 args) and elsewhere; `module_inception` in
  `mounty/src/fs/remote_fs/tests.rs` (module named `tests` inside a file also
  reached via `mod tests`).

## 6. Specs do not document the real API surface

`filer/specs.md` and `mounty/specs.md` list only:

```text
GET    /list/<path>
GET    /files/<path>
PUT    /files/<path>
POST   /mkdir/<path>
DELETE /files/<path>
```

The implementation also uses `GET /meta/<path>` and `PATCH /files/<path>`,
neither of which is documented. `PATCH /files/<path>` is load-bearing: it is
how `mounty` implements atomic server-side rename. Neither spec file has been
updated to reflect this.

## Not re-verified

The following claims from `spec_compliance_critical_review.md` were not
re-checked here because they require load/concurrency testing rather than
code reading:

- cache invalidation correctness under concurrent modification;
- behavior under sustained heavy concurrent load (`fio`-style workloads).
