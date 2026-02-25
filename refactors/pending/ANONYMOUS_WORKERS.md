# Anonymous Worker Model

**Status:** Future work. Depends on completing the inotify race fix first.

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

With daemon-assigned IDs:

```
agents/
├── <id>.task.json      # daemon writes
└── <id>.outcome.json   # agent writes
```

## Why This Requires Anonymous Workers

Flattening agents requires a new registration mechanism. Currently, agents register by creating a directory. Without directories, agents need another way to signal existence.

Options considered:
1. Agent creates empty task file, daemon overwrites - hacky
2. Agent creates registration file - added complexity
3. Daemon assigns IDs - clean, but requires anonymous worker model

Option 3 is cleanest: daemon assigns an ID when agent calls `get_task`, tells agent where to write outcome.

## Changes Required

1. Remove agent identity tracking from core state machine
2. `get_task` returns task content + outcome file path
3. Simplify `AgentMap` to track pending outcomes by task ID
4. Remove kicked state tracking
5. Consolidate CLI commands (`register`, `next_task` → just `get_task`?)
6. Update `AGENT_PROTOCOL.md`

## Heartbeats

Still useful for detecting stuck workers waiting in queue:
- If worker waits too long without getting a task, send heartbeat
- This is about queue starvation, not task completion (task timeouts handle that)

## Agent Names

Keep for debugging/logging only:
- No semantic meaning
- No uniqueness requirement
- Multi-threaded agent can register multiple times with same name
