#!/bin/bash
# Process action: reads task JSON, outputs next tasks
# Receives: {"kind": "Process", "value": {...}} on stdin
# Outputs: array of next tasks to stdout

input=$(cat)

# Extract the item from the value
item=$(echo "$input" | jq -r '.value.item')

# Log what we're processing (to stderr so it doesn't interfere with output)
echo "Processing item '$item'" >&2

# Return a Cleanup task with the result
echo "[{\"kind\": \"Cleanup\", \"value\": {\"result\": \"processed-$item\"}}]"
