#!/bin/bash
set -euo pipefail

free_port() {
    python3 - <<'PY'
import socket
with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
}

MOUNT_DIR="${MOUNT_DIR:-$(mktemp -d /tmp/mounty-latency-smoke-mnt.XXXXXX)}"
BASE_DIR="${BASE_DIR:-$(mktemp -d /tmp/mounty-latency-smoke-base.XXXXXX)}"
FILER_PORT="${FILER_PORT:-$(free_port)}"
SERVER_URL="http://127.0.0.1:${FILER_PORT}"
ITERATIONS=10
THRESHOLD_MS=500
SIZE_KB=64
SKIP_BUILD="${SKIP_BUILD:-}"

FILER_PID=""
MOUNTY_PID=""
OS_TYPE=$(uname -s)
WARNINGS=0

usage() {
    echo "Usage: $0 [--iterations N] [--threshold-ms MS] [--size-kb KB]"
    exit 1
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --iterations) ITERATIONS="$2"; shift ;;
        --threshold-ms) THRESHOLD_MS="$2"; shift ;;
        --size-kb) SIZE_KB="$2"; shift ;;
        -h|--help) usage ;;
        *) echo "Unknown parameter: $1"; usage ;;
    esac
    shift
done

cleanup() {
    echo "[*] Cleaning up..."
    if [[ -n "$MOUNTY_PID" ]]; then
        if [[ "$OS_TYPE" = "Linux" ]]; then
            fusermount3 -u "$MOUNT_DIR" 2>/dev/null || fusermount -u "$MOUNT_DIR" 2>/dev/null || umount "$MOUNT_DIR" 2>/dev/null || true
        else
            umount "$MOUNT_DIR" 2>/dev/null || diskutil unmount "$MOUNT_DIR" >/dev/null 2>&1 || true
        fi
        kill "$MOUNTY_PID" 2>/dev/null || true
        wait "$MOUNTY_PID" 2>/dev/null || true
    fi
    if [[ -n "$FILER_PID" ]]; then
        kill "$FILER_PID" 2>/dev/null || true
        wait "$FILER_PID" 2>/dev/null || true
    fi
    rm -rf "$MOUNT_DIR" "$BASE_DIR"
}
trap cleanup EXIT

now_ms() {
    if command -v python3 >/dev/null 2>&1; then
        python3 -c 'import time; print(time.time_ns() // 1_000_000)'
    else
        date +%s000
    fi
}

measure() {
    local label="$1"
    shift
    local start end elapsed
    start=$(now_ms)
    "$@"
    end=$(now_ms)
    elapsed=$((end - start))
    printf "[latency] %-24s %5d ms\n" "$label" "$elapsed"
    if (( elapsed > THRESHOLD_MS )); then
        WARNINGS=$((WARNINGS + 1))
        echo "[!] WARNING: ${label} exceeded ${THRESHOLD_MS}ms (${elapsed}ms)"
        if [[ -n "${GITHUB_ACTIONS:-}" ]]; then
            echo "::warning title=Mounty latency smoke::${label} took ${elapsed}ms, above ${THRESHOLD_MS}ms"
        fi
    fi
}

wait_for_http() {
    local deadline=$((SECONDS + 10))
    while (( SECONDS < deadline )); do
        if curl -fsS "$SERVER_URL/list" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.1
    done
    echo "[!] ERROR: filer did not become ready"
    return 1
}

wait_for_mount() {
    local deadline=$((SECONDS + 15))
    local canonical_mount_dir
    canonical_mount_dir=$(cd "$MOUNT_DIR" && pwd -P)
    while (( SECONDS < deadline )); do
        if mount | grep -Fq "$MOUNT_DIR" || mount | grep -Fq "$canonical_mount_dir"; then
            return 0
        fi
        sleep 0.1
    done
    echo "[!] ERROR: mounty did not mount ${MOUNT_DIR}"
    return 1
}

if [[ -z "$SKIP_BUILD" ]]; then
    echo "[*] Building filer and mounty in release mode..."
    cargo build --release --manifest-path ./filer/Cargo.toml
    cargo build --release --manifest-path ./mounty/Cargo.toml
else
    echo "[*] Skipping build step (SKIP_BUILD is set)."
fi

rm -rf "$MOUNT_DIR" "$BASE_DIR"
mkdir -p "$MOUNT_DIR" "$BASE_DIR"

echo "[*] Starting filer on ${SERVER_URL}..."
BASE_DIR="$BASE_DIR" ./filer/target/release/remote-fs-server --port "$FILER_PORT" &
FILER_PID=$!
wait_for_http

echo "[*] Starting mounty on ${MOUNT_DIR}..."
./mounty/target/release/mounty "$SERVER_URL" "$MOUNT_DIR" &
MOUNTY_PID=$!
wait_for_mount

PAYLOAD="/tmp/mounty-latency-smoke-payload.$$"
if [[ "$OS_TYPE" = "Darwin" ]]; then
    mkfile "${SIZE_KB}k" "$PAYLOAD"
else
    dd if=/dev/urandom of="$PAYLOAD" bs=1024 count="$SIZE_KB" status=none
fi

echo "[*] Running ${ITERATIONS} latency smoke iterations with threshold ${THRESHOLD_MS}ms..."
for i in $(seq 1 "$ITERATIONS"); do
    DIR="$MOUNT_DIR/latency_${i}"
    EMPTY_DIR="$MOUNT_DIR/latency_empty_${i}"
    FILE="$DIR/file.bin"
    PARTIAL_FILE="$DIR/partial.bin"
    CAT_FILE="$DIR/cat.bin"
    MOVE_FILE="$DIR/move.bin"
    MOVED="$DIR/file_moved.bin"

    measure "mkdir" mkdir -p "$DIR"
    measure "rmdir" bash -c 'mkdir "$1" && rmdir "$1"' _ "$EMPTY_DIR"
    measure "write ${SIZE_KB}KB" cp "$PAYLOAD" "$FILE"
    measure "ls" bash -c 'ls "$1" >/dev/null' _ "$DIR"
    measure "stat" bash -c 'stat "$1" >/dev/null' _ "$FILE"
    cp "$PAYLOAD" "$PARTIAL_FILE"
    measure "partial read" bash -c 'dd if="$1" of=/dev/null bs=4096 count=1 2>/dev/null' _ "$PARTIAL_FILE"
    cp "$PAYLOAD" "$CAT_FILE"
    measure "cat" bash -c 'cat "$1" >/dev/null' _ "$CAT_FILE"
    cp "$PAYLOAD" "$MOVE_FILE"
    measure "mv" mv "$MOVE_FILE" "$MOVED"
    measure "rm" rm "$MOVED"
done

rm -f "$PAYLOAD"

if (( WARNINGS > 0 )); then
    echo "[*] Latency smoke completed with ${WARNINGS} warning(s)."
else
    echo "[*] Latency smoke completed without warnings."
fi
