#!/bin/bash
# Start the cmd pool daemon for Claude Code sandbox escape.
#
# Usage: ./scripts/start-cmd-pool.sh

set -e

cd "$(dirname "$0")/.."

# Kill any stale agents from previous runs (both shell scripts and CLI subprocesses)
pkill -9 -f "command-agent.sh --pool cmd" 2>/dev/null || true
pkill -9 -f "troupe get_task --pool cmd" 2>/dev/null || true

# Build if needed
echo -n "Building... "
cargo build -p troupe_cli --quiet
echo "done"

> /tmp/daemon.log
RUST_LOG=debug ./target/debug/troupe start --pool cmd --stop 2>&1 | tee /tmp/daemon.log
