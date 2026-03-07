//! Tests for edge cases and boundary conditions.

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]
#![expect(clippy::doc_markdown)]

mod common;

use common::{
    AgentPoolHandle, GsdTestAgent, cleanup_test_dir, create_test_invoker, is_ipc_available,
    setup_test_dir,
};
use gsd_config::{CompiledSchemas, ConfigFile, RunnerConfig, Task};
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
        cleanup_test_dir(TEST_DIR);
        return;
    }

    let pool = AgentPoolHandle::start(&root);

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
    let runner_config = RunnerConfig {
        agent_pool_root: pool.pool_path(),
        working_dir: Path::new("."),
        wake_script: None,
        initial_tasks: vec![], // Empty!
        invoker: &create_test_invoker(),
    };

    // Should complete immediately without error
    gsd_config::run(&config, &schemas, runner_config).expect("run failed");

    cleanup_test_dir(TEST_DIR);
}

/// Test that unknown step in initial_tasks is skipped gracefully.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn unknown_initial_step_skipped() {
    let root = setup_test_dir(&format!("{TEST_DIR}_unknown_step"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&format!("{TEST_DIR}_unknown_step"));
        return;
    }

    let pool = AgentPoolHandle::start(&root);
    let _agent = GsdTestAgent::terminator(&root, Duration::from_millis(10));

    // Wait for agent to be ready (has processed initial heartbeat)

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
    let runner_config = RunnerConfig {
        agent_pool_root: pool.pool_path(),
        working_dir: Path::new("."),
        wake_script: None,
        initial_tasks: vec![
            Task::new("Unknown", serde_json::json!({})), // Unknown step
            Task::new("Known", serde_json::json!({})),   // Known step
        ],
        invoker: &create_test_invoker(),
    };

    // Should complete - unknown task skipped, known task processed
    gsd_config::run(&config, &schemas, runner_config).expect("run failed");

    cleanup_test_dir(&format!("{TEST_DIR}_unknown_step"));
}

/// Test that task with invalid value schema is skipped.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn invalid_value_schema_skipped() {
    let root = setup_test_dir(&format!("{TEST_DIR}_invalid_schema"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&format!("{TEST_DIR}_invalid_schema"));
        return;
    }

    let pool = AgentPoolHandle::start(&root);

    let call_count = Arc::new(AtomicUsize::new(0));
    let count_clone = call_count.clone();

    let _agent = GsdTestAgent::start(&root, Duration::from_millis(10), move |_| {
        count_clone.fetch_add(1, Ordering::SeqCst);
        "[]".to_string()
    });

    // Wait for agent to be ready (has processed initial heartbeat)

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
    let runner_config = RunnerConfig {
        agent_pool_root: pool.pool_path(),
        working_dir: Path::new("."),
        wake_script: None,
        initial_tasks: vec![
            Task::new("Validated", serde_json::json!({})), // Missing required "name"
            Task::new("Validated", serde_json::json!({"name": "ok"})), // Valid
        ],
        invoker: &create_test_invoker(),
    };

    gsd_config::run(&config, &schemas, runner_config).expect("run failed");

    // Only the valid task should be processed
    assert_eq!(
        call_count.load(Ordering::SeqCst),
        1,
        "Only valid task should be processed"
    );

    cleanup_test_dir(&format!("{TEST_DIR}_invalid_schema"));
}

/// Test that large fan-out works correctly.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn large_fan_out() {
    let root = setup_test_dir(&format!("{TEST_DIR}_large_fanout"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&format!("{TEST_DIR}_large_fanout"));
        return;
    }

    let pool = AgentPoolHandle::start(&root);

    let task_count = Arc::new(AtomicUsize::new(0));
    let count_clone = task_count.clone();

    let _agent = GsdTestAgent::start(&root, Duration::from_millis(5), move |payload| {
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
    let runner_config = RunnerConfig {
        agent_pool_root: pool.pool_path(),
        working_dir: Path::new("."),
        wake_script: None,
        initial_tasks: vec![Task::new("Distribute", serde_json::json!({}))],
        invoker: &create_test_invoker(),
    };

    gsd_config::run(&config, &schemas, runner_config).expect("run failed");

    // 1 Distribute + 20 Workers = 21 tasks
    assert_eq!(
        task_count.load(Ordering::SeqCst),
        21,
        "Should process 21 tasks (1 distribute + 20 workers)"
    );

    cleanup_test_dir(&format!("{TEST_DIR}_large_fanout"));
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
    let runner_config = RunnerConfig {
        agent_pool_root: &root,
        working_dir: Path::new("."),
        wake_script: None,
        initial_tasks: vec![Task::new("Echo", serde_json::json!({"message": "hello"}))],
        invoker: &create_test_invoker(),
    };

    // Should complete without error
    gsd_config::run(&config, &schemas, runner_config).expect("run failed");

    cleanup_test_dir(&format!("{TEST_DIR}_command"));
}

/// Test that runner handles rapid task completion.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn rapid_task_completion() {
    let root = setup_test_dir(&format!("{TEST_DIR}_rapid"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&format!("{TEST_DIR}_rapid"));
        return;
    }

    let pool = AgentPoolHandle::start(&root);

    // Agent with minimal delay (zero can cause races)
    let _agent = GsdTestAgent::terminator(&root, Duration::from_millis(1));

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
        .map(|i| Task::new("Fast", serde_json::json!({"id": i})))
        .collect();

    let runner_config = RunnerConfig {
        agent_pool_root: pool.pool_path(),
        working_dir: Path::new("."),
        wake_script: None,
        initial_tasks,
        invoker: &create_test_invoker(),
    };

    gsd_config::run(&config, &schemas, runner_config).expect("run failed");

    cleanup_test_dir(&format!("{TEST_DIR}_rapid"));
}
