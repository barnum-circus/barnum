//! Tests for edge cases and boundary conditions.

#![expect(clippy::expect_used)]

mod common;

use barnum_config::{Config, RunnerConfig, StepInputValue, Task};
use common::{cleanup_test_dir, setup_test_dir, test_state_log_path};
use rstest::rstest;
use std::path::Path;
use std::time::Duration;

const TEST_DIR: &str = "edge_cases";

/// Test that empty `initial_tasks` completes immediately.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn empty_initial_tasks_completes() {
    let root = setup_test_dir(TEST_DIR);

    let config: Config = serde_json::from_str(
        r#"{
            "steps": [
                {
                    "name": "Start",
                    "action": {"kind": "Bash", "script": "echo '[]'"},
                    "next": []
                }
            ]
        }"#,
    )
    .expect("parse config");

    let initial_tasks = vec![]; // Empty!
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        state_log_path: &state_log,
    };

    // Should complete immediately without error
    barnum_config::run(&config, &runner_config, initial_tasks).expect("run failed");

    cleanup_test_dir(&root);
}

/// Test that large fan-out works correctly.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn large_fan_out() {
    let root = setup_test_dir(&format!("{TEST_DIR}_large_fanout"));

    // Distribute echoes 20 follow-up tasks, each transitioning to Worker.
    // Worker is terminal (echoes empty array).
    //
    // The script uses jq to generate the fan-out array dynamically.
    let config: Config = serde_json::from_str(
        r#"{
            "steps": [
                {
                    "name": "Distribute",
                    "action": {"kind": "Bash", "script": "echo '[{\"kind\":\"Worker\",\"value\":{}},{\"kind\":\"Worker\",\"value\":{}},{\"kind\":\"Worker\",\"value\":{}},{\"kind\":\"Worker\",\"value\":{}},{\"kind\":\"Worker\",\"value\":{}},{\"kind\":\"Worker\",\"value\":{}},{\"kind\":\"Worker\",\"value\":{}},{\"kind\":\"Worker\",\"value\":{}},{\"kind\":\"Worker\",\"value\":{}},{\"kind\":\"Worker\",\"value\":{}},{\"kind\":\"Worker\",\"value\":{}},{\"kind\":\"Worker\",\"value\":{}},{\"kind\":\"Worker\",\"value\":{}},{\"kind\":\"Worker\",\"value\":{}},{\"kind\":\"Worker\",\"value\":{}},{\"kind\":\"Worker\",\"value\":{}},{\"kind\":\"Worker\",\"value\":{}},{\"kind\":\"Worker\",\"value\":{}},{\"kind\":\"Worker\",\"value\":{}},{\"kind\":\"Worker\",\"value\":{}}]'"},
                    "next": ["Worker"]
                },
                {
                    "name": "Worker",
                    "action": {"kind": "Bash", "script": "echo '[]'"},
                    "next": []
                }
            ]
        }"#,
    )
    .expect("parse config");

    let initial_tasks = vec![Task::new(
        "Distribute",
        StepInputValue(serde_json::json!({})),
    )];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        state_log_path: &state_log,
    };

    barnum_config::run(&config, &runner_config, initial_tasks).expect("run failed");

    cleanup_test_dir(&root);
}

/// Test Command action executes script correctly.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn command_action_executes() {
    let root = setup_test_dir(&format!("{TEST_DIR}_command"));

    let config: Config = serde_json::from_str(
        r#"{
            "steps": [
                {
                    "name": "Echo",
                    "action": {"kind": "Bash", "script": "jq -c '[{kind: \"Done\", value: .value}]'"},
                    "next": ["Done"]
                },
                {
                    "name": "Done",
                    "action": {"kind": "Bash", "script": "jq -c '[]'"},
                    "next": []
                }
            ]
        }"#,
    )
    .expect("parse config");

    let initial_tasks = vec![Task::new(
        "Echo",
        StepInputValue(serde_json::json!({"message": "hello"})),
    )];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        state_log_path: &state_log,
    };

    // Should complete without error
    barnum_config::run(&config, &runner_config, initial_tasks).expect("run failed");

    cleanup_test_dir(&root);
}

/// Test that runner handles rapid task completion.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn rapid_task_completion() {
    let root = setup_test_dir(&format!("{TEST_DIR}_rapid"));

    let config: Config = serde_json::from_str(
        r#"{
            "steps": [
                {
                    "name": "Fast",
                    "action": {"kind": "Bash", "script": "echo '[]'"},
                    "next": []
                }
            ]
        }"#,
    )
    .expect("parse config");

    // Submit many tasks
    let initial_tasks: Vec<Task> = (0..50)
        .map(|i| Task::new("Fast", StepInputValue(serde_json::json!({"id": i}))))
        .collect();

    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        state_log_path: &state_log,
    };

    barnum_config::run(&config, &runner_config, initial_tasks).expect("run failed");

    cleanup_test_dir(&root);
}

/// Test that unknown step in initial tasks returns an error.
#[rstest]
#[timeout(Duration::from_secs(5))]
fn unknown_step_in_initial_tasks_returns_error() {
    let root = setup_test_dir(&format!("{TEST_DIR}_unknown_step"));

    let config: Config = serde_json::from_str(
        r#"{
            "steps": [
                {
                    "name": "Known",
                    "action": {"kind": "Bash", "script": "echo '[]'"},
                    "next": []
                }
            ]
        }"#,
    )
    .expect("parse config");

    let initial_tasks = vec![
        Task::new("Unknown", StepInputValue(serde_json::json!({}))), // Unknown step - should error
    ];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        state_log_path: &state_log,
    };

    // Should return an error for unknown step
    let result = barnum_config::run(&config, &runner_config, initial_tasks);
    assert!(result.is_err(), "should error for unknown step");
    let err = result.expect_err("should error").to_string();
    assert!(
        err.contains("E019") && err.contains("Unknown"),
        "error should mention unknown step: {err}"
    );

    cleanup_test_dir(&root);
}
