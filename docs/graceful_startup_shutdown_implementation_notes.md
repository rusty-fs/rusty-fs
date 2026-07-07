# Graceful Startup and Shutdown: Implementation Notes

This document describes the graceful startup/shutdown work implemented in the
separate worktree:

```text
/private/tmp/rusty-fs-graceful-startup-shutdown
branch: graceful-startup-shutdown
```

It replaces the initial planning-only version of this document with a record of
what was changed, why those choices were made, what is covered by tests, and
what still needs end-to-end validation.

## Confidence Level

I am not claiming 100% certainty.

The design is materially better than the previous implementation because:

- `filer` now has explicit signal-aware graceful shutdown through Axum;
- `mounty` now handles `SIGINT`/`SIGTERM` by unmounting the FUSE session;
- `mounty` has a global drain path for open file handles;
- dirty buffered writes are flushed/finalized through one shared code path;
- shutdown failures are recorded and propagated as a non-zero process result;
- unit tests cover the new internal shutdown state and handle enumeration.

However, the implementation still needs an end-to-end FUSE test before it should
be treated as fully proven. In particular, unit tests do not prove real kernel
FUSE behavior during:

- `SIGTERM` while writes are buffered;
- external unmount while handles are still open;
- backend/network failure during shutdown;
- concurrent kernel requests arriving as shutdown begins.

So the right status is: implemented with a coherent design and passing unit
tests, but not yet 100% validated.

## What Changed in `filer`

Changed file:

```text
filer/src/main.rs
```

### Startup

The server startup path now validates configuration before serving requests.

Implemented behavior:

- tracing is initialized first;
- `BASE_DIR` must be present;
- `BASE_DIR` must exist;
- `BASE_DIR` must be a directory;
- `--port` is parsed explicitly and invalid values now fail startup;
- bind failures are returned as errors instead of panicking through `unwrap()`;
- `main` returns `Result<(), Box<dyn Error>>`.

Why:

- the previous server could fail late or panic with low-context errors;
- validating `BASE_DIR` before building the service makes startup failure
  deterministic;
- returning `Result` keeps normal operational errors out of panic paths.

### Shutdown

The server now runs:

```rust
axum::serve(listener, app)
    .with_graceful_shutdown(shutdown_signal())
    .await?;
```

`shutdown_signal()` waits for:

- `SIGINT`;
- `SIGTERM` on Unix;
- Ctrl-C fallback on non-Unix.

Why:

- Axum's graceful shutdown stops accepting new connections;
- in-flight request futures are allowed to finish;
- `filer` does not have a separate long-lived write buffer to drain at process
  shutdown because each upload handler already flushes and syncs before
  returning success.

What this does not solve:

- it does not add transactional multi-request semantics;
- it does not add a readiness endpoint;
- it does not recover interrupted request bodies after process death.

## What Changed in `mounty`

Changed files:

```text
mounty/src/main.rs
mounty/src/fs/fuse/mod.rs
mounty/src/fs/http/client.rs
mounty/src/fs/remote_fs/dir_ops.rs
mounty/src/fs/remote_fs/file_ops.rs
mounty/src/fs/remote_fs/mod.rs
mounty/src/fs/remote_fs/tests.rs
mounty/src/fs/utils/file_handle.rs
```

## `mounty` Startup

`mounty/src/main.rs` now performs explicit startup checks:

- parses exactly `<server_url> <mountpoint>`;
- validates that the mountpoint exists;
- validates that the mountpoint is a directory;
- creates the HTTP client;
- performs a pre-mount backend probe with `list_directory("/")`;
- only mounts FUSE after the probe succeeds.

Why:

- the previous implementation could mount a broken filesystem even when the
  backend was obviously unavailable or incompatible;
- mountpoint validation catches local configuration errors before FUSE is
  attached;
- early failure is easier to reason about than a mounted filesystem that fails
  every operation afterward.

## FUSE Session Ownership

The previous code used:

```rust
fuser::mount2(fs, mountpoint, &options)?;
```

The implementation now uses:

```rust
let mut session = Session::new(fs, &args.mountpoint, &options)?;
install_shutdown_unmounter(session.unmount_callable());
let run_result = session.run();
drop(session);
```

Why `Session` instead of the original `mount2`:

- `mount2` blocks until unmount and gives `main` no explicit unmount handle;
- `Session::unmount_callable()` gives a thread-safe unmount trigger that can be
  used from a signal-waiting thread;
- keeping `Session::run()` in the main thread avoids introducing an
  `Arc<Mutex<RemoteFileSystem>>` adapter just to share state with
  `spawn_mount2`;
- dropping the session after `run()` returns invokes the FUSE lifecycle teardown,
  including `Filesystem::destroy`.

Why not `spawn_mount2` in this implementation:

- `spawn_mount2` takes ownership of the filesystem and returns a
  `BackgroundSession`;
- draining the actual `RemoteFileSystem` from `main` would require either a
  delegation adapter or a broader shared-state refactor;
- using `Session` directly gives controlled unmount behavior with a smaller
  change surface.

## Signal Handling and Normal Close

`mounty` now starts a small signal-waiting thread.

That thread waits for:

- `SIGINT`;
- `SIGTERM` on Unix;
- Ctrl-C fallback on non-Unix.

When a signal is received, it calls:

```rust
SessionUnmounter::unmount()
```

Expected normal shutdown sequence:

1. User sends `SIGINT` or `SIGTERM`.
2. Signal thread calls FUSE unmount.
3. `Session::run()` returns.
4. `main` drops the `Session`.
5. `RemoteFileSystem::destroy()` runs.
6. `destroy()` drains open handles through `shutdown_flush_all()`.
7. `main` checks whether the drain failed.
8. Process exits successfully or returns an error.

This means normal `mounty` shutdown is intended to unmount the filesystem.

## Shutdown State

`RemoteFileSystem` now has:

```rust
shutting_down: AtomicBool
shutdown_failed: Arc<AtomicBool>
```

Added methods:

- `begin_shutdown()`;
- `is_shutting_down()`;
- `ensure_running()`;
- `clear_caches()`;
- `shutdown_failed_flag()`;
- `mark_shutdown_failed()`.

Why:

- shutdown must be visible to normal filesystem operations;
- new remote work should not start while open handles are being drained;
- `main` needs a way to know whether `destroy()` encountered data-persistence
  failures.

`HttpError` gained:

```rust
HttpError::ShuttingDown
```

mapped to:

```rust
libc::ESHUTDOWN
```

Why:

- callers should not see shutdown as a generic `EIO` when a more specific errno
  is available;
- this makes shutdown behavior easier to distinguish in logs and tests.

## Operation Gating

The following operations now call `ensure_running()` before starting new remote
work:

- `lookup`;
- `getattr`;
- `readdir`;
- `create_directory`;
- `delete_directory`;
- `rename`;
- `open`;
- `create_file`;
- `write_bytes`;
- `setattr`.

Why:

- once shutdown starts, the system should converge toward draining existing
  state rather than accepting new mutations;
- `flush` and `release` must remain possible, because they are part of shutdown.

Important nuance:

- reads through `read_bytes` are not currently gated directly;
- reads usually depend on already-open handles and do not create dirty state;
- if stricter shutdown semantics are desired, read gating should be added after
  E2E testing clarifies kernel behavior during unmount.

## Handle Enumeration

`FhManager` now has:

```rust
open_handles() -> Vec<(u64, u64)>
```

It snapshots open handles as `(fh, ino)` pairs.

Why:

- global shutdown needs to drain every open handle;
- the code must not hold a mutable borrow of the handle map while performing
  HTTP calls;
- snapshotting handles first avoids borrow conflicts and keeps the drain loop
  simple.

## Shared Flush and Finalization

The old `release` path contained its own logic to:

- clear read-ahead buffers;
- flush the write buffer;
- send final size information for dirty files;
- inspect pending write errors;
- release the file handle.

That logic is now centralized in:

```rust
flush_and_finalize_handle(ino, fh)
```

It performs:

1. clear read-ahead state;
2. resolve inode to path;
3. call `flush_write_buffer`;
4. if the handle is dirty, send the zero-body finalization `PUT` with final size;
5. fail if dirty state has no known final size;
6. fail if pending write errors exist;
7. mark the handle clean on success.

Why:

- shutdown and regular `release` must use the same data-integrity path;
- duplicating this logic would make it easy for normal close and signal shutdown
  to diverge;
- a single helper makes future bug fixes apply to both paths.

## Global Drain

`RemoteFileSystem` now has:

```rust
shutdown_flush_all() -> Result<(), Vec<(u64, HttpError)>>
```

Behavior:

1. set the shutdown flag;
2. snapshot all open handles;
3. flush/finalize each handle;
4. release successfully finalized handles;
5. record failures without discarding them;
6. clear caches;
7. mark shutdown as failed if any handle failed.

Why:

- signal-driven shutdown needs a global flush path, not just per-handle
  `release`;
- failures must be visible to the process exit path;
- successful handles should be released even if another handle fails.

## FUSE `release`

`release` now delegates to:

```rust
flush_and_finalize_handle(ino, fh)
```

and then removes the file handle.

Why:

- normal file close and global shutdown use the same flush/finalization rules;
- this reduces the risk that graceful shutdown persists data differently from
  ordinary close.

## FUSE `destroy`

`RemoteFileSystem` now implements:

```rust
Filesystem::destroy()
```

It calls:

```rust
shutdown_flush_all()
```

Why:

- `destroy` is the FUSE lifecycle hook reached when the session is torn down;
- signal-triggered unmount and external unmount should both converge on the same
  drain behavior;
- this makes external unmount best-effort graceful instead of purely external
  cleanup.

Important caveat:

- `destroy` can log drain failures and mark shared failure state;
- it cannot itself decide process exit status;
- `main` reads `shutdown_failed` after dropping the session and returns an error
  if the drain failed.

## Exit Behavior

`mounty` now keeps:

```rust
let shutdown_failed = fs.shutdown_failed_flag();
```

After `Session::run()` returns, `main` drops the session so `destroy()` can run,
then checks:

```rust
if shutdown_failed.load(Ordering::SeqCst) {
    anyhow::bail!("shutdown drain failed; see logs for failed file handles");
}
```

Why:

- data loss risk must not be silently hidden behind a clean exit;
- if the backend is unreachable or a dirty handle cannot be finalized, the
  process should return non-zero.

## Tests Added or Updated

Unit tests now cover:

- `FhManager::open_handles()` snapshots handle/inode pairs;
- shutdown state rejects new `open`;
- `shutdown_flush_all()` releases clean open handles and marks the filesystem as
  shutting down.

Commands run in the implementation worktree:

```text
cd filer && cargo test
```

Result:

```text
7 passed
```

```text
cd mounty && cargo test
```

Result:

```text
34 passed
```

## Clippy Status

I also ran:

```text
cargo clippy --all-targets -- -D warnings
```

It is not green yet.

The failures are mostly pre-existing warnings already consistent with the
critical review, including examples in:

- `filer/src/handlers/mod.rs`;
- `filer/src/handlers/tests.rs`;
- `mounty/src/fs/config.rs`;
- `mounty/src/fs/utils/path.rs`;
- `mounty/src/fs/http/client.rs`;
- `mounty/src/fs/remote_fs/dir_ops.rs`;
- `mounty/src/fs/remote_fs/file_ops.rs`;
- `mounty/src/fs/remote_fs/mod.rs`;
- `mounty/src/fs/utils/file_handle.rs`.

One warning introduced by the new code (`&PathBuf` instead of `&Path`) was fixed.

I intentionally did not fold the broader clippy cleanup into this shutdown
change, because that would mix a lifecycle feature with unrelated style and
cleanup edits.

## Remaining Risks

### No real FUSE E2E yet

The largest remaining gap is that no test has mounted a real FUSE filesystem and
verified shutdown behavior through kernel operations.

Required E2E cases:

- write file through mount;
- leave data buffered;
- send `SIGTERM` to `mounty`;
- verify mountpoint is unmounted;
- verify server file content is correct;
- repeat with `SIGINT`;
- repeat with external unmount;
- repeat while backend is unreachable and assert non-zero exit.

### Dirty write test uses unit-level logic only

The current unit tests validate shutdown state and clean-handle drain. They do
not fully prove dirty buffered write persistence because the existing fake HTTP
backend does not model partial range `PUT` semantics faithfully enough for a
strong dirty-write test.

Options:

- improve the fake backend to apply offset writes and final size hints;
- or prefer an E2E test against a real `filer` process.

The second option is more valuable for this feature.

### Race around concurrent operations

The design sets `shutting_down` before global handle enumeration, which is the
right first defense. Still, a real kernel FUSE session may have in-flight
operations as unmount begins.

This needs E2E and possibly stress testing to confirm that:

- existing operations finish or fail predictably;
- new operations see `ESHUTDOWN`;
- shutdown does not deadlock waiting on HTTP operations.

### `destroy` is best-effort

`destroy` is the best available central lifecycle hook in this design, but it is
still a teardown callback. If the process is killed with `SIGKILL`, crashes, or
the runtime aborts, no graceful shutdown code can run.

This is expected and should be documented as an operational limit.

## Why This Design Is Reasonable

The implementation takes the smallest coherent step that changes actual process
lifecycle behavior:

- `filer` uses Axum's supported graceful shutdown mechanism;
- `mounty` gets an explicit unmount path for normal close;
- dirty handle drain is centralized and reused;
- failures are no longer silently ignored during shutdown;
- the solution avoids a large `Arc<Mutex<RemoteFileSystem>>` adapter refactor;
- the code remains close to the current synchronous FUSE/HTTP architecture.

It is not the final word on shutdown correctness, but it is a defensible
foundation. The next quality step is not more design discussion; it is a real
E2E shutdown/data-integrity test.

## Recommended Next Step

Add an E2E test under `test/` that:

1. starts `filer` with a temporary `BASE_DIR`;
2. starts `mounty` on a temporary mountpoint;
3. writes data through the mounted filesystem;
4. sends `SIGTERM` to `mounty`;
5. verifies that the mountpoint is unmounted;
6. verifies that file content in `BASE_DIR` matches what was written;
7. shuts down `filer` with `SIGTERM`;
8. fails if external cleanup (`umount`) was needed.

Until that exists, the implementation should be considered architecturally sound
but not proven at the level required for a filesystem data-integrity guarantee.
