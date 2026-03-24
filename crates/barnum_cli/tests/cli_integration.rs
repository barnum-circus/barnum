//! Barnum CLI integration tests.
//!
//! These tests verify the CLI runs correctly with various configurations.
//! They use Bash actions that run shell scripts to verify execution.

#![expect(clippy::expect_used)]
#![expect(clippy::unwrap_used)]

mod common;

use common::{BarnumRunner, cleanup_test_dir, setup_test_dir};
use rstest::rstest;
use std::fs;
use std::time::Duration;

const TEST_DIR: &str = "cli";

// =============================================================================
// Basic Config Tests
// =============================================================================

#[rstest]
#[timeout(Duration::from_secs(20))]
fn single_step_terminates() {
    let test_name = format!("{TEST_DIR}_single_step");
    let root = setup_test_dir(&test_name);

    let config = r#"{
        "steps": [{
            "name": "Start",
            "action": {"kind": "Bash", "script": "echo '[]'"},
            "next": []
        }]
    }"#;

    let barnum = BarnumRunner::new();
    let result = barnum
        .run(config, r#"[{"kind": "Start", "value": {}}]"#)
        .expect("run barnum");

    assert!(
        result.status.success(),
        "Barnum should succeed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );

    cleanup_test_dir(&root);
}

#[rstest]
#[timeout(Duration::from_secs(20))]
fn multi_stage_linear() {
    let test_name = format!("{TEST_DIR}_multi_stage");
    let root = setup_test_dir(&test_name);

    let config = r#"{
        "steps": [
            {"name": "Start", "action": {"kind": "Bash", "script": "echo '[{\"kind\": \"Middle\", \"value\": {}}]'"}, "next": ["Middle"]},
            {"name": "Middle", "action": {"kind": "Bash", "script": "echo '[{\"kind\": \"End\", \"value\": {}}]'"}, "next": ["End"]},
            {"name": "End", "action": {"kind": "Bash", "script": "echo '[]'"}, "next": []}
        ]
    }"#;

    let barnum = BarnumRunner::new();
    let result = barnum
        .run(config, r#"[{"kind": "Start", "value": {}}]"#)
        .expect("run barnum");

    assert!(
        result.status.success(),
        "Barnum should succeed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );

    cleanup_test_dir(&root);
}

// =============================================================================
// Empty Initial Tasks
// =============================================================================

#[rstest]
#[timeout(Duration::from_secs(20))]
fn empty_initial_tasks_succeeds() {
    let test_name = format!("{TEST_DIR}_empty_initial");
    let root = setup_test_dir(&test_name);

    let config = r#"{
        "steps": [{
            "name": "Start",
            "action": {"kind": "Bash", "script": "echo '[]'"},
            "next": []
        }]
    }"#;

    let barnum = BarnumRunner::new();
    let result = barnum.run(config, "[]").expect("run barnum");

    assert!(
        result.status.success(),
        "Barnum should succeed with empty tasks"
    );

    cleanup_test_dir(&root);
}

// =============================================================================
// CLI Subcommands
// =============================================================================

#[rstest]
#[timeout(Duration::from_secs(5))]
fn validate_valid_config() {
    let config = r#"{
        "steps": [
            {"name": "A", "action": {"kind": "Bash", "script": "echo '[]'"}, "next": ["B"]},
            {"name": "B", "action": {"kind": "Bash", "script": "echo '[]'"}, "next": []}
        ]
    }"#;

    let barnum = BarnumRunner::new();
    let result = barnum.validate(config).expect("validate");

    assert!(result.status.success(), "Valid config should pass");
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("Config is valid"), "Should say valid");
}

#[rstest]
#[timeout(Duration::from_secs(5))]
fn validate_invalid_config_missing_step() {
    let config = r#"{
        "steps": [
            {"name": "A", "action": {"kind": "Bash", "script": "echo '[]'"}, "next": ["NonExistent"]}
        ]
    }"#;

    let barnum = BarnumRunner::new();
    let result = barnum.validate(config).expect("validate");

    assert!(!result.status.success(), "Invalid config should fail");
}

#[rstest]
#[timeout(Duration::from_secs(5))]
fn docs_generates_markdown() {
    let config = r#"{
        "steps": [
            {"name": "Start", "action": {"kind": "Bash", "script": "echo '[]'"}, "next": []}
        ]
    }"#;

    let barnum = BarnumRunner::new();
    let result = barnum.docs(config).expect("docs");

    assert!(result.status.success(), "Docs should succeed");
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("Start"), "Should contain step name");
}

#[rstest]
#[timeout(Duration::from_secs(5))]
fn graph_generates_dot() {
    let config = r#"{
        "steps": [
            {"name": "A", "action": {"kind": "Bash", "script": "echo '[]'"}, "next": ["B"]},
            {"name": "B", "action": {"kind": "Bash", "script": "echo '[]'"}, "next": []}
        ]
    }"#;

    let barnum = BarnumRunner::new();
    let result = barnum.graph(config).expect("graph");

    assert!(result.status.success(), "Graph should succeed");
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("digraph"), "Should be DOT format");
    assert!(stdout.contains("\"A\" -> \"B\""), "Should have edge");
}

// =============================================================================
// Config From File
// =============================================================================

#[rstest]
#[timeout(Duration::from_secs(20))]
fn config_from_file() {
    let test_name = format!("{TEST_DIR}_config_file");
    let root = setup_test_dir(&test_name);

    // Write config to file
    let config_path = root.join("config.json");
    fs::write(
        &config_path,
        r#"{
            "steps": [{
                "name": "FileStep",
                "action": {"kind": "Bash", "script": "echo '[]'"},
                "next": []
            }]
        }"#,
    )
    .expect("write config");

    // Write initial tasks to file
    let initial_path = root.join("initial.json");
    fs::write(&initial_path, r#"[{"kind": "FileStep", "value": {}}]"#).expect("write initial");

    let barnum = BarnumRunner::new();
    let result = barnum
        .run(
            config_path.to_str().unwrap(),
            initial_path.to_str().unwrap(),
        )
        .expect("run barnum");

    assert!(
        result.status.success(),
        "Barnum should succeed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );

    cleanup_test_dir(&root);
}
