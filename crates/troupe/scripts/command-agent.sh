#!/bin/bash
# Agent that executes shell commands from tasks.
#
# Usage: ./command-agent.sh --pool <POOL_ID> [--root <ROOT>] [--log <LOG_FILE>]
#
# This agent:
# 1. Connects to the pool and waits for tasks
# 2. Extracts the "cmd" field from the task value
# 3. Executes it with bash
# 4. Writes stdout to the response file
# 5. Loops for the next task
#
# Task format expected:
#   {"task": {"kind": "...", "value": {"cmd": "cargo test"}}, "instructions": "..."}
#
# Response format:
#   The raw stdout from the command (agent pool wraps this in {"kind":"Processed","stdout":"..."})

set -e

# Parse arguments
POOL_ID=""
POOL_ROOT=""
LOG_FILE=""
while [[ $# -gt 0 ]]; do
    case $1 in
        --pool) POOL_ID="$2"; shift 2 ;;
        --root) POOL_ROOT="$2"; shift 2 ;;
        --log) LOG_FILE="$2"; shift 2 ;;
        *) echo "Unknown option: $1" >&2; exit 1 ;;
    esac
done

if [ -z "$POOL_ID" ]; then
    echo "Usage: $0 --pool <POOL_ID> [--root <ROOT>] [--log <LOG_FILE>]" >&2
    exit 1
fi

# Redirect all output to log file if specified
if [ -n "$LOG_FILE" ]; then
    exec > >(tee -a "$LOG_FILE") 2>&1
    echo "=== Agent started at $(date) ===" >&2
fi

# Find troupe binary:
# 1. Use TROUPE env var if set
# 2. Try target/debug/troupe relative to repo root
# 3. Fall back to PATH
if [ -n "$TROUPE" ]; then
    : # Use env var
elif [ -f "$(dirname "$0")/../../../target/debug/troupe" ]; then
    TROUPE="$(dirname "$0")/../../../target/debug/troupe"
else
    TROUPE="troupe"
fi

# Generate agent name once at startup (8 random alphanumeric chars)
NAME=$(LC_ALL=C tr -dc 'a-z0-9' < /dev/urandom | head -c 8)

cleanup() {
    echo "[$NAME] Shutting down..." >&2
    # With anonymous workers, there's nothing to clean up - just exit.
    # The daemon handles file cleanup.
    exit 0
}
trap cleanup SIGINT SIGTERM

RECONNECT_DELAY=2

# Build CLI args for pool-root (goes before subcommand)
CLI_GLOBAL_ARGS=""
if [ -n "$POOL_ROOT" ]; then
    CLI_GLOBAL_ARGS="--root $POOL_ROOT"
    POOL_DIR="$POOL_ROOT/pools/$POOL_ID"
else
    POOL_DIR="/tmp/troupe/pools/$POOL_ID"
fi

# Track the daemon PID we're connected to (to detect daemon restarts)
DAEMON_PID=""

# Outer loop: reconnect on eviction or failure
while true; do
    # Check if pool is still running (status file exists when daemon is up)
    if [ ! -f "$POOL_DIR/status" ]; then
        echo "[$NAME] Pool not running (no status file at $POOL_DIR/status), exiting." >&2
        exit 0
    fi

    # Check daemon PID - if it changed, a new daemon started, we should exit
    CURRENT_PID=$(cat "$POOL_DIR/daemon.lock" 2>/dev/null || echo "")
    if [ -n "$DAEMON_PID" ] && [ "$CURRENT_PID" != "$DAEMON_PID" ]; then
        echo "[$NAME] Daemon restarted (PID changed from $DAEMON_PID to $CURRENT_PID), exiting." >&2
        exit 0
    fi
    DAEMON_PID="$CURRENT_PID"

    echo "[$NAME] Connecting to pool $POOL_ID (daemon PID: $DAEMON_PID)..." >&2

    # Inner loop: process tasks
    while true; do
        set +e
        TASK_JSON=$("$TROUPE" $CLI_GLOBAL_ARGS get_task --pool "$POOL_ID" --name "$NAME" --log-level trace)
        GET_TASK_EXIT=$?
        set -e

        if [ $GET_TASK_EXIT -ne 0 ]; then
            echo "[$NAME] get_task failed (exit=$GET_TASK_EXIT), reconnecting in ${RECONNECT_DELAY}s..." >&2
            sleep "$RECONNECT_DELAY"
            break  # Break inner loop to reconnect
        fi

        # Extract response file path, kind, and command
        RESPONSE_FILE=$(echo "$TASK_JSON" | jq -r '.response_file')
        KIND=$(echo "$TASK_JSON" | jq -r '.kind // "Task"')
        CMD=$(echo "$TASK_JSON" | jq -r '.content.data.cmd // empty')

        echo "[$NAME] Got task (kind=$KIND)" >&2

        # Handle kicked - reconnect
        if [ "$KIND" = "Kicked" ]; then
            echo "[$NAME] Kicked by daemon, reconnecting in ${RECONNECT_DELAY}s..." >&2
            sleep "$RECONNECT_DELAY"
            break  # Break inner loop to reconnect
        fi

        # Handle heartbeat - respond immediately
        if [ "$KIND" = "Heartbeat" ]; then
            echo "[$NAME] Heartbeat" >&2
            echo "{}" > "$RESPONSE_FILE"
            continue
        fi

        if [ -z "$CMD" ]; then
            echo "[$NAME] No 'cmd' field in task, responding with empty" >&2
            echo "[]" > "$RESPONSE_FILE"
            continue
        fi

        echo "[$NAME] Executing: $CMD" >&2

        # Execute the command and capture output
        set +e
        OUTPUT=$(bash -c "$CMD" 2>&1)
        EXIT_CODE=$?
        set -e

        if [ $EXIT_CODE -eq 0 ]; then
            echo "[$NAME] Command succeeded" >&2
        else
            echo "[$NAME] Command failed with exit code $EXIT_CODE" >&2
        fi

        # Write response (just the output - troupe wraps it)
        echo "$OUTPUT" > "$RESPONSE_FILE"
    done
done
