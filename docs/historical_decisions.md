# Mounty: Historical Architectural Decisions & Bug Root Causes

This document serves as an archive of the critical architectural choices, performance optimizations, and significant bug resolutions discovered during the development of Mounty (the FUSE client) and Filer (the HTTP server).

## 1. Performance Architecture: The FOPEN_DIRECT_IO Decision

Early in development, Mounty exhibited severe read performance degradation on local gigabit networks (~350 KB/s for 16KB blocks vs ~19 MB/s for 1MB blocks).

### The Dilemma
The FUSE module was enforcing the `FOPEN_DIRECT_IO` flag, which entirely bypasses the macOS/Linux kernel page cache. This forced every small 16KB `read()` from user applications to become a synchronous, blocking HTTP GET request with a ~45ms round-trip penalty.

### The Decision: Keep `FOPEN_DIRECT_IO` and use Client-Side Buffering
Instead of dropping `FOPEN_DIRECT_IO` and relying on the kernel's page cache (which would provide free read-ahead and write coalescing), the decision was made to **keep `FOPEN_DIRECT_IO` active** and implement custom Read-Ahead (4MB) and Write Batching (1MB) inside Mounty's `FhState`.

**Why?**
Relying on the kernel's page cache with an HTTP backend introduced unacceptable risks for data integrity. The kernel performs asynchronous writebacks and flushes pages in an arbitrary order. If the `mounty` daemon crashed before the kernel flushed the dirty pages, data would be silently lost. Furthermore, streaming `PUT` requests require sequential data. By keeping direct I/O, `mounty` maintained absolute control over the data flow, ensuring that `fsync()` and `flush()` correctly wait for HTTP `PUT` confirmations before reporting success to the kernel.

## 2. The "Zero-Byte Corruption" Bug (cp -X issue)

A critical intermittent bug (~80% reproduction rate) occurred when copying large files (e.g., 200MB `.mp4` videos) to the Mounty volume using macOS's `cp -X`. The resulting file on the server had the correct size and correct ending bytes, but the first several megabytes were overwritten entirely with zeros (`0x00`).

### Root Cause Analysis
The bug was a race condition born from the interaction between macOS's VFS `mmap` behavior and FUSE `setattr` commands.

1. **macOS `mmap` behavior**: `cp` mapped the file into memory to write it.
2. **The `setattr` race**: Before or during the flush of these dirty pages, `cp -X` invoked `setattr` to finalize the file size (e.g., to 211MB).
3. **The `filer` server bug**: The HTTP server was ignoring the `total_size` hint in the `Content-Range` header of early partial `PUT` requests. Therefore, the server file was physically smaller than the macOS kernel expected.
4. **The `mounty` client bug**: `mounty` did not explicitly flush its internal write buffers before executing `setattr`, nor did it update its internal `metadata_cache` upon truncation/extension.
5. **The Cascade**: Because the server file was artificially short, when macOS VFS performed a page fault read to populate a dirty page, it received an EOF (0 bytes) from the server. The kernel interpreted this EOF as "this file is sparse / empty here" and filled the page with zeros. It then flushed these zero-filled pages back to `mounty`, which dutifully uploaded the zeros to the server, overwriting the actual video data.

### The Resolution
- **Server Side (`filer`)**: The server was updated to immediately `set_len(total_size)` upon receiving the first `PUT` with a `total_size` hint (Pre-allocation). It also added explicit `sync_all()` calls to guarantee persistence.
- **Client Side (`mounty`)**:
  - `setattr` was modified to explicitly flush all pending write buffers before sending truncation/extension commands to the server.
  - The local `metadata_cache` was updated to immediately reflect size changes from `setattr` and `open(O_TRUNC)`.
  - `getattr` was updated to consult the open file handles (`FhState`) to return the true file size (including data residing in local write buffers), preventing the kernel from becoming confused about the file's boundaries.

## 3. End-to-End Performance Testing Framework

To ensure the filesystem's performance regressions could be reliably tracked, an automated E2E performance framework was established.

### The Decision: Orchestrating External Tools (Bash/Docker) over Pure Rust
While unit and internal integration tests were written natively in Rust, the infrastructure-level E2E tests were implemented using a Bash script (`test/run_perf_tests.sh`) and Docker containerization.

**Why?**
The test required simulating realistic network bottlenecks and measuring true TCP line-rate. 
- Using raw Rust `TcpStream` tests would artificially report lower speeds due to CPU bottlenecking and lack of TCP window scaling features.
- Instead, the script uses **`iperf3`** to accurately measure the true network baseline before each test.
- It leverages OS-specific network shaping (`tc` on Linux and `dnctl` on macOS) to throttle loopback interfaces, simulating high-latency/low-bandwidth remote networks.
- It tests FUSE via actual filesystem tools (`cp`, `dd`) rather than programmatic memory writes, ensuring the entire kernel VFS stack is part of the test matrix.

### Network Behavior: iperf3 vs Mounty (HTTP/FUSE) under Packet Loss
During performance testing with artificial bandwidth limits (e.g., 100 Mbit/s) and Packet Loss (5%), a massive discrepancy was observed: `iperf3` maintained near-100 Mbit/s speeds, while Mounty collapsed to ~5-8 Mbit/s.

This is expected and documents a critical architectural limitation of FUSE over HTTP:
1. **Async Flow vs Stop-and-Wait**: `iperf3` pushes a continuous asynchronous stream. When a packet drops, TCP Fast Retransmit kicks in immediately because subsequent packets trigger duplicate ACKs, allowing the stream to remain saturated. Mounty, conversely, buffers data into 1MB chunks and sends synchronous HTTP `PUT` requests. It must wait for a `200 OK` before sending the next chunk.
2. **The TCP Timeout Trap (RTO)**: Because Mounty stops sending data at the end of each 1MB chunk, a packet lost near the end of the chunk has no subsequent packets to trigger Fast Retransmit. The TCP stack is forced to wait for a Retransmission Timeout (RTO) of ~200-500ms before resending. 
3. **Bandwidth Scaling**: At higher bandwidths (e.g., 500 Mbit/s), the TCP window is large enough to absorb these losses more gracefully and trigger Fast Recovery before hitting RTO, leading to significantly better relative performance (recovering to ~50% of the baseline instead of ~5%).

### Optimization: The Chunk Size Matrix Experiment
To mitigate the TCP RTO penalty described above, an automated matrix experiment was conducted testing various file sizes (10MB to 2GB) against different FUSE chunk write sizes (1MB, 4MB, 16MB) under 5% packet loss and a 100 Mbit/s limit.

**Findings:**
- **Small files (≤ 50MB)**: All chunk sizes performed exceptionally well (~85-92 Mbit/s). This is attributed to `reqwest`'s TCP Keep-Alive connection reuse. The connection stays open across chunks, maintaining a large TCP congestion window. The file finishes transferring before the router buffer (dummynet queue) permanently saturates.
- **Large files (≥ 200MB)**: The 1MB chunk size collapsed entirely (dropping to ~4-17 Mbit/s). The router queue overflows, causing massive tail-drop packet loss. With a 1MB chunk, there isn't enough in-flight data to trigger Fast Retransmit consistently, plunging the connection into a catastrophic RTO death spiral.
- **Large Chunks (4MB & 16MB)**: Maintained >88 Mbit/s even on 2GB payloads. The larger bursts ensure the TCP congestion window stays wide enough that dropped packets are almost always recovered via Fast Retransmit within 1 RTT, completely bypassing the RTO penalty.

**The Decision: 4MB Chunk Size (Write) / 8MB Read-Ahead (Read)**
While 16MB yielded slightly better throughput, 4MB was selected as the optimal default `chunk_size` because:
1. **Memory Pressure**: FUSE buffers these chunks in RAM per file handle. A 16MB chunk could consume excessive memory on a high-concurrency Filer server.
2. **Timeout Risks**: On very slow connections, uploading a single 16MB chunk synchronously could trigger HTTP proxy/load-balancer 504 timeouts.
3. **Read-Ahead**: The `max_buffer_size` (which governs read prefetching) was increased to 8MB. Since reading is generally sequential and doesn't suffer from upload timeout risks, an 8MB read-ahead provides excellent streaming performance without blowing up RAM usage.
