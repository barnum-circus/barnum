# Open Refactoring Questions

Issues found during coding standards pass.

---

## gsd_config

### Code Duplication: Pre-hook handling in dispatch.rs

`dispatch_pool_task` and `dispatch_command_task` have identical pre-hook handling logic (~20 lines each). Could extract:

```rust
fn run_pre_hook_or_error(
    ctx: &TaskContext,
    original_value: &serde_json::Value,
    tx: &mpsc::Sender<InFlightResult>,
) -> Option<serde_json::Value>
```

### Code Duplication: Shell command execution in hooks.rs

`run_pre_hook`, `run_post_hook`, and `run_command_action` all spawn shell commands with similar patterns. Could extract:

```rust
fn run_shell_command(script: &str, stdin: &str, working_dir: Option<&Path>) -> Result<String, String>
```

### Double Match: gsd_cli generate_graphviz

In `main.rs:generate_graphviz`, `step.action` is matched twice (lines 431 and 461). Could match once:

```rust
let (shape, color) = match &step.action {
    Action::Pool { .. } => ("box", "#e3f2fd"),
    Action::Command { .. } => ("diamond", "#fff3e0"),
};
```

### API Design: `initial_tasks` in `RunnerConfig`

`RunnerConfig` mixes configuration with input data. `initial_tasks` should be a separate parameter:

```rust
pub fn run(
    config: &Config,
    schemas: &CompiledSchemas,
    runner_config: RunnerConfig,
    initial_tasks: Vec<Task>,  // separate parameter
) -> Result<...>
```

---

## agent_pool

### File Length: wiring.rs (~1050 lines)

`daemon/wiring.rs` is over 1000 lines. Consider splitting:
- Pool state cleanup → own module
- Socket handling → own module or merged with existing socket code
- Payload resolution → could live with Payload type

### File Length: io.rs (~570 lines)

`daemon/io.rs` is moderately long. The `TransportMap` generic machinery might warrant its own module.

### Too Many Arguments: io_loop

`io_loop` takes 14 parameters. Signs of missing abstraction:

```rust
fn io_loop(
    fs_rx: Receiver<notify::Event>,
    socket_rx: Receiver<(String, Stream)>,
    effect_rx: Receiver<Effect>,
    events_tx: &Sender<Event>,
    worker_map: &mut WorkerMap,
    submission_map: &mut SubmissionMap,
    id_allocator: &mut IdAllocator,
    pending_responses: &mut HashSet<WorkerId>,
    kicked_paths: &mut HashSet<PathBuf>,
    root: &Path,
    agents_dir: &Path,
    submissions_dir: &Path,
    io_config: &IoConfig,
    stop_notifier: &Arc<StopNotifier>,
) -> io::Result<()>
```

Consider grouping into `IoState` struct:

```rust
struct IoState {
    worker_map: WorkerMap,
    submission_map: SubmissionMap,
    id_allocator: IdAllocator,
    pending_responses: HashSet<WorkerId>,
    kicked_paths: HashSet<PathBuf>,
}
```

And `Paths`:

```rust
struct Paths {
    root: PathBuf,
    agents_dir: PathBuf,
    submissions_dir: PathBuf,
}
```

### Too Many Arguments: handle_fs_event

Same issue - 11 parameters could be reduced with `IoState` and `Paths` structs.
