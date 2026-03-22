# Barnum TUI Design

A ratatui-based terminal dashboard for observing barnum workflow runs in real-time.

## Scope

**Phase 1 (this design):** Live dashboard — view-only, real-time observation of running workflows. Read from state logs, no runner modifications.

**Phase 2 (future):** Control panel — manage troupes, create/edit configs, launch runs. Requires event channel integration with the runner.

## Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Starting point | Live dashboard (phase 1) | Higher-value feature; CLI can't visualize running workflows today |
| Data source | Tail NDJSON state log | Zero coupling to runner, works for replay, no runner changes needed |
| Layout | Workflow graph + task list | Natural mapping to barnum's model, scales well with fan-out |
| Interactivity | View-only | Dashboard is for observability; controls come in phase 2 with event channel |
| Crate location | New `barnum_tui` crate + thin shim in `barnum_cli` | Keeps TUI deps out of core CLI |

## Architecture & Data Flow

**New crate:** `crates/barnum_tui/` with binary target `barnum-tui`. Dependencies:

- `barnum_config` — parse config files to extract the step graph (names, `next` relationships, options)
- `barnum_state` — parse NDJSON state log entries (`TaskSubmitted`, `TaskCompleted`)
- `ratatui` + `crossterm` — terminal rendering and input
- `notify` — watch state log file for new lines (same crate already in workspace)

**Data flow:**

```
state.log (NDJSON) --[tail/watch]--> LogWatcher --> AppState --> UI render loop
config.jsonc --[parse once]--> StepGraph (static)                     |
                                                                       |
keyboard events --[crossterm]--> EventHandler --> AppState mutations ---+
```

**Invocation:**

```bash
# Direct
barnum-tui --config workflow.jsonc --state-log /tmp/run.log

# Via shim (added to barnum_cli)
barnum tui --config workflow.jsonc --state-log /tmp/run.log
```

The shim in `barnum_cli` is a `Command::new("barnum-tui").args(remaining).exec()` — no TUI deps pulled into the main binary.

**Core loop:** Standard ratatui pattern — `crossterm` event poll with ~100ms tick rate. On each tick: check `LogWatcher` for new state log entries, update `AppState`, re-render.

## UI Layout

```
+---------------------------------------------------------------------+
| barnum-tui > workflow.jsonc              Running * 12 tasks  00:34  |
+---------------------------+-----------------------------------------+
|                           |                                         |
|   +----------+            |  Step: Implement  (filter: all)         |
|   | Analyze  |--+         |                                         |
|   |  [check]3|  |  +----+ |  ID    Status      Duration  Value      |
|   +----------+  +->|Impl| |  t-04  * in-flight  12s     {path: ..}  |
|                 |  |*2 ?1| |  t-05  * in-flight   8s     {path: ..}  |
|   +----------+  |  +-+--+ |  t-06  ? pending      -     {path: ..}  |
|   |  Test    |<-+    |    |  t-03  [check] done  22s     {path: ..}  |
|   | [check]1 x1|     |    |  t-07  x failed     45s     timeout     |
|   +----------+  +----+-+  |                                         |
|                 |Summar.|  |                                         |
|                 | ?1    |  |                                         |
|                 +-------+  |                                         |
|                           |                                         |
|  [graph]                  |  [task list]                            |
+---------------------------+-----------------------------------------+
| Task t-04 > Implement > in-flight 12s > parent: t-01 (Analyze)     |
| Value: {"path": "/src/auth.rs", "description": "Add OAuth flow"}   |
+---------------------------------------------------------------------+
| j/k: navigate  Enter: select step  f: filter  q: quit  ?: help    |
+---------------------------------------------------------------------+
```

**Five regions:**

1. **Header bar** — Config name, run status (running/completed/failed), total task count, elapsed time.
2. **Step graph (left)** — Static workflow structure from config. Each node shows step name + badge counts (completed, in-flight, pending, failed). Selected step highlighted.
3. **Task list (right)** — Tasks for the selected step. Scrollable, sortable. Shows task ID, status icon, duration, truncated value. Selecting a task populates the detail pane.
4. **Detail pane (bottom)** — Selected task's full info: value JSON, parent chain, error message (if failed), retry info.
5. **Footer** — Keybinding hints, contextual to current focus.

**Focus model:** Tab cycles focus between graph, task list, and detail pane. Each panel has its own navigation (arrow keys in graph, j/k in list).

## Step Graph Rendering

The step graph is derived from the config (static structure) with live task counts overlaid. Configs can have dozens of steps, so the graph panel must handle scale gracefully.

**Layout algorithm:**

1. Parse steps and `next` relationships into a DAG.
2. Assign layers via topological sort (entrypoint = layer 0, each step at `max(parent layers) + 1`).
3. Within each layer, order nodes to minimize edge crossings (barycenter heuristic — sort nodes by average position of their neighbors in adjacent layer, iterate a few passes).
4. Render left-to-right: layers as columns, arrows between them.

**Node rendering:**

```
+----------+
| Analyze  |    Step name
| [check]3 *1 ?2|    Status badges with counts
+----------+
```

Badges only shown when count > 0. Colors: green for completed, yellow for in-flight/pending, red for failed.

**Edge rendering:** Unicode box-drawing characters. Barycenter ordering reduces crossings, but when they do occur, draw overlapping edges with distinct colors or dim the non-selected paths.

**Scaling strategies for large graphs:**

- **Viewport with pan:** The graph lives in a virtual canvas larger than the panel. Arrow keys pan the viewport. Selected node auto-scrolls into view.
- **Collapse/expand:** Layers or subgraphs can be collapsed into a single summary node showing aggregate counts (e.g., `[3 steps: completed 12 *4]`). Press `Enter` to expand. Collapsed by default when >15 steps visible.
- **Minimap:** When the graph exceeds 2x the panel size, render a small minimap in the corner (each step as a single colored dot) showing viewport position.
- **Zoom levels:** Three levels toggled with `+`/`-`:
  - **Full:** Name + badges (default)
  - **Compact:** Name only, smaller boxes
  - **Dot:** Single character per node with color indicating dominant status

**Selection:** Arrow keys move between nodes respecting graph topology (left/right = prev/next layer, up/down = within layer). Selected node highlighted with bold border. Selecting a step filters the task list to show only that step's tasks. An "All" pseudo-selection shows all tasks.

## Task List Panel

The task list shows what's actually happening in the run. It displays tasks filtered by the selected step (or all tasks when no step is selected).

**Columns:**

| Column | Content | Sortable? |
|--------|---------|-----------|
| ID | Short task ID (e.g., `t-04`) | Yes |
| Status | Icon + label: pending, in-flight, completed, failed, retried | Yes (default: pending/in-flight first) |
| Step | Step name (shown when "All" filter active, hidden when filtered to a step) | Yes |
| Duration | Elapsed time (live-updating for in-flight, final for completed) | Yes |
| Value | Truncated JSON preview of task value | No |
| Parent | Parent task ID (if spawned) | Yes |

**Default sort:** Status priority (in-flight, pending, failed, completed), then by submission time. User can cycle sort column with `s`, reverse with `S`.

**Filtering:**

- Step filter: driven by graph selection, or press `f` to pick from a list.
- Status filter: press `1-5` to toggle status types. Multiple can be active.
- Text search: `/` opens search, filters to tasks whose value or ID matches.

**Navigation:** `j`/`k` to move, `g`/`G` for top/bottom. Selecting a task updates the detail pane below.

**Live behavior:** New tasks from the state log append to the list respecting current sort. If the user is scrolled to the top, new in-flight/pending tasks appear naturally. If scrolled elsewhere, a subtle indicator shows "N new tasks" without disrupting position.

## Detail Pane & State Log Tailing

### Detail Pane

Shows full context for the selected task. Content varies by task status:

**All tasks show:**

- Full task ID, step name, status
- Parent chain: `t-07 <- t-04 (Implement) <- t-01 (Analyze)` — selecting a parent navigates to it
- Full value JSON, pretty-printed with syntax highlighting

**In-flight tasks additionally show:**

- Elapsed time (live-updating)
- Retry count if applicable (`attempt 2 of 3`)

**Completed tasks additionally show:**

- Final duration
- Spawned children: list of child task IDs with their step names
- Finally value (if the task had a finally hook)

**Failed tasks additionally show:**

- Failure reason: `Timeout`, `AgentLost`, `InvalidResponse {message}`
- Whether a retry was spawned (and link to retry task ID)
- Original value for easy inspection of what failed

**Detail pane navigation:** Scrollable when content exceeds height. `y` to yank/copy selected task's value JSON to clipboard (via OSC 52 or platform clipboard).

### State Log Tailing (LogWatcher)

The `LogWatcher` component feeds live data into the app.

- Opens the state log file and seeks to end (or reads from beginning for replay mode).
- Uses `notify` to watch for file modifications — no polling loop.
- On change: reads new lines, parses each as a `barnum_state` log entry.
- Parsed entries dispatched to `AppState` which updates task records and step counts.
- Handles truncation/rotation gracefully (detects file shrink, re-reads from start).

**Two modes:**

- `--live` (default): seek to end, show only new events as they arrive.
- `--replay`: read from beginning, play through events at configurable speed (`--replay-speed 10x`) or instant. Useful for post-mortem analysis of past runs.

## AppState

Central data structure driving the UI. Updated by the `LogWatcher`, read by the renderer.

```rust
struct AppState {
    // Static (from config)
    graph: StepGraph,
    config_path: String,

    // Dynamic (from state log)
    tasks: BTreeMap<LogTaskId, TaskRecord>,
    step_counts: HashMap<StepName, StatusCounts>,
    run_status: RunStatus,         // Running, Completed, Failed
    start_time: Option<Instant>,

    // UI state
    focus: PanelFocus,             // Graph | TaskList | Detail
    selected_step: Option<StepName>,
    selected_task: Option<LogTaskId>,
    task_list_state: ListState,    // Scroll position, sort, filters
    graph_viewport: Viewport,      // Pan position, zoom level
    status_filters: HashSet<TaskStatus>,
    search_query: Option<String>,
}

struct TaskRecord {
    id: LogTaskId,
    step: StepName,
    status: TaskStatus,
    value: serde_json::Value,
    parent_id: Option<LogTaskId>,
    children: Vec<LogTaskId>,
    submitted_at: Instant,
    completed_at: Option<Instant>,
    outcome: Option<TaskOutcome>,
    origin: TaskOrigin,            // Initial/Spawned/Retry/Finally
}
```

**Run status detection:** `Running` while state log is being appended to. `Completed` when all tasks are completed with no pending/in-flight remaining. `Failed` if any task exhausted retries. Derived from task states, not a log entry.

## Keybindings

| Key | Action |
|-----|--------|
| `Tab` / `Shift-Tab` | Cycle focus: graph, list, detail |
| `j`/`k` or Up/Down | Navigate within focused panel |
| Left/Right | Pan graph (when graph focused) |
| `Enter` | Select step (graph) or task (list) |
| `Esc` | Clear selection / close search |
| `f` | Step filter picker |
| `1`-`5` | Toggle status filters |
| `/` | Search tasks |
| `s` / `S` | Cycle sort column / reverse sort |
| `+`/`-` | Zoom graph |
| `c` | Toggle collapse/expand graph node |
| `y` | Copy selected task value to clipboard |
| `q` | Quit |
| `?` | Help overlay |

## Crate Structure

```
crates/barnum_tui/
  Cargo.toml
  src/
    main.rs           # CLI args, setup, run loop
    app.rs            # AppState + update logic
    log_watcher.rs    # State log tailing with notify
    graph/
      mod.rs          # StepGraph construction from config
      layout.rs       # Topological sort, layer assignment, barycenter
      render.rs       # Ratatui widget for graph panel
    task_list/
      mod.rs          # Task filtering, sorting logic
      render.rs       # Ratatui widget for task list panel
    detail/
      render.rs       # Ratatui widget for detail pane
    header.rs         # Header bar widget
    footer.rs         # Keybinding hints widget
    event.rs          # Crossterm event handling, key dispatch
    theme.rs          # Colors, styles, status icons
```

**Shim in barnum_cli:**

Added as a subcommand that delegates without pulling in TUI deps:

```rust
// In barnum_cli's main.rs, when "tui" subcommand matched:
let status = std::process::Command::new("barnum-tui")
    .args(remaining_args)
    .status()?;
std::process::exit(status.code().unwrap_or(1));
```

**New workspace dependencies:**

| Crate | Purpose |
|-------|---------|
| `ratatui` | Terminal UI framework |
| `crossterm` | Terminal backend (input, raw mode) |

`notify`, `serde`, `serde_json`, `clap` already in workspace.

## Implementation Features (Independent Work Units)

Each of these is independently implementable with clear interface boundaries:

1. **LogWatcher** — State log tailing, parsing, replay mode
2. **StepGraph** — DAG construction, layout algorithm, graph widget
3. **TaskList** — Task data model, filtering/sorting, list widget
4. **Detail pane** — Detail rendering, parent chain navigation, clipboard
5. **App shell** — Main loop, event handling, AppState, header/footer
6. **CLI shim** — Subcommand in `barnum_cli` + integration tests

The app shell wires all components together. Features 1-4 can be developed in parallel by independent agents, with the app shell integrating them.
