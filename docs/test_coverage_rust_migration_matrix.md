# Test Coverage and Rust Migration Matrix

This document summarizes what is currently covered by:

- Rust unit tests already present in `filer/` and `mounty/`;
- script-based tests under `test/`;
- missing Rust integration/E2E tests that should be added.

The main point: the project already has useful Rust unit coverage, but it is
mostly module-level coverage. Startup, shutdown, real binary behavior, and real
FUSE behavior are still mostly covered by scripts or missing.

## Rust Tests Already Present

| Area | Current Rust test location | What it covers | Main limitation |
|---|---|---|---|
| `filer` range parsing | `filer/src/handlers/tests.rs` | `Content-Range` and `Range` parser success/error cases, including property-based cases | Parser-only; no real server process |
| `filer` mkdir/delete handlers | `filer/src/handlers/tests.rs` | Direct async call to `mkdir` and `delete_path` using a temp directory | Handler-level only; not HTTP over TCP, no startup/shutdown |
| `mounty` HTTP client | `mounty/src/fs/http/client.rs` | `list_directory`, `get_file_metadata`, `read_range` against `httpmock` | Mock server only; does not test real `filer` |
| `mounty` config | `mounty/src/fs/config.rs` | Default config and builder behavior | Unit-level only |
| `mounty` path utilities | `mounty/src/fs/utils/path.rs` | Path join/parent behavior | Unit-level only |
| `mounty` inode mapper | `mounty/src/fs/utils/inode_map.rs` | Root mapping, deterministic inode allocation, remove, rename, clear; includes property tests | Unit-level only; no FUSE/kernel behavior |
| `mounty` file handle manager | `mounty/src/fs/utils/file_handle.rs` | Handle allocation/release, lookup by inode, file-size tracking; includes property tests | Does not perform HTTP flushes or FUSE release |
| `mounty` runtime helper | `mounty/src/fs/utils/runtime.rs` | Reuse of the global Tokio runtime | Unit-level only |
| `mounty` fake backend | `mounty/src/fs/utils/test_utils.rs` | Fake backend construction | Fake backend is simplified and does not fully model partial range writes |
| `mounty` remote filesystem core | `mounty/src/fs/remote_fs/tests.rs` | Inode/path mapping, attr conversion, readdir, lookup, getattr, open/read flow using `FakeBackend` | No real HTTP server, no real FUSE mount |

On the `graceful-startup-shutdown` branch, additional Rust unit tests were added
for internal shutdown behavior:

| Area | Branch test location | What it covers | Main limitation |
|---|---|---|---|
| `mounty` handle snapshot | `mounty/src/fs/utils/file_handle.rs` | `open_handles()` returns `(fh, ino)` pairs | Does not test global shutdown with dirty data |
| `mounty` shutdown state | `mounty/src/fs/remote_fs/tests.rs` | New `open` calls are rejected after shutdown begins | Unit-level only |
| `mounty` clean-handle drain | `mounty/src/fs/remote_fs/tests.rs` | `shutdown_flush_all()` releases clean open handles | Does not prove dirty buffered write persistence |

## Existing Script Tests Under `test/`

| Script | What it covers | Main limitation |
|---|---|---|
| `test/test_chaos.py` | Builds `filer`/`mounty`, starts both, mounts FUSE, runs concurrent create/read/rename/delete/list operations through the mount, verifies checksums during normal operation | Cleanup performs external `umount`/`fusermount` before terminating `mounty`, so it does not prove graceful `mounty` shutdown |
| `test/run_perf_tests.sh` | Builds both binaries, starts both, mounts FUSE, checks mount exists, copies a large file, verifies checksum, compares throughput with `iperf3` | Performance-focused; checksum happens before shutdown; cleanup externally unmounts |
| `test/run_chunk_experiment.sh` | Runs `run_perf_tests.sh` over a file-size/chunk-size matrix | Performance experiment only |
| `test/Dockerfile.perf` and `docker-compose.perf.yml` | Containerized Linux/FUSE/performance environment | Orchestration, not an assertion suite by itself |

## Coverage Matrix

| Area | Necessary check | Rust unit coverage now? | Script coverage under `test/`? | Current location | Missing / Rust migration target |
|---|---|---|---|---|---|
| `filer` startup without `BASE_DIR` | Binary must fail immediately | No | No | None | Add `filer/tests/startup_shutdown.rs` |
| `filer` startup with nonexistent `BASE_DIR` | Binary must fail before serving | No | No | None | Add `filer/tests/startup_shutdown.rs` |
| `mounty` nonexistent mountpoint | Binary must fail before mount | No | No | None | Add `mounty/tests/startup.rs` |
| `mounty` backend down | Binary must fail before mount/probe | No | No | None | Add `mounty/tests/startup.rs` |
| `filer` HTTP `GET /list` | Direct server HTTP test | No | Indirect through FUSE only | `test/test_chaos.py` uses `os.listdir` through mount | Add `filer/tests/http_api.rs` with `reqwest` |
| `filer` HTTP file read | Direct `GET /files` test | Handler/client pieces only, not real server HTTP | Indirect through FUSE only | `test/test_chaos.py`, `test/run_perf_tests.sh` read through mount | Add `filer/tests/http_api.rs` |
| `filer` HTTP file write | Direct `PUT /files` test | Parser/handler pieces only, not real server HTTP | Indirect through FUSE only | `test/test_chaos.py`, `test/run_perf_tests.sh` write through mount | Add `filer/tests/http_api.rs` |
| `filer` HTTP mkdir | Direct `POST /mkdir` test | Handler-level direct call exists | Indirect through FUSE | `filer/src/handlers/tests.rs`; `test/test_chaos.py` uses `os.makedirs` | Add real HTTP coverage in `filer/tests/http_api.rs` |
| `filer` HTTP delete | Direct `DELETE /files` test | Handler-level direct call exists | Indirect through FUSE | `filer/src/handlers/tests.rs`; `test/test_chaos.py` uses `os.remove` | Add real HTTP coverage in `filer/tests/http_api.rs` |
| `filer` HTTP rename | Direct `PATCH /files` test | No direct real HTTP test | Indirect through FUSE only | `test/test_chaos.py` uses `os.rename` | Add `filer/tests/http_api.rs` |
| `filer` shutdown with `SIGTERM` | Process must exit within timeout | No | Partial, cleanup only | `test/test_chaos.py`, `test/run_perf_tests.sh` terminate/kill `filer` | Add explicit assert in `filer/tests/startup_shutdown.rs` |
| Real FUSE mount | Verify `mounty` actually mounts | No Rust test | Yes | `test/run_perf_tests.sh` checks the output of `mount` for the mountpoint | Can stay shell; optional ignored Rust E2E later |
| Concurrent FUSE operations | Stress create/read/rename/delete/list | No Rust FUSE test | Yes | `test/test_chaos.py` | Can stay Python; not first Rust migration target |
| Checksum through FUSE | Data correctness during normal operations | No Rust FUSE test | Yes | `test/test_chaos.py`, `test/run_perf_tests.sh` | Current coverage is useful; missing checksum after shutdown |
| `mounty` shutdown with `SIGTERM` | `mounty` must unmount by itself | Internal shutdown unit coverage only on branch | No | Cleanup currently does external `umount`/`fusermount` before terminating `mounty` | Add ignored E2E Rust test in `mounty/tests/fuse_shutdown.rs` |
| `mounty` shutdown preserves data | Write through mount, send `SIGTERM`, verify file in `BASE_DIR` | Internal clean-handle drain only on branch | No | Existing checksum happens before cleanup/shutdown | Add ignored E2E Rust test in `mounty/tests/fuse_shutdown.rs` |
| External unmount behavior | External unmount should make `mounty` exit/drain | No | No explicit assertion | Cleanup uses external unmount but treats it as cleanup, not a test | Add ignored Rust E2E or adapt `test/test_chaos.py` |
| Performance baseline | Throughput compared with `iperf3` | No | Yes | `test/run_perf_tests.sh` | Keep shell |
| Network shaping | `tc`, `dnctl`, `pfctl` bandwidth/loss setup | No | Yes | `test/run_perf_tests.sh` | Keep shell |
| Chunk matrix | File-size/chunk-size experiment | No | Yes | `test/run_chunk_experiment.sh` | Keep shell |
| Docker FUSE/perf environment | Containerized perf/FUSE setup | No | Yes | `test/Dockerfile.perf`, `docker-compose.perf.yml` | Keep Docker/shell |

## Recommended Rust Test Additions

| Priority | Test | Target file |
|---:|---|---|
| 1 | `filer` startup failure cases | `filer/tests/startup_shutdown.rs` |
| 2 | `mounty` startup failure cases | `mounty/tests/startup.rs` |
| 3 | Direct `filer` HTTP API coverage | `filer/tests/http_api.rs` |
| 4 | Explicit `filer` `SIGTERM` shutdown | `filer/tests/startup_shutdown.rs` |
| 5 | `mounty` `SIGTERM` + unmount + persisted data | `mounty/tests/fuse_shutdown.rs`, ignored by default |

## CI Performance Policy

The full chunk/file-size matrix should not run as a required PR check. It is too
expensive and too environment-sensitive for normal CI.

For automated performance checks, use only the two best chunk configurations:

| Chunk size | Max buffer size | Purpose |
|---:|---:|---|
| 4 MB (`4194304`) | 8 MB (`8388608`) | Stable high-throughput default candidate |
| 16 MB (`16777216`) | 32 MB (`33554432`) | Best-case large-chunk throughput candidate |

Use a 75% warning threshold against the `iperf3` baseline:

```text
if mounty_throughput < iperf_baseline * 0.75:
    emit warning
else:
    pass
```

This should be a warning/report, not a hard failure, at least until we have
stable historical data from the CI runner.

Suggested commands:

```bash
MOUNTY_CHUNK_SIZE=4194304 \
MOUNTY_MAX_BUFFER_SIZE=8388608 \
./test/run_perf_tests.sh --size-mb 200

MOUNTY_CHUNK_SIZE=16777216 \
MOUNTY_MAX_BUFFER_SIZE=33554432 \
./test/run_perf_tests.sh --size-mb 200
```

Recommended CI split:

| CI level | What runs | Required? |
|---|---|---:|
| PR fast CI | Rust unit/integration tests without FUSE | Yes |
| PR FUSE smoke, if runner supports it | One small correctness E2E without performance threshold | Optional |
| Nightly/manual perf | 4 MB and 16 MB chunk configs with 75% warning threshold | Warning/report only |
| Manual benchmark | Full chunk matrix and network shaping experiments | No |

The packet-loss experiments should remain report-oriented. Even if `iperf3`
includes the same network loss, it is still a continuous TCP stream and can
behave very differently from Mounty's chunked HTTP/FUSE workload.

## `test/test_chaos.py` Rust Migration Analysis

`test/test_chaos.py` is the script under `test/` that makes the most sense to
migrate to Rust.

Unlike the performance scripts, it is primarily a functional E2E test:

- start `filer`;
- start `mounty`;
- mount FUSE;
- create directories;
- write files;
- read and verify checksums;
- rename files;
- verify checksums after rename;
- delete files;
- create nested directories;
- list directories;
- run the above concurrently across workers.

That logic maps cleanly to Rust integration test code. The main difficulty is
not the logic itself; it is the environment:

- the test requires FUSE/macFUSE;
- the test spawns real binaries;
- the test needs robust readiness checks;
- the test needs robust mount cleanup;
- the test must avoid running by default in CI environments without FUSE.

### Current Python Behavior

The current Python cleanup flow is:

1. externally unmount the mountpoint with `umount` or `fusermount -u`;
2. terminate `mounty`;
3. terminate `filer`;
4. remove temp files/directories.

This is fine as cleanup, but it does not prove graceful `mounty` shutdown. It
specifically avoids testing whether `mounty` can receive `SIGTERM`, drain its
state, and unmount the filesystem by itself.

### Recommended Rust Replacement

Add a Rust E2E test such as:

```text
mounty/tests/e2e_chaos.rs
```

or, if we want to emphasize shutdown semantics:

```text
mounty/tests/fuse_shutdown_chaos.rs
```

The test should be ignored by default:

```rust
#[test]
#[ignore = "requires FUSE/macFUSE and local mount privileges"]
fn chaos_e2e_through_fuse() {
    // ...
}
```

Run manually with:

```text
cd mounty
cargo test --test e2e_chaos -- --ignored
```

Alternatively, gate it behind an environment variable:

```text
RUN_FUSE_E2E=1 cargo test --test e2e_chaos
```

### Rust Mapping

| Python behavior | Rust equivalent |
|---|---|
| `subprocess.Popen` for `filer` | `std::process::Command` |
| `subprocess.Popen` for `mounty` | `std::process::Command` |
| `/tmp/filer-test-base` | `tempfile::TempDir` |
| `/tmp/mounty-chaos-mnt` | `tempfile::TempDir` or a temp path preserved while mounted |
| `ThreadPoolExecutor` | `std::thread`, `crossbeam`, or `rayon` |
| `os.urandom` | `rand` crate or deterministic pseudo-random data |
| `hashlib.sha256` | `sha2` crate |
| `os.makedirs` | `std::fs::create_dir_all` |
| file writes | `std::fs::write` or `File::create` + `write_all` |
| file reads | `std::fs::read` |
| `os.rename` | `std::fs::rename` |
| `os.remove` | `std::fs::remove_file` |
| `os.listdir` | `std::fs::read_dir` |
| `terminate()` | send `SIGTERM`, preferably with `nix` on Unix |
| fallback `umount`/`fusermount` | cleanup-only fallback after test failure |

Suggested dev-dependencies:

```toml
tempfile = "3"
rand = "0.8"
sha2 = "0.10"
nix = { version = "0.29", features = ["signal"] }
```

`nix` is useful because `std::process::Child::kill()` sends a hard kill on Unix.
For graceful shutdown validation we need `SIGTERM`, not `SIGKILL`.

### Better Semantics Than the Python Test

The Rust migration should not copy the Python cleanup order exactly.

Instead, the Rust E2E should test the new graceful shutdown contract:

1. start `filer`;
2. start `mounty`;
3. wait until the mount is active;
4. run the chaos workload through the mount;
5. send `SIGTERM` to `mounty`;
6. assert that `mounty` exits within a timeout;
7. assert that the mountpoint is no longer mounted;
8. verify relevant data persisted in `BASE_DIR`;
9. terminate `filer`;
10. use external `umount` only as fallback cleanup if the test fails.

This makes the Rust version stronger than the Python version. It verifies the
actual lifecycle behavior introduced by graceful shutdown.

### Migration Recommendation

Recommended path:

1. Keep `test/test_chaos.py` for now.
2. Add the Rust E2E equivalent as ignored/manual.
3. Run both locally until the Rust version is stable on the target platforms.
4. Once the Rust E2E covers the same functional workload and the new shutdown
   assertions, remove or deprecate `test/test_chaos.py`.

The bash performance scripts should remain as scripts. The Python chaos test is
the one script that is a realistic Rust migration candidate.

## Notes

- The existing `test/` directory is valuable, but it mostly covers FUSE behavior
  and performance through scripts.
- It does not currently prove graceful shutdown behavior for `mounty`, because
  cleanup performs external unmount before terminating `mounty`.
- Startup validation and direct HTTP API checks are low-friction Rust integration
  tests and should be added first.
- FUSE shutdown tests can be written in Rust, but they should be ignored by
  default or gated behind an environment variable such as `RUN_FUSE_E2E=1`.
