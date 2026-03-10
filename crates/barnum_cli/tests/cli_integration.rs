//! Barnum CLI integration tests.
//!
//! These tests verify the CLI runs correctly with various configurations.
//! They use a file-writer agent that creates marker files to verify execution.

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]
#![expect(clippy::unwrap_used)]

mod common;

use common::{
    BarnumRunner, FileWriterAgent, TroupeHandle, cleanup_test_dir, is_ipc_available, setup_test_dir,
};
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
    let output_dir = root.join("output");

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&test_name);
        return;
    }

    let pool_root = root.join("pool");
    fs::create_dir_all(&pool_root).expect("create pool dir");

    let _pool = TroupeHandle::start(&pool_root);
    let agent = FileWriterAgent::start(
        &pool_root,
        &output_dir,
        vec![("Start".to_string(), String::new())], // Terminate
    );

    let config = r#"{
        "steps": [{
            "name": "Start",
            "action": {"kind": "Pool", "instructions": {"inline": "Start step"}},
            "next": []
        }]
    }"#;

    let barnum = BarnumRunner::new();
    let result = barnum
        .run(config, r#"[{"kind": "Start", "value": {}}]"#, &pool_root)
        .expect("run barnum");

    agent.stop();

    assert!(
        result.status.success(),
        "Barnum should succeed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        output_dir.join("Start.done").exists(),
        "Start step should have executed"
    );

    cleanup_test_dir(&test_name);
}

#[rstest]
#[timeout(Duration::from_secs(20))]
fn multi_stage_linear() {
    let test_name = format!("{TEST_DIR}_multi_stage");
    let root = setup_test_dir(&test_name);
    let output_dir = root.join("output");

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&test_name);
        return;
    }

    let pool_root = root.join("pool");
    fs::create_dir_all(&pool_root).expect("create pool dir");

    let _pool = TroupeHandle::start(&pool_root);
    let agent = FileWriterAgent::start(
        &pool_root,
        &output_dir,
        vec![
            ("Start".to_string(), "Middle".to_string()),
            ("Middle".to_string(), "End".to_string()),
            ("End".to_string(), String::new()),
        ],
    );

    let config = r#"{
        "steps": [
            {"name": "Start", "action": {"kind": "Pool", "instructions": {"inline": "Start"}}, "next": ["Middle"]},
            {"name": "Middle", "action": {"kind": "Pool", "instructions": {"inline": "Middle"}}, "next": ["End"]},
            {"name": "End", "action": {"kind": "Pool", "instructions": {"inline": "End"}}, "next": []}
        ]
    }"#;

    let barnum = BarnumRunner::new();
    let result = barnum
        .run(config, r#"[{"kind": "Start", "value": {}}]"#, &pool_root)
        .expect("run barnum");

    agent.stop();

    assert!(
        result.status.success(),
        "Barnum should succeed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        output_dir.join("Start.done").exists(),
        "Start step should have executed"
    );
    assert!(
        output_dir.join("Middle.done").exists(),
        "Middle step should have executed"
    );
    assert!(
        output_dir.join("End.done").exists(),
        "End step should have executed"
    );

    cleanup_test_dir(&test_name);
}

// =============================================================================
// Empty Initial Tasks
// =============================================================================

#[rstest]
#[timeout(Duration::from_secs(20))]
fn empty_initial_tasks_succeeds() {
    let test_name = format!("{TEST_DIR}_empty_initial");
    let root = setup_test_dir(&test_name);

    // No IPC needed - empty tasks should complete immediately without pool
    let config = r#"{
        "steps": [{
            "name": "Start",
            "action": {"kind": "Pool", "instructions": {"inline": "Start"}},
            "next": []
        }]
    }"#;

    let barnum = BarnumRunner::new();
    let result = barnum.run(config, "[]", &root).expect("run barnum");

    assert!(
        result.status.success(),
        "Barnum should succeed with empty tasks"
    );

    cleanup_test_dir(&test_name);
}

// =============================================================================
// CLI Subcommands
// =============================================================================

#[rstest]
#[timeout(Duration::from_secs(5))]
fn validate_valid_config() {
    let config = r#"{
        "steps": [
            {"name": "A", "action": {"kind": "Pool", "instructions": {"inline": "A"}}, "next": ["B"]},
            {"name": "B", "action": {"kind": "Pool", "instructions": {"inline": "B"}}, "next": []}
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
            {"name": "A", "action": {"kind": "Pool", "instructions": {"inline": "A"}}, "next": ["NonExistent"]}
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
            {"name": "Start", "action": {"kind": "Pool", "instructions": {"inline": "Do something"}}, "next": []}
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
            {"name": "A", "action": {"kind": "Pool", "instructions": {"inline": "A"}}, "next": ["B"]},
            {"name": "B", "action": {"kind": "Pool", "instructions": {"inline": "B"}}, "next": []}
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
    let output_dir = root.join("output");

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&test_name);
        return;
    }

    let pool_root = root.join("pool");
    fs::create_dir_all(&pool_root).expect("create pool dir");

    // Write config to file
    let config_path = root.join("config.json");
    fs::write(
        &config_path,
        r#"{
            "steps": [{
                "name": "FileStep",
                "action": {"kind": "Pool", "instructions": {"inline": "From file"}},
                "next": []
            }]
        }"#,
    )
    .expect("write config");

    // Write initial tasks to file
    let initial_path = root.join("initial.json");
    fs::write(&initial_path, r#"[{"kind": "FileStep", "value": {}}]"#).expect("write initial");

    let _pool = TroupeHandle::start(&pool_root);
    let agent = FileWriterAgent::start(
        &pool_root,
        &output_dir,
        vec![("FileStep".to_string(), String::new())],
    );

    let barnum = BarnumRunner::new();
    let result = barnum
        .run(
            config_path.to_str().unwrap(),
            initial_path.to_str().unwrap(),
            &pool_root,
        )
        .expect("run barnum");

    agent.stop();

    assert!(
        result.status.success(),
        "Barnum should succeed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        output_dir.join("FileStep.done").exists(),
        "FileStep should have executed"
    );

    cleanup_test_dir(&test_name);
}
