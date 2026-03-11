//! Tests for edge cases and boundary conditions.

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]
#![expect(clippy::doc_markdown)]

mod common;

use barnum_config::{CompiledSchemas, ConfigFile, RunnerConfig, StepInputValue, Task};
use common::{
    BarnumTestAgent, TroupeHandle, cleanup_test_dir, create_test_invoker, is_ipc_available,
    setup_test_dir,
};
use rstest::rstest;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const TEST_DIR: &str = "edge_cases";

/// Test that empty initial_tasks completes immediately.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn empty_initial_tasks_completes() {
    let root = setup_test_dir(TEST_DIR);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&root);
        return;
    }

    let pool = TroupeHandle::start(&root);

    // No sleep needed - pool is ready after start() returns, and no tasks means no agent needed

    let config_file: ConfigFile = serde_json::from_str(
        r#"{
            "steps": [
                {"name": "Start", "action": {"kind": "Pool", "instructions": {"inline": ""}}, "next": []}
            ]
        }"#,
    )
    .expect("parse config");
    let config = config_file.resolve(Path::new(".")).expect("resolve config");

    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");
    let initial_tasks = vec![]; // Empty!
    let runner_config = RunnerConfig {
        troupe_root: pool.pool_path(),
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &create_test_invoker(),
        state_log_path: None,
    };

    // Should complete immediately without error
    barnum_config::run(&config, &schemas, &runner_config, initial_tasks).expect("run failed");

    cleanup_test_dir(&root);
}

/// Test that large fan-out works correctly.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn large_fan_out() {
    let root = setup_test_dir(&format!("{TEST_DIR}_large_fanout"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&root);
        return;
    }

    let pool = TroupeHandle::start(&root);

    let task_count = Arc::new(AtomicUsize::new(0));
    let count_clone = task_count.clone();

    let _agent = BarnumTestAgent::start(&root, Duration::from_millis(5), move |payload| {
        count_clone.fetch_add(1, Ordering::SeqCst);

        let v: serde_json::Value = serde_json::from_str(payload).unwrap_or_default();
        let kind = v["task"]["kind"].as_str().unwrap_or("");

        if kind == "Distribute" {
            // Fan out to 20 workers
            let workers: Vec<String> = (0..20)
                .map(|i| format!(r#"{{"kind": "Worker", "value": {{"id": {i}}}}}"#))
                .collect();
            format!("[{}]", workers.join(", "))
        } else {
            "[]".to_string()
        }
    });

    // Wait for agent to be ready (has processed initial heartbeat)

    let config_file: ConfigFile = serde_json::from_str(
        r#"{
            "steps": [
                {"name": "Distribute", "action": {"kind": "Pool", "instructions": {"inline": ""}}, "next": ["Worker"]},
                {"name": "Worker", "action": {"kind": "Pool", "instructions": {"inline": ""}}, "next": []}
            ]
        }"#,
    )
    .expect("parse config");
    let config = config_file.resolve(Path::new(".")).expect("resolve config");

    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");
    let initial_tasks = vec![Task::new(
        "Distribute",
        StepInputValue(serde_json::json!({})),
    )];
    let runner_config = RunnerConfig {
        troupe_root: pool.pool_path(),
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &create_test_invoker(),
        state_log_path: None,
    };

    barnum_config::run(&config, &schemas, &runner_config, initial_tasks).expect("run failed");

    // 1 Distribute + 20 Workers = 21 tasks
    assert_eq!(
        task_count.load(Ordering::SeqCst),
        21,
        "Should process 21 tasks (1 distribute + 20 workers)"
    );

    cleanup_test_dir(&root);
}

/// Test Command action executes script correctly.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn command_action_executes() {
    let root = setup_test_dir(&format!("{TEST_DIR}_command"));

    // Command action doesn't need IPC - it runs locally
    // No pool needed since we're only using Command actions
    let config_file: ConfigFile = serde_json::from_str(
        r#"{
            "steps": [
                {
                    "name": "Echo",
                    "action": {"kind": "Command", "script": "jq -c '[{kind: \"Done\", value: .value}]'"},
                    "next": ["Done"]
                },
                {
                    "name": "Done",
                    "action": {"kind": "Command", "script": "jq -c '[]'"},
                    "next": []
                }
            ]
        }"#,
    )
    .expect("parse config");
    let config = config_file.resolve(Path::new(".")).expect("resolve config");

    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");
    let initial_tasks = vec![Task::new(
        "Echo",
        StepInputValue(serde_json::json!({"message": "hello"})),
    )];
    let runner_config = RunnerConfig {
        troupe_root: &root,
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &create_test_invoker(),
        state_log_path: None,
    };

    // Should complete without error
    barnum_config::run(&config, &schemas, &runner_config, initial_tasks).expect("run failed");

    cleanup_test_dir(&root);
}

/// Test that runner handles rapid task completion.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn rapid_task_completion() {
    let root = setup_test_dir(&format!("{TEST_DIR}_rapid"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&root);
        return;
    }

    let pool = TroupeHandle::start(&root);

    // Agent with minimal delay (zero can cause races)
    let _agent = BarnumTestAgent::terminator(&root, Duration::from_millis(1));

    // Wait for agent to be ready (has processed initial heartbeat)

    let config_file: ConfigFile = serde_json::from_str(
        r#"{
            "steps": [
                {"name": "Fast", "action": {"kind": "Pool", "instructions": {"inline": ""}}, "next": []}
            ]
        }"#,
    )
    .expect("parse config");
    let config = config_file.resolve(Path::new(".")).expect("resolve config");

    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");

    // Submit many tasks
    let initial_tasks: Vec<Task> = (0..50)
        .map(|i| Task::new("Fast", StepInputValue(serde_json::json!({"id": i}))))
        .collect();

    let runner_config = RunnerConfig {
        troupe_root: pool.pool_path(),
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &create_test_invoker(),
        state_log_path: None,
    };

    barnum_config::run(&config, &schemas, &runner_config, initial_tasks).expect("run failed");

    cleanup_test_dir(&root);
}

/// Test that unknown step in initial tasks returns an error.
#[rstest]
#[timeout(Duration::from_secs(5))]
fn unknown_step_in_initial_tasks_returns_error() {
    let root = setup_test_dir(&format!("{TEST_DIR}_unknown_step"));

    let config_file: ConfigFile = serde_json::from_str(
        r#"{
            "steps": [
                {"name": "Known", "action": {"kind": "Pool", "instructions": {"inline": ""}}, "next": []}
            ]
        }"#,
    )
    .expect("parse config");
    let config = config_file.resolve(Path::new(".")).expect("resolve config");

    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");
    let initial_tasks = vec![
        Task::new("Unknown", StepInputValue(serde_json::json!({}))), // Unknown step - should error
    ];
    let runner_config = RunnerConfig {
        troupe_root: &root,
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &create_test_invoker(),
        state_log_path: None,
    };

    // Should return an error for unknown step
    let result = barnum_config::run(&config, &schemas, &runner_config, initial_tasks);
    assert!(result.is_err(), "should error for unknown step");
    let err = result.expect_err("should error").to_string();
    assert!(
        err.contains("E019") && err.contains("Unknown"),
        "error should mention unknown step: {err}"
    );

    cleanup_test_dir(&root);
}

/// Test that invalid value schema in initial tasks returns an error.
#[rstest]
#[timeout(Duration::from_secs(5))]
fn invalid_value_schema_in_initial_tasks_returns_error() {
    let root = setup_test_dir(&format!("{TEST_DIR}_invalid_schema"));

    // Config with schema requiring "name" field
    let config_file: ConfigFile = serde_json::from_str(
        r#"{
            "steps": [
                {
                    "name": "Validated",
                    "value_schema": {
                        "type": "object",
                        "required": ["name"],
                        "properties": {"name": {"type": "string"}}
                    },
                    "action": {"kind": "Pool", "instructions": {"inline": ""}},
                    "next": []
                }
            ]
        }"#,
    )
    .expect("parse config");
    let config = config_file.resolve(Path::new(".")).expect("resolve config");

    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");
    let initial_tasks = vec![
        Task::new("Validated", StepInputValue(serde_json::json!({}))), // Missing required "name" - should error
    ];
    let runner_config = RunnerConfig {
        troupe_root: &root,
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &create_test_invoker(),
        state_log_path: None,
    };

    // Should return an error for invalid value schema
    let result = barnum_config::run(&config, &schemas, &runner_config, initial_tasks);
    assert!(result.is_err(), "should error for invalid value schema");
    let err = result.expect_err("should error").to_string();
    assert!(
        err.contains("E020"),
        "error should mention validation failure: {err}"
    );

    cleanup_test_dir(&root);
}
