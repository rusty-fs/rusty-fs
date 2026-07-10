# Test Coverage and Rust Migration Matrix

Updated: 2026-07-10

This document tracks Rust test coverage, script-based E2E coverage under
`test/`, and the remaining gaps after the current Rust migration work.

## Completed Coverage

| Area | Coverage | Location | Status |
|---|---|---|---|
| `filer` startup without `BASE_DIR` | Binary fails immediately | `filer/tests/startup_shutdown.rs` | Complete |
| `filer` startup with nonexistent `BASE_DIR` | Binary fails before serving | `filer/tests/startup_shutdown.rs` | Complete |
| `filer` `SIGTERM` shutdown | Process exits after signal | `filer/tests/startup_shutdown.rs` | Complete |
| `mounty` missing CLI args | Binary exits with usage error | `mounty/tests/startup.rs` | Complete |
| `mounty` nonexistent mountpoint | Fails before mount | `mounty/tests/startup.rs` | Complete |
| `mounty` file mountpoint | Fails before mount | `mounty/tests/startup.rs` | Complete |
| `mounty` backend down | Fails during backend probe | `mounty/tests/startup.rs` | Complete |
| `mounty` incompatible backend | Fails during backend probe and does not mount | `mounty/tests/startup.rs` | Complete |
| `filer` HTTP mkdir | Direct `POST /mkdir` against real server | `filer/tests/http_api.rs` | Complete |
| `filer` HTTP file write/read | Direct `PUT /files` and `GET /files` against real server | `filer/tests/http_api.rs` | Complete |
| `filer` HTTP list | Direct `GET /list` against real server | `filer/tests/http_api.rs` | Complete |
| `filer` HTTP delete | Direct `DELETE /files` against real server | `filer/tests/http_api.rs` | Complete |
| `filer` HTTP rename | Direct `PATCH /meta` rename against real server | `filer/tests/http_api.rs` | Complete |
| `filer` HTTP metadata | Direct `GET /meta` for files/directories | `filer/tests/http_api.rs` | Complete |
| `filer` metadata updates | `PATCH /meta` mode and timestamp updates | `filer/tests/http_api.rs` | Complete |
| `filer` HTTP range read | Valid, open-ended, unsatisfiable, and invalid `Range` | `filer/tests/http_api.rs` | Complete |
| `filer` HTTP range write | `Content-Range` partial write and final size handling | `filer/tests/http_api.rs` | Complete |
| `filer` HTTP negative cases | Missing metadata, missing delete, bad range, traversal | `filer/tests/http_api.rs` | Complete |
| Real FUSE normal operations | Write/read, mkdir/rmdir, rename, chmod, truncate, copy | `mounty/tests/e2e_mount_test.rs` | Complete, ignored/manual |
| `mounty` `SIGTERM` FUSE shutdown | Signal triggers unmount and data persistence check | `mounty/tests/fuse_shutdown.rs` | Complete, ignored/manual |
| External FUSE unmount | External unmount makes `mounty` exit and persists data | `mounty/tests/fuse_shutdown.rs` | Complete, ignored/manual |
| FUSE chaos workload | Concurrent create/read/rename/delete/list with checksums | `test/test_chaos.py` | Complete, script/manual |
| Throughput baseline | Large copy through mount and `iperf3` baseline warning | `test/run_perf_tests.sh` | Complete, script/manual |
| Chunk matrix | Repeats performance script over file/chunk matrix | `test/run_chunk_experiment.sh` | Complete, script/manual |
| Latency smoke | `mkdir`, `cp`, `ls`, `stat`, `cat`, `head`, `mv`, `rm`, `rmdir` through mount | `test/run_latency_smoke.sh` | Complete, script/manual |

## Test Execution Policy

| Test type | Command | Default expectation |
|---|---|---|
| `filer` fast tests | `cd filer && cargo test` | Required |
| `mounty` fast tests | `cd mounty && cargo test` | Required; FUSE tests are ignored |
| FUSE Rust E2E | `cd mounty && cargo test --test e2e_mount_test -- --ignored` | Manual/local |
| FUSE shutdown E2E | `cd mounty && cargo test --test fuse_shutdown -- --ignored` | Manual/local |
| Chaos script | `./test/test_chaos.py --workers 2 --files-per-worker 2 --max-file-kb 64` | Manual/local |
| Latency smoke | `./test/run_latency_smoke.sh --iterations 5 --size-kb 64 --threshold-ms 500` | Manual/local |
| Perf smoke | `./test/run_perf_tests.sh --size-mb 10` | Manual/local |
| Chunk matrix smoke | `SKIP_BUILD=1 RESULTS_FILE=/tmp/chunk_matrix_smoke.md ./test/run_chunk_experiment.sh --file-sizes 1 --chunks 1048576` | Manual/local |
| Full chunk matrix | `./test/run_chunk_experiment.sh` | Manual/ad-hoc only |

FUSE tests and scripts require FUSE/macFUSE, local mount privileges, and platform
unmount tools. They should not run as mandatory PR checks unless the runner is
explicitly provisioned for FUSE.

## Current Script Coverage Under `test/`

| Script | Purpose | Current behavior |
|---|---|---|
| `test/test_chaos.py` | Functional FUSE stress test | Uses dynamic temp dirs and ports, readiness polling, configurable workload size, checksum verification |
| `test/run_perf_tests.sh` | Throughput/correctness/perf warning | Uses dynamic temp dirs and ports, readiness polling, checksum verification, `iperf3` baseline |
| `test/run_latency_smoke.sh` | Small-operation latency smoke | Uses dynamic temp dirs and ports, reports per-operation latency warnings, fails on functional errors |
| `test/run_chunk_experiment.sh` | Ad-hoc chunk matrix | Calls `run_perf_tests.sh` repeatedly with chunk/buffer env vars |

## Remaining Gaps

| Gap | Why it remains | Suggested next action |
|---|---|---|
| Symlink escape policy in `filer` | `safe_join_path` blocks `..`, but does not enforce sandbox after symlink resolution | Decide whether to reject/follow/sandbox symlinks, then enable the ignored reproducer in `filer/tests/http_api.rs` |
| Automated FUSE CI | Local FUSE/macFUSE and mount privileges are environment-sensitive | Add an optional CI job only on a runner proven to support FUSE |
| Hard latency threshold | Current latency smoke reports warnings only | Convert to hard failure only after stable runner baseline data exists |
| Full performance matrix in CI | Too expensive and noisy for PR checks | Keep `run_chunk_experiment.sh` manual/ad-hoc |

## Verification Snapshot

Latest local non-FUSE verification:

```text
cd filer && cargo test
23 passed, 1 ignored

cd mounty && cargo test
46 passed, 8 ignored

bash -n test/run_perf_tests.sh
bash -n test/run_chunk_experiment.sh
bash -n test/run_latency_smoke.sh
python3 -m py_compile test/test_chaos.py
all passed

SKIP_BUILD=1 ./test/run_latency_smoke.sh --iterations 1 --size-kb 1 --threshold-ms 5000
passed

./test/test_chaos.py --skip-build --workers 1 --files-per-worker 1 --max-file-kb 4
passed

SKIP_BUILD=1 ./test/run_perf_tests.sh --size-mb 1
passed with expected performance warning for tiny payload

SKIP_BUILD=1 RESULTS_FILE=/tmp/chunk_matrix_smoke.md ./test/run_chunk_experiment.sh --file-sizes 1 --chunks 1048576
passed

cd mounty && cargo test --test e2e_mount_test -- --ignored --test-threads=1
6 passed

cd mounty && cargo test --test fuse_shutdown -- --ignored --test-threads=1
2 passed
```

The ignored/manual FUSE commands must be run on a machine with FUSE/macFUSE and
mount permissions before claiming end-to-end FUSE behavior is proven in that
environment.
