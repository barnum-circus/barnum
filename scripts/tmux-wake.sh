#!/bin/bash
# Wake script for Claude Code agents running in tmux.
#
# Usage: ./tmux-wake.sh <troupe-root>
#
# Sends a wake message to tmux windows matching agent directory names.
# The agent name must exactly match the tmux window name.
#
# To name a tmux window: Ctrl-b , (then type the name)

set -e

ROOT="$1"

if [ -z "$ROOT" ]; then
    echo "Usage: $0 <troupe-root>" >&2
    exit 1
fi

AGENTS_DIR="$ROOT/agents"

if [ ! -d "$AGENTS_DIR" ]; then
    exit 0
fi

MESSAGE="You have work waiting. Check your agent directory for a .input file.

Protocol:
1. Find the *.input file (e.g., 1.input)
2. Read it - contains your task JSON
3. Do the work
4. If the .input file still exists, write {id}.output and delete {id}.input
5. If the .input file is gone, you were timed out - skip it"

for agent_dir in "$AGENTS_DIR"/*/; do
    if [ -d "$agent_dir" ]; then
        agent_name=$(basename "$agent_dir")
        agent_message="$MESSAGE

Your directory: $agent_dir"

        if tmux send-keys -t "$agent_name" "$agent_message" Enter 2>/dev/null; then
            echo "Woke: $agent_name"
        fi
    fi
done
