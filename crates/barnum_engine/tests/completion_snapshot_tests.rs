//! Completion snapshot tests.
//!
//! Each JSON file in `tests/completion/` specifies a workflow config, input,
//! and a sequence of completions (task results). The test replays the
//! completions and builds a trace of engine state at every step.

#![allow(clippy::unwrap_used)]

use barnum_ast::Config;
use barnum_ast::flat::flatten;
use barnum_engine::{Engine, TaskId};
use serde::Deserialize;
use serde_json::Value;
use std::fmt::Write;

/// A single task completion event in the fixture.
#[derive(Deserialize)]
struct Completion {
    task_id: u32,
    value: Value,
}

/// Test case deserialized from a JSON file.
#[derive(Deserialize)]
struct TestCase {
    config: Config,
    input: Value,
    completions: Vec<Completion>,
}

#[test]
fn completion_snapshots() {
    insta::glob!("completion/*.json", |path| {
        let contents = std::fs::read_to_string(path).unwrap();
        let test_case: TestCase = serde_json::from_str(&contents).unwrap();

        let flat_config = flatten(test_case.config).unwrap();
        let mut engine = Engine::new(flat_config);
        let root = engine.workflow_root();

        let mut trace = String::new();

        // Initial advance
        engine.advance(root, test_case.input, None).unwrap();
        let dispatches = engine.take_pending_dispatches();
        writeln!(trace, "--- After advance ---").unwrap();
        writeln!(trace, "{engine:#?}").unwrap();
        writeln!(trace, "Pending: {dispatches:#?}").unwrap();

        // Replay completions
        for completion in &test_case.completions {
            let task_id = TaskId(completion.task_id);
            let result = engine.complete(task_id, completion.value.clone()).unwrap();
            let dispatches = engine.take_pending_dispatches();

            writeln!(trace).unwrap();
            writeln!(
                trace,
                "--- Completion: task_id={}, value={} ---",
                completion.task_id, completion.value
            )
            .unwrap();
            writeln!(trace, "Result: {result:?}").unwrap();
            writeln!(trace, "{engine:#?}").unwrap();
            writeln!(trace, "Pending: {dispatches:#?}").unwrap();
        }

        insta::assert_snapshot!(trace);
    });
}
