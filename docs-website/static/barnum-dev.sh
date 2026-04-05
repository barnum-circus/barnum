#!/bin/bash
# Barnum development session launcher
#
# Sets up a tmux session with a troupe pool, a Claude Code orchestrator,
# and configurable agent windows. Currently configured for Claude Code.
#
# Usage: ./barnum-dev.sh [num_agents]  (default: 1)

set -e

NUM_AGENTS="${1:-1}"
SESSION="barnum-dev"
POOL_NAME="agents"

# Kill existing session if it exists
tmux kill-session -t "$SESSION" 2>/dev/null || true

# Create session with first window (Term)
tmux new-session -d -s "$SESSION" -n "Term"

# Claude window - orchestrator
tmux new-window -t "$SESSION" -n "Claude"

# Pool window - troupe daemon
tmux new-window -t "$SESSION" -n "Pool"
tmux send-keys -t "$SESSION:Pool" "pnpm dlx @barnum/troupe start --pool $POOL_NAME --stop" Enter

# Create agent windows
for i in $(seq 1 "$NUM_AGENTS"); do
    if [ "$NUM_AGENTS" -eq 1 ]; then
        WINDOW_NAME="Agent"
    else
        WINDOW_NAME="Agent$i"
    fi

    tmux new-window -t "$SESSION" -n "$WINDOW_NAME"
done

# Setup: wait for windows to be ready, then send commands
(
    sleep 3

    # Start Claude Code in the orchestrator window
    tmux send-keys -t "$SESSION:Claude" "claude --dangerously-skip-permissions" Enter

    # Start Claude Code in each agent window
    for i in $(seq 1 "$NUM_AGENTS"); do
        if [ "$NUM_AGENTS" -eq 1 ]; then
            WINDOW_NAME="Agent"
        else
            WINDOW_NAME="Agent$i"
        fi

        tmux send-keys -t "$SESSION:$WINDOW_NAME" "claude --dangerously-skip-permissions" Enter
    done

    sleep 5

    # Send protocol instructions to each agent
    for i in $(seq 1 "$NUM_AGENTS"); do
        if [ "$NUM_AGENTS" -eq 1 ]; then
            WINDOW_NAME="Agent"
            AGENT_NAME="C1"
        else
            WINDOW_NAME="Agent$i"
            AGENT_NAME="C$i"
        fi

        tmux send-keys -t "$SESSION:$WINDOW_NAME" "You are an AI agent. Run this first: pnpm dlx @barnum/troupe protocol --pool $POOL_NAME --name $AGENT_NAME "
        sleep 0.5
        tmux send-keys -t "$SESSION:$WINDOW_NAME" "Then follow the protocol. Quick summary: 1. Loop until shutdown 2. Call get_task 3. Do the task 4. Write to response_file 5. Repeat "
        sleep 0.5
        tmux send-keys -t "$SESSION:$WINDOW_NAME" "6. If Kicked, exit."
        sleep 0.5
        tmux send-keys -t "$SESSION:$WINDOW_NAME" Enter
    done
) &

# Create exit window (select it to kill the session)
tmux new-window -t "$SESSION" -n "exit"
(sleep 2 && tmux send-keys -t "$SESSION:exit" "tmux kill-session -t $SESSION") &

# Select Term window and attach
tmux select-window -t "$SESSION:Term"
tmux attach-session -t "$SESSION"
