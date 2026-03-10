#!/bin/bash
# List files in the demo directory and emit AnalyzeFile tasks for each
# Input (stdin): {"kind": "ListFiles", "value": {"folder": "..."}}
# Output: JSON array of AnalyzeFile tasks

set -e

# Read task from stdin
INPUT=$(cat)

# Extract folder from input
FOLDER=$(echo "$INPUT" | jq -r '.value.folder')

# Find files and emit AnalyzeFile tasks
find "$FOLDER" -maxdepth 1 -type f -name "*.sh" -o -name "*.jsonc" | jq -R -s -c '
  split("\n") |
  map(select(length > 0)) |
  map({"kind": "AnalyzeFile", "value": {"file": .}})
'
