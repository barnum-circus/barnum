# SharedTypes

Create the shared types used by all TUI components: theme constants, AppState, TaskRecord, and supporting types.

## Input

```json
{"workspace_root": "/path/to/barnum-worktree"}
```

## Context

The `barnum_tui` crate skeleton already exists at `{workspace_root}/crates/barnum_tui/`. You're creating the core types that all widget modules will depend on.

Read these files for type information:
- `{workspace_root}/crates/barnum_types/src/lib.rs` — `StepName`, `LogTaskId`, `StepInputValue`
- `{workspace_root}/crates/barnum_state/src/types.rs` — `TaskOrigin`, `TaskOutcome`, `FailureReason`

## Instructions

### 1. Create `crates/barnum_tui/src/theme.rs`

Define colors, icons, and styles used across all widgets:

- **Task statuses:** `Pending`, `InFlight`, `Completed`, `Failed`, `Retried` — each with a color, Unicode icon, label, and sort priority
- **Colors:** Green (completed), Yellow (in-flight), DarkGray (pending), Red (failed), Magenta (retried)
- **Icons:** Checkmark, filled circle, question mark, X mark, clockwise arrow
- **Panel styles:** `header_style()`, `selected_style()`, `focused_border_style()`, `unfocused_border_style()`

### 2. Create `crates/barnum_tui/src/app.rs`

Central application state:

**StatusCounts** — per-step task counts with `increment(status)` and `decrement(status)` methods.

**TaskRecord** — full lifecycle record of a task:
- `id: LogTaskId`, `step: StepName`, `status: TaskStatus`
- `value: serde_json::Value`, `parent_id: Option<LogTaskId>`, `children: Vec<LogTaskId>`
- `submitted_at: Instant`, `completed_at: Option<Instant>`
- `outcome: Option<TaskOutcome>`, `origin: TaskOrigin`

**PanelFocus** — enum: `Graph`, `TaskList`, `Detail` with `next()` and `prev()` for cycling.

**RunStatus** — enum: `Running`, `Completed`, `Failed`, `Waiting`.

**Viewport** — graph viewport: `scroll_x: u16`, `scroll_y: u16`, `zoom: ZoomLevel`.

**ZoomLevel** — enum: `Full`, `Compact`, `Dot`.

**SortColumn** — enum: `Id`, `Status`, `Step`, `Duration`, `Parent` with `next()` cycling.

**AppState** — the central struct:
- Static: `config_path: String`
- Dynamic: `tasks: BTreeMap<LogTaskId, TaskRecord>`, `step_counts: HashMap<StepName, StatusCounts>`, `run_status: RunStatus`, `start_time: Option<Instant>`, `total_events: u64`
- UI: `focus: PanelFocus`, `selected_step: Option<StepName>`, `selected_task: Option<LogTaskId>`, `task_list_state: ListState`, `graph_viewport: Viewport`, `status_filters: HashSet<TaskStatus>`, `search_query: Option<String>`, `sort_column: SortColumn`, `sort_reversed: bool`

**Methods on AppState:**
- `new(config_path) -> Self`
- `apply_submitted(task_id, step, value, parent_id, origin)` — creates TaskRecord, updates step_counts, sets run_status to Running
- `apply_completed(task_id, outcome)` — updates status based on outcome (Success -> Completed, Failed with retry -> Retried, Failed without -> Failed), updates step_counts
- `update_run_status()` — derives from task states: Running if any pending/in-flight, Failed if any failed, Completed if all done
- `visible_tasks() -> Vec<LogTaskId>` — filtered by selected_step, status_filters, search_query, then sorted by sort_column

### 3. Wire modules

Add `mod app;` and `mod theme;` to `main.rs`.

### 4. Verify

Run `cargo build -p barnum_tui`. Must compile.

### 5. Commit

```bash
git add crates/barnum_tui/src/
git commit -m "feat(tui): add shared types — AppState, TaskRecord, theme"
```

## Output

Return five follow-up tasks to spawn the parallel implementation phase:

```json
[
  {"kind": "LogWatcher", "value": {"workspace_root": "<same>"}},
  {"kind": "StepGraph", "value": {"workspace_root": "<same>"}},
  {"kind": "TaskList", "value": {"workspace_root": "<same>"}},
  {"kind": "DetailPane", "value": {"workspace_root": "<same>"}},
  {"kind": "HeaderFooter", "value": {"workspace_root": "<same>"}}
]
```
