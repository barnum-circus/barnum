# agent_pool

Agent pool daemon for managing workers with file-based task dispatch.

## Overview

`agent_pool` provides a daemon that coordinates task distribution to worker agents. Communication happens via:
- **Submitters** → Daemon: Unix socket or file-based submission
- **Daemon** → Agents: Filesystem polling (`task.json`, `response.json`)

## Usage

```bash
# Start daemon
agent_pool start --pool my-pool

# Submit a task (in another terminal)
agent_pool submit_task --pool my-pool --data '{"kind":"Task","task":{"instructions":"...","data":{}}}'

# Stop daemon
agent_pool stop --pool my-pool
```

## Protocol

See `protocols/AGENT_PROTOCOL.md` for the agent communication protocol.
See `protocols/SUBMISSION_PROTOCOL.md` for the task submission protocol.

## License

MIT
