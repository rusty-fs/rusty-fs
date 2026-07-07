# E2E Performance Tests

This directory contains the end-to-end performance test for `mounty` and `filer`.
The test starts the real `filer` server, mounts `mounty` through FUSE, copies a generated payload through the mountpoint, verifies the checksum, and reports throughput against an `iperf3` baseline.

## Requirements

To run the test locally, you need:

- `iperf3` (for baseline network measurement)
- `fuse3` (for mounting)
- `cargo` and `rustc`
- `dnctl` (on macOS) or `tc` (on Linux) for bandwidth shaping

## How to Run

Run the script from the project root.

**Run with unlimited bandwidth:**

```bash
./test/run_perf_tests.sh
```

**Run with simulated bandwidth limit and custom payload size:**

```bash
./test/run_perf_tests.sh --bw-limit 100 --size-mb 500
```

**Run with simulated packet loss (e.g. 5% packet loss):**

```bash
./test/run_perf_tests.sh --packet-loss 5
```

Using `--bw-limit` or `--packet-loss` on macOS uses `dnctl` and `pfctl`, which can prompt for your `sudo` password to temporarily configure firewall rules. On Linux it uses `tc`. The script removes the shaping rules during cleanup.

## CI Policy

The GitHub Actions E2E workflow runs this script directly on the Ubuntu runner. The old Docker-based perf path has been removed because it duplicated setup, required privileged FUSE/container wiring, and was not the path exercised by CI.

The CI matrix is intentionally small:

- `MOUNTY_CHUNK_SIZE=4MB`, `MOUNTY_MAX_BUFFER_SIZE=8MB`
- `MOUNTY_CHUNK_SIZE=16MB`, `MOUNTY_MAX_BUFFER_SIZE=32MB`

Both jobs currently use a 200MB payload with `--bw-limit 100 --packet-loss 5`.

These two configurations are the high-throughput candidates kept as CI coverage. Wider chunk-size experiments remain better suited to ad-hoc/local runs because they are slower and more sensitive to runner noise.

## Performance Threshold

The script compares the measured FUSE copy throughput with the `iperf3` baseline and uses `EXPECTED_THRESHOLD_PERCENT`, defaulting to `75`.

If throughput is below 75% of the baseline, the script emits a warning but does not fail the run. This keeps CI useful for reporting regressions without making packet-loss or runner-noise cases flaky. Correctness still fails hard through process startup checks, mount checks, and checksum verification.

## What the Test Does

1. **Compiles** both `filer` and `mounty` in release mode.
2. **Shapes Network** optionally via loopback interface limits.
3. **Spawns** the server and client processes in the background.
4. **Baselines** the network by running an `iperf3` client-server test to find the true line-rate.
5. **Transfers Data** by generating a 1GB randomized payload (configurable via `--size-mb`) and copying it over the FUSE mount.
6. **Validates** by comparing throughput against the `iperf3` baseline and verifying the `SHA-256` checksums of the original and transferred files.
7. **Cleans up** all temporary files, mountpoints, and background processes regardless of success or failure.
