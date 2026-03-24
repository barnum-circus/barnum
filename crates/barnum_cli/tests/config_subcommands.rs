//! Tests for `barnum config` subcommands.
//!
//! Tests validate, docs, graph, and schema subcommands with various configs.

#![expect(clippy::expect_used)]
#![expect(clippy::unwrap_used)]

mod common;

use common::BarnumRunner;
use rstest::rstest;
use std::time::Duration;

const BASH_ACTION: &str = r#"{"kind": "Bash", "script": "echo '[]'"}"#;

/// Build a step JSON string with the required action field.
fn step(name: &str, next: &[&str]) -> String {
    let next_json: Vec<String> = next.iter().map(|n| format!("\"{n}\"")).collect();
    format!(
        r#"{{"name": "{name}", "action": {BASH_ACTION}, "next": [{}]}}"#,
        next_json.join(", ")
    )
}

// =============================================================================
// barnum config schema
// =============================================================================

#[rstest]
#[timeout(Duration::from_secs(5))]
fn schema_outputs_valid_json() {
    let barnum = BarnumRunner::new();
    let result = barnum.schema_json().expect("schema");

    assert!(result.status.success(), "Schema should succeed");

    let stdout = String::from_utf8_lossy(&result.stdout);
    let schema: serde_json::Value = serde_json::from_str(&stdout).expect("Should be valid JSON");

    // Verify key schema properties
    assert_eq!(schema["$schema"], "http://json-schema.org/draft-07/schema#");
    assert_eq!(schema["title"], "ConfigFile");
    assert_eq!(schema["type"], "object");
}

#[rstest]
#[timeout(Duration::from_secs(5))]
fn schema_has_required_steps_field() {
    let barnum = BarnumRunner::new();
    let result = barnum.schema_json().expect("schema");
    let stdout = String::from_utf8_lossy(&result.stdout);
    let schema: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    let required = schema["required"]
        .as_array()
        .expect("required should be array");
    assert!(
        required.iter().any(|v| v == "steps"),
        "steps should be required"
    );
}

#[rstest]
#[timeout(Duration::from_secs(5))]
fn schema_defines_step_type() {
    let barnum = BarnumRunner::new();
    let result = barnum.schema_json().expect("schema");
    let stdout = String::from_utf8_lossy(&result.stdout);
    let schema: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert!(
        schema["definitions"]["StepFile"].is_object(),
        "Should define StepFile type"
    );
    assert!(
        schema["definitions"]["ActionFile"].is_object(),
        "Should define ActionFile type"
    );
    assert!(
        schema["definitions"]["Options"].is_object(),
        "Should define Options type"
    );
}

#[rstest]
#[timeout(Duration::from_secs(5))]
fn schema_action_has_bash_variant() {
    let barnum = BarnumRunner::new();
    let result = barnum.schema_json().expect("schema");
    let stdout = String::from_utf8_lossy(&result.stdout);
    let schema: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    let action = &schema["definitions"]["ActionFile"];
    let variants = action["oneOf"]
        .as_array()
        .expect("ActionFile should have oneOf");

    // Find Bash variant
    let has_bash = variants.iter().any(|v| {
        v["properties"]["kind"]["enum"]
            .as_array()
            .is_some_and(|e| e.iter().any(|k| k == "Bash"))
    });
    assert!(has_bash, "Action should have Bash variant");

    // Verify Pool variant does not exist
    let has_pool = variants.iter().any(|v| {
        v["properties"]["kind"]["enum"]
            .as_array()
            .is_some_and(|e| e.iter().any(|k| k == "Pool"))
    });
    assert!(!has_pool, "Action should not have Pool variant");
}

#[rstest]
#[timeout(Duration::from_secs(5))]
fn schema_defaults_to_zod() {
    let barnum = BarnumRunner::new();
    let result = barnum.schema().expect("schema");
    let stdout = String::from_utf8_lossy(&result.stdout);

    assert!(result.status.success(), "Schema should succeed");
    assert!(
        stdout.starts_with("import { z } from \"zod\";"),
        "Default output should be Zod"
    );
    assert!(
        stdout.contains("export const configFileSchema ="),
        "Should export configFileSchema"
    );
    assert!(
        stdout.contains("export type ConfigFile ="),
        "Should export ConfigFile type"
    );
    assert!(
        stdout.contains("export function defineConfig("),
        "Should export defineConfig helper"
    );
}

// =============================================================================
// barnum config validate - Valid configs
// =============================================================================

#[rstest]
#[timeout(Duration::from_secs(5))]
fn validate_minimal_config() {
    let config = r#"{"steps": []}"#;

    let barnum = BarnumRunner::new();
    let result = barnum.validate(config).expect("validate");

    assert!(result.status.success(), "Empty steps should be valid");
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("Config is valid"));
    assert!(stdout.contains("Steps: 0"));
}

#[rstest]
#[timeout(Duration::from_secs(5))]
fn validate_single_terminal_step() {
    let config = format!(r#"{{"steps": [{}]}}"#, step("Start", &[]));

    let barnum = BarnumRunner::new();
    let result = barnum.validate(&config).expect("validate");

    assert!(result.status.success());
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("Steps: 1"));
    assert!(stdout.contains("Start -> (terminal)"));
}

#[rstest]
#[timeout(Duration::from_secs(5))]
fn validate_linear_chain() {
    let config = format!(
        r#"{{"steps": [{}, {}, {}]}}"#,
        step("A", &["B"]),
        step("B", &["C"]),
        step("C", &[])
    );

    let barnum = BarnumRunner::new();
    let result = barnum.validate(&config).expect("validate");

    assert!(result.status.success());
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("A -> B"));
    assert!(stdout.contains("B -> C"));
    assert!(stdout.contains("C -> (terminal)"));
}

#[rstest]
#[timeout(Duration::from_secs(5))]
fn validate_branching_config() {
    let config = format!(
        r#"{{"steps": [{}, {}, {}, {}]}}"#,
        step("Start", &["PathA", "PathB"]),
        step("PathA", &["End"]),
        step("PathB", &["End"]),
        step("End", &[])
    );

    let barnum = BarnumRunner::new();
    let result = barnum.validate(&config).expect("validate");

    assert!(result.status.success());
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("Start -> PathA, PathB"));
}

#[rstest]
#[timeout(Duration::from_secs(5))]
fn validate_config_with_options() {
    let config = format!(
        r#"{{"options": {{"timeout": 60, "maxRetries": 3, "maxConcurrency": 5}}, "steps": [{}]}}"#,
        step("Task", &[])
    );

    let barnum = BarnumRunner::new();
    let result = barnum.validate(&config).expect("validate");

    assert!(result.status.success());
}

// =============================================================================
// barnum config validate - Invalid configs
// =============================================================================

#[rstest]
#[timeout(Duration::from_secs(5))]
fn validate_fails_missing_next_step() {
    let config = format!(r#"{{"steps": [{}]}}"#, step("A", &["NonExistent"]));

    let barnum = BarnumRunner::new();
    let result = barnum.validate(&config).expect("validate");

    assert!(!result.status.success(), "Should fail for missing step");
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("non-existent step"));
}

#[rstest]
#[timeout(Duration::from_secs(5))]
fn validate_fails_duplicate_step_names() {
    let config = format!(
        r#"{{"steps": [{}, {}]}}"#,
        step("Duplicate", &[]),
        step("Duplicate", &[])
    );

    let barnum = BarnumRunner::new();
    let result = barnum.validate(&config).expect("validate");

    assert!(!result.status.success(), "Should fail for duplicate names");
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("duplicate"));
}

#[rstest]
#[timeout(Duration::from_secs(5))]
fn validate_fails_invalid_json() {
    let config = r"{ not valid json }";

    let barnum = BarnumRunner::new();
    let result = barnum.validate(config).expect("validate");

    assert!(!result.status.success(), "Should fail for invalid JSON");
}

#[rstest]
#[timeout(Duration::from_secs(5))]
fn validate_fails_missing_steps_field() {
    let config = r#"{"options": {}}"#;

    let barnum = BarnumRunner::new();
    let result = barnum.validate(config).expect("validate");

    assert!(!result.status.success(), "Should fail without steps field");
}

#[rstest]
#[timeout(Duration::from_secs(5))]
fn validate_fails_unknown_field() {
    let config = r#"{
        "steps": [],
        "unknown_field": true
    }"#;

    let barnum = BarnumRunner::new();
    let result = barnum.validate(config).expect("validate");

    assert!(
        !result.status.success(),
        "Should fail for unknown field (deny_unknown_fields)"
    );
}

// =============================================================================
// barnum config docs
// =============================================================================

#[rstest]
#[timeout(Duration::from_secs(5))]
fn docs_generates_markdown_header() {
    let config = format!(r#"{{"steps": [{}]}}"#, step("Task", &[]));

    let barnum = BarnumRunner::new();
    let result = barnum.docs(&config).expect("docs");

    assert!(result.status.success());
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains('#'), "Should contain markdown headers");
}

#[rstest]
#[timeout(Duration::from_secs(5))]
fn docs_includes_step_names() {
    let config = r#"{
        "steps": [
            {"name": "Analyze", "action": {"kind": "Bash", "script": "echo '[]'"}, "next": ["Implement"]},
            {"name": "Implement", "action": {"kind": "Bash", "script": "echo '[]'"}, "next": []}
        ]
    }"#;

    let barnum = BarnumRunner::new();
    let result = barnum.docs(config).expect("docs");

    assert!(result.status.success());
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("Analyze"), "Should include Analyze step");
    assert!(
        stdout.contains("Implement"),
        "Should include Implement step"
    );
}

#[rstest]
#[timeout(Duration::from_secs(5))]
fn docs_includes_terminal_step_info() {
    let config = r#"{
        "steps": [{
            "name": "Task",
            "action": {"kind": "Bash", "script": "echo '[]'"},
            "next": []
        }]
    }"#;

    let barnum = BarnumRunner::new();
    let result = barnum.docs(config).expect("docs");

    assert!(result.status.success());
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("Task"), "Should include step name");
    assert!(stdout.contains("terminal"), "Should indicate terminal step");
}

#[rstest]
#[timeout(Duration::from_secs(5))]
fn docs_fails_invalid_config() {
    let config = format!(r#"{{"steps": [{}]}}"#, step("A", &["Missing"]));

    let barnum = BarnumRunner::new();
    let _result = barnum.docs(&config).expect("docs");

    // Docs doesn't validate transitions, so invalid next refs still work
    // But completely broken JSON should fail
    let broken = r"not json";
    let result2 = barnum.docs(broken).expect("docs");
    assert!(!result2.status.success(), "Should fail for invalid JSON");
}

// =============================================================================
// barnum config graph
// =============================================================================

#[rstest]
#[timeout(Duration::from_secs(5))]
fn graph_outputs_dot_format() {
    let config = format!(
        r#"{{"steps": [{}, {}]}}"#,
        step("A", &["B"]),
        step("B", &[])
    );

    let barnum = BarnumRunner::new();
    let result = barnum.graph(&config).expect("graph");

    assert!(result.status.success());
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(
        stdout.contains("digraph Barnum"),
        "Should start with digraph"
    );
    assert!(stdout.contains("\"A\" -> \"B\""), "Should have edge A->B");
}

#[rstest]
#[timeout(Duration::from_secs(5))]
fn graph_marks_terminal_steps() {
    let config = format!(r#"{{"steps": [{}]}}"#, step("End", &[]));

    let barnum = BarnumRunner::new();
    let result = barnum.graph(&config).expect("graph");

    assert!(result.status.success());
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(
        stdout.contains("peripheries=2"),
        "Terminal step should have double border"
    );
}

#[rstest]
#[timeout(Duration::from_secs(5))]
fn graph_bash_uses_box_shape() {
    let config = r#"{
        "steps": [
            {"name": "StepA", "action": {"kind": "Bash", "script": "echo '[]'"}, "next": ["StepB"]},
            {"name": "StepB", "action": {"kind": "Bash", "script": "echo '[]'"}, "next": []}
        ]
    }"#;

    let barnum = BarnumRunner::new();
    let result = barnum.graph(config).expect("graph");

    assert!(result.status.success());
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(
        stdout.contains("shape=box"),
        "Bash steps should use box shape"
    );
}

#[rstest]
#[timeout(Duration::from_secs(5))]
fn graph_fails_invalid_config() {
    let config = format!(r#"{{"steps": [{}]}}"#, step("A", &["Missing"]));

    let barnum = BarnumRunner::new();
    let result = barnum.graph(&config).expect("graph");

    assert!(
        !result.status.success(),
        "Graph should fail for invalid config"
    );
}

#[rstest]
#[timeout(Duration::from_secs(5))]
fn graph_shows_hooks() {
    let config = format!(
        r#"{{"steps": [{{
            "name": "WithHooks",
            "action": {BASH_ACTION},
            "finally": {{"kind": "Bash", "script": "echo finally"}},
            "next": []
        }}]}}"#
    );

    let barnum = BarnumRunner::new();
    let result = barnum.graph(&config).expect("graph");

    assert!(result.status.success());
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("finally"), "Should show finally hook");
}

// =============================================================================
// Edge cases
// =============================================================================

#[rstest]
#[timeout(Duration::from_secs(5))]
fn all_commands_handle_empty_steps() {
    let config = r#"{"steps": []}"#;
    let barnum = BarnumRunner::new();

    // All should succeed with empty steps
    assert!(barnum.validate(config).unwrap().status.success());
    assert!(barnum.docs(config).unwrap().status.success());
    assert!(barnum.graph(config).unwrap().status.success());
}

#[rstest]
#[timeout(Duration::from_secs(5))]
fn validate_cycle_is_allowed() {
    // Cycles are valid - a step can transition back to an earlier step
    let config = format!(r#"{{"steps": [{}]}}"#, step("Loop", &["Loop"]));

    let barnum = BarnumRunner::new();
    let result = barnum.validate(&config).expect("validate");

    assert!(result.status.success(), "Self-loop should be valid");
}

// =============================================================================
// Entrypoint validation
// =============================================================================

#[rstest]
#[timeout(Duration::from_secs(5))]
fn validate_valid_entrypoint() {
    let config = format!(
        r#"{{"entrypoint": "Start", "steps": [{}]}}"#,
        step("Start", &[])
    );

    let barnum = BarnumRunner::new();
    let result = barnum.validate(&config).expect("validate");

    assert!(result.status.success(), "Valid entrypoint should pass");
}

#[rstest]
#[timeout(Duration::from_secs(5))]
fn validate_invalid_entrypoint_fails() {
    let config = format!(
        r#"{{"entrypoint": "NonExistent", "steps": [{}]}}"#,
        step("Start", &[])
    );

    let barnum = BarnumRunner::new();
    let result = barnum.validate(&config).expect("validate");

    assert!(
        !result.status.success(),
        "Invalid entrypoint should fail validation"
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("NonExistent") || stderr.contains("entrypoint"),
        "Error should mention invalid entrypoint"
    );
}

#[rstest]
#[timeout(Duration::from_secs(5))]
fn validate_entrypoint() {
    let config = format!(
        r#"{{
        "entrypoint": "Start",
        "steps": [{{
            "name": "Start",
            "action": {BASH_ACTION},
            "next": []
        }}]
    }}"#
    );

    let barnum = BarnumRunner::new();
    let result = barnum.validate(&config).expect("validate");

    assert!(result.status.success(), "Entrypoint should be valid");
}
