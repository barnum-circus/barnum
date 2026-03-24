//! Tests for branching task queues (one step -> multiple possible next steps).

#![expect(clippy::expect_used)]

mod common;

use barnum_config::{Config, RunnerConfig, StepInputValue, Task};
use common::{cleanup_test_dir, setup_test_dir, test_state_log_path};
use rstest::rstest;
use std::path::Path;
use std::time::Duration;

const TEST_DIR: &str = "branching_transitions";

/// Config where Decide always branches to `PathA`.
fn branching_config_path_a() -> Config {
    serde_json::from_str(
        r#"{
            "steps": [
                {
                    "name": "Decide",
                    "action": {"kind": "Bash", "script": "echo '[{\"kind\":\"PathA\",\"value\":{}}]'"},
                    "next": ["PathA", "PathB"]
                },
                {
                    "name": "PathA",
                    "action": {"kind": "Bash", "script": "echo '[{\"kind\":\"Done\",\"value\":{}}]'"},
                    "next": ["Done"]
                },
                {
                    "name": "PathB",
                    "action": {"kind": "Bash", "script": "echo '[{\"kind\":\"Done\",\"value\":{}}]'"},
                    "next": ["Done"]
                },
                {
                    "name": "Done",
                    "action": {"kind": "Bash", "script": "echo '[]'"},
                    "next": []
                }
            ]
        }"#,
    )
    .expect("parse config")
}

/// Config where Decide always branches to `PathB`.
fn branching_config_path_b() -> Config {
    serde_json::from_str(
        r#"{
            "steps": [
                {
                    "name": "Decide",
                    "action": {"kind": "Bash", "script": "echo '[{\"kind\":\"PathB\",\"value\":{}}]'"},
                    "next": ["PathA", "PathB"]
                },
                {
                    "name": "PathA",
                    "action": {"kind": "Bash", "script": "echo '[{\"kind\":\"Done\",\"value\":{}}]'"},
                    "next": ["Done"]
                },
                {
                    "name": "PathB",
                    "action": {"kind": "Bash", "script": "echo '[{\"kind\":\"Done\",\"value\":{}}]'"},
                    "next": ["Done"]
                },
                {
                    "name": "Done",
                    "action": {"kind": "Bash", "script": "echo '[]'"},
                    "next": []
                }
            ]
        }"#,
    )
    .expect("parse config")
}

#[rstest]
#[timeout(Duration::from_secs(20))]
fn branch_to_path_a() {
    let root = setup_test_dir(TEST_DIR);

    let config = branching_config_path_a();
    let initial_tasks = vec![Task::new("Decide", StepInputValue(serde_json::json!({})))];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        state_log_path: &state_log,
    };

    barnum_config::run(&config, &runner_config, initial_tasks).expect("run failed");

    cleanup_test_dir(&root);
}

#[rstest]
#[timeout(Duration::from_secs(20))]
fn branch_to_path_b() {
    let root = setup_test_dir(&format!("{TEST_DIR}_path_b"));

    let config = branching_config_path_b();
    let initial_tasks = vec![Task::new("Decide", StepInputValue(serde_json::json!({})))];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        state_log_path: &state_log,
    };

    barnum_config::run(&config, &runner_config, initial_tasks).expect("run failed");

    cleanup_test_dir(&root);
}

#[rstest]
#[timeout(Duration::from_secs(20))]
fn invalid_transition_from_branch() {
    let root = setup_test_dir(&format!("{TEST_DIR}_invalid"));

    // Decide tries to transition to Done directly (not a valid next step).
    let config: Config = serde_json::from_str(
        r#"{
            "options": { "maxRetries": 0 },
            "steps": [
                {
                    "name": "Decide",
                    "action": {"kind": "Bash", "script": "echo '[{\"kind\":\"Done\",\"value\":{}}]'"},
                    "next": ["PathA", "PathB"]
                },
                {
                    "name": "PathA",
                    "action": {"kind": "Bash", "script": "echo '[{\"kind\":\"Done\",\"value\":{}}]'"},
                    "next": ["Done"]
                },
                {
                    "name": "PathB",
                    "action": {"kind": "Bash", "script": "echo '[{\"kind\":\"Done\",\"value\":{}}]'"},
                    "next": ["Done"]
                },
                {
                    "name": "Done",
                    "action": {"kind": "Bash", "script": "echo '[]'"},
                    "next": []
                }
            ]
        }"#,
    )
    .expect("parse config");

    let initial_tasks = vec![Task::new("Decide", StepInputValue(serde_json::json!({})))];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        state_log_path: &state_log,
    };

    let result = barnum_config::run(&config, &runner_config, initial_tasks);
    assert!(result.is_err(), "run should fail on invalid transition");

    cleanup_test_dir(&root);
}
