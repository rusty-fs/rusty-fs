#!/usr/bin/env bash
set -e

# Configurazione del test
BW_LIMIT=100
PACKET_LOSS=5

echo "=========================================================="
echo " Esperimento Dimensione Blocco FUSE (Matrice File/Chunk)"
echo " Banda: $BW_LIMIT Mbit/s | Loss: $PACKET_LOSS%"
echo "=========================================================="

# Matrice dei parametri
FILE_SIZES=("10" "50" "200" "500" "2048")  # Dimensioni file in MB (2048 = 2 GB)
CHUNKS=("1048576" "4194304" "16777216") # Dimensioni chunk: 1MB, 4MB, 16MB
RESULTS_FILE="/tmp/chunk_matrix_results.md"

echo "# Risultati Esperimento a Matrice" > $RESULTS_FILE
echo "| Payload (MB) | Chunk Size | Velocità (Mbit/s) |" >> $RESULTS_FILE
echo "|--------------|------------|-------------------|" >> $RESULTS_FILE

for SIZE_MB in "${FILE_SIZES[@]}"; do
    echo ""
    echo "=========================================================="
    echo " Inizio test con Payload: $SIZE_MB MB"
    echo "=========================================================="
    
    for CHUNK in "${CHUNKS[@]}"; do
        BUFFER=$((CHUNK * 2))
        
        if [ "$CHUNK" -ge 1048576 ]; then
            LABEL="$((CHUNK / 1048576)) MB"
        else
            LABEL="$((CHUNK / 1024)) KB"
        fi
        
        echo "[*] -> Test: Payload $SIZE_MB MB | Chunk: $LABEL ..."
        
        export MOUNTY_CHUNK_SIZE=$CHUNK
        export MOUNTY_MAX_BUFFER_SIZE=$BUFFER
        
        OUTPUT=$(./test/run_perf_tests.sh --bw-limit "$BW_LIMIT" --size-mb "$SIZE_MB" --packet-loss "$PACKET_LOSS" 2>&1) || true
        
        SPEED=$(echo "$OUTPUT" | grep "Actual Transfer Speed:" | awk '{print $5}')
        
        if [ -z "$SPEED" ]; then
            SPEED="FALLITO"
            echo "$OUTPUT"
        fi
        
        echo "    -> Risultato: $SPEED Mbit/s"
        echo "| $SIZE_MB MB | $LABEL | $SPEED |" >> $RESULTS_FILE
    done
done

echo ""
echo "=========================================================="
echo " Esperimento Matrice Completato!"
echo "=========================================================="
cat $RESULTS_FILE
