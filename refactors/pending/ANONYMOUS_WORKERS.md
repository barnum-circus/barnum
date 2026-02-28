# Anonymous Worker Model

**Status:** Future work. The inotify race is now fixed (submissions flattened), so this can proceed when desired.

## Overview

Simplify agent protocol from "named agents with persistent identity" to "anonymous workers pulling from a task queue."

## Current Model (Problems)

- Agents have persistent identities (names/directories)
- Complex state machine (idle, working, kicked)
- Agent names carry semantic meaning
- Agents create their own directories: `agents/<name>/`

## Proposed Model

- Workers are anonymous
- Worker calls `get_task`, blocks until assigned
- Daemon returns task content + outcome file path
- Worker completes task, writes to assigned path
- Worker calls `get_task` again (back of queue)
- Heartbeats for queue starvation detection
- Names are debug-only metadata, no uniqueness requirement

## Flat File Structure

Workers use three files per task:

```
agents/
├── <id>.ready.json     # worker writes (signals readiness)
├── <id>.task.json      # daemon writes (assigns task)
└── <id>.response.json  # worker writes (task result)
```

## Worker Registration Protocol

1. **Worker generates UUID** and creates `<id>.ready.json`
2. **Worker waits** for `<id>.task.json` to appear (blocking via watcher or polling)
3. **Worker reads task** from `<id>.task.json`
4. **Worker writes response** to `<id>.response.json`
5. **Worker cleans up** all three files (or daemon cleans on completion)
6. **Repeat** from step 1 with new UUID

This is similar to how submissions work (flat files, no directories) and eliminates the inotify race entirely.

## Why Three Files?

- **ready.json**: Signals to daemon "I'm available for work"
- **task.json**: Daemon's response with task content
- **response.json**: Worker's result

Alternatives considered:
1. Agent creates empty task file, daemon overwrites - hacky, can't distinguish "ready" from "processing"
2. Daemon assigns IDs via socket - requires socket access, doesn't work in sandboxed environments
3. Single registration directory per worker - reintroduces inotify race

The three-file protocol is cleanest because each file has a single writer (no races) and clear semantics.

## Changes Required

1. **Flatten agents directory** - Remove per-agent subdirectories
2. **New path categorization** - Add `AgentReady`, `AgentResponse` for flat files
3. **Simplify core state machine** - Remove agent identity tracking; track pending tasks by ID
4. **Update CLI commands** - `get_task` generates UUID, writes ready file, waits for task
5. **Consolidate commands** - Merge `register`/`next_task` into single `get_task` command
6. **Remove kicked state** - Workers just stop calling `get_task`; daemon ignores stale ready files
7. **Update `AGENT_PROTOCOL.md`** - Document new three-file protocol

## Heartbeats

Still useful for detecting stuck workers waiting in queue:
- If worker waits too long without getting a task, send heartbeat
- This is about queue starvation, not task completion (task timeouts handle that)

## Agent Names

Keep for debugging/logging only:
- No semantic meaning
- No uniqueness requirement
- Multi-threaded agent can register multiple times with same name
