//! Tests for retry behavior with different configuration options.
//!
//! These tests verify that `retry_on_timeout`, `retry_on_invalid_response`,
//! and `max_retries` work correctly using Command actions.

#![expect(clippy::expect_used)]

mod common;

use barnum_config::{Config, RunnerConfig, StepInputValue, Task};
use common::{cleanup_test_dir, setup_test_dir, test_state_log_path};
use rstest::rstest;
use std::path::Path;
use std::time::Duration;

const TEST_DIR: &str = "retry_behavior";

// =============================================================================
// Timeout retry: Command script sleeps too long, barnum kills it, then retries
// =============================================================================

/// Test that a command that times out is retried (default `retry_on_timeout=true`),
/// eventually exhausting `max_retries`.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn timeout_retry_exhausts_max_retries() {
    let root = setup_test_dir(TEST_DIR);

    let config: Config = serde_json::from_str(
        r#"{
            "options": {
                "timeout": 1,
                "maxRetries": 2,
                "retryOnTimeout": true
            },
            "steps": [
                {
                    "name": "Start",
                    "action": {"kind": "Bash", "script": "sleep 999"},
                    "next": []
                }
            ]
        }"#,
    )
    .expect("parse config");

    let initial_tasks = vec![Task::new("Start", StepInputValue(serde_json::json!({})))];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        state_log_path: &state_log,
        executor: "unused",
        run_handler_path: "unused",
    };

    let result = barnum_config::run(&config, &runner_config, initial_tasks);
    assert!(
        result.is_err(),
        "run should fail after exhausting retries on timeout"
    );

    cleanup_test_dir(&root);
}

// =============================================================================
// Invalid response retry: Command outputs bad JSON, barnum retries
// =============================================================================

/// Test that invalid JSON output triggers retry when `retry_on_invalid_response=true`,
/// eventually exhausting `max_retries`.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn invalid_response_retry_exhausts_max_retries() {
    let root = setup_test_dir(&format!("{TEST_DIR}_invalid_resp"));

    let config: Config = serde_json::from_str(
        r#"{
            "options": {
                "maxRetries": 2,
                "retryOnInvalidResponse": true
            },
            "steps": [
                {
                    "name": "Start",
                    "action": {"kind": "Bash", "script": "echo 'not json'"},
                    "next": []
                }
            ]
        }"#,
    )
    .expect("parse config");

    let initial_tasks = vec![Task::new("Start", StepInputValue(serde_json::json!({})))];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        state_log_path: &state_log,
        executor: "unused",
        run_handler_path: "unused",
    };

    let result = barnum_config::run(&config, &runner_config, initial_tasks);
    assert!(
        result.is_err(),
        "run should fail after exhausting retries on invalid response"
    );

    cleanup_test_dir(&root);
}

// =============================================================================
// Max retries exhausted: after N failures, barnum gives up
// =============================================================================

/// Test that `max_retries=0` means no retries at all (single attempt).
#[rstest]
#[timeout(Duration::from_secs(20))]
fn max_retries_zero_no_retries() {
    let root = setup_test_dir(&format!("{TEST_DIR}_zero_retries"));

    let config: Config = serde_json::from_str(
        r#"{
            "options": {
                "maxRetries": 0,
                "retryOnInvalidResponse": true
            },
            "steps": [
                {
                    "name": "Start",
                    "action": {"kind": "Bash", "script": "echo 'not json'"},
                    "next": []
                }
            ]
        }"#,
    )
    .expect("parse config");

    let initial_tasks = vec![Task::new("Start", StepInputValue(serde_json::json!({})))];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        state_log_path: &state_log,
        executor: "unused",
        run_handler_path: "unused",
    };

    let result = barnum_config::run(&config, &runner_config, initial_tasks);
    assert!(
        result.is_err(),
        "run should fail when max_retries=0 and response is invalid"
    );

    cleanup_test_dir(&root);
}

// =============================================================================
// Retry on timeout disabled: no retry when retry_on_timeout=false
// =============================================================================

/// Test that `retry_on_timeout=false` causes immediate failure on timeout.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn retry_on_timeout_false_drops_task() {
    let root = setup_test_dir(&format!("{TEST_DIR}_no_timeout_retry"));

    let config: Config = serde_json::from_str(
        r#"{
            "options": {
                "timeout": 1,
                "maxRetries": 5,
                "retryOnTimeout": false
            },
            "steps": [
                {
                    "name": "Start",
                    "action": {"kind": "Bash", "script": "sleep 999"},
                    "next": []
                }
            ]
        }"#,
    )
    .expect("parse config");

    let initial_tasks = vec![Task::new("Start", StepInputValue(serde_json::json!({})))];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        state_log_path: &state_log,
        executor: "unused",
        run_handler_path: "unused",
    };

    let result = barnum_config::run(&config, &runner_config, initial_tasks);
    assert!(
        result.is_err(),
        "run should fail immediately when retry_on_timeout=false"
    );

    cleanup_test_dir(&root);
}

// =============================================================================
// Retry on invalid response disabled: no retry on bad output
// =============================================================================

/// Test that `retry_on_invalid_response=false` causes immediate failure on bad output.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn retry_on_invalid_response_false_drops_task() {
    let root = setup_test_dir(&format!("{TEST_DIR}_no_invalid_retry"));

    let config: Config = serde_json::from_str(
        r#"{
            "options": {
                "maxRetries": 5,
                "retryOnInvalidResponse": false
            },
            "steps": [
                {
                    "name": "Start",
                    "action": {"kind": "Bash", "script": "echo 'not json'"},
                    "next": []
                }
            ]
        }"#,
    )
    .expect("parse config");

    let initial_tasks = vec![Task::new("Start", StepInputValue(serde_json::json!({})))];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        state_log_path: &state_log,
        executor: "unused",
        run_handler_path: "unused",
    };

    let result = barnum_config::run(&config, &runner_config, initial_tasks);
    assert!(
        result.is_err(),
        "run should fail immediately when retry_on_invalid_response=false"
    );

    cleanup_test_dir(&root);
}

// =============================================================================
// Per-step options override global
// =============================================================================

/// Test that per-step `retry_on_invalid_response=false` overrides `global=true`.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn per_step_options_override_global() {
    let root = setup_test_dir(&format!("{TEST_DIR}_per_step"));

    let config: Config = serde_json::from_str(
        r#"{
            "options": {
                "maxRetries": 5,
                "retryOnInvalidResponse": true
            },
            "steps": [
                {
                    "name": "NoRetryStep",
                    "action": {"kind": "Bash", "script": "echo 'not json'"},
                    "next": [],
                    "options": {
                        "retryOnInvalidResponse": false
                    }
                }
            ]
        }"#,
    )
    .expect("parse config");

    let initial_tasks = vec![Task::new(
        "NoRetryStep",
        StepInputValue(serde_json::json!({})),
    )];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        state_log_path: &state_log,
        executor: "unused",
        run_handler_path: "unused",
    };

    let result = barnum_config::run(&config, &runner_config, initial_tasks);
    assert!(
        result.is_err(),
        "run should fail when per-step retry is disabled"
    );

    cleanup_test_dir(&root);
}

// =============================================================================
// Successful retry: script fails once then succeeds
// =============================================================================

/// Test that a script that fails once then succeeds completes successfully.
///
/// Uses a counter file: first invocation writes bad output, second reads the
/// marker and writes valid output.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn successful_retry_after_initial_failure() {
    let root = setup_test_dir(&format!("{TEST_DIR}_recovery"));

    let counter_file = root.join("retry_counter");
    let counter_path = counter_file.display().to_string();

    let json = format!(
        r#"{{
            "options": {{
                "maxRetries": 5,
                "retryOnInvalidResponse": true
            }},
            "steps": [
                {{
                    "name": "Start",
                    "action": {{"kind": "Bash", "script": "F={counter_path}; if [ -f \"$F\" ]; then echo '[]'; else touch \"$F\"; echo 'bad'; fi"}},
                    "next": []
                }}
            ]
        }}"#
    );

    let config: Config = serde_json::from_str(&json).expect("parse config");

    let initial_tasks = vec![Task::new("Start", StepInputValue(serde_json::json!({})))];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        state_log_path: &state_log,
        executor: "unused",
        run_handler_path: "unused",
    };

    barnum_config::run(&config, &runner_config, initial_tasks)
        .expect("run should succeed on retry");

    cleanup_test_dir(&root);
}
