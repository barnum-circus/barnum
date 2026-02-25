#!/bin/bash
# Start the cmd pool daemon for Claude Code sandbox escape.
#
# Usage: ./scripts/start-cmd-pool.sh

set -e

cd "$(dirname "$0")/.."

# Build if needed (silent unless error)
cargo build -p agent_pool_cli --quiet

> /tmp/daemon.log
RUST_LOG=debug ./target/debug/agent_pool start --pool cmd --force 2>&1 | tee /tmp/daemon.log
