# CLI Reference

GSD provides two command-line tools: `gsd` for running workflows and `agent_pool` for managing worker agents.

## GSD CLI

The main orchestrator for running task queues.

```
gsd [OPTIONS] <COMMAND>

Commands:
  run      Run the task queue
  config   Config file operations (docs, validate, graph, schema)
  version  Print version information
  help     Print this message or the help of the given subcommand(s)

Options:
  --root <ROOT>  Root directory for pools. Defaults to /tmp/agent_pool
  -h, --help     Print help
```

### gsd run

Execute a workflow defined in a config file.

```
gsd run [OPTIONS] <CONFIG>

Arguments:
  <CONFIG>  Config file path or inline JSON

Options:
  --initial-state <INITIAL_STATE>
      Initial tasks (JSON array or path to file)
      Required if config has no `entrypoint`

  --entrypoint-value <ENTRYPOINT_VALUE>
      Initial value for the entrypoint step (JSON or path)
      Only valid when config has an `entrypoint`
      Defaults to `{}` if not provided

  --pool <POOL>
      Agent pool ID (e.g., `agents` resolves to `<root>/pools/agents/`)

  --wake <WAKE>
      Wake script to call before starting

  --log-file <LOG_FILE>
      Log file path (logs emitted in addition to stderr)

  --root <ROOT>
      Root directory for pools

  -h, --help
      Print help
```

**Examples:**

```bash
# Run with entrypoint (config defines entrypoint step)
gsd run config.json --pool agents --entrypoint-value '{"file": "main.rs"}'

# Run with entrypoint, default value ({})
gsd run config.json --pool agents

# Run with logging
gsd run config.json --pool agents --log-file /tmp/gsd.log

# Run without entrypoint (manual initial state)
gsd run config.json --pool agents --initial-state '[{"kind": "Start", "value": {}}]'
```

### gsd config

Operations on config files.

```
gsd config <COMMAND>

Commands:
  docs      Generate markdown documentation from config
  validate  Validate a config file
  graph     Generate DOT visualization (for GraphViz)
  schema    Print the JSON schema for config files
```

**Examples:**

```bash
# Validate a config
gsd config validate config.json

# Generate documentation
gsd config docs config.json > WORKFLOW.md

# Generate graph visualization
gsd config graph config.json > workflow.dot
dot -Tpng workflow.dot -o workflow.png

# Get the JSON schema
gsd config schema > gsd-config-schema.json
```

## Agent Pool CLI

Daemon for managing worker agents.

```
agent_pool [OPTIONS] <COMMAND>

Commands:
  start        Start the agent pool server
  stop         Stop a running agent pool server
  submit_task  Submit a task and wait for the result
  list         List all pools
  protocol     Print the agent protocol documentation
  get_task     Wait for and return the next task (for agents)
  version      Print version information
  help         Print this message or the help of the given subcommand(s)

Options:
  --root <ROOT>            Root directory for pools
  -l, --log-level <LEVEL>  Log level (off, error, warn, info, debug, trace)
  -h, --help               Print help
```

### agent_pool start

Start the pool daemon.

```bash
# Start a pool named "agents"
agent_pool start --pool agents

# Start with custom root directory
agent_pool start --pool agents --root /var/gsd
```

### agent_pool stop

Stop a running pool.

```bash
agent_pool stop --pool agents
```

### agent_pool submit_task

Submit a task and wait for a response (used by GSD internally).

```bash
agent_pool submit_task --pool agents --data '{"task": "data"}'
```

### agent_pool get_task

Wait for the next available task (used by agents).

```bash
# Agent waits for a task
agent_pool get_task --pool agents --name agent1
```

### agent_pool protocol

Print the full agent protocol documentation.

```bash
agent_pool protocol
```

### agent_pool list

List all active pools.

```bash
agent_pool list
```

## Environment Variables

Both tools respect:

- `AGENT_POOL_ROOT` - Default root directory (overridden by `--root`)

## Exit Codes

- `0` - Success
- `1` - Error (invalid config, pool not found, etc.)
- `124` - Timeout (when using timeouts)
