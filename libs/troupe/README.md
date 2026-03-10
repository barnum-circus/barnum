# @barnum/troupe

Troupe - agent pool daemon for managing workers with file-based task dispatch.

## Installation

```bash
npm install -g @barnum/troupe
```

## Usage

```bash
# Start the agent pool server
troupe start ./workspace

# Submit a task and wait for result
troupe submit ./workspace "task payload"

# Stop a running server
troupe stop ./workspace
```

Or with npx:

```bash
npx @barnum/troupe start ./workspace
```

## Agent Protocol

See [AGENT_PROTOCOL.md](https://github.com/barnum-circus/barnum/blob/master/crates/troupe/protocols/AGENT_PROTOCOL.md) for how agents communicate with the daemon.
