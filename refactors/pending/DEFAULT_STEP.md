# Default Step Refactor

## Motivation

Currently, running `gsd run` requires `--initial` with a full JSON array of tasks:

```bash
gsd run config.json --initial '[{"kind": "Analyze", "value": {"path": "/foo"}}]'
```

This is verbose for the common case where you just want to kick off a single task at a known entry point. A `default` step would allow:

```bash
gsd run config.json --initial-value '{"path": "/foo"}'
# Or even simpler:
gsd run config.json  # Uses {} as the value
```

## Current State

### Config structure (`crates/gsd_config/src/config.rs:10-24`)

```rust
pub struct Config {
    #[serde(rename = "$schema", default, skip_serializing)]
    pub schema_ref: Option<String>,

    #[serde(default)]
    pub options: Options,

    pub steps: Vec<Step>,
}
```

No `default` field exists.

### CLI (`crates/gsd_cli/src/main.rs:26-45`)

```rust
Command::Run {
    config: String,

    /// Initial tasks (JSON string or path to file) - required
    #[arg(long)]
    initial: String,
    // ...
}
```

`--initial` is required and takes a full task array.

### Task creation (`crates/gsd_cli/src/main.rs:86-87`)

```rust
let initial_tasks = parse_initial_tasks(&initial)?;
```

Parses the `--initial` argument as `Vec<Task>`.

## Proposed Changes

### 1. Add `default` field to Config

Add an optional `default: StepName` field to `Config`:

```rust
pub struct Config {
    #[serde(rename = "$schema", default, skip_serializing)]
    pub schema_ref: Option<String>,

    #[serde(default)]
    pub options: Options,

    /// Default step name. If set, allows `--initial-value` instead of `--initial`.
    #[serde(default)]
    pub default: Option<StepName>,

    pub steps: Vec<Step>,
}
```

### 2. Add validation for default step

In `Config::validate()`, add a check that `default` (if set) references an existing step:

```rust
// Validate default step exists
if let Some(ref default) = self.default {
    if !step_names.contains(default.as_str()) {
        return Err(ConfigError::InvalidDefaultStep {
            name: default.clone(),
        });
    }
}
```

Add new error variant:

```rust
pub enum ConfigError {
    // ... existing variants ...
    InvalidDefaultStep {
        name: StepName,
    },
}
```

### 3. Modify CLI to support `--initial-value`

Change the CLI from:

```rust
#[arg(long)]
initial: String,
```

To:

```rust
/// Initial tasks (JSON string or path to file)
#[arg(long, conflicts_with = "initial_value")]
initial: Option<String>,

/// Initial value for default step (JSON string or path to file)
#[arg(long, conflicts_with = "initial")]
initial_value: Option<String>,
```

### 4. Add logic to construct initial tasks

In `main.rs`, after parsing config:

```rust
let initial_tasks = match (initial, initial_value, &cfg.default) {
    // Explicit --initial provided
    (Some(init), None, _) => parse_initial_tasks(&init)?,

    // --initial-value with default step
    (None, Some(val), Some(default_step)) => {
        let value = parse_initial_value(&val)?;
        vec![Task::new(default_step.clone(), value)]
    }

    // No --initial or --initial-value, but config has default
    (None, None, Some(default_step)) => {
        let value = serde_json::json!({});
        // Validate {} against the step's schema
        schemas.validate(default_step, &value)?;
        vec![Task::new(default_step.clone(), value)]
    }

    // --initial-value without default step
    (None, Some(_), None) => {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "--initial-value requires config to have a 'default' step",
        ));
    }

    // No --initial and no default step
    (None, None, None) => {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "either --initial or config 'default' step is required",
        ));
    }
};
```

### 5. Add helper to parse initial value

```rust
fn parse_initial_value(input: &str) -> io::Result<serde_json::Value> {
    let content = {
        let path = PathBuf::from(input);
        if path.exists() {
            std::fs::read_to_string(path)?
        } else {
            input.to_string()
        }
    };

    json5::from_str(&content).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid initial value: {e}"),
        )
    })
}
```

## Example Usage

Config with default step:

```json
{
  "default": "Analyze",
  "steps": [
    {
      "name": "Analyze",
      "value_schema": {"type": "object", "properties": {"path": {"type": "string"}}},
      "next": ["Process"]
    },
    {"name": "Process", "next": []}
  ]
}
```

Run options:

```bash
# Full explicit initial (still works)
gsd run config.json --initial '[{"kind": "Analyze", "value": {"path": "/foo"}}]'

# Using default step with value
gsd run config.json --initial-value '{"path": "/foo"}'

# Using default step with empty object (if schema allows)
gsd run config.json
```

## Open Questions

1. **Schema validation timing**: Should we validate the initial value against the default step's schema immediately, or let the runner handle it? Currently proposing immediate validation to fail fast.

2. **Multiple initial tasks**: The current design only creates a single task from `--initial-value`. Is there a use case for creating multiple tasks at the default step with different values? Could support `--initial-value` being an array in the future if needed.
