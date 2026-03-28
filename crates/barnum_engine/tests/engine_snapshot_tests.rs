//! Snapshot tests for the engine's advance function.
//!
//! Each JSON file in `tests/advance/` specifies a workflow config and an input.
//! The harness flattens the config, creates an engine, starts it, and snapshots
//! the resulting dispatches and frame tree.

#![allow(clippy::unwrap_used)]

use barnum_ast::Config;
use barnum_ast::flat::flatten;
use barnum_engine::Engine;
use barnum_engine::test_support::AdvanceSnapshot;
use serde::Deserialize;
use serde_json::Value;

/// Test case deserialized from a JSON file.
#[derive(Deserialize)]
struct TestCase {
    config: Config,
    input: Value,
}

fn run_advance(test_case: TestCase) -> AdvanceSnapshot {
    let flat_config = flatten(test_case.config).unwrap();
    let mut engine = Engine::new(flat_config);
    engine.start(test_case.input).unwrap();

    let dispatches = engine.take_pending_dispatches();
    engine.snapshot(&dispatches)
}

#[test]
fn advance_snapshots() {
    insta::glob!("advance/*.json", |path| {
        let contents = std::fs::read_to_string(path).unwrap();
        let test_case: TestCase = serde_json::from_str(&contents).unwrap();
        let snapshot = run_advance(test_case);
        insta::assert_json_snapshot!(snapshot);
    });
}
