---
image: /img/og/reference-cli.png
---

# CLI Reference

Barnum provides two command-line tools: `barnum` for running workflows and `troupe` for managing worker agents.

## Barnum CLI

The main orchestrator for running task queues.

```
barnum [OPTIONS] <COMMAND>

Commands:
  run      Run the task queue
  config   Config file operations (docs, validate, graph, schema)
  version  Print version information
  help     Print this message or the help of the given subcommand(s)

Options:
  --root <ROOT>  Root directory for pools. Defaults to /tmp/troupe
  -h, --help     Print help
```

### barnum run

Execute a workflow defined in a config file.

```
barnum run [OPTIONS]

Options:
  --config <CONFIG>
      Config file path or inline JSON
      Required unless --resume-from is used

  --initial-state <INITIAL_STATE>
      Initial tasks (JSON array or path to file)
      Required if config has no `entrypoint`

  --entrypoint-value <ENTRYPOINT_VALUE>
      Initial value for the entrypoint step (JSON or path)
      Only valid when config has an `entrypoint`
      Defaults to `{}` if not provided

  --pool <POOL>
      Agent pool ID (e.g., `my-pool` resolves to `<root>/pools/my-pool/`)
      Defaults to `default`

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
barnum run --config config.json --entrypoint-value '{"file": "main.rs"}'

# Run with entrypoint, default value ({})
barnum run --config config.json

# Run with logging
barnum run --config config.json --log-file /tmp/barnum.log

# Run without entrypoint (manual initial state)
barnum run --config config.json --initial-state '[{"kind": "Start", "value": {}}]'

# Run with a specific pool (default: "default")
barnum run --config config.json --pool my-pool --entrypoint-value '{}'
```

### barnum config

Operations on config files.

```
barnum config <COMMAND>

Commands:
  docs      Generate markdown documentation from config
  validate  Validate a config file
  graph     Generate DOT visualization (for GraphViz)
  schema    Print the config schema (Zod TypeScript by default, or JSON)
```

**Examples:**

```bash
# Validate a config
barnum config validate --config config.json

# Generate documentation
barnum config docs --config config.json > WORKFLOW.md

# Generate graph visualization
barnum config graph --config config.json > workflow.dot
dot -Tpng workflow.dot -o workflow.png

# Get the Zod TypeScript schema (default)
barnum config schema

# Get the JSON schema
barnum config schema --type json
```

## Troupe CLI

Daemon for managing worker agents.

```
troupe [OPTIONS] <COMMAND>

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

### troupe start

Start the pool daemon.

```bash
# Start the default pool
troupe start

# Start a named pool
troupe start --pool my-pool

# Start with custom root directory
troupe start --pool my-pool --root /var/barnum
```

### troupe stop

Stop a running pool.

```bash
troupe stop
```

### troupe submit_task

Submit a task and wait for a response (used by Barnum internally).

```bash
troupe submit_task --data '{"task": "data"}'
```

### troupe get_task

Wait for the next available task (used by agents).

```bash
# Agent waits for a task
troupe get_task --name agent1
```

### troupe protocol

Print the full agent protocol documentation.

```bash
troupe protocol
```

### troupe list

List all active pools.

```bash
troupe list
```

## Environment Variables

Both tools respect:

- `AGENT_POOL_ROOT` - Default root directory (overridden by `--root`)

## Exit Codes

- `0` - Success
- `1` - Error (invalid config, pool not found, etc.)
- `124` - Timeout (when using timeouts)
