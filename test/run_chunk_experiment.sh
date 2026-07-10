#!/usr/bin/env bash
set -euo pipefail

BW_LIMIT="${BW_LIMIT:-}"
PACKET_LOSS="${PACKET_LOSS:-}"
FILE_SIZES_CSV="${FILE_SIZES_CSV:-10,50,200,500,2048}"
CHUNKS_CSV="${CHUNKS_CSV:-1048576,4194304,16777216}"
RESULTS_FILE="${RESULTS_FILE:-/tmp/chunk_matrix_results.md}"
SKIP_BUILD_AFTER_FIRST="${SKIP_BUILD_AFTER_FIRST:-1}"

usage() {
    echo "Usage: $0 [--file-sizes 10,50] [--chunks 1048576,4194304] [--bw-limit Mbit/s] [--packet-loss %] [--results-file path]"
    exit 1
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --file-sizes) FILE_SIZES_CSV="$2"; shift ;;
        --chunks) CHUNKS_CSV="$2"; shift ;;
        --bw-limit) BW_LIMIT="$2"; shift ;;
        --packet-loss) PACKET_LOSS="$2"; shift ;;
        --results-file) RESULTS_FILE="$2"; shift ;;
        -h|--help) usage ;;
        *) echo "Unknown parameter: $1"; usage ;;
    esac
    shift
done

IFS=',' read -r -a FILE_SIZES <<< "$FILE_SIZES_CSV"
IFS=',' read -r -a CHUNKS <<< "$CHUNKS_CSV"

echo "=========================================================="
echo " FUSE Chunk Matrix Experiment"
echo " File sizes: $FILE_SIZES_CSV MB"
echo " Chunks: $CHUNKS_CSV bytes"
echo " Bandwidth: ${BW_LIMIT:-unlimited} | Loss: ${PACKET_LOSS:-0}%"
echo " Results: $RESULTS_FILE"
echo "=========================================================="

{
    echo "# Chunk Matrix Results"
    echo ""
    echo "| Payload (MB) | Chunk Size | Max Buffer | Speed (Mbit/s) | Status |"
    echo "|---:|---:|---:|---:|---|"
} > "$RESULTS_FILE"

FIRST_RUN=1
for SIZE_MB in "${FILE_SIZES[@]}"; do
    for CHUNK in "${CHUNKS[@]}"; do
        BUFFER=$((CHUNK * 2))
        echo ""
        echo "[*] Test: payload=${SIZE_MB}MB chunk=${CHUNK} buffer=${BUFFER}"

        export MOUNTY_CHUNK_SIZE="$CHUNK"
        export MOUNTY_MAX_BUFFER_SIZE="$BUFFER"

        ARGS=(--size-mb "$SIZE_MB")
        if [[ -n "$BW_LIMIT" ]]; then
            ARGS+=(--bw-limit "$BW_LIMIT")
        fi
        if [[ -n "$PACKET_LOSS" ]]; then
            ARGS+=(--packet-loss "$PACKET_LOSS")
        fi

        if [[ "$FIRST_RUN" -eq 0 && "$SKIP_BUILD_AFTER_FIRST" = "1" ]]; then
            OUTPUT=$(SKIP_BUILD=1 ./test/run_perf_tests.sh "${ARGS[@]}" 2>&1) || STATUS=$?
        else
            OUTPUT=$(./test/run_perf_tests.sh "${ARGS[@]}" 2>&1) || STATUS=$?
        fi
        FIRST_RUN=0

        STATUS=${STATUS:-0}
        SPEED=$(printf '%s\n' "$OUTPUT" | awk '/Actual Transfer Speed:/ {print $5; exit}')
        if [[ -z "$SPEED" ]]; then
            SPEED="N/A"
        fi

        if [[ "$STATUS" -eq 0 ]]; then
            RESULT="ok"
        else
            RESULT="failed"
            printf '%s\n' "$OUTPUT"
        fi

        echo "| $SIZE_MB | $CHUNK | $BUFFER | $SPEED | $RESULT |" >> "$RESULTS_FILE"
        echo "[*] Result: $RESULT speed=${SPEED}"
        unset STATUS
    done
done

echo ""
echo "[*] Results written to $RESULTS_FILE"
