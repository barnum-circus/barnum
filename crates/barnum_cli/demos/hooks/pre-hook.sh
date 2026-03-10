#!/bin/bash
# Pre-hook: Transforms the input value before the action runs
# Receives: original value JSON on stdin
# Outputs: transformed value JSON to stdout

input=$(cat)

# Add a timestamp to the input
timestamp=$(date -u +%Y-%m-%dT%H:%M:%SZ)
echo "$input" | jq --arg ts "$timestamp" '. + {timestamp: $ts}'
