# agent_pool

Agent pool daemon for managing workers with file-based task dispatch.

## Overview

`agent_pool` provides a daemon that coordinates task distribution to worker agents. Communication happens via:
- **Submitters** → Daemon: Unix socket or file-based submission
- **Daemon** → Agents: Filesystem polling (`task.json`, `response.json`)

## Usage

```rust
use agent_pool::{spawn, submit_file};

// Start daemon
let handle = spawn("/tmp/my-pool")?;

// Submit a task
let response = submit_file("/tmp/my-pool", r#"{"kind":"Task","task":{"instructions":"...","data":{}}}"#)?;

// Shutdown
handle.shutdown()?;
```

## Protocol

See `AGENT_PROTOCOL.md` for the agent communication protocol.
See `SUBMISSION_PROTOCOL.md` for the task submission protocol.

## License

MIT
