//! Completion snapshot tests.
//!
//! Each JSON file in `tests/completion/` specifies a workflow config, input,
//! and a sequence of completions (task results). The test replays the
//! completions and builds a trace of engine state at every step.

#![allow(clippy::unwrap_used)]

use barnum_ast::Config;
use barnum_ast::flat::flatten;
use barnum_engine::advance::advance;
use barnum_engine::complete::complete;
use barnum_engine::{CompletionEvent, DispatchEvent, TaskId, WorkflowState};
use serde::Deserialize;
use serde_json::Value;
use std::fmt::Write;

/// Drain all pending dispatches into a Vec (for snapshot traces).
fn drain_pending_dispatches(engine: &mut WorkflowState) -> Vec<DispatchEvent> {
    let mut dispatches = Vec::new();
    while let Some(dispatch_event) = engine.pop_pending_dispatch() {
        dispatches.push(dispatch_event);
    }
    dispatches
}

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
        let mut engine = WorkflowState::new(flat_config);
        let root = engine.workflow_root();

        let mut trace = String::new();

        // Initial advance
        advance(&mut engine, root, test_case.input, None).unwrap();
        let dispatches = drain_pending_dispatches(&mut engine);
        writeln!(trace, "--- After advance ---").unwrap();
        writeln!(trace, "{engine:#?}").unwrap();
        writeln!(trace, "Pending: {dispatches:#?}").unwrap();

        // Replay completions
        for completion in &test_case.completions {
            let task_id = TaskId(completion.task_id);
            let completion_event = CompletionEvent {
                task_id,
                value: completion.value.clone(),
            };
            let result = complete(&mut engine, completion_event).unwrap();
            let dispatches = drain_pending_dispatches(&mut engine);

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
