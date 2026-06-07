# Remote File System Testing Plan & Report

## 1. Testing Plan

Our goal is to thoroughly test the remote file system (`filer` server and `mounty` client) focusing on functionality, correctness, and stress testing, including random testing techniques.

### 1.1 Unit Testing
**Objective**: Verify the correctness of individual components in isolation.
- **`filer`**: Test REST endpoints (`GET`, `PUT`, `POST`, `DELETE`), file manipulation logic, and edge cases (e.g., path traversal prevention, large file handling).
- **`mounty`**: Test the `FhManager` (file handle manager), FUSE request routing, caching logic, and state transitions.
- **Random Testing (Fuzzing/Property-based)**: We will introduce `proptest` (a property-based testing framework in Rust) to generate random sequences of operations, random file contents, and random file paths to ensure the logic doesn't panic and maintains a consistent state.

### 1.2 Integration Testing
**Objective**: Ensure that the `mounty` FUSE client correctly interacts with the `filer` REST API.
- **Setup**: Start an in-memory or temporary `filer` instance and a mock or real `mounty` instance.
- **Scenarios**:
  - Uploading a file from the client and verifying it exists on the server.
  - Creating nested directories.
  - Handling server timeouts or disconnections gracefully.
  - Concurrency: Multiple clients accessing the server simultaneously.

### 1.3 End-to-End (E2E) & Stress Testing
**Objective**: Test the system as a real user would, stressing the implementation to expose race conditions or memory leaks.
- **Setup**: A fully deployed `filer` server and a running `mounty` FUSE mount.
- **Scenarios**:
  - **Randomized FUSE Operations**: using tools like `fsstress` or custom Python scripts that perform random sequences of `mkdir`, `rm`, `touch`, `write`, `truncate`, and `read`.
  - **Large File Streaming**: Uploading and downloading files > 100MB while performing other concurrent operations.
  - **Concurrent Workload**: Simulating heavy read/write loads using tools like `fio` on the mounted FUSE directory.

---

## 2. Report: Testing Choices & Execution Status

### 2.1 Testing Choices
- **Frameworks**: 
  - Standard Rust `cargo test` for unit and integration testing.
  - `proptest` for property-based (random) testing of internal logic.
  - Custom bash/python scripts for E2E testing on the mounted file system.
- **Stress Testing Tools**: `fsstress` or standard Linux filesystem benchmarking tools like `fio` to simulate heavy random workloads.

### 2.2 What Has Been Tested (Current Status)
*We are currently in the process of implementing this plan. The status below will be updated as we progress.*

- [x] **Initial Assessment**: Both `filer` and `mounty` compile. Minor test compilation errors in `mounty` (due to missing arguments in `FhManager::alloc_fh`) have been fixed.
- [x] **Unit Tests (`filer`)**: Complete. Added property-based tests using `proptest` for request parsing logic (`parse_content_range`, `parse_range_header`), and async integration tests for `mkdir` and `delete_path`.
- [x] **Unit Tests (`mounty`)**: Complete. Added property-based tests using `proptest` for `FhManager` and `InodeMapper`. Also fixed a bounds bug identified during property testing!
- [x] **Integration Tests**: Addressed indirectly via the basic scripts and the new E2E harness.
- [x] **E2E & Stress Tests**: Complete. A new chaotic and concurrent test harness (`test_chaos.py`) has been developed by the E2E testing agent. It effectively tests the system under heavy random loads alongside the existing `run_perf_tests.sh`.

### 2.3 Bugs Discovered & Fixed
- **Rename Endpoint Missing**: Chaos testing revealed that the `rename` FUSE operation was failing because `mounty` expected a `PATCH /files/{path}` endpoint to handle renames, which was not yet implemented in `filer`. We implemented the `PATCH` endpoint to allow `filer` to process file rename requests, ensuring that file system moves and renames correctly reflect on the server.
- **Filer Port Binding**: Discovered that `filer` ignored the `--port` argument. Fixed.
- **Noise Reduction**: Reduced log levels for `ENOENT` errors from `ERROR` to `DEBUG`.
