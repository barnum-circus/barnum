#!/bin/bash
# Wake script for Claude Code agents running in tmux.
#
# Usage: ./tmux-wake.sh <agent-pool-root>
#
# This script looks at all agent directories in the pool and sends a wake
# message to tmux panes/windows with matching names.
#
# Restriction: The agent directory name must exactly match the tmux target name
# (window name or session:window).
#
# Example:
#   If you have agents registered as "claude-1" and "claude-2", you need
#   tmux windows named "claude-1" and "claude-2".
#
# To name a tmux window: Ctrl-b , (then type the name)
# Or: tmux rename-window "claude-1"

set -e

ROOT="$1"

if [ -z "$ROOT" ]; then
    echo "Usage: $0 <agent-pool-root>" >&2
    exit 1
fi

AGENTS_DIR="$ROOT/agents"

if [ ! -d "$AGENTS_DIR" ]; then
    echo "No agents directory at $AGENTS_DIR" >&2
    exit 0
fi

MESSAGE="Wakey wakey, rise and shine. Start checkin' shit."

for agent_dir in "$AGENTS_DIR"/*/; do
    if [ -d "$agent_dir" ]; then
        agent_name=$(basename "$agent_dir")

        # Try to send to tmux target matching the agent name
        if tmux send-keys -t "$agent_name" "$MESSAGE" Enter 2>/dev/null; then
            echo "Woke agent: $agent_name"
        else
            echo "Could not wake agent: $agent_name (no matching tmux target)" >&2
        fi
    fi
done
