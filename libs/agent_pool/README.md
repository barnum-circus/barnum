# @gsd-now/agent-pool

Agent pool daemon for managing workers with file-based task dispatch.

## Installation

```bash
npm install -g @gsd-now/agent-pool
```

## Usage

```bash
# Start the agent pool server
agent_pool start ./workspace

# Submit a task and wait for result
agent_pool submit ./workspace "task payload"

# Stop a running server
agent_pool stop ./workspace
```

Or with npx:

```bash
npx @gsd-now/agent-pool start ./workspace
```

## Agent Protocol

See [AGENT_PROTOCOL.md](https://github.com/gsd-now/gsd/blob/master/crates/agent_pool/protocols/AGENT_PROTOCOL.md) for how agents communicate with the daemon.
