#!/bin/bash
# Start the cmd pool daemon for Claude Code sandbox escape.
#
# Usage: ./scripts/start-cmd-pool.sh

set -e

cd "$(dirname "$0")/.."

# Kill any stale agents from previous runs
pkill -f "command-agent.sh --pool cmd" 2>/dev/null || true

# Build if needed
echo -n "Building... "
cargo build -p agent_pool_cli --quiet
echo "done"

> /tmp/daemon.log
RUST_LOG=debug ./target/debug/agent_pool start --pool cmd --stop 2>&1 | tee /tmp/daemon.log
