# StepGraph

Implement the step graph: DAG construction from config, topological layer assignment, barycenter ordering, and the ratatui graph rendering widget.

## Input

```json
{"workspace_root": "/path/to/barnum-worktree"}
```

## Context

The `barnum_tui` crate exists with `app.rs` and `theme.rs`. You're building the left panel of the TUI — a visual DAG of workflow steps.

Read these files:
- `{workspace_root}/crates/barnum_config/src/config.rs` — `ConfigFile`, `StepFile` (has `name: StepName` and `next: Vec<StepName>`)
- `{workspace_root}/crates/barnum_tui/src/app.rs` — `StatusCounts`, `Viewport`, `ZoomLevel`
- `{workspace_root}/crates/barnum_tui/src/theme.rs` — colors, icons, styles

## Instructions

### 1. Create `crates/barnum_tui/src/graph/mod.rs` — DAG construction

**StepGraph struct:**
- `steps: Vec<StepNode>` — all nodes
- `edges: Vec<(usize, usize)>` — (from_index, to_index)
- `index_by_name: HashMap<StepName, usize>`

**StepNode struct:**
- `name: StepName`
- `next: Vec<StepName>`
- `layer: u16` — assigned by layout
- `order: u16` — position within layer

**`StepGraph::from_config(config: &ConfigFile) -> Self`:**
1. Iterate `config.steps`, create a `StepNode` per step, build `index_by_name`
2. Build edges from each step's `next` field
3. Call `layout::assign_layers()` and `layout::order_within_layers()`
4. Return the graph

**Helper methods:** `get(name)`, `layers() -> Vec<Vec<usize>>` (steps grouped by layer, sorted by order), `step_count()`

### 2. Create `crates/barnum_tui/src/graph/layout.rs` — layer assignment + ordering

**`assign_layers(graph: &mut StepGraph)`:**
- Longest-path algorithm via Kahn's topological sort
- Source nodes (in-degree 0) get layer 0
- Each child gets `max(parent_layers) + 1`

**`order_within_layers(graph: &mut StepGraph)`:**
- Barycenter heuristic: 4 passes alternating forward/backward
- Forward: order each layer's nodes by average position of their parents in the previous layer
- Backward: order by average position of children in the next layer
- Reduces edge crossings in the rendered graph

### 3. Create `crates/barnum_tui/src/graph/render.rs` — ratatui Widget

**GraphWidget** — implements `ratatui::widgets::Widget`:
- Takes: `&StepGraph`, `&HashMap<StepName, StatusCounts>`, `selected: Option<&StepName>`, `&Viewport`
- **Node rendering:** Each node is a box (14 wide x 3 tall) with name on line 1 and status badge counts on line 2. Selected node has cyan bold border. Node positions are computed from layer/order.
- **Edge rendering:** Unicode box-drawing characters (`─`, `│`, `▶`). Straight horizontal for same-row edges, L-shaped for different rows (horizontal → vertical → horizontal).
- **Viewport:** All positions offset by `viewport.scroll_x/scroll_y`. Nodes outside the visible area are skipped.
- Render edges first (behind), then nodes on top.

### 4. Wire into main.rs

Add `mod graph;` to main.rs.

### 5. Verify

Run `cargo build -p barnum_tui`. Must compile.

### 6. Commit

```bash
git add crates/barnum_tui/src/graph/ crates/barnum_tui/src/main.rs
git commit -m "feat(tui): add step graph — DAG construction, layout, and rendering"
```

## Output

```json
[{"kind": "EventHandling", "value": {"workspace_root": "<same>"}}]
```
