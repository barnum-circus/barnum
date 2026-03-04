# State Persistence and Resume

**Status:** Not started

## Motivation

Long-running GSD jobs can be interrupted (network issues, machine restart, user Ctrl+C). Currently, all progress is lost and users must restart from scratch. State persistence enables:

1. **Resume**: Continue from where you left off after interruption
2. **Debugging**: Inspect state to understand what happened
3. **Monitoring**: External tools can read state file to track progress

## Current State

- `Runner` in `crates/gsd_config/src/runner.rs` maintains in-memory `TaskQueue`
- No way to serialize current state
- No way to resume a partially-completed run

## Proposed Changes

### 1. Define serializable state format

```rust
// crates/gsd_config/src/state.rs
#[derive(Serialize, Deserialize)]
pub struct RunState {
    /// Config file path (for validation on resume)
    pub config_path: PathBuf,
    /// Hash of config content (detect changes)
    pub config_hash: String,
    /// Completed task IDs
    pub completed: Vec<String>,
    /// Failed task IDs with error messages
    pub failed: Vec<(String, String)>,
    /// Pending task IDs (in priority order)
    pub pending: Vec<String>,
    /// In-progress task IDs
    pub in_progress: Vec<String>,
    /// Timestamp of last update
    pub updated_at: DateTime<Utc>,
}
```

### 2. Add --state-file flag to run command

```bash
gsd run config.jsonc --state-file state.json
```

Writes state after each task completion. If state file exists and config hash matches, resume from saved state.

### 3. Add resume subcommand

```bash
gsd resume state.json
```

Validates config still exists and matches, then resumes execution.

### 4. Atomic state writes

Use atomic_write (already in agent_pool) to prevent corruption:

```rust
atomic_write(&state_dir, &state_path, &serde_json::to_vec(&state)?)?;
```

## Open Questions

1. **Config changes**: What if config changed since state was saved?
   - Option A: Refuse to resume, require fresh start
   - Option B: Resume but warn about changes
   - Option C: Diff and only re-run affected tasks

2. **Task identity**: How to identify tasks across runs?
   - Currently tasks are identified by step name + input hash
   - Need stable IDs that survive config edits

3. **Concurrent access**: Multiple GSD instances with same state file?
   - Probably just document "don't do this"

4. **State file location**: Default to `.gsd-state.json` in config dir?

## Files to Change

- `crates/gsd_config/src/state.rs` - new file for state types
- `crates/gsd_config/src/runner.rs` - add state persistence hooks
- `crates/gsd_cli/src/main.rs` - add --state-file flag, resume subcommand
