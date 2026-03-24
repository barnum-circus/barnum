//! Tests for invalid task queue transitions.

#![expect(clippy::expect_used)]

mod common;

use barnum_config::{Config, ConfigFile, RunnerConfig, StepInputValue, Task};
use common::{cleanup_test_dir, setup_test_dir, test_state_log_path};
use rstest::rstest;
use std::path::Path;
use std::time::Duration;

const TEST_DIR: &str = "invalid_transitions";

fn strict_config() -> Config {
    let config_file: ConfigFile = serde_json::from_str(
        r#"{
            "options": {
                "max_retries": 1
            },
            "steps": [
                {
                    "name": "Start",
                    "action": {"kind": "Command", "params": {"script": "echo '[{\"kind\":\"End\",\"value\":{}}]'"}},
                    "next": ["Middle"]
                },
                {
                    "name": "Middle",
                    "action": {"kind": "Command", "params": {"script": "echo '[{\"kind\":\"End\",\"value\":{}}]'"}},
                    "next": ["End"]
                },
                {
                    "name": "End",
                    "action": {"kind": "Command", "params": {"script": "echo '[]'"}},
                    "next": []
                }
            ]
        }"#,
    )
    .expect("parse config");
    config_file.resolve(Path::new("."))
}

/// Start's script returns `[{"kind":"End"}]` but `next` only allows `["Middle"]`.
/// With `max_retries: 1`, this should exhaust retries and fail.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn invalid_transition_causes_retry() {
    let root = setup_test_dir(TEST_DIR);

    let config = strict_config();
    let initial_tasks = vec![Task::new("Start", StepInputValue(serde_json::json!({})))];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        state_log_path: &state_log,
    };

    let result = barnum_config::run(&config, &runner_config, initial_tasks);
    assert!(result.is_err(), "run should fail when tasks are dropped");

    cleanup_test_dir(&root);
}

/// Start's script returns `[{"kind":"NonExistent"}]` — a step that doesn't exist.
/// With `max_retries: 1`, this should exhaust retries and fail.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn unknown_step_causes_retry() {
    let root = setup_test_dir(&format!("{TEST_DIR}_unknown"));

    let config_file: ConfigFile = serde_json::from_str(
        r#"{
            "options": {
                "max_retries": 1
            },
            "steps": [
                {
                    "name": "Start",
                    "action": {"kind": "Command", "params": {"script": "echo '[{\"kind\":\"NonExistent\",\"value\":{}}]'"}},
                    "next": ["Middle"]
                },
                {
                    "name": "Middle",
                    "action": {"kind": "Command", "params": {"script": "echo '[]'"}},
                    "next": []
                }
            ]
        }"#,
    )
    .expect("parse config");
    let config = config_file.resolve(Path::new("."));

    let initial_tasks = vec![Task::new("Start", StepInputValue(serde_json::json!({})))];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        state_log_path: &state_log,
    };

    let result = barnum_config::run(&config, &runner_config, initial_tasks);
    assert!(result.is_err(), "run should fail when tasks are dropped");

    cleanup_test_dir(&root);
}

/// Start's script uses a counter file to return an invalid transition on the first
/// attempt, then a valid one on retry. The workflow should recover and complete.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn recovery_after_invalid_then_valid() {
    let root = setup_test_dir(&format!("{TEST_DIR}_recovery"));

    // Use a counter file so the script returns different output on retry.
    // First call: counter absent -> outputs End (invalid for next:["Middle"]).
    // Second call: counter exists -> outputs Middle (valid).
    let counter_path = root.join("start_counter");
    let start_script = format!(
        "if [ -f '{}' ]; then echo '[{{\"kind\":\"Middle\",\"value\":{{}}}}]'; else touch '{}'; echo '[{{\"kind\":\"End\",\"value\":{{}}}}]'; fi",
        counter_path.display(),
        counter_path.display(),
    );

    let json_value = serde_json::json!({
        "options": {
            "max_retries": 3
        },
        "steps": [
            {
                "name": "Start",
                "action": {"kind": "Command", "params": {"script": start_script}},
                "next": ["Middle"]
            },
            {
                "name": "Middle",
                "action": {"kind": "Command", "params": {"script": "echo '[{\"kind\":\"End\",\"value\":{}}]'"}},
                "next": ["End"]
            },
            {
                "name": "End",
                "action": {"kind": "Command", "params": {"script": "echo '[]'"}},
                "next": []
            }
        ]
    });

    let config_file: ConfigFile = serde_json::from_value(json_value).expect("parse config");
    let config = config_file.resolve(Path::new("."));

    let initial_tasks = vec![Task::new("Start", StepInputValue(serde_json::json!({})))];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        state_log_path: &state_log,
    };

    barnum_config::run(&config, &runner_config, initial_tasks).expect("run failed");

    cleanup_test_dir(&root);
}
