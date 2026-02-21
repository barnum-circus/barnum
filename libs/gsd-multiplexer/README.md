# @gsd-now/multiplexer

Multiplexer daemon for managing agent pools with file-based task dispatch.

## Installation

```bash
npm install -g @gsd-now/multiplexer@main
```

## Usage

```bash
# Start the multiplexer server
multiplexer start ./workspace

# Submit a task and wait for result
multiplexer submit ./workspace "task payload"

# Stop a running server
multiplexer stop ./workspace
```

Or with npx:

```bash
npx @gsd-now/multiplexer start ./workspace
```

## Agent Protocol

See [AGENT_PROTOCOL.md](https://github.com/rbalicki2/gsd/blob/main/crates/gsd_multiplexer/AGENT_PROTOCOL.md) for how agents communicate with the daemon.
