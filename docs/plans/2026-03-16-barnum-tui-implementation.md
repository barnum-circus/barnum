# Barnum TUI Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a ratatui-based terminal dashboard for observing barnum workflow runs in real-time, reading from NDJSON state logs.

**Architecture:** New `barnum_tui` crate with a standard ratatui event loop. A `LogWatcher` tails the state log via `notify`, feeding parsed events into `AppState`. The UI renders a step graph (left), task list (right), and detail pane (bottom). A thin CLI shim in `barnum_cli` delegates to the `barnum-tui` binary.

**Tech Stack:** Rust, ratatui 0.29+, crossterm, notify 7.0, barnum_state, barnum_config, serde_json, clap 4.5

**Design doc:** `docs/plans/2026-03-16-barnum-tui-design.md`

---

## Task Dependency DAG

```
T1:CrateSkeleton -> T2:SharedTypes -> T3:LogWatcher    -\
                                   -> T4:StepGraph      --> T8:EventHandling -> T9:MainLoop -> T10:CLIShim
                                   -> T5:TaskList       -/
                                   -> T6:DetailPane    -/
                                   -> T7:HeaderFooter  -/
```

**Parallelizable:** Tasks 3, 4, 5, 6, 7 are fully independent once Task 2 lands.

---

## Task 1: Crate Skeleton

**Files:**
- Create: `crates/barnum_tui/Cargo.toml`
- Create: `crates/barnum_tui/src/main.rs`
- Modify: `Cargo.toml` (workspace root — add member + dependencies)

**Step 1: Add workspace dependencies**

Add to the workspace root `Cargo.toml` under `[workspace.dependencies]`:

```toml
ratatui = "0.29"
crossterm = "0.28"
```

And add `"crates/barnum_tui"` to the `[workspace] members` array.

**Step 2: Create Cargo.toml**

Create `crates/barnum_tui/Cargo.toml`:

```toml
[package]
name = "barnum_tui"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "barnum-tui"
path = "src/main.rs"

[dependencies]
barnum_config = { path = "../barnum_config" }
barnum_state = { path = "../barnum_state" }
barnum_types = { path = "../barnum_types" }
ratatui = { workspace = true }
crossterm = { workspace = true }
notify = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
clap = { workspace = true }
anyhow = "1"
```

**Step 3: Create minimal main.rs**

Create `crates/barnum_tui/src/main.rs`:

```rust
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "barnum-tui", about = "Terminal dashboard for barnum workflows")]
struct Cli {
    /// Path to the workflow config file (JSON/JSONC)
    #[arg(long)]
    config: PathBuf,

    /// Path to the NDJSON state log file
    #[arg(long)]
    state_log: PathBuf,

    /// Replay mode: read log from beginning instead of tailing
    #[arg(long, default_value_t = false)]
    replay: bool,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    println!(
        "barnum-tui: config={}, state_log={}, replay={}",
        cli.config.display(),
        cli.state_log.display(),
        cli.replay
    );
    Ok(())
}
```

**Step 4: Verify it builds**

Run: `cargo build -p barnum_tui`
Expected: Compiles successfully with no errors.

**Step 5: Commit**

```bash
git add crates/barnum_tui/ Cargo.toml Cargo.lock
git commit -m "feat(tui): add barnum_tui crate skeleton with CLI args"
```

---

## Task 2: Shared Types (theme + AppState)

**Files:**
- Create: `crates/barnum_tui/src/theme.rs`
- Create: `crates/barnum_tui/src/app.rs`

**Step 1: Create theme.rs**

Defines colors, styles, and status icons used across all widgets.

```rust
use ratatui::style::{Color, Modifier, Style};

// Status colors
pub const COLOR_COMPLETED: Color = Color::Green;
pub const COLOR_IN_FLIGHT: Color = Color::Yellow;
pub const COLOR_PENDING: Color = Color::DarkGray;
pub const COLOR_FAILED: Color = Color::Red;
pub const COLOR_RETRIED: Color = Color::Magenta;

// Status icons (Unicode)
pub const ICON_COMPLETED: &str = "\u{2714}"; // checkmark
pub const ICON_IN_FLIGHT: &str = "\u{25CF}"; // filled circle
pub const ICON_PENDING: &str = "?";
pub const ICON_FAILED: &str = "\u{2718}";    // X mark
pub const ICON_RETRIED: &str = "\u{21BB}";   // clockwise arrow

// Panel styles
pub fn header_style() -> Style {
    Style::default().fg(Color::White).bg(Color::DarkGray)
}

pub fn selected_style() -> Style {
    Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan)
}

pub fn focused_border_style() -> Style {
    Style::default().fg(Color::Cyan)
}

pub fn unfocused_border_style() -> Style {
    Style::default().fg(Color::DarkGray)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskStatus {
    Pending,
    InFlight,
    Completed,
    Failed,
    Retried,
}

impl TaskStatus {
    pub fn color(self) -> Color {
        match self {
            Self::Completed => COLOR_COMPLETED,
            Self::InFlight => COLOR_IN_FLIGHT,
            Self::Pending => COLOR_PENDING,
            Self::Failed => COLOR_FAILED,
            Self::Retried => COLOR_RETRIED,
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::Completed => ICON_COMPLETED,
            Self::InFlight => ICON_IN_FLIGHT,
            Self::Pending => ICON_PENDING,
            Self::Failed => ICON_FAILED,
            Self::Retried => ICON_RETRIED,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::InFlight => "in-flight",
            Self::Pending => "pending",
            Self::Failed => "failed",
            Self::Retried => "retried",
        }
    }

    /// Sort priority: in-flight first, then pending, failed, retried, completed last
    pub fn sort_priority(self) -> u8 {
        match self {
            Self::InFlight => 0,
            Self::Pending => 1,
            Self::Failed => 2,
            Self::Retried => 3,
            Self::Completed => 4,
        }
    }
}
```

**Step 2: Create app.rs with AppState and TaskRecord**

```rust
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Instant;

use barnum_state::types::{TaskOrigin, TaskOutcome};
use barnum_types::{LogTaskId, StepName};
use ratatui::widgets::ListState;

use crate::theme::TaskStatus;

/// Counts of tasks in each status for a given step
#[derive(Debug, Default, Clone)]
pub struct StatusCounts {
    pub completed: u32,
    pub in_flight: u32,
    pub pending: u32,
    pub failed: u32,
    pub retried: u32,
}

impl StatusCounts {
    pub fn total(&self) -> u32 {
        self.completed + self.in_flight + self.pending + self.failed + self.retried
    }

    pub fn increment(&mut self, status: TaskStatus) {
        match status {
            TaskStatus::Completed => self.completed += 1,
            TaskStatus::InFlight => self.in_flight += 1,
            TaskStatus::Pending => self.pending += 1,
            TaskStatus::Failed => self.failed += 1,
            TaskStatus::Retried => self.retried += 1,
        }
    }

    pub fn decrement(&mut self, status: TaskStatus) {
        match status {
            TaskStatus::Completed => self.completed = self.completed.saturating_sub(1),
            TaskStatus::InFlight => self.in_flight = self.in_flight.saturating_sub(1),
            TaskStatus::Pending => self.pending = self.pending.saturating_sub(1),
            TaskStatus::Failed => self.failed = self.failed.saturating_sub(1),
            TaskStatus::Retried => self.retried = self.retried.saturating_sub(1),
        }
    }
}

/// Record of a single task's lifecycle
#[derive(Debug, Clone)]
pub struct TaskRecord {
    pub id: LogTaskId,
    pub step: StepName,
    pub status: TaskStatus,
    pub value: serde_json::Value,
    pub parent_id: Option<LogTaskId>,
    pub children: Vec<LogTaskId>,
    pub submitted_at: Instant,
    pub completed_at: Option<Instant>,
    pub outcome: Option<TaskOutcome>,
    pub origin: TaskOrigin,
}

/// Which panel has keyboard focus
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelFocus {
    Graph,
    TaskList,
    Detail,
}

impl PanelFocus {
    pub fn next(self) -> Self {
        match self {
            Self::Graph => Self::TaskList,
            Self::TaskList => Self::Detail,
            Self::Detail => Self::Graph,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Graph => Self::Detail,
            Self::TaskList => Self::Graph,
            Self::Detail => Self::TaskList,
        }
    }
}

/// Overall run status, derived from task states
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    /// State log is being appended to, tasks in-flight or pending
    Running,
    /// All tasks completed successfully
    Completed,
    /// At least one task failed with no retry
    Failed,
    /// No events received yet
    Waiting,
}

/// Graph viewport for panning/zooming
#[derive(Debug, Clone)]
pub struct Viewport {
    pub scroll_x: u16,
    pub scroll_y: u16,
    pub zoom: ZoomLevel,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            scroll_x: 0,
            scroll_y: 0,
            zoom: ZoomLevel::Full,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoomLevel {
    Full,
    Compact,
    Dot,
}

/// Sort column for task list
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortColumn {
    Id,
    Status,
    Step,
    Duration,
    Parent,
}

impl SortColumn {
    pub fn next(self) -> Self {
        match self {
            Self::Id => Self::Status,
            Self::Status => Self::Step,
            Self::Step => Self::Duration,
            Self::Duration => Self::Parent,
            Self::Parent => Self::Id,
        }
    }
}

/// Central application state
pub struct AppState {
    // Static (from config)
    pub config_path: String,

    // Dynamic (from state log)
    pub tasks: BTreeMap<LogTaskId, TaskRecord>,
    pub step_counts: HashMap<StepName, StatusCounts>,
    pub run_status: RunStatus,
    pub start_time: Option<Instant>,
    pub total_events: u64,

    // UI state
    pub focus: PanelFocus,
    pub selected_step: Option<StepName>,
    pub selected_task: Option<LogTaskId>,
    pub task_list_state: ListState,
    pub graph_viewport: Viewport,
    pub status_filters: HashSet<TaskStatus>,
    pub search_query: Option<String>,
    pub sort_column: SortColumn,
    pub sort_reversed: bool,
}

impl AppState {
    pub fn new(config_path: String) -> Self {
        Self {
            config_path,
            tasks: BTreeMap::new(),
            step_counts: HashMap::new(),
            run_status: RunStatus::Waiting,
            start_time: None,
            total_events: 0,
            focus: PanelFocus::Graph,
            selected_step: None,
            selected_task: None,
            task_list_state: ListState::default(),
            graph_viewport: Viewport::default(),
            status_filters: HashSet::new(),
            search_query: None,
            sort_column: SortColumn::Status,
            sort_reversed: false,
        }
    }

    /// Apply a TaskSubmitted event
    pub fn apply_submitted(
        &mut self,
        task_id: LogTaskId,
        step: StepName,
        value: serde_json::Value,
        parent_id: Option<LogTaskId>,
        origin: TaskOrigin,
    ) {
        if self.start_time.is_none() {
            self.start_time = Some(Instant::now());
        }
        self.run_status = RunStatus::Running;

        let status = TaskStatus::Pending;

        // Update parent's children list
        if let Some(pid) = parent_id {
            if let Some(parent) = self.tasks.get_mut(&pid) {
                parent.children.push(task_id);
            }
        }

        // Update step counts
        self.step_counts
            .entry(step.clone())
            .or_default()
            .increment(status);

        let record = TaskRecord {
            id: task_id,
            step,
            status,
            value,
            parent_id,
            children: Vec::new(),
            submitted_at: Instant::now(),
            completed_at: None,
            outcome: None,
            origin,
        };
        self.tasks.insert(task_id, record);
        self.total_events += 1;
    }

    /// Apply a TaskCompleted event
    pub fn apply_completed(&mut self, task_id: LogTaskId, outcome: TaskOutcome) {
        if let Some(record) = self.tasks.get_mut(&task_id) {
            let old_status = record.status;

            record.status = match &outcome {
                TaskOutcome::Success(_) => TaskStatus::Completed,
                TaskOutcome::Failed(f) => {
                    if f.retry_task_id.is_some() {
                        TaskStatus::Retried
                    } else {
                        TaskStatus::Failed
                    }
                }
            };
            record.completed_at = Some(Instant::now());
            record.outcome = Some(outcome);

            // Update step counts
            if let Some(counts) = self.step_counts.get_mut(&record.step) {
                counts.decrement(old_status);
                counts.increment(record.status);
            }
        }
        self.total_events += 1;
        self.update_run_status();
    }

    /// Derive run status from current task states
    fn update_run_status(&mut self) {
        let has_active = self.tasks.values().any(|t| {
            matches!(t.status, TaskStatus::Pending | TaskStatus::InFlight)
        });
        let has_failed = self.tasks.values().any(|t| {
            matches!(t.status, TaskStatus::Failed)
        });

        self.run_status = if has_active {
            RunStatus::Running
        } else if has_failed {
            RunStatus::Failed
        } else if self.tasks.is_empty() {
            RunStatus::Waiting
        } else {
            RunStatus::Completed
        };
    }

    /// Get filtered and sorted task list for current view
    pub fn visible_tasks(&self) -> Vec<LogTaskId> {
        let mut tasks: Vec<_> = self
            .tasks
            .values()
            .filter(|t| {
                // Step filter
                if let Some(ref step) = self.selected_step {
                    if &t.step != step {
                        return false;
                    }
                }
                // Status filter
                if !self.status_filters.is_empty() && !self.status_filters.contains(&t.status) {
                    return false;
                }
                // Search filter
                if let Some(ref query) = self.search_query {
                    let id_str = format!("t-{:02}", t.id.0);
                    let val_str = t.value.to_string();
                    if !id_str.contains(query) && !val_str.contains(query) {
                        return false;
                    }
                }
                true
            })
            .collect();

        // Sort
        tasks.sort_by(|a, b| {
            let cmp = match self.sort_column {
                SortColumn::Id => a.id.0.cmp(&b.id.0),
                SortColumn::Status => a.status.sort_priority().cmp(&b.status.sort_priority()),
                SortColumn::Step => a.step.cmp(&b.step),
                SortColumn::Duration => {
                    let dur_a = a.completed_at.unwrap_or_else(Instant::now).duration_since(a.submitted_at);
                    let dur_b = b.completed_at.unwrap_or_else(Instant::now).duration_since(b.submitted_at);
                    dur_a.cmp(&dur_b)
                }
                SortColumn::Parent => a.parent_id.map(|p| p.0).cmp(&b.parent_id.map(|p| p.0)),
            };
            if self.sort_reversed { cmp.reverse() } else { cmp }
        });

        tasks.into_iter().map(|t| t.id).collect()
    }
}
```

**Step 3: Wire modules into main.rs**

Add to `main.rs` at the top:

```rust
mod app;
mod theme;
```

**Step 4: Verify it builds**

Run: `cargo build -p barnum_tui`
Expected: Compiles successfully.

**Step 5: Commit**

```bash
git add crates/barnum_tui/src/
git commit -m "feat(tui): add shared types — AppState, TaskRecord, theme"
```

---

## Task 3: LogWatcher

**Files:**
- Create: `crates/barnum_tui/src/log_watcher.rs`

**Dependencies:** Task 2 (uses `AppState`)

**Step 1: Write LogWatcher**

The `LogWatcher` tails the NDJSON state log file, parsing new entries as they appear. It uses `notify` for filesystem events and returns parsed entries via a channel.

```rust
use std::fs::File;
use std::io::{self, BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use barnum_state::types::StateLogEntry;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

pub enum LogEvent {
    Entry(StateLogEntry),
    Error(String),
    FileRotated,
}

pub struct LogWatcher {
    rx: mpsc::Receiver<LogEvent>,
    // Keep watcher alive — dropping it stops file monitoring
    _watcher: RecommendedWatcher,
}

impl LogWatcher {
    /// Create a new LogWatcher.
    /// If `replay` is true, reads from the beginning. Otherwise seeks to end.
    pub fn new(path: &Path, replay: bool) -> anyhow::Result<Self> {
        let path = path.to_path_buf();
        let (tx, rx) = mpsc::channel();

        let mut file = File::open(&path)?;
        if !replay {
            file.seek(SeekFrom::End(0))?;
        }

        let mut reader = BufReader::new(file);

        // Read any existing content (for replay mode or catching up)
        read_new_lines(&mut reader, &tx);

        // Set up file watcher
        let watch_path = path.clone();
        let mut watcher = notify::recommended_watcher(move |res: Result<Event, _>| {
            match res {
                Ok(event) => {
                    if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                        read_new_lines(&mut reader, &tx);
                    }
                }
                Err(e) => {
                    let _ = tx.send(LogEvent::Error(format!("Watch error: {e}")));
                }
            }
        })?;

        // Watch the parent directory (more reliable for file modifications)
        let watch_dir = watch_path.parent().unwrap_or(Path::new("."));
        watcher.watch(watch_dir, RecursiveMode::NonRecursive)?;

        Ok(Self { rx, _watcher: watcher })
    }

    /// Drain all pending events (non-blocking)
    pub fn poll(&self) -> Vec<LogEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.rx.try_recv() {
            events.push(event);
        }
        events
    }
}

fn read_new_lines(reader: &mut BufReader<File>, tx: &mpsc::Sender<LogEvent>) {
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break, // No more data
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match serde_json::from_str::<StateLogEntry>(trimmed) {
                    Ok(entry) => {
                        let _ = tx.send(LogEvent::Entry(entry));
                    }
                    Err(e) => {
                        let _ = tx.send(LogEvent::Error(format!("Parse error: {e}")));
                    }
                }
            }
            Err(e) => {
                let _ = tx.send(LogEvent::Error(format!("Read error: {e}")));
                break;
            }
        }
    }
}
```

**Important note on the notify closure:** The above uses a closure that captures `reader` by move. However, `notify`'s callback requires `Fn` (not `FnMut`), and `BufReader` needs mutable access. The actual implementation must handle this differently — likely by using `Arc<Mutex<BufReader<File>>>` or by having the watcher just signal "something changed" and doing the reading on the main thread. Here's the corrected approach:

```rust
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;
use std::sync::mpsc;

use barnum_state::types::StateLogEntry;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

pub enum LogEvent {
    Entry(StateLogEntry),
    Error(String),
}

pub struct LogWatcher {
    reader: BufReader<File>,
    notify_rx: mpsc::Receiver<()>,
    _watcher: RecommendedWatcher,
}

impl LogWatcher {
    pub fn new(path: &Path, replay: bool) -> anyhow::Result<Self> {
        let mut file = File::open(path)?;
        if !replay {
            file.seek(SeekFrom::End(0))?;
        }
        let reader = BufReader::new(file);

        let (notify_tx, notify_rx) = mpsc::channel();

        let watched_path = path.to_path_buf();
        let mut watcher = notify::recommended_watcher(move |res: Result<Event, _>| {
            if let Ok(event) = res {
                if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                    let _ = notify_tx.send(());
                }
            }
        })?;

        let watch_dir = path.parent().unwrap_or(Path::new("."));
        watcher.watch(watch_dir, RecursiveMode::NonRecursive)?;

        let mut watcher_instance = Self {
            reader,
            notify_rx,
            _watcher: watcher,
        };

        // For replay mode, read existing content immediately
        if replay {
            // Initial read happens on first poll
        }

        Ok(watcher_instance)
    }

    /// Drain all pending events (non-blocking). Call this every tick.
    pub fn poll(&mut self) -> Vec<LogEvent> {
        // Drain notify signals
        while self.notify_rx.try_recv().is_ok() {}

        // Read any new lines
        let mut events = Vec::new();
        let mut line = String::new();
        loop {
            line.clear();
            match self.reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<StateLogEntry>(trimmed) {
                        Ok(entry) => events.push(LogEvent::Entry(entry)),
                        Err(e) => events.push(LogEvent::Error(format!("Parse: {e}"))),
                    }
                }
                Err(e) => {
                    events.push(LogEvent::Error(format!("Read: {e}")));
                    break;
                }
            }
        }
        events
    }
}
```

**Step 2: Wire into main.rs**

Add `mod log_watcher;` to main.rs.

**Step 3: Verify it builds**

Run: `cargo build -p barnum_tui`
Expected: Compiles.

**Step 4: Commit**

```bash
git add crates/barnum_tui/src/log_watcher.rs crates/barnum_tui/src/main.rs
git commit -m "feat(tui): add LogWatcher — tails NDJSON state log via notify"
```

---

## Task 4: Step Graph (Construction + Layout)

**Files:**
- Create: `crates/barnum_tui/src/graph/mod.rs`
- Create: `crates/barnum_tui/src/graph/layout.rs`
- Create: `crates/barnum_tui/src/graph/render.rs`

**Dependencies:** Task 2

**Step 1: Create graph/mod.rs — DAG construction from config**

```rust
use std::collections::HashMap;
use barnum_types::StepName;
use barnum_config::config::ConfigFile;

pub mod layout;
pub mod render;

/// Static step graph extracted from config
#[derive(Debug, Clone)]
pub struct StepGraph {
    pub steps: Vec<StepNode>,
    pub edges: Vec<(usize, usize)>, // (from_index, to_index)
    pub index_by_name: HashMap<StepName, usize>,
}

#[derive(Debug, Clone)]
pub struct StepNode {
    pub name: StepName,
    pub next: Vec<StepName>,
    pub layer: u16,       // Assigned by layout
    pub order: u16,       // Position within layer
}

impl StepGraph {
    /// Build from a parsed config file
    pub fn from_config(config: &ConfigFile) -> Self {
        let mut steps: Vec<StepNode> = Vec::new();
        let mut index_by_name: HashMap<StepName, usize> = HashMap::new();

        for step_file in &config.steps {
            let idx = steps.len();
            index_by_name.insert(step_file.name.clone(), idx);
            steps.push(StepNode {
                name: step_file.name.clone(),
                next: step_file.next.clone(),
                layer: 0,
                order: 0,
            });
        }

        let mut edges = Vec::new();
        for (from_idx, step) in steps.iter().enumerate() {
            for next_name in &step.next {
                if let Some(&to_idx) = index_by_name.get(next_name) {
                    edges.push((from_idx, to_idx));
                }
            }
        }

        let mut graph = Self {
            steps,
            edges,
            index_by_name,
        };
        layout::assign_layers(&mut graph);
        layout::order_within_layers(&mut graph);
        graph
    }

    /// Get step node by name
    pub fn get(&self, name: &StepName) -> Option<&StepNode> {
        self.index_by_name.get(name).map(|&idx| &self.steps[idx])
    }

    /// Get all step names in layer order
    pub fn layers(&self) -> Vec<Vec<usize>> {
        let max_layer = self.steps.iter().map(|s| s.layer).max().unwrap_or(0);
        let mut layers = vec![Vec::new(); (max_layer + 1) as usize];
        for (idx, step) in self.steps.iter().enumerate() {
            layers[step.layer as usize].push(idx);
        }
        // Sort each layer by order
        for layer in &mut layers {
            layer.sort_by_key(|&idx| self.steps[idx].order);
        }
        layers
    }

    pub fn step_count(&self) -> usize {
        self.steps.len()
    }
}
```

**Step 2: Create graph/layout.rs — topological sort + barycenter heuristic**

```rust
use super::StepGraph;
use std::collections::HashMap;

/// Assign layers using longest-path from sources (topological sort)
pub fn assign_layers(graph: &mut StepGraph) {
    let n = graph.steps.len();
    if n == 0 {
        return;
    }

    // Build in-degree map and adjacency list
    let mut in_degree = vec![0u32; n];
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];

    for &(from, to) in &graph.edges {
        in_degree[to] += 1;
        children[from].push(to);
    }

    // Kahn's algorithm for topological order + longest path
    let mut queue: Vec<usize> = (0..n).filter(|&i| in_degree[i] == 0).collect();
    let mut layers = vec![0u16; n];

    while let Some(node) = queue.pop() {
        for &child in &children[node] {
            layers[child] = layers[child].max(layers[node] + 1);
            in_degree[child] -= 1;
            if in_degree[child] == 0 {
                queue.push(child);
            }
        }
    }

    for (i, step) in graph.steps.iter_mut().enumerate() {
        step.layer = layers[i];
    }
}

/// Order nodes within layers using barycenter heuristic to minimize edge crossings
pub fn order_within_layers(graph: &mut StepGraph) {
    let layers = graph.layers();
    if layers.len() < 2 {
        // Nothing to optimize, assign sequential orders
        for (order, step) in graph.steps.iter_mut().enumerate() {
            step.order = order as u16;
        }
        return;
    }

    // Build position lookup: node_index -> position within its layer
    let mut positions: Vec<f64> = vec![0.0; graph.steps.len()];
    for layer in &layers {
        for (pos, &node_idx) in layer.iter().enumerate() {
            positions[node_idx] = pos as f64;
        }
    }

    // Build parent map: for each node, which nodes point to it?
    let mut parents: Vec<Vec<usize>> = vec![Vec::new(); graph.steps.len()];
    for &(from, to) in &graph.edges {
        parents[to].push(from);
    }

    // Build children map
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); graph.steps.len()];
    for &(from, to) in &graph.edges {
        children[from].push(to);
    }

    // Run barycenter iterations (forward + backward passes)
    let num_passes = 4;
    for pass in 0..num_passes {
        if pass % 2 == 0 {
            // Forward pass: order each layer based on parent positions
            for layer_idx in 1..layers.len() {
                let layer = &layers[layer_idx];
                let mut barycenters: Vec<(usize, f64)> = layer
                    .iter()
                    .map(|&node| {
                        let pars = &parents[node];
                        if pars.is_empty() {
                            (node, positions[node])
                        } else {
                            let avg = pars.iter().map(|&p| positions[p]).sum::<f64>()
                                / pars.len() as f64;
                            (node, avg)
                        }
                    })
                    .collect();
                barycenters.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
                for (new_pos, &(node, _)) in barycenters.iter().enumerate() {
                    positions[node] = new_pos as f64;
                }
            }
        } else {
            // Backward pass: order each layer based on children positions
            for layer_idx in (0..layers.len() - 1).rev() {
                let layer = &layers[layer_idx];
                let mut barycenters: Vec<(usize, f64)> = layer
                    .iter()
                    .map(|&node| {
                        let kids = &children[node];
                        if kids.is_empty() {
                            (node, positions[node])
                        } else {
                            let avg = kids.iter().map(|&c| positions[c]).sum::<f64>()
                                / kids.len() as f64;
                            (node, avg)
                        }
                    })
                    .collect();
                barycenters.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
                for (new_pos, &(node, _)) in barycenters.iter().enumerate() {
                    positions[node] = new_pos as f64;
                }
            }
        }
    }

    // Apply final positions as order
    for (i, step) in graph.steps.iter_mut().enumerate() {
        step.order = positions[i] as u16;
    }
}
```

**Step 3: Create graph/render.rs — ratatui widget stub**

Start with a placeholder that renders node boxes and edges using Unicode.

```rust
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Widget};

use barnum_types::StepName;
use std::collections::HashMap;

use super::StepGraph;
use crate::app::{StatusCounts, Viewport};
use crate::theme::{self, ZoomLevel};

/// Dimensions for graph node rendering
const NODE_WIDTH: u16 = 14;
const NODE_HEIGHT: u16 = 3;
const LAYER_GAP: u16 = 4; // Horizontal gap between layers
const NODE_GAP: u16 = 1;  // Vertical gap between nodes in same layer

pub struct GraphWidget<'a> {
    graph: &'a StepGraph,
    counts: &'a HashMap<StepName, StatusCounts>,
    selected: Option<&'a StepName>,
    viewport: &'a Viewport,
}

impl<'a> GraphWidget<'a> {
    pub fn new(
        graph: &'a StepGraph,
        counts: &'a HashMap<StepName, StatusCounts>,
        selected: Option<&'a StepName>,
        viewport: &'a Viewport,
    ) -> Self {
        Self { graph, counts, selected, viewport }
    }

    /// Calculate the position of a node in the virtual canvas
    fn node_position(&self, layer: u16, order: u16) -> (u16, u16) {
        let x = layer * (NODE_WIDTH + LAYER_GAP);
        let y = order * (NODE_HEIGHT + NODE_GAP);
        (x, y)
    }

    /// Render a single node box into the buffer
    fn render_node(
        &self,
        buf: &mut Buffer,
        area: Rect,
        node_idx: usize,
        vx: u16,
        vy: u16,
    ) {
        let step = &self.graph.steps[node_idx];
        let (nx, ny) = self.node_position(step.layer, step.order);

        // Check if node is visible in viewport
        let x = nx.saturating_sub(vx);
        let y = ny.saturating_sub(vy);
        if x >= area.width || y >= area.height {
            return;
        }

        let is_selected = self.selected.map_or(false, |s| s == &step.name);
        let border_style = if is_selected {
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        // Draw box borders
        let w = NODE_WIDTH.min(area.width - x);
        let h = NODE_HEIGHT.min(area.height - y);
        let node_rect = Rect::new(area.x + x, area.y + y, w, h);

        // Top border
        if h > 0 && w > 1 {
            let top = format!("+{}+", "-".repeat((w - 2) as usize));
            buf.set_string(node_rect.x, node_rect.y, &top, border_style);
        }
        // Bottom border
        if h > 2 && w > 1 {
            let bot = format!("+{}+", "-".repeat((w - 2) as usize));
            buf.set_string(node_rect.x, node_rect.y + h - 1, &bot, border_style);
        }
        // Side borders + content
        for row in 1..h.saturating_sub(1) {
            buf.set_string(node_rect.x, node_rect.y + row, "|", border_style);
            if w > 1 {
                buf.set_string(node_rect.x + w - 1, node_rect.y + row, "|", border_style);
            }
        }

        // Node name (centered on line 1)
        if h > 1 && w > 2 {
            let name = step.name.as_ref();
            let max_len = (w - 2) as usize;
            let display = if name.len() > max_len {
                &name[..max_len]
            } else {
                name
            };
            let pad = (max_len.saturating_sub(display.len())) / 2;
            let name_style = if is_selected {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            buf.set_string(
                node_rect.x + 1 + pad as u16,
                node_rect.y + 1,
                display,
                name_style,
            );
        }

        // Status badges (line 2 if space)
        if h > 2 && w > 2 {
            if let Some(counts) = self.counts.get(&step.name) {
                let mut badges = String::new();
                if counts.completed > 0 {
                    badges.push_str(&format!("{}{}", theme::ICON_COMPLETED, counts.completed));
                }
                if counts.in_flight > 0 {
                    if !badges.is_empty() { badges.push(' '); }
                    badges.push_str(&format!("{}{}", theme::ICON_IN_FLIGHT, counts.in_flight));
                }
                if counts.failed > 0 {
                    if !badges.is_empty() { badges.push(' '); }
                    badges.push_str(&format!("{}{}", theme::ICON_FAILED, counts.failed));
                }
                let max_len = (w - 2) as usize;
                let display = if badges.len() > max_len {
                    &badges[..max_len]
                } else {
                    &badges
                };
                buf.set_string(
                    node_rect.x + 1,
                    node_rect.y + 2,
                    display,
                    Style::default(),
                );
            }
        }
    }

    /// Render edges between nodes
    fn render_edges(&self, buf: &mut Buffer, area: Rect, vx: u16, vy: u16) {
        for &(from_idx, to_idx) in &self.graph.edges {
            let from = &self.graph.steps[from_idx];
            let to = &self.graph.steps[to_idx];

            let (fx, fy) = self.node_position(from.layer, from.order);
            let (tx, ty) = self.node_position(to.layer, to.order);

            // Arrow from right side of 'from' to left side of 'to'
            let start_x = fx + NODE_WIDTH;
            let start_y = fy + NODE_HEIGHT / 2;
            let end_x = tx;
            let end_y = ty + NODE_HEIGHT / 2;

            // Translate to viewport
            let sx = start_x.saturating_sub(vx);
            let sy = start_y.saturating_sub(vy);
            let ex = end_x.saturating_sub(vx);
            let ey = end_y.saturating_sub(vy);

            let edge_style = Style::default().fg(Color::DarkGray);

            // Simple horizontal line with arrow
            if sy == ey && sx < ex {
                for x in sx..ex.min(area.width) {
                    buf.set_string(area.x + x, area.y + sy.min(area.height - 1), "\u{2500}", edge_style);
                }
                if ex < area.width {
                    buf.set_string(area.x + ex, area.y + ey.min(area.height - 1), "\u{25B6}", edge_style);
                }
            } else if sx < ex {
                // Angled edge: go right, then up/down, then right
                let mid_x = sx + (ex - sx) / 2;
                // Horizontal segment from start
                for x in sx..mid_x.min(area.width) {
                    if sy < area.height {
                        buf.set_string(area.x + x, area.y + sy, "\u{2500}", edge_style);
                    }
                }
                // Vertical segment
                let (min_y, max_y) = if sy < ey { (sy, ey) } else { (ey, sy) };
                for y in min_y..=max_y.min(area.height - 1) {
                    if mid_x < area.width {
                        buf.set_string(area.x + mid_x, area.y + y, "\u{2502}", edge_style);
                    }
                }
                // Horizontal segment to end
                for x in mid_x..ex.min(area.width) {
                    if ey < area.height {
                        buf.set_string(area.x + x, area.y + ey, "\u{2500}", edge_style);
                    }
                }
                if ex < area.width && ey < area.height {
                    buf.set_string(area.x + ex, area.y + ey, "\u{25B6}", edge_style);
                }
            }
        }
    }
}

impl<'a> Widget for GraphWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let vx = self.viewport.scroll_x;
        let vy = self.viewport.scroll_y;

        // Render edges first (behind nodes)
        self.render_edges(buf, area, vx, vy);

        // Render nodes on top
        for idx in 0..self.graph.steps.len() {
            self.render_node(buf, area, idx, vx, vy);
        }
    }
}
```

**Step 4: Wire into main.rs**

Add `mod graph;` to main.rs.

**Step 5: Verify it builds**

Run: `cargo build -p barnum_tui`

**Step 6: Commit**

```bash
git add crates/barnum_tui/src/graph/ crates/barnum_tui/src/main.rs
git commit -m "feat(tui): add step graph — DAG construction, layout, and rendering"
```

---

## Task 5: Task List Panel

**Files:**
- Create: `crates/barnum_tui/src/task_list/mod.rs`
- Create: `crates/barnum_tui/src/task_list/render.rs`

**Dependencies:** Task 2

**Step 1: Create task_list/mod.rs**

Re-exports and any shared task list logic.

```rust
pub mod render;
```

**Step 2: Create task_list/render.rs**

```rust
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Row, Table, TableState, Widget, StatefulWidget};

use barnum_types::LogTaskId;
use std::time::Instant;

use crate::app::{AppState, TaskRecord};
use crate::theme::{self, TaskStatus};

pub struct TaskListWidget<'a> {
    tasks: &'a [LogTaskId],
    app: &'a AppState,
    focused: bool,
}

impl<'a> TaskListWidget<'a> {
    pub fn new(tasks: &'a [LogTaskId], app: &'a AppState, focused: bool) -> Self {
        Self { tasks, app, focused }
    }

    fn format_duration(record: &TaskRecord) -> String {
        let elapsed = record
            .completed_at
            .unwrap_or_else(Instant::now)
            .duration_since(record.submitted_at);
        let secs = elapsed.as_secs();
        if secs < 60 {
            format!("{secs}s")
        } else if secs < 3600 {
            format!("{}m{:02}s", secs / 60, secs % 60)
        } else {
            format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
        }
    }

    fn truncate_value(value: &serde_json::Value, max_len: usize) -> String {
        let s = value.to_string();
        if s.len() > max_len {
            format!("{}...", &s[..max_len.saturating_sub(3)])
        } else {
            s
        }
    }
}

impl<'a> TaskListWidget<'a> {
    pub fn render_with_state(self, area: Rect, buf: &mut Buffer, state: &mut TableState) {
        let border_style = if self.focused {
            theme::focused_border_style()
        } else {
            theme::unfocused_border_style()
        };

        let show_step = self.app.selected_step.is_none();

        let header_cells = if show_step {
            vec!["ID", "Status", "Step", "Duration", "Value"]
        } else {
            vec!["ID", "Status", "Duration", "Value"]
        };

        let header = Row::new(header_cells)
            .style(Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED));

        let rows: Vec<Row> = self
            .tasks
            .iter()
            .filter_map(|id| self.app.tasks.get(id))
            .map(|record| {
                let id_str = format!("t-{:02}", record.id.0);
                let status_span = format!("{} {}", record.status.icon(), record.status.label());
                let dur = Self::format_duration(record);
                let val = Self::truncate_value(&record.value, 40);

                let style = Style::default().fg(record.status.color());

                if show_step {
                    Row::new(vec![
                        id_str,
                        status_span,
                        record.step.to_string(),
                        dur,
                        val,
                    ])
                    .style(style)
                } else {
                    Row::new(vec![id_str, status_span, dur, val]).style(style)
                }
            })
            .collect();

        let widths = if show_step {
            vec![
                Constraint::Length(6),
                Constraint::Length(14),
                Constraint::Length(12),
                Constraint::Length(8),
                Constraint::Fill(1),
            ]
        } else {
            vec![
                Constraint::Length(6),
                Constraint::Length(14),
                Constraint::Length(8),
                Constraint::Fill(1),
            ]
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(if let Some(ref step) = self.app.selected_step {
                format!(" Tasks: {} ", step)
            } else {
                " Tasks: All ".to_string()
            });

        let table = Table::new(rows, widths)
            .header(header)
            .block(block)
            .highlight_style(theme::selected_style());

        StatefulWidget::render(table, area, buf, state);
    }
}
```

**Step 3: Wire into main.rs**

Add `mod task_list;` to main.rs.

**Step 4: Verify it builds**

Run: `cargo build -p barnum_tui`

**Step 5: Commit**

```bash
git add crates/barnum_tui/src/task_list/ crates/barnum_tui/src/main.rs
git commit -m "feat(tui): add task list panel — table widget with filtering and sorting"
```

---

## Task 6: Detail Pane

**Files:**
- Create: `crates/barnum_tui/src/detail/mod.rs`
- Create: `crates/barnum_tui/src/detail/render.rs`

**Dependencies:** Task 2

**Step 1: Create detail/mod.rs**

```rust
pub mod render;
```

**Step 2: Create detail/render.rs**

```rust
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget, Wrap};

use barnum_state::types::{FailureReason, TaskOutcome};
use barnum_types::LogTaskId;

use crate::app::{AppState, TaskRecord};
use crate::theme::{self, TaskStatus};

pub struct DetailWidget<'a> {
    app: &'a AppState,
    focused: bool,
}

impl<'a> DetailWidget<'a> {
    pub fn new(app: &'a AppState, focused: bool) -> Self {
        Self { app, focused }
    }

    fn build_lines(&self, record: &TaskRecord) -> Vec<Line<'a>> {
        let mut lines = Vec::new();

        // Header: ID, Step, Status
        lines.push(Line::from(vec![
            Span::styled("Task ", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("t-{:02}", record.id.0),
                Style::default().fg(Color::Cyan),
            ),
            Span::raw(" > "),
            Span::styled(record.step.to_string(), Style::default().fg(Color::White)),
            Span::raw(" > "),
            Span::styled(
                format!("{} {}", record.status.icon(), record.status.label()),
                Style::default().fg(record.status.color()),
            ),
        ]));

        // Duration
        let elapsed = record
            .completed_at
            .unwrap_or_else(std::time::Instant::now)
            .duration_since(record.submitted_at);
        lines.push(Line::from(format!("Duration: {:.1}s", elapsed.as_secs_f64())));

        // Origin
        let origin_str = match &record.origin {
            barnum_state::types::TaskOrigin::Initial => "initial".to_string(),
            barnum_state::types::TaskOrigin::Spawned => "spawned".to_string(),
            barnum_state::types::TaskOrigin::Retry { replaces } => {
                format!("retry (replaces t-{:02})", replaces.0)
            }
            barnum_state::types::TaskOrigin::Finally { finally_for } => {
                format!("finally (for t-{:02})", finally_for.0)
            }
        };
        lines.push(Line::from(format!("Origin: {origin_str}")));

        // Parent chain
        if let Some(parent_id) = record.parent_id {
            let chain = self.build_parent_chain(parent_id);
            lines.push(Line::from(format!("Parent chain: {chain}")));
        }

        // Children
        if !record.children.is_empty() {
            let children_str: Vec<String> = record
                .children
                .iter()
                .map(|id| {
                    let step = self
                        .app
                        .tasks
                        .get(id)
                        .map(|t| t.step.to_string())
                        .unwrap_or_else(|| "?".to_string());
                    format!("t-{:02} ({})", id.0, step)
                })
                .collect();
            lines.push(Line::from(format!("Children: {}", children_str.join(", "))));
        }

        // Outcome details
        if let Some(ref outcome) = record.outcome {
            lines.push(Line::raw(""));
            match outcome {
                TaskOutcome::Success(success) => {
                    if !success.spawned_task_ids.is_empty() {
                        let ids: Vec<String> = success
                            .spawned_task_ids
                            .iter()
                            .map(|id| format!("t-{:02}", id.0))
                            .collect();
                        lines.push(Line::from(format!("Spawned: {}", ids.join(", "))));
                    }
                }
                TaskOutcome::Failed(failed) => {
                    let reason = match &failed.reason {
                        FailureReason::Timeout => "Timeout".to_string(),
                        FailureReason::AgentLost => "Agent lost".to_string(),
                        FailureReason::InvalidResponse { message } => {
                            format!("Invalid response: {message}")
                        }
                    };
                    lines.push(Line::from(vec![
                        Span::styled("Failure: ", Style::default().fg(Color::Red)),
                        Span::raw(reason),
                    ]));
                    if let Some(retry_id) = failed.retry_task_id {
                        lines.push(Line::from(format!("Retried as: t-{:02}", retry_id.0)));
                    }
                }
            }
        }

        // Value (pretty-printed JSON)
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "Value:",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        let pretty = serde_json::to_string_pretty(&record.value).unwrap_or_default();
        for json_line in pretty.lines() {
            lines.push(Line::from(Span::styled(
                json_line.to_string(),
                Style::default().fg(Color::Gray),
            )));
        }

        lines
    }

    fn build_parent_chain(&self, start: LogTaskId) -> String {
        let mut chain = Vec::new();
        let mut current = Some(start);
        while let Some(id) = current {
            if let Some(task) = self.app.tasks.get(&id) {
                chain.push(format!("t-{:02} ({})", id.0, task.step));
                current = task.parent_id;
            } else {
                chain.push(format!("t-{:02} (?)", id.0));
                break;
            }
        }
        chain.join(" <- ")
    }
}

impl<'a> Widget for DetailWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let border_style = if self.focused {
            theme::focused_border_style()
        } else {
            theme::unfocused_border_style()
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(" Detail ");

        let inner = block.inner(area);
        block.render(area, buf);

        let lines = match self.app.selected_task {
            Some(task_id) => {
                if let Some(record) = self.app.tasks.get(&task_id) {
                    self.build_lines(record)
                } else {
                    vec![Line::raw("Task not found")]
                }
            }
            None => vec![Line::styled(
                "Select a task to view details",
                Style::default().fg(Color::DarkGray),
            )],
        };

        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
        paragraph.render(inner, buf);
    }
}
```

**Step 3: Wire into main.rs and verify**

Add `mod detail;` to main.rs. Run `cargo build -p barnum_tui`.

**Step 4: Commit**

```bash
git add crates/barnum_tui/src/detail/ crates/barnum_tui/src/main.rs
git commit -m "feat(tui): add detail pane — full task info, parent chain, JSON value"
```

---

## Task 7: Header & Footer

**Files:**
- Create: `crates/barnum_tui/src/header.rs`
- Create: `crates/barnum_tui/src/footer.rs`

**Dependencies:** Task 2

**Step 1: Create header.rs**

```rust
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::app::{AppState, RunStatus};
use crate::theme;

pub struct HeaderWidget<'a> {
    app: &'a AppState,
}

impl<'a> HeaderWidget<'a> {
    pub fn new(app: &'a AppState) -> Self {
        Self { app }
    }
}

impl<'a> Widget for HeaderWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let (status_icon, status_color) = match self.app.run_status {
            RunStatus::Running => (theme::ICON_IN_FLIGHT, theme::COLOR_IN_FLIGHT),
            RunStatus::Completed => (theme::ICON_COMPLETED, theme::COLOR_COMPLETED),
            RunStatus::Failed => (theme::ICON_FAILED, theme::COLOR_FAILED),
            RunStatus::Waiting => ("...", Color::DarkGray),
        };

        let total_tasks: u32 = self.app.step_counts.values().map(|c| c.total()).sum();

        let elapsed = self
            .app
            .start_time
            .map(|t| {
                let secs = t.elapsed().as_secs();
                format!("{:02}:{:02}", secs / 60, secs % 60)
            })
            .unwrap_or_else(|| "--:--".to_string());

        let line = Line::from(vec![
            Span::styled(
                " barnum-tui ",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("> ", Style::default().fg(Color::DarkGray)),
            Span::raw(&self.app.config_path),
            Span::raw("  "),
            Span::styled(
                format!("{status_icon} {}", match self.app.run_status {
                    RunStatus::Running => "Running",
                    RunStatus::Completed => "Completed",
                    RunStatus::Failed => "Failed",
                    RunStatus::Waiting => "Waiting",
                }),
                Style::default().fg(status_color),
            ),
            Span::raw(format!("  {total_tasks} tasks  {elapsed}")),
        ]);

        buf.set_style(area, theme::header_style());
        let y = area.y;
        buf.set_line(area.x, y, &line, area.width);
    }
}
```

**Step 2: Create footer.rs**

```rust
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::app::PanelFocus;

pub struct FooterWidget {
    pub focus: PanelFocus,
}

impl Widget for FooterWidget {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let common_keys = vec![
            ("Tab", "switch panel"),
            ("q", "quit"),
            ("?", "help"),
        ];

        let context_keys: Vec<(&str, &str)> = match self.focus {
            PanelFocus::Graph => vec![
                ("\u{2190}\u{2192}", "pan"),
                ("\u{2191}\u{2193}", "select"),
                ("Enter", "filter tasks"),
                ("+/-", "zoom"),
            ],
            PanelFocus::TaskList => vec![
                ("j/k", "navigate"),
                ("Enter", "select"),
                ("s", "sort"),
                ("f", "filter"),
                ("/", "search"),
            ],
            PanelFocus::Detail => vec![
                ("j/k", "scroll"),
                ("y", "copy value"),
            ],
        };

        let mut spans = Vec::new();
        for (key, desc) in context_keys.iter().chain(common_keys.iter()) {
            if !spans.is_empty() {
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled(
                *key,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                format!(": {desc}"),
                Style::default().fg(Color::DarkGray),
            ));
        }

        let line = Line::from(spans);
        buf.set_line(area.x + 1, area.y, &line, area.width.saturating_sub(1));
    }
}
```

**Step 3: Wire into main.rs and verify**

Add `mod header;` and `mod footer;` to main.rs. Run `cargo build -p barnum_tui`.

**Step 4: Commit**

```bash
git add crates/barnum_tui/src/header.rs crates/barnum_tui/src/footer.rs crates/barnum_tui/src/main.rs
git commit -m "feat(tui): add header and footer widgets"
```

---

## Task 8: Event Handling

**Files:**
- Create: `crates/barnum_tui/src/event.rs`

**Dependencies:** Tasks 3-7 (needs all widgets and AppState to exist)

**Step 1: Create event.rs**

```rust
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use std::time::Duration;

use crate::app::{AppState, PanelFocus, SortColumn};
use crate::theme::TaskStatus;

/// Result of processing an event
pub enum EventResult {
    Continue,
    Quit,
}

/// Poll for crossterm events with a timeout
pub fn poll_event(timeout: Duration) -> Option<Event> {
    if event::poll(timeout).unwrap_or(false) {
        event::read().ok()
    } else {
        None
    }
}

/// Handle a key event, updating AppState
pub fn handle_key(key: KeyEvent, app: &mut AppState) -> EventResult {
    // Global keys (regardless of focus)
    match key.code {
        KeyCode::Char('q') => return EventResult::Quit,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            return EventResult::Quit
        }
        KeyCode::Tab => {
            app.focus = app.focus.next();
            return EventResult::Continue;
        }
        KeyCode::BackTab => {
            app.focus = app.focus.prev();
            return EventResult::Continue;
        }
        KeyCode::Esc => {
            app.search_query = None;
            app.selected_task = None;
            return EventResult::Continue;
        }
        _ => {}
    }

    // Panel-specific keys
    match app.focus {
        PanelFocus::Graph => handle_graph_key(key, app),
        PanelFocus::TaskList => handle_task_list_key(key, app),
        PanelFocus::Detail => handle_detail_key(key, app),
    }

    EventResult::Continue
}

fn handle_graph_key(key: KeyEvent, app: &mut AppState) {
    match key.code {
        KeyCode::Left => app.graph_viewport.scroll_x = app.graph_viewport.scroll_x.saturating_sub(2),
        KeyCode::Right => app.graph_viewport.scroll_x += 2,
        KeyCode::Up => app.graph_viewport.scroll_y = app.graph_viewport.scroll_y.saturating_sub(1),
        KeyCode::Down => app.graph_viewport.scroll_y += 1,
        KeyCode::Char('+') | KeyCode::Char('=') => {
            app.graph_viewport.zoom = match app.graph_viewport.zoom {
                crate::app::ZoomLevel::Dot => crate::app::ZoomLevel::Compact,
                crate::app::ZoomLevel::Compact => crate::app::ZoomLevel::Full,
                crate::app::ZoomLevel::Full => crate::app::ZoomLevel::Full,
            };
        }
        KeyCode::Char('-') => {
            app.graph_viewport.zoom = match app.graph_viewport.zoom {
                crate::app::ZoomLevel::Full => crate::app::ZoomLevel::Compact,
                crate::app::ZoomLevel::Compact => crate::app::ZoomLevel::Dot,
                crate::app::ZoomLevel::Dot => crate::app::ZoomLevel::Dot,
            };
        }
        _ => {}
    }
}

fn handle_task_list_key(key: KeyEvent, app: &mut AppState) {
    let task_ids = app.visible_tasks();
    let len = task_ids.len();

    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            let i = app.task_list_state.selected().map(|i| (i + 1).min(len.saturating_sub(1))).unwrap_or(0);
            app.task_list_state.select(Some(i));
            app.selected_task = task_ids.get(i).copied();
        }
        KeyCode::Char('k') | KeyCode::Up => {
            let i = app.task_list_state.selected().map(|i| i.saturating_sub(1)).unwrap_or(0);
            app.task_list_state.select(Some(i));
            app.selected_task = task_ids.get(i).copied();
        }
        KeyCode::Char('g') => {
            app.task_list_state.select(Some(0));
            app.selected_task = task_ids.first().copied();
        }
        KeyCode::Char('G') => {
            let last = len.saturating_sub(1);
            app.task_list_state.select(Some(last));
            app.selected_task = task_ids.get(last).copied();
        }
        KeyCode::Char('s') => {
            app.sort_column = app.sort_column.next();
        }
        KeyCode::Char('S') => {
            app.sort_reversed = !app.sort_reversed;
        }
        KeyCode::Char('1') => toggle_filter(app, TaskStatus::Pending),
        KeyCode::Char('2') => toggle_filter(app, TaskStatus::InFlight),
        KeyCode::Char('3') => toggle_filter(app, TaskStatus::Completed),
        KeyCode::Char('4') => toggle_filter(app, TaskStatus::Failed),
        KeyCode::Char('5') => toggle_filter(app, TaskStatus::Retried),
        KeyCode::Enter => {
            // Select the task under cursor
            if let Some(i) = app.task_list_state.selected() {
                app.selected_task = task_ids.get(i).copied();
                app.focus = PanelFocus::Detail;
            }
        }
        _ => {}
    }
}

fn handle_detail_key(key: KeyEvent, app: &mut AppState) {
    match key.code {
        // Scroll would be handled via Paragraph scroll state — for now, noop
        KeyCode::Char('y') => {
            // Copy value to clipboard via OSC 52 (works in most terminals)
            if let Some(task_id) = app.selected_task {
                if let Some(record) = app.tasks.get(&task_id) {
                    let json = serde_json::to_string_pretty(&record.value).unwrap_or_default();
                    // OSC 52 clipboard sequence
                    use base64::Engine;
                    let encoded = base64::engine::general_purpose::STANDARD.encode(json.as_bytes());
                    print!("\x1b]52;c;{encoded}\x07");
                }
            }
        }
        _ => {}
    }
}

fn toggle_filter(app: &mut AppState, status: TaskStatus) {
    if app.status_filters.contains(&status) {
        app.status_filters.remove(&status);
    } else {
        app.status_filters.insert(status);
    }
}
```

**Note:** The clipboard feature requires `base64` as a dependency. Add it to Cargo.toml or defer clipboard support.

**Step 2: Wire into main.rs and verify**

Add `mod event;` to main.rs. Run `cargo build -p barnum_tui`.

**Step 3: Commit**

```bash
git add crates/barnum_tui/src/event.rs crates/barnum_tui/src/main.rs
git commit -m "feat(tui): add event handling — keybindings for all panels"
```

---

## Task 9: Main Loop Integration

**Files:**
- Modify: `crates/barnum_tui/src/main.rs`

**Dependencies:** Tasks 3-8 (all components)

**Step 1: Rewrite main.rs with the full TUI loop**

```rust
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use crossterm::event::Event;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::execute;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::Terminal;

use barnum_config::config::ConfigFile;
use barnum_state::types::StateLogEntry;

mod app;
mod detail;
mod event;
mod footer;
mod graph;
mod header;
mod log_watcher;
mod task_list;
mod theme;

use app::AppState;
use detail::render::DetailWidget;
use footer::FooterWidget;
use graph::render::GraphWidget;
use graph::StepGraph;
use header::HeaderWidget;
use log_watcher::{LogEvent, LogWatcher};
use task_list::render::TaskListWidget;

#[derive(Parser)]
#[command(name = "barnum-tui", about = "Terminal dashboard for barnum workflows")]
struct Cli {
    /// Path to the workflow config file (JSON/JSONC)
    #[arg(long)]
    config: PathBuf,

    /// Path to the NDJSON state log file
    #[arg(long)]
    state_log: PathBuf,

    /// Replay mode: read log from beginning instead of tailing
    #[arg(long, default_value_t = false)]
    replay: bool,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Parse config to build step graph
    let config_text = std::fs::read_to_string(&cli.config)?;
    let config_file: ConfigFile = json5::from_str(&config_text)?;
    config_file.validate()?;
    let step_graph = StepGraph::from_config(&config_file);

    // Initialize app state
    let config_name = cli
        .config
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| cli.config.display().to_string());
    let mut app = AppState::new(config_name);

    // Set up log watcher
    let mut watcher = LogWatcher::new(&cli.state_log, cli.replay)?;

    // Set up terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, &mut app, &step_graph, &mut watcher);

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut AppState,
    step_graph: &StepGraph,
    watcher: &mut LogWatcher,
) -> anyhow::Result<()> {
    let tick_rate = Duration::from_millis(100);

    loop {
        // Process new log events
        for log_event in watcher.poll() {
            match log_event {
                LogEvent::Entry(entry) => apply_log_entry(app, entry),
                LogEvent::Error(e) => {
                    // TODO: show errors in UI
                    eprintln!("Log error: {e}");
                }
            }
        }

        // Render
        terminal.draw(|frame| {
            let size = frame.area();

            // Main layout: header (1) + body + footer (1)
            let outer = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),     // header
                    Constraint::Min(10),       // body
                    Constraint::Length(1),      // footer
                ])
                .split(size);

            // Header
            frame.render_widget(HeaderWidget::new(app), outer[0]);

            // Body: graph (left 35%) + task list (right 65%)
            // Then detail pane below
            let body = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(8),        // graph + task list
                    Constraint::Length(8),      // detail
                ])
                .split(outer[1]);

            let main_panels = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(35), // graph
                    Constraint::Percentage(65), // task list
                ])
                .split(body[0]);

            // Step graph
            frame.render_widget(
                GraphWidget::new(
                    step_graph,
                    &app.step_counts,
                    app.selected_step.as_ref(),
                    &app.graph_viewport,
                ),
                main_panels[0],
            );

            // Task list (stateful)
            let visible = app.visible_tasks();
            let focused = app.focus == app::PanelFocus::TaskList;
            let widget = TaskListWidget::new(&visible, app, focused);
            let mut table_state = app.task_list_state.clone();
            widget.render_with_state(main_panels[1], frame.buffer_mut(), &mut table_state);
            app.task_list_state = table_state;

            // Detail pane
            frame.render_widget(
                DetailWidget::new(app, app.focus == app::PanelFocus::Detail),
                body[1],
            );

            // Footer
            frame.render_widget(FooterWidget { focus: app.focus }, outer[2]);
        })?;

        // Handle input
        if let Some(Event::Key(key)) = event::poll_event(tick_rate) {
            match event::handle_key(key, app) {
                event::EventResult::Quit => break,
                event::EventResult::Continue => {}
            }
        }
    }

    Ok(())
}

fn apply_log_entry(app: &mut AppState, entry: StateLogEntry) {
    match entry {
        StateLogEntry::Config(_) => {
            // Config entry is informational, step graph already built from file
        }
        StateLogEntry::TaskSubmitted(sub) => {
            app.apply_submitted(
                sub.task_id,
                sub.step,
                sub.value.0,
                sub.parent_id,
                sub.origin,
            );
        }
        StateLogEntry::TaskCompleted(comp) => {
            app.apply_completed(comp.task_id, comp.outcome);
        }
    }
}
```

**Step 2: Verify it builds**

Run: `cargo build -p barnum_tui`
Expected: Compiles. There may be minor type mismatches to resolve (e.g., `StepInputValue` unwrapping). Fix any compilation errors.

**Step 3: Commit**

```bash
git add crates/barnum_tui/src/main.rs
git commit -m "feat(tui): integrate main loop — full TUI with all panels"
```

---

## Task 10: CLI Shim

**Files:**
- Modify: `crates/barnum_cli/src/main.rs`

**Dependencies:** Task 9 (TUI binary must exist)

**Step 1: Read barnum_cli main.rs to understand current structure**

Read `crates/barnum_cli/src/main.rs` to find the right place to add the `tui` subcommand.

**Step 2: Add Tui subcommand**

Add a `Tui` variant to the `Command` enum. It should collect remaining args and delegate:

```rust
/// Launch the TUI dashboard (requires barnum-tui binary)
Tui {
    /// Arguments passed through to barnum-tui
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
},
```

In the match arm:

```rust
Command::Tui { args } => {
    let status = std::process::Command::new("barnum-tui")
        .args(&args)
        .status()
        .context("Failed to run barnum-tui. Is it installed? Try: cargo install --path crates/barnum_tui")?;
    std::process::exit(status.code().unwrap_or(1));
}
```

**Step 3: Verify both binaries build**

Run: `cargo build -p barnum_cli -p barnum_tui`

**Step 4: Commit**

```bash
git add crates/barnum_cli/src/main.rs
git commit -m "feat(cli): add 'barnum tui' shim subcommand"
```

---

## Post-Implementation

After all tasks are complete:

1. **Manual test:** Run a demo workflow with `--state-log /tmp/test.log`, then `barnum-tui --config <config> --state-log /tmp/test.log --replay` to verify the TUI renders correctly.
2. **Fix compilation errors:** The plan provides approximate code — expect type mismatches between `barnum_state` types and the TUI's internal types that need resolving.
3. **Polish:** Adjust column widths, colors, edge rendering based on visual testing.
