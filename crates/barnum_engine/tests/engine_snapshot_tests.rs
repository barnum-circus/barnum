//! Snapshot tests for the engine and the flatten function.
//!
//! Each JSON file in `tests/advance/` specifies a workflow config and an input.
//! - `advance_snapshots`: flattens, starts the engine, debug-snapshots the engine
//!   (including frames and pending dispatches).
//! - `flatten_snapshots`: flattens the config and debug-snapshots the `FlatConfig`.

#![allow(clippy::unwrap_used)]

use barnum_ast::Config;
use barnum_ast::flat::flatten;
use barnum_engine::Engine;
use serde::Deserialize;
use serde_json::Value;

/// Test case deserialized from a JSON file.
#[derive(Deserialize)]
struct TestCase {
    config: Config,
    input: Value,
}

#[test]
fn advance_snapshots() {
    insta::glob!("advance/*.json", |path| {
        let contents = std::fs::read_to_string(path).unwrap();
        let test_case: TestCase = serde_json::from_str(&contents).unwrap();
        let flat_config = flatten(test_case.config).unwrap();
        let mut engine = Engine::new(flat_config);
        engine.start(test_case.input).unwrap();
        insta::assert_debug_snapshot!(engine);
    });
}

#[test]
fn flatten_snapshots() {
    insta::glob!("advance/*.json", |path| {
        let contents = std::fs::read_to_string(path).unwrap();
        let test_case: TestCase = serde_json::from_str(&contents).unwrap();
        let flat_config = flatten(test_case.config).unwrap();
        insta::assert_debug_snapshot!(flat_config);
    });
}
