#!/bin/bash
# Finally hook: Runs after task AND all its children complete
# Receives: the task's value JSON on stdin
# Outputs: array of tasks to spawn (usually empty), or empty array

input=$(cat)

# Log that finally is running (to stderr)
item=$(echo "$input" | jq -r '.item')
echo "Finally: cleanup complete for item '$item'" >&2

# Output empty array (no additional tasks)
echo "[]"
