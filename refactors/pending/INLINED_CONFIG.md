# Inlined Config

**Status:** Not started

**Depends on:** Nothing

**Blocks:** STATE_PERSISTENCE (config must be fully resolved before serializing)

## Motivation

Currently, config parsing may defer file reads (e.g., instruction links). This creates issues:
1. FS reads during execution can fail unexpectedly (file deleted, permissions changed)
2. Can't serialize config to state file without resolving references first
3. Harder to reason about what the config "is" at any point
4. Same file read multiple times (once per task of that step)

## Goal

After config loading, all file references are resolved. The resulting struct is fully self-contained - no further FS reads needed during execution.

## Current State

File reads happen at different times:

| Reference Type | When Read | Location | Status |
|----------------|-----------|----------|--------|
| `SchemaRef::Link` | Startup (`CompiledSchemas::compile()`) | `value_schema.rs:34` | OK |
| `Instructions::Link` | Per task execution | `docs.rs:23` | **PROBLEM** |
| `Action::Command { script }` | N/A (inline string) | - | OK |

The main issue is `Instructions::Link` - linked markdown files are read every time we build the agent payload.

## Before/After: Config JSON

**Before (config.jsonc):**
```jsonc
{
  "steps": [
    {
      "name": "Analyze",
      "action": {
        "kind": "Pool",
        "instructions": {"link": "instructions/analyze.md"}  // File reference
      },
      "value_schema": {"link": "schemas/analyze.json"},  // File reference
      "next": ["Report"]
    }
  ]
}
```

**After (inlined, what gets serialized):**
```json
{
  "steps": [
    {
      "name": "Analyze",
      "action": {
        "kind": "Pool",
        "instructions": "# Analyze Step\n\nYou are analyzing code..."  // Inlined content
      },
      "value_schema": {"type": "object", "properties": {...}},  // Inlined schema
      "next": ["Report"]
    }
  ]
}
```

## Before/After: Rust Types

### Before

```rust
// crates/gsd_config/src/config.rs

/// Instructions can be inline or linked to a file
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Instructions {
    Inline(String),
    Link { link: String },  // Stores path, read later
}

/// Schema can be inline or linked to a file
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SchemaRef {
    Link(String),  // Stores path, read in CompiledSchemas::compile()
    Inline(serde_json::Value),
}
```

### After

```rust
// crates/gsd_config/src/config.rs

/// Raw config as parsed from JSON (may contain file references)
#[derive(Debug, Clone, Deserialize)]
pub struct ConfigFile {
    pub steps: Vec<StepFile>,
}

/// Raw step (may contain file references)
#[derive(Debug, Clone, Deserialize)]
pub struct StepFile {
    pub name: StepName,
    pub instructions: MaybeLinked<String>,
    pub value_schema: Option<MaybeLinked<serde_json::Value>>,
    pub next: Vec<StepName>,
    // ...
}

/// Fully resolved config (no file references)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub steps: Vec<Step>,
}

/// Fully resolved step - just values, no MaybeLinked
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub name: StepName,
    pub instructions: String,  // Just String
    pub value_schema: Option<serde_json::Value>,  // Just Value
    pub next: Vec<StepName>,
    // ...
}

impl ConfigFile {
    /// Load from JSON file
    pub fn load(path: &Path) -> io::Result<Self> { ... }

    /// Resolve all file references, returning Config with just values
    pub fn resolve(self, base_path: &Path) -> io::Result<Config> {
        let steps = self.steps.into_iter()
            .map(|s| s.resolve(base_path))
            .collect::<io::Result<Vec<_>>>()?;
        Ok(Config { steps })
    }
}

impl StepFile {
    pub fn resolve(self, base_path: &Path) -> io::Result<Step> {
        Ok(Step {
            name: self.name,
            instructions: self.instructions.resolve(base_path)?,
            value_schema: self.value_schema.map(|s| s.resolve(base_path)).transpose()?,
            next: self.next,
        })
    }
}
```

## Before/After: Doc Generation

### Before (docs.rs)

```rust
fn write_instructions(doc: &mut String, action: &Action, base_path: &Path) {
    let instructions = match action {
        Action::Pool { instructions } | Action::Command { instructions, .. } => instructions,
    };

    match instructions {
        Instructions::Inline(text) => {
            writeln!(doc, "{}", text.trim()).ok();
        }
        Instructions::Link { link } => {
            // FILE READ HAPPENS HERE - per task execution!
            let full_path = base_path.join(link);
            match fs::read_to_string(&full_path) {
                Ok(content) => writeln!(doc, "{}", content.trim()).ok(),
                Err(e) => writeln!(doc, "*Error loading instructions: {e}*").ok(),
            };
        }
    }
}
```

### After (docs.rs)

```rust
fn write_instructions(doc: &mut String, action: &Action) {
    // No base_path needed - instructions already resolved
    let instructions = match action {
        Action::Pool { instructions } | Action::Command { instructions, .. } => instructions,
    };

    // Instructions is just a String now, always inline
    writeln!(doc, "{}", instructions.0.trim()).ok();
}
```

## Before/After: TaskRunner

### Before

```rust
pub struct TaskRunner<'a> {
    config: &'a Config,
    config_base_path: &'a Path,  // Needed for resolving links at runtime
    // ...
}

impl TaskRunner<'_> {
    fn process_task(&self, task: Task) {
        let docs = generate_step_docs(step, self.config, self.config_base_path);
        //                                              ^^^^^^^^^^^^^^^^^^^^
        //                                              Passed through for file reads
    }
}
```

### After

```rust
pub struct TaskRunner {
    config: Config,  // Owned, fully resolved
    // No config_base_path needed
    // ...
}

impl TaskRunner {
    fn process_task(&self, task: Task) {
        let docs = generate_step_docs(step, &self.config);
        //                                  No base_path needed
    }
}
```

## Implementation Phases

### Phase 0: Generic MaybeLinked / Inlined Types

First, introduce generic types to handle the inline-vs-linked pattern.

#### Core Type

```rust
// crates/gsd_config/src/maybe_linked.rs

use std::path::Path;
use std::io;

/// Content that may be inline or linked to a file.
///
/// Used during config parsing. Call `resolve()` to get the actual value.
#[derive(Debug, Clone)]
pub enum MaybeLinked<T> {
    Inline(T),
    Link(String),
}
```

No wrapper type needed - `resolve()` returns `T` directly, and the final config just stores `T`.

#### Serde Strategies

Different `MaybeLinked<T>` need different serde implementations:

**1. String-or-object (for Instructions):**
```rust
// "some text" OR {"link": "path/to/file.md"}

impl<'de> Deserialize<'de> for MaybeLinked<String> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum StringOrLink {
            Inline(String),
            Link { link: String },
        }

        match StringOrLink::deserialize(deserializer)? {
            StringOrLink::Inline(s) => Ok(MaybeLinked::Inline(s)),
            StringOrLink::Link { link } => Ok(MaybeLinked::Link(link)),
        }
    }
}
```

**2. Value-or-string (for Schema):**
```rust
// {"type": "object", ...} OR "path/to/schema.json"

impl<'de> Deserialize<'de> for MaybeLinked<serde_json::Value> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        match value {
            serde_json::Value::String(path) => Ok(MaybeLinked::Link(path)),
            other => Ok(MaybeLinked::Inline(other)),
        }
    }
}
```

#### Resolution (concrete impls, no trait)

```rust
impl MaybeLinked<String> {
    pub fn resolve(self, base_path: &Path) -> io::Result<String> {
        match self {
            MaybeLinked::Inline(s) => Ok(s),
            MaybeLinked::Link(link) => {
                let path = base_path.join(&link);
                std::fs::read_to_string(&path).map_err(|e| {
                    io::Error::new(e.kind(), format!("failed to read '{}': {e}", path.display()))
                })
            }
        }
    }
}

impl MaybeLinked<serde_json::Value> {
    pub fn resolve(self, base_path: &Path) -> io::Result<serde_json::Value> {
        match self {
            MaybeLinked::Inline(v) => Ok(v),
            MaybeLinked::Link(link) => {
                let path = base_path.join(&link);
                let content = std::fs::read_to_string(&path).map_err(|e| {
                    io::Error::new(e.kind(), format!("failed to read '{}': {e}", path.display()))
                })?;
                serde_json::from_str(&content).map_err(|e| {
                    io::Error::new(io::ErrorKind::InvalidData, format!("invalid JSON in '{}': {e}", path.display()))
                })
            }
        }
    }
}
```

#### Usage in Config Types

```rust
// Before resolution (ConfigFile) - may have links
pub struct StepFile {
    pub name: StepName,
    pub instructions: MaybeLinked<String>,
    pub value_schema: Option<MaybeLinked<serde_json::Value>>,
    // ...
}

// After resolution (Config) - just the values, no wrapper
pub struct Step {
    pub name: StepName,
    pub instructions: String,  // Just String, not MaybeLinked or Inlined
    pub value_schema: Option<serde_json::Value>,  // Just Value
    // ...
}
```

**This phase is pure addition** - doesn't change existing code. Just adds `MaybeLinked<T>` to use in later phases.

### Phase 1: Add ConfigFile type

1. Rename current `Config` to `ConfigFile`
2. Rename current `Step` to `StepFile`
3. Add new `Config` and `Step` types with inlined fields
4. Implement `ConfigFile::resolve(base_path) -> io::Result<Config>`
5. Update `Config::load()` to call `ConfigFile::load()` then `resolve()`

**Tests still pass** - external API unchanged, just internal restructuring.

### Phase 2: Update docs.rs

1. Remove `base_path` parameter from `write_instructions()`
2. Remove `base_path` parameter from `generate_step_docs()`
3. `Instructions` becomes a newtype `Instructions(String)` - no Link variant

### Phase 3: Update TaskRunner

1. Change `config: &'a Config` to `config: Config` (owned)
2. Remove `config_base_path` field
3. Update all call sites

### Phase 4: Cleanup

1. Remove `Instructions::Link` variant entirely
2. Remove `SchemaRef::Link` variant (already resolved during `ConfigFile::resolve()`)
3. `CompiledSchemas::compile()` now receives pre-resolved schemas

## Error Handling

File read errors during `ConfigFile::resolve()`:

```rust
pub fn resolve(self, base_path: &Path) -> io::Result<Config> {
    for step in &self.steps {
        if let Instructions::Link { link } = &step.action.instructions {
            let path = base_path.join(link);
            let content = fs::read_to_string(&path).map_err(|e| {
                io::Error::new(
                    e.kind(),
                    format!("[E070] failed to read instructions file '{}': {e}", path.display())
                )
            })?;
            // Store content...
        }
    }
    // ...
}
```

All file errors surface at startup, not during task execution.

## Serialization

The resolved `Config` serializes cleanly to JSON:

```rust
let config: Config = ConfigFile::load("config.jsonc")?.resolve(base_path)?;

// For STATE_PERSISTENCE - serialize to run folder
let json = serde_json::to_string_pretty(&config)?;
fs::write(run_folder.join("config.json"), json)?;

// On resume - deserialize directly (no file resolution needed)
let config: Config = serde_json::from_str(&fs::read_to_string("config.json")?)?;
```
