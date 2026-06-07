# E2E Performance Testing Framework

This directory contains the automated end-to-end performance testing suite for `mounty` and `filer`. The framework simulates realistic network conditions and tests the actual FUSE mounting and file copy speeds.

## Requirements

To run the tests locally, you need:
- `iperf3` (for baseline network measurement)
- `fuse3` (for mounting)
- `cargo` and `rustc`
- `dnctl` (on macOS) or `tc` (on Linux) for bandwidth shaping

## How to Run

You have two options for running the tests: locally on your host machine or headlessly via Docker.

### Option 1: Local Execution (Recommended for Dev)

Run the bash script from the root of the project.

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

*Note: Using `--bw-limit` or `--packet-loss` on macOS utilizes `dnctl` and `pfctl`, which will prompt for your `sudo` password to temporarily configure the firewall rules. The script will safely clean up these rules upon exit.*

### Option 2: Containerized Execution (Headless / CI)

If you prefer an isolated environment or want to run tests in a CI/CD pipeline, you can use the pre-configured Docker Compose setup. This environment automatically provides the necessary `SYS_ADMIN` capabilities and maps `/dev/fuse` to allow FUSE to work inside the container.

From the project root, run:
```bash
docker-compose -f docker-compose.perf.yml up --build --abort-on-container-exit
```

*Tip: To simulate network constraints within Docker without requiring `sudo` locally, you can modify the `command` override in `docker-compose.perf.yml` to append `--bw-limit <Mbit/s>`.*

## What the Test Does
1. **Compiles** both `filer` and `mounty` in release mode.
2. **Shapes Network** (optional) via loopback interface limits.
3. **Spawns** the server and client processes in the background.
4. **Baselines** the network by running an `iperf3` client-server test to find the true line-rate.
5. **Transfers Data** by generating a 1GB randomized payload (configurable via `--size-mb`) and copying it over the FUSE mount.
6. **Validates** by comparing the actual transfer speed against the `iperf3` baseline (warning if < 50%) and verifying the `SHA-256` checksums of the original and transferred files.
7. **Cleans up** all temporary files, mountpoints, and background processes regardless of success or failure.
