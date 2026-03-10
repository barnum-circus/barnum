#!/bin/bash
# Post-hook: Called after action completes, can modify spawned tasks
# Receives: PostHookInput JSON on stdin (kind: Success/Timeout/Error/PreHookError)
# Outputs: modified PostHookInput JSON to stdout

input=$(cat)

# Log the outcome (to stderr)
kind=$(echo "$input" | jq -r '.kind')
echo "Post-hook: action completed with $kind" >&2

# Pass through the input unchanged
# (could modify .next array to filter/add tasks)
echo "$input"
