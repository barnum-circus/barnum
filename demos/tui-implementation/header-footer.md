# HeaderFooter

Implement the header bar (config name, run status, task count, elapsed time) and footer (contextual keybinding hints).

## Input

```json
{"workspace_root": "/path/to/barnum-worktree"}
```

## Context

Read:
- `{workspace_root}/crates/barnum_tui/src/app.rs` — AppState, RunStatus, PanelFocus, StatusCounts
- `{workspace_root}/crates/barnum_tui/src/theme.rs` — icons, colors, header_style()

## Instructions

### 1. Create `crates/barnum_tui/src/header.rs`

**HeaderWidget** — implements Widget, renders a single line:

```
 barnum-tui > workflow.jsonc  ● Running  12 tasks  01:23
```

Components:
- "barnum-tui" — bold white
- Config path from `app.config_path`
- Run status icon + label, colored by status (yellow running, green completed, red failed, gray waiting)
- Total task count (sum of all step_counts)
- Elapsed time formatted as MM:SS from `app.start_time`

Use `theme::header_style()` as the background for the entire line.

### 2. Create `crates/barnum_tui/src/footer.rs`

**FooterWidget** — renders contextual keybinding hints based on current `PanelFocus`:

**Graph focused:** `←→: pan  ↑↓: select  Enter: filter tasks  +/-: zoom  Tab: switch panel  q: quit  ?: help`

**TaskList focused:** `j/k: navigate  Enter: select  s: sort  f: filter  /: search  Tab: switch panel  q: quit  ?: help`

**Detail focused:** `j/k: scroll  y: copy value  Tab: switch panel  q: quit  ?: help`

Format: key in cyan bold, description in dark gray, separated by double spaces.

### 3. Wire into main.rs

Add `mod header;` and `mod footer;` to main.rs.

### 4. Verify

Run `cargo build -p barnum_tui`. Must compile.

### 5. Commit

```bash
git add crates/barnum_tui/src/header.rs crates/barnum_tui/src/footer.rs crates/barnum_tui/src/main.rs
git commit -m "feat(tui): add header and footer widgets"
```

## Output

```json
[{"kind": "EventHandling", "value": {"workspace_root": "<same>"}}]
```
