#!/bin/bash
set -e

# Configuration
MOUNT_DIR="/tmp/mounty-test-mnt"
TEST_FILE_SIZE_MB=1024
EXPECTED_THRESHOLD_PERCENT=50
FILER_PORT=3000
SERVER_URL="http://localhost:${FILER_PORT}"

# Variables
BW_LIMIT=""
PACKET_LOSS=""
OS_TYPE=$(uname -s)
FILER_PID=""
MOUNTY_PID=""
IPERF_SERVER_PID=""

# Help function
print_usage() {
    echo "Usage: $0 [--bw-limit <Mbit/s>] [--size-mb <MB>] [--packet-loss <%>]"
    echo "  --bw-limit:    Limit localhost bandwidth (requires sudo)"
    echo "  --size-mb:     Set test file size in megabytes (default: 1024)"
    echo "  --packet-loss: Set packet loss percentage (e.g., 5 for 5%)"
    exit 1
}

# Parse arguments
while [[ "$#" -gt 0 ]]; do
    case $1 in
        --bw-limit) BW_LIMIT="$2"; shift ;;
        --size-mb) TEST_FILE_SIZE_MB="$2"; shift ;;
        --packet-loss) PACKET_LOSS="$2"; shift ;;
        -h|--help) print_usage ;;
        *) echo "Unknown parameter: $1"; print_usage ;;
    esac
    shift
done

# Cleanup function
cleanup() {
    echo "[*] Cleaning up..."
    # Kill iperf server
    if [ -n "$IPERF_SERVER_PID" ]; then
        kill "$IPERF_SERVER_PID" 2>/dev/null || true
    fi

    # Unmount mounty
    if [ -n "$MOUNTY_PID" ]; then
        if [ "$OS_TYPE" = "Linux" ]; then
            fusermount -u "$MOUNT_DIR" 2>/dev/null || true
        else
            umount "$MOUNT_DIR" 2>/dev/null || true
        fi
        kill "$MOUNTY_PID" 2>/dev/null || true
    fi

    # Kill filer
    if [ -n "$FILER_PID" ]; then
        kill "$FILER_PID" 2>/dev/null || true
    fi

    # Remove network limits
    if [ -n "$BW_LIMIT" ] || [ -n "$PACKET_LOSS" ]; then
        echo "[*] Removing network shaping rules..."
        if [ "$OS_TYPE" = "Darwin" ]; then
            sudo dnctl -q flush
            sudo pfctl -f /etc/pf.conf 2>/dev/null || true
            sudo pfctl -d 2>/dev/null || true
        elif [ "$OS_TYPE" = "Linux" ]; then
            sudo tc qdisc del dev lo root 2>/dev/null || true
        fi
    fi

    # Clean test files
    rm -f /tmp/mounty_test_source.dat
    rm -rf "$MOUNT_DIR"
    rm -rf "/tmp/filer_base_dir"
    
    echo "[*] Cleanup complete."
}
trap cleanup EXIT

# Apply network shaping if requested
if [ -n "$BW_LIMIT" ] || [ -n "$PACKET_LOSS" ]; then
    echo "[*] Applying network limits (BW: ${BW_LIMIT:-Unlimited} Mbit/s, Loss: ${PACKET_LOSS:-0}%)..."
    if [ "$OS_TYPE" = "Darwin" ]; then
        # macOS dnctl setup
        DNCTL_CONFIG="pipe 1 config"
        if [ -n "$BW_LIMIT" ]; then
            DNCTL_CONFIG="$DNCTL_CONFIG bw ${BW_LIMIT}Mbit/s"
        fi
        if [ -n "$PACKET_LOSS" ]; then
            PLR=$(awk "BEGIN {print $PACKET_LOSS / 100}")
            DNCTL_CONFIG="$DNCTL_CONFIG plr $PLR"
        fi
        sudo dnctl $DNCTL_CONFIG
        (
            echo "dummynet out quick proto tcp to 127.0.0.1 port $FILER_PORT pipe 1"
            echo "dummynet out quick proto tcp to 127.0.0.1 port 5201 pipe 1"
        ) | sudo pfctl -f -
        sudo pfctl -e 2>/dev/null || true
    elif [ "$OS_TYPE" = "Linux" ]; then
        # Linux tc setup on loopback
        if [ -n "$BW_LIMIT" ]; then
            sudo tc qdisc add dev lo root handle 1: htb default 12
            sudo tc class add dev lo parent 1: classid 1:1 htb rate "${BW_LIMIT}mbit"
            sudo tc class add dev lo parent 1:1 classid 1:12 htb rate "${BW_LIMIT}mbit" ceil "${BW_LIMIT}mbit"
            if [ -n "$PACKET_LOSS" ]; then
                sudo tc qdisc add dev lo parent 1:12 handle 10: netem loss "${PACKET_LOSS}%"
            fi
        else
            if [ -n "$PACKET_LOSS" ]; then
                sudo tc qdisc add dev lo root netem loss "${PACKET_LOSS}%"
            fi
        fi
    else
        echo "Unsupported OS for network shaping: $OS_TYPE"
        exit 1
    fi
fi

# Build binaries
if [ -z "$SKIP_BUILD" ]; then
    echo "[*] Building mounty and filer (release mode)..."
    cargo build --release --manifest-path ./filer/Cargo.toml
    cargo build --release --manifest-path ./mounty/Cargo.toml
else
    echo "[*] Skipping build step (SKIP_BUILD is set)..."
fi

# Start filer
echo "[*] Starting filer server on port $FILER_PORT..."
export BASE_DIR="/tmp/filer_base_dir"
mkdir -p "$BASE_DIR"
./filer/target/release/remote-fs-server --port $FILER_PORT &
FILER_PID=$!
sleep 2 # Wait for server to start

if ! kill -0 $FILER_PID 2>/dev/null; then
    echo "[!] ERROR: Filer failed to start or crashed immediately."
    exit 1
fi

# Start mounty
echo "[*] Creating mountpoint $MOUNT_DIR and starting mounty..."
mkdir -p "$MOUNT_DIR"
./mounty/target/release/mounty "$SERVER_URL" "$MOUNT_DIR" &
MOUNTY_PID=$!
sleep 3 # Wait for mount to be ready

if ! kill -0 $MOUNTY_PID 2>/dev/null; then
    echo "[!] ERROR: Mounty failed to start or crashed immediately."
    exit 1
fi

if ! mount | grep -q "$MOUNT_DIR"; then
    echo "[!] ERROR: FUSE mount was not successfully created at $MOUNT_DIR."
    exit 1
fi

# iperf3 baseline test
echo "[*] Running iperf3 baseline test..."
iperf3 -s -p 5201 &
IPERF_SERVER_PID=$!
sleep 1

# Run iperf3 client and parse Mbit/s (using grep and awk safely)
IPERF_OUT=$(iperf3 -c 127.0.0.1 -p 5201 -t 5 -f m)
echo "$IPERF_OUT"
# Extract the receiver Mbit/s speed
BASELINE_MBITS=$(echo "$IPERF_OUT" | grep sender | awk '{print $7}')
if [ -z "$BASELINE_MBITS" ]; then
    echo "[!] Failed to determine iperf3 baseline. Defaulting to 0."
    BASELINE_MBITS=0
fi
echo "[*] Baseline Network Speed: $BASELINE_MBITS Mbit/s"

# Generate test payload
echo "[*] Generating ${TEST_FILE_SIZE_MB}MB test payload..."
if [ "$OS_TYPE" = "Darwin" ]; then
    mkfile "${TEST_FILE_SIZE_MB}m" /tmp/mounty_test_source.dat
else
    dd if=/dev/urandom of=/tmp/mounty_test_source.dat bs=1M count="$TEST_FILE_SIZE_MB" status=none
fi

# Calculate Original Checksum
echo "[*] Calculating SHA256 of original file..."
if command -v sha256sum >/dev/null 2>&1; then
    ORIGINAL_HASH=$(sha256sum /tmp/mounty_test_source.dat | awk '{print $1}')
else
    ORIGINAL_HASH=$(shasum -a 256 /tmp/mounty_test_source.dat | awk '{print $1}')
fi

# Perform file copy test
echo "[*] Copying file to mounty mountpoint..."
START_TIME=$(date +%s%N)
dd if=/tmp/mounty_test_source.dat of="$MOUNT_DIR/mounty_test_dest.dat" bs=1M status=none
END_TIME=$(date +%s%N)

# Calculate duration and speed
# Note: macOS date doesn't support %N out of the box without coreutils, fallback to seconds if %N fails
if [[ "$START_TIME" == *"N" ]]; then
    START_TIME=$(date +%s)
    END_TIME=$(date +%s)
    DURATION=$((END_TIME - START_TIME))
    if [ "$DURATION" -eq 0 ]; then DURATION=1; fi # Prevent division by zero
else
    DURATION_MS=$(( (END_TIME - START_TIME) / 1000000 ))
    DURATION=$(awk "BEGIN {print $DURATION_MS/1000}")
fi

ACTUAL_MBPS=$(awk "BEGIN {print ($TEST_FILE_SIZE_MB * 8) / $DURATION}")

echo "[*] File Copy Completed in $DURATION seconds."
echo "[*] Actual Transfer Speed: $ACTUAL_MBPS Mbit/s"

# Verify Checksum
echo "[*] Verifying copied file checksum..."
if command -v sha256sum >/dev/null 2>&1; then
    COPIED_HASH=$(sha256sum "$MOUNT_DIR/mounty_test_dest.dat" | awk '{print $1}')
else
    COPIED_HASH=$(shasum -a 256 "$MOUNT_DIR/mounty_test_dest.dat" | awk '{print $1}')
fi

if [ "$ORIGINAL_HASH" != "$COPIED_HASH" ]; then
    echo "[!] ERROR: Checksum mismatch!"
    echo "  Original: $ORIGINAL_HASH"
    echo "  Copied:   $COPIED_HASH"
    exit 1
else
    echo "[*] SUCCESS: Checksums match."
fi

# Reporting
if (( $(echo "$BASELINE_MBITS > 0" | bc -l) )); then
    THRESHOLD=$(echo "$BASELINE_MBITS * ($EXPECTED_THRESHOLD_PERCENT / 100)" | bc -l)
    if (( $(echo "$ACTUAL_MBPS < $THRESHOLD" | bc -l) )); then
        echo "======================================================"
        echo "[!] PERFORMANCE WARNING"
        echo "Actual speed ($ACTUAL_MBPS Mbit/s) is below threshold ($THRESHOLD Mbit/s)."
        echo "Baseline: $BASELINE_MBITS Mbit/s"
        echo "======================================================"
    else
        echo "[*] Performance meets expectations (>= ${EXPECTED_THRESHOLD_PERCENT}% of baseline)."
    fi
else
    echo "[!] Skipping performance threshold check (baseline invalid)."
fi

echo "[*] All tests completed successfully!"
exit 0
