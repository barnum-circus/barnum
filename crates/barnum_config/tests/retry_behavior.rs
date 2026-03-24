//! Tests for retry behavior with different configuration options.
//!
//! These tests verify that `retry_on_timeout`, `retry_on_invalid_response`,
//! and `max_retries` work correctly.

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]
#![expect(clippy::doc_markdown)]
#![expect(clippy::should_panic_without_expect)]

mod common;

use barnum_config::{ConfigFile, RunnerConfig, StepInputValue, Task};
use common::{
    BarnumTestAgent, TroupeHandle, cleanup_test_dir, create_test_invoker, inject_pool_config,
    is_ipc_available, setup_test_dir, test_state_log_path,
};
use rstest::rstest;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const TEST_DIR: &str = "retry_behavior";

/// Test that retry_on_invalid_response=false drops tasks immediately.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn retry_on_invalid_response_false_drops_task() {
    let root = setup_test_dir(TEST_DIR);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&root);
        return;
    }

    let _pool = TroupeHandle::start(&root);

    let call_count = Arc::new(AtomicUsize::new(0));
    let count_clone = call_count.clone();

    // Agent that always returns invalid response
    let _agent = BarnumTestAgent::start(&root, Duration::from_millis(10), move |_| {
        count_clone.fetch_add(1, Ordering::SeqCst);
        // Invalid: returns a step not in `next`
        r#"[{"kind": "NonExistent", "value": {}}]"#.to_string()
    });

    // Wait for agent to be ready (has processed initial heartbeat)

    let json = inject_pool_config(
        r#"{
            "options": {
                "max_retries": 5,
                "retry_on_invalid_response": false
            },
            "steps": [
                {
                    "name": "Start",
                    "action": {"kind": "Pool", "params": {"instructions": {"kind": "Inline", "value": ""}}},
                    "next": ["End"]
                },
                {
                    "name": "End",
                    "action": {"kind": "Pool", "params": {"instructions": {"kind": "Inline", "value": ""}}},
                    "next": []
                }
            ]
        }"#,
        &root,
    );
    let config_file: ConfigFile = serde_json::from_str(&json).expect("parse config");
    let config = config_file.resolve(Path::new(".")).expect("resolve config");

    let initial_tasks = vec![Task::new("Start", StepInputValue(serde_json::json!({})))];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &create_test_invoker(),
        state_log_path: &state_log,
    };

    // Run should return error because task is dropped
    let result = barnum_config::run(&config, &runner_config, initial_tasks);
    assert!(result.is_err(), "run should fail when tasks are dropped");

    // With retry_on_invalid_response=false, should only try once
    assert_eq!(
        call_count.load(Ordering::SeqCst),
        1,
        "Should only attempt once when retry_on_invalid_response=false"
    );

    cleanup_test_dir(&root);
}

/// Test that retry_on_invalid_response=true retries up to max_retries.
#[rstest]
#[timeout(Duration::from_secs(20))]
#[should_panic]
fn retry_on_invalid_response_true_retries() {
    let root = setup_test_dir(&format!("{TEST_DIR}_retry_true"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&root);
        return;
    }

    let _pool = TroupeHandle::start(&root);

    let call_count = Arc::new(AtomicUsize::new(0));
    let count_clone = call_count.clone();

    // Agent that always returns invalid response
    let _agent = BarnumTestAgent::start(&root, Duration::from_millis(10), move |_| {
        count_clone.fetch_add(1, Ordering::SeqCst);
        r#"[{"kind": "NonExistent", "value": {}}]"#.to_string()
    });

    // Wait for agent to be ready (has processed initial heartbeat)

    let json = inject_pool_config(
        r#"{
            "options": {
                "max_retries": 3,
                "retry_on_invalid_response": true
            },
            "steps": [
                {
                    "name": "Start",
                    "action": {"kind": "Pool", "params": {"instructions": {"kind": "Inline", "value": ""}}},
                    "next": ["End"]
                },
                {
                    "name": "End",
                    "action": {"kind": "Pool", "params": {"instructions": {"kind": "Inline", "value": ""}}},
                    "next": []
                }
            ]
        }"#,
        &root,
    );
    let config_file: ConfigFile = serde_json::from_str(&json).expect("parse config");
    let config = config_file.resolve(Path::new(".")).expect("resolve config");

    let initial_tasks = vec![Task::new("Start", StepInputValue(serde_json::json!({})))];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &create_test_invoker(),
        state_log_path: &state_log,
    };

    // Run should return error because task is dropped after all retries
    let result = barnum_config::run(&config, &runner_config, initial_tasks);
    assert!(result.is_err(), "run should fail when tasks are dropped");

    // With max_retries=3, should try 1 original + 3 retries = 4 total
    assert_eq!(
        call_count.load(Ordering::SeqCst),
        4,
        "Should attempt 4 times (1 original + 3 retries)"
    );

    cleanup_test_dir(&root);
}

/// Test that agent returning malformed JSON triggers retry.
#[rstest]
#[timeout(Duration::from_secs(20))]
#[should_panic]
fn malformed_json_triggers_retry() {
    let root = setup_test_dir(&format!("{TEST_DIR}_malformed"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&root);
        return;
    }

    let _pool = TroupeHandle::start(&root);

    let call_count = Arc::new(AtomicUsize::new(0));
    let count_clone = call_count.clone();

    // Agent that returns invalid JSON
    let _agent = BarnumTestAgent::start(&root, Duration::from_millis(10), move |_| {
        count_clone.fetch_add(1, Ordering::SeqCst);
        "not valid json {{{".to_string()
    });

    // Wait for agent to be ready (has processed initial heartbeat)

    let json = inject_pool_config(
        r#"{
            "options": {
                "max_retries": 2,
                "retry_on_invalid_response": true
            },
            "steps": [
                {
                    "name": "Start",
                    "action": {"kind": "Pool", "params": {"instructions": {"kind": "Inline", "value": ""}}},
                    "next": []
                }
            ]
        }"#,
        &root,
    );
    let config_file: ConfigFile = serde_json::from_str(&json).expect("parse config");
    let config = config_file.resolve(Path::new(".")).expect("resolve config");

    let initial_tasks = vec![Task::new("Start", StepInputValue(serde_json::json!({})))];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &create_test_invoker(),
        state_log_path: &state_log,
    };

    // Run should return error because task is dropped after all retries
    let result = barnum_config::run(&config, &runner_config, initial_tasks);
    assert!(result.is_err(), "run should fail when tasks are dropped");

    // 1 original + 2 retries = 3 total
    assert_eq!(
        call_count.load(Ordering::SeqCst),
        3,
        "Should attempt 3 times for malformed JSON"
    );

    cleanup_test_dir(&root);
}

/// Test that per-step options override global options.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn per_step_options_override_global() {
    let root = setup_test_dir(&format!("{TEST_DIR}_per_step"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&root);
        return;
    }

    let _pool = TroupeHandle::start(&root);

    let call_count = Arc::new(AtomicUsize::new(0));
    let count_clone = call_count.clone();

    // Agent that always returns invalid response
    let _agent = BarnumTestAgent::start(&root, Duration::from_millis(10), move |_| {
        count_clone.fetch_add(1, Ordering::SeqCst);
        r#"[{"kind": "NonExistent", "value": {}}]"#.to_string()
    });

    // Wait for agent to be ready (has processed initial heartbeat)

    // Global: retry=true, max_retries=5
    // Step: retry=false (override)
    let json = inject_pool_config(
        r#"{
            "options": {
                "max_retries": 5,
                "retry_on_invalid_response": true
            },
            "steps": [
                {
                    "name": "NoRetryStep",
                    "action": {"kind": "Pool", "params": {"instructions": {"kind": "Inline", "value": ""}}},
                    "next": ["End"],
                    "options": {
                        "retry_on_invalid_response": false
                    }
                },
                {
                    "name": "End",
                    "action": {"kind": "Pool", "params": {"instructions": {"kind": "Inline", "value": ""}}},
                    "next": []
                }
            ]
        }"#,
        &root,
    );
    let config_file: ConfigFile = serde_json::from_str(&json).expect("parse config");
    let config = config_file.resolve(Path::new(".")).expect("resolve config");

    let initial_tasks = vec![Task::new(
        "NoRetryStep",
        StepInputValue(serde_json::json!({})),
    )];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &create_test_invoker(),
        state_log_path: &state_log,
    };

    // Run should return error because task is dropped
    let result = barnum_config::run(&config, &runner_config, initial_tasks);
    assert!(result.is_err(), "run should fail when tasks are dropped");

    // Per-step override should prevent retries
    assert_eq!(
        call_count.load(Ordering::SeqCst),
        1,
        "Per-step retry_on_invalid_response=false should override global"
    );

    cleanup_test_dir(&root);
}

/// Test successful recovery after initial failures.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn recovery_on_nth_attempt() {
    let root = setup_test_dir(&format!("{TEST_DIR}_recovery"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&root);
        return;
    }

    let _pool = TroupeHandle::start(&root);

    let call_count = Arc::new(AtomicUsize::new(0));
    let count_clone = call_count.clone();

    // Agent that fails twice, then succeeds
    let _agent = BarnumTestAgent::start(&root, Duration::from_millis(10), move |_| {
        let count = count_clone.fetch_add(1, Ordering::SeqCst);
        if count < 2 {
            // First two attempts: invalid
            r#"[{"kind": "Invalid", "value": {}}]"#.to_string()
        } else {
            // Third attempt: valid
            "[]".to_string()
        }
    });

    // Wait for agent to be ready (has processed initial heartbeat)

    let json = inject_pool_config(
        r#"{
            "options": {
                "max_retries": 5,
                "retry_on_invalid_response": true
            },
            "steps": [
                {
                    "name": "Start",
                    "action": {"kind": "Pool", "params": {"instructions": {"kind": "Inline", "value": ""}}},
                    "next": []
                }
            ]
        }"#,
        &root,
    );
    let config_file: ConfigFile = serde_json::from_str(&json).expect("parse config");
    let config = config_file.resolve(Path::new(".")).expect("resolve config");

    let initial_tasks = vec![Task::new("Start", StepInputValue(serde_json::json!({})))];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &create_test_invoker(),
        state_log_path: &state_log,
    };

    barnum_config::run(&config, &runner_config, initial_tasks).expect("run failed");

    // Should succeed on third attempt
    assert_eq!(
        call_count.load(Ordering::SeqCst),
        3,
        "Should succeed on third attempt after two failures"
    );

    cleanup_test_dir(&root);
}

/// Test that max_retries=0 means no retries at all.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn max_retries_zero_no_retries() {
    let root = setup_test_dir(&format!("{TEST_DIR}_zero_retries"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&root);
        return;
    }

    let _pool = TroupeHandle::start(&root);

    let call_count = Arc::new(AtomicUsize::new(0));
    let count_clone = call_count.clone();

    let _agent = BarnumTestAgent::start(&root, Duration::from_millis(10), move |_| {
        count_clone.fetch_add(1, Ordering::SeqCst);
        r#"[{"kind": "Invalid", "value": {}}]"#.to_string()
    });

    // Wait for agent to be ready (has processed initial heartbeat)

    let json = inject_pool_config(
        r#"{
            "options": {
                "max_retries": 0,
                "retry_on_invalid_response": true
            },
            "steps": [
                {
                    "name": "Start",
                    "action": {"kind": "Pool", "params": {"instructions": {"kind": "Inline", "value": ""}}},
                    "next": []
                }
            ]
        }"#,
        &root,
    );
    let config_file: ConfigFile = serde_json::from_str(&json).expect("parse config");
    let config = config_file.resolve(Path::new(".")).expect("resolve config");

    let initial_tasks = vec![Task::new("Start", StepInputValue(serde_json::json!({})))];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &create_test_invoker(),
        state_log_path: &state_log,
    };

    // Run should return error because task is dropped
    let result = barnum_config::run(&config, &runner_config, initial_tasks);
    assert!(result.is_err(), "run should fail when tasks are dropped");

    // max_retries=0 means only the original attempt
    assert_eq!(
        call_count.load(Ordering::SeqCst),
        1,
        "max_retries=0 should only allow original attempt"
    );

    cleanup_test_dir(&root);
}
