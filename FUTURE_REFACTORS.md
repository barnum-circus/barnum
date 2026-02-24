# Future Refactors

Ideas and improvements to implement later.

## Command Agent Improvements

### Reconnect on Timeout

When the command agent is kicked due to heartbeat timeout, it should automatically reconnect instead of exiting. Currently if the agent is slow to respond to a heartbeat (e.g., because it's running a long command), it gets kicked and the agent script exits.

**Current behavior:**
- Agent receives Kicked message → exits
- User must manually restart the agent

**Desired behavior:**
- Agent receives Kicked message → re-registers with same name
- Seamlessly continues processing tasks

### Command Timeout

The command agent should have its own configurable timeout for executing commands, separate from the daemon's heartbeat timeout.

**Problem:**
- Daemon heartbeat timeout is ~60s
- Some commands take longer than that
- Agent gets kicked while command is still running

**Solution:**
- Add `--timeout` flag to command agent
- Execute commands with timeout wrapper
- If command times out, return error response instead of hanging

## GSD Configuration

### Default Step Feature

Add the ability to mark one step as the "default" entry point, so users don't need to specify `--initial` with the full task structure.

**Current:**
```bash
gsd run config.json --pool p1 --initial '[{"kind": "Analyze", "value": {"file_url": "/path/to/file"}}]'
```

**Desired:**
```bash
gsd run config.json --pool p1 --file /path/to/file
```

#### Current Architecture

**Relevant files:**
- `crates/gsd_cli/src/main.rs:26-44` - CLI args definition
- `crates/gsd_cli/src/main.rs:86-88` - `parse_initial_tasks()` call
- `crates/gsd_cli/src/main.rs:177-195` - `parse_initial_tasks()` implementation
- `crates/gsd_config/src/config.rs:10-24` - `Config` struct
- `crates/gsd_config/src/types.rs` - `Task` struct

**Current flow:**
1. CLI receives `--initial` as JSON string or file path
2. `parse_initial_tasks()` deserializes to `Vec<Task>`
3. Each `Task` has `{kind: StepName, value: serde_json::Value}`
4. Runner executes these tasks

#### Design Options

**Option A: Config-level default step**
```json
{
  "default_step": "Analyze",
  "steps": [...]
}
```

**Option B: Step-level marker**
```json
{
  "steps": [
    {"name": "Analyze", "default": true, ...}
  ]
}
```

**Option C: First step is default**
- No config change needed
- First step in `steps` array is the entry point

#### Open Questions

1. **Which approach?** Config-level `default_step`, step-level `default: true`, or implicit first-step?

2. **CLI mapping** - How should CLI args map to the value schema?
   - Option A: Named flags matching schema properties (`--file_url /path`)
   - Option B: Positional args in schema order
   - Option C: Single `--value` flag with simpler JSON (`--value '{"file_url": "/path"}'`)
   - Option D: Generate CLI flags from schema at runtime

3. **Multiple initial tasks?** - Current `--initial` supports an array. Should we keep that capability alongside the simplified form?

4. **Validation** - Should we validate that the default step exists during config validation?
