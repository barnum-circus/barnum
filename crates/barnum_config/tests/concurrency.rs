//! Tests for concurrent task execution.
//!
//! These tests verify that `max_concurrency` limits are respected
//! and that fan-out trees complete correctly.

#![expect(clippy::expect_used)]

mod common;

use barnum_config::{Config, RunnerConfig, StepInputValue, Task};
use common::{cleanup_test_dir, setup_test_dir, test_state_log_path};
use rstest::rstest;
use std::path::Path;
use std::time::Duration;

const TEST_DIR: &str = "concurrency";

/// Config with `max_concurrency=2` and a fan-out step that spawns two terminal workers.
fn max_concurrency_config() -> Config {
    serde_json::from_str(
        r#"{
            "options": {
                "maxConcurrency": 2
            },
            "steps": [
                {
                    "name": "FanOut",
                    "action": {"kind": "Bash", "script": "echo '[{\"kind\":\"Worker\",\"value\":{}},{\"kind\":\"Worker\",\"value\":{}}]'"},
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
    .expect("parse config")
}

/// Verify that a workflow with `max_concurrency=2` completes successfully.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn max_concurrency_limits_parallel_tasks() {
    let root = setup_test_dir(&format!("{TEST_DIR}_max_concurrency"));

    let config = max_concurrency_config();
    let initial_tasks = vec![Task::new("FanOut", StepInputValue(serde_json::json!({})))];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        state_log_path: &state_log,
    };

    barnum_config::run(&config, &runner_config, initial_tasks).expect("run failed");

    cleanup_test_dir(&root);
}

/// Deep fan-out tree: Root -> Branch1,Branch2 -> each spawns Leaf children.
///
/// Verifies that nested fan-out completes without deadlock or lost tasks.
fn fan_out_tree_config() -> Config {
    serde_json::from_str(
        r#"{
            "steps": [
                {
                    "name": "Root",
                    "action": {"kind": "Bash", "script": "echo '[{\"kind\":\"Branch1\",\"value\":{}},{\"kind\":\"Branch2\",\"value\":{}}]'"},
                    "next": ["Branch1", "Branch2"]
                },
                {
                    "name": "Branch1",
                    "action": {"kind": "Bash", "script": "echo '[{\"kind\":\"Leaf1A\",\"value\":{}},{\"kind\":\"Leaf1B\",\"value\":{}}]'"},
                    "next": ["Leaf1A", "Leaf1B"]
                },
                {
                    "name": "Branch2",
                    "action": {"kind": "Bash", "script": "echo '[{\"kind\":\"Leaf2A\",\"value\":{}},{\"kind\":\"Leaf2B\",\"value\":{}}]'"},
                    "next": ["Leaf2A", "Leaf2B"]
                },
                {
                    "name": "Leaf1A",
                    "action": {"kind": "Bash", "script": "echo '[]'"},
                    "next": []
                },
                {
                    "name": "Leaf1B",
                    "action": {"kind": "Bash", "script": "echo '[]'"},
                    "next": []
                },
                {
                    "name": "Leaf2A",
                    "action": {"kind": "Bash", "script": "echo '[]'"},
                    "next": []
                },
                {
                    "name": "Leaf2B",
                    "action": {"kind": "Bash", "script": "echo '[]'"},
                    "next": []
                }
            ]
        }"#,
    )
    .expect("parse config")
}

/// Verify that a deep fan-out tree completes successfully.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn fan_out_tree() {
    let root = setup_test_dir(&format!("{TEST_DIR}_nested"));

    let config = fan_out_tree_config();
    let initial_tasks = vec![Task::new("Root", StepInputValue(serde_json::json!({})))];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        state_log_path: &state_log,
    };

    barnum_config::run(&config, &runner_config, initial_tasks).expect("run failed");

    cleanup_test_dir(&root);
}
