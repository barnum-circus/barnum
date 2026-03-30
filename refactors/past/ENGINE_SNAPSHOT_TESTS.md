# Engine Snapshot Tests

Data-driven snapshot tests for the engine. Each test case is a JSON file in a directory. The test harness parses the JSON, runs the engine, and compares the output against auto-maintained snapshots.

**Depends on:** FRAME_STORAGE_AND_ADVANCE.md (advance milestone)

**Scope:** Test infrastructure for the advance milestone. Extended in the completion milestone to cover the full advance/complete cycle.

## Why snapshots

The unit tests in FRAME_STORAGE_AND_ADVANCE.md manually assert dispatch counts, handler identities, and values. This works for 10 tests but doesn't scale:

1. **Tedious to maintain.** Adding a test means writing a new function with manual assertions. Changing a type means updating every assertion.
2. **Incomplete coverage.** Manual tests check specific properties (dispatch count, handler id) but miss structural details (frame tree shape, parent relationships, dispatch ordering).
3. **Can't see the full picture.** A test that asserts `dispatches.len() == 3` tells you the count is right but not *what* the 3 dispatches are.

Snapshot tests capture the full output and diff it. Adding a test case means writing a JSON input file and running `cargo insta review` to approve the snapshot. The snapshot shows everything — every dispatch, every frame, the full state.

## Library: insta

[`insta`](https://insta.rs) is the standard Rust snapshot testing library. Features we use:

- **`assert_json_snapshot!`**: Serialize a value to JSON, compare against a stored `.snap` file.
- **`glob!`**: Run a test function over every file matching a glob pattern. Each file gets its own named snapshot.
- **`cargo insta review`**: Interactive TUI for reviewing and approving snapshot changes.
- **Redactions**: Replace volatile values (like interned string keys) with stable placeholders.

Add to `barnum_event_loop/Cargo.toml`:
```toml
[dev-dependencies]
insta = { version = "1", features = ["json", "glob", "redactions"] }
```

## Test case format

Each test case is a JSON file:

```json
{
  "name": "all_three_invokes",
  "config": {
    "workflow": {
      "All": {
        "actions": [
          { "Invoke": { "handler": { "TypeScript": { "module": "./a.ts", "func": "a" } } } },
          { "Invoke": { "handler": { "TypeScript": { "module": "./b.ts", "func": "b" } } } },
          { "Invoke": { "handler": { "TypeScript": { "module": "./c.ts", "func": "c" } } } }
        ]
      }
    },
    "steps": {}
  },
  "input": { "shared": true }
}
```

Fields:
- **`name`**: Human-readable name for the snapshot file. Optional — defaults to the filename.
- **`config`**: A `Config` (tree AST) serialized as JSON. Uses the existing serde `Serialize`/`Deserialize` impls on `Config`, `Action`, `HandlerKind`, etc.
- **`input`**: The `Value` passed to `engine.start()`.

The test harness:
1. Deserializes `config` into `Config`
2. Calls `flatten(config)` to get `FlatConfig`
3. Creates `Engine::new(flat_config)`
4. Calls `engine.start(input)`
5. Snapshots the result

## What gets snapshotted

### Advance milestone

After `start()`, snapshot:

```json
{
  "dispatches": [
    {
      "handler": { "TypeScript": { "module": "./a.ts", "func": "a" } },
      "value": { "shared": true }
    },
    {
      "handler": { "TypeScript": { "module": "./b.ts", "func": "b" } },
      "value": { "shared": true }
    }
  ],
  "frame_tree": {
    "Root": {
      "children": [
        {
          "All": {
            "results": [null, null],
            "children": [
              { "Invoke": {} },
              { "Invoke": {} }
            ]
          }
        }
      ]
    }
  }
}
```

Two sections:
- **`dispatches`**: The pending dispatches, with `handler_id` resolved to the full `HandlerKind`. Ordered by emission order.
- **`frame_tree`**: The frame slab rendered as a tree (walking parent references). Shows frame kinds, structural state (results vecs, rest ActionIds), and nesting.

### Completion milestone (future extension)

Add a `"completions"` field to test cases — a sequence of `(task_index, TaskResult)` pairs applied after the initial advance:

```json
{
  "config": { ... },
  "input": { ... },
  "completions": [
    { "dispatch_index": 0, "result": { "Success": { "value": "a_result" } } },
    { "dispatch_index": 1, "result": { "Success": { "value": "b_result" } } }
  ]
}
```

`dispatch_index` references the Nth dispatch from the initial `take_pending_dispatches()` call. The harness feeds each completion to `on_task_completed` in order, taking snapshots at each step (or just at the end).

## Snapshot output type

A serializable struct for the snapshot:

```rust
#[derive(Serialize)]
struct AdvanceSnapshot {
    dispatches: Vec<SnapshotDispatch>,
    frame_tree: serde_json::Value,
}

#[derive(Serialize)]
struct SnapshotDispatch {
    handler: HandlerKind,
    value: Value,
}
```

`frame_tree` is a `serde_json::Value` built by walking the slab and reconstructing the tree structure from parent pointers. This avoids exposing internal types (FrameId, ActionId raw values) in the snapshot — those are implementation details that would make snapshots brittle.

### Frame tree rendering

Walk the slab to build a JSON tree:

```rust
fn render_frame_tree(engine: &Engine) -> serde_json::Value {
    // Find the Root frame (parent: None)
    let root_key = engine.frames.iter()
        .find(|(_, f)| matches!(f.kind, FrameKind::Root))
        .map(|(k, _)| k)
        .expect("no root frame");

    render_frame(engine, FrameId(root_key))
}

fn render_frame(engine: &Engine, frame_id: FrameId) -> serde_json::Value {
    let frame = &engine.frames[frame_id.0];
    let children = find_children(engine, frame_id);

    match &frame.kind {
        FrameKind::Root => json!({
            "kind": "Root",
            "children": children.into_iter()
                .map(|c| render_frame(engine, c))
                .collect::<Vec<_>>()
        }),
        FrameKind::Invoke => json!({ "kind": "Invoke" }),
        FrameKind::Chain { rest } => json!({
            "kind": "Chain",
            "rest_action_id": rest.0,
            "child": children.first().map(|c| render_frame(engine, *c))
        }),
        FrameKind::All { results } => json!({
            "kind": "All",
            "results": results,
            "children": children.into_iter()
                .map(|c| render_frame(engine, c))
                .collect::<Vec<_>>()
        }),
        // ... etc
    }
}

fn find_children(engine: &Engine, parent_id: FrameId) -> Vec<FrameId> {
    engine.frames.iter()
        .filter(|(_, f)| f.parent.map(|p| p.frame_id()) == Some(parent_id))
        .map(|(k, _)| FrameId(k))
        .collect()
}
```

This is test-only code — the engine doesn't need tree rendering for production.

### Redactions

Interned strings (module paths, function names) serialize as their string content, not as internal IDs. `HandlerKind` already serializes nicely via serde. No redactions needed for the initial version.

If ActionIds or FrameIds appear in snapshots, redact them — they're unstable across runs if the slab or flattener changes ordering:

```rust
insta::assert_json_snapshot!(snapshot, {
    ".frame_tree..rest_action_id" => "[action_id]",
});
```

But ideally, the snapshot format avoids raw IDs entirely by using structural representation (tree nesting) rather than ID references.

## Directory layout

```
crates/barnum_event_loop/
  src/
    engine.rs
  tests/
    snapshots/           ← insta snapshot files (.snap), auto-generated
    advance/             ← test case JSON files
      single_invoke.json
      chain_two_steps.json
      all_three.json
      foreach_array.json
      foreach_empty.json
      branch_matching.json
      loop_body.json
      attempt_child.json
      step_named.json
      nested_chain_in_all.json
      deep_chain.json
      all_empty.json
      foreach_empty_in_chain.json
    engine_snapshot_tests.rs  ← test harness
```

`tests/advance/` contains the JSON test cases. `tests/snapshots/` is where insta stores the `.snap` files (auto-generated, checked in). `engine_snapshot_tests.rs` is an integration test file.

## Test harness

```rust
// crates/barnum_event_loop/tests/engine_snapshot_tests.rs

use barnum_ast::flat::flatten;
use barnum_ast::Config;
use barnum_event_loop::engine::Engine;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct TestCase {
    #[serde(default)]
    name: Option<String>,
    config: Config,
    input: Value,
}

#[test]
fn advance_snapshots() {
    insta::glob!("advance/*.json", |path| {
        let contents = std::fs::read_to_string(path).unwrap();
        let test_case: TestCase = serde_json::from_str(&contents).unwrap();

        let flat_config = flatten(test_case.config).expect("flatten failed");
        let mut engine = Engine::new(flat_config);
        engine.start(test_case.input);

        let dispatches = engine.take_pending_dispatches();
        let snapshot = build_snapshot(&engine, &dispatches);

        insta::assert_json_snapshot!(snapshot);
    });
}
```

The `glob!` macro iterates over every `.json` file in `advance/`, runs the closure, and names the snapshot after the file. Adding a test case = adding a JSON file + running `cargo insta review`.

## Flat table snapshot (optional)

For debugging, it may be useful to also snapshot the flat table itself — the output of `flatten()`. This verifies the flattener and gives context for the frame tree.

```json
{
  "flat_table": [
    { "action_id": 0, "kind": "Chain", "rest": 2 },
    { "action_id": 1, "kind": "Invoke", "handler": "..." },
    { "action_id": 2, "kind": "Invoke", "handler": "..." }
  ],
  "dispatches": [ ... ],
  "frame_tree": { ... }
}
```

This makes snapshots larger but more self-contained. The reviewer can see the flat table layout and understand why the frame tree looks the way it does.

## Workflow

1. Write a JSON test case in `tests/advance/`
2. Run `cargo test` — test fails because no snapshot exists
3. Run `cargo insta review` — see the generated snapshot, approve it
4. Snapshot file is created in `tests/snapshots/`
5. Commit both the JSON test case and the `.snap` file

When the engine changes behavior:
1. Run `cargo test` — snapshot tests fail (output differs)
2. Run `cargo insta review` — see the diff, approve or reject
3. Commit updated `.snap` files

## Migration from unit tests

The existing unit tests (in FRAME_STORAGE_AND_ADVANCE.md) can coexist with snapshot tests. They test different things:
- Unit tests: specific behavioral properties ("dispatch count is 1", "handler is a.ts")
- Snapshot tests: full structural output ("here's everything that happened")

Keep both. Unit tests catch regressions with clear error messages ("expected 1 dispatch, got 2"). Snapshot tests catch structural drift and serve as documentation ("this is what chain(a, b) produces").
