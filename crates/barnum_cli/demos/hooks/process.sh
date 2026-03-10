#!/bin/bash
# Process action: reads task JSON, outputs next tasks
# Receives: {"kind": "Process", "value": {...}} on stdin
# Outputs: array of next tasks to stdout

input=$(cat)

# Extract the item and timestamp from the value
item=$(echo "$input" | jq -r '.value.item')
timestamp=$(echo "$input" | jq -r '.value.timestamp')

# Log what we're processing (to stderr so it doesn't interfere with output)
echo "Processing item '$item' (received at $timestamp)" >&2

# Return a Cleanup task with the result
echo "[{\"kind\": \"Cleanup\", \"value\": {\"result\": \"processed-$item\"}}]"
