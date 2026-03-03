//! Tests for edge cases and boundary conditions.

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]
#![expect(clippy::doc_markdown)]

mod common;

use common::{
    AgentPoolHandle, GsdTestAgent, cleanup_test_dir, find_agent_pool_binary, is_ipc_available,
    setup_test_dir,
};
use gsd_config::{CompiledSchemas, Config, RunnerConfig, Task, TaskRunner};
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

    let _pool = AgentPoolHandle::start(&root);

    // No sleep needed - pool is ready after start() returns, and no tasks means no agent needed

    let config: Config = serde_json::from_str(
        r#"{
            "steps": [
                {"name": "Start", "action": {"kind": "Pool", "instructions": ""}, "next": []}
            ]
        }"#,
    )
    .expect("parse config");

    let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");
    let runner_config = RunnerConfig {
        agent_pool_root: &root,
        config_base_path: Path::new("."),
        wake_script: None,
        initial_tasks: vec![], // Empty!
        agent_pool_binary: Some(&find_agent_pool_binary()),
    };

    // Should complete immediately without error
    gsd_config::run(&config, &schemas, runner_config).expect("run failed");

    cleanup_test_dir(TEST_DIR);
}

/// Test that TaskRunner with empty initial_tasks is_empty from start.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn empty_runner_is_empty() {
    let root = setup_test_dir(&format!("{TEST_DIR}_empty_runner"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&format!("{TEST_DIR}_empty_runner"));
        return;
    }

    let _pool = AgentPoolHandle::start(&root);

    // No sleep needed - pool is ready after start() returns, and no tasks means no agent needed

    let config: Config =
        serde_json::from_str(r#"{"steps": [{"name": "X", "next": []}]}"#).expect("parse config");

    let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");
    let runner_config = RunnerConfig {
        agent_pool_root: &root,
        config_base_path: Path::new("."),
        wake_script: None,
        initial_tasks: vec![],
        agent_pool_binary: Some(&find_agent_pool_binary()),
    };

    let mut runner = TaskRunner::new(&config, &schemas, runner_config).expect("create runner");

    assert!(runner.is_empty(), "Runner with no tasks should be empty");
    assert_eq!(runner.pending(), 0, "Pending count should be 0");
    assert!(
        runner.next().is_none(),
        "next() should return None immediately"
    );

    cleanup_test_dir(&format!("{TEST_DIR}_empty_runner"));
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

    let _pool = AgentPoolHandle::start(&root);
    let _agent = GsdTestAgent::terminator(&root, "agent", Duration::from_millis(10));

    // Wait for agent to be ready (has processed initial heartbeat)

    let config: Config = serde_json::from_str(
        r#"{
            "steps": [
                {"name": "Known", "action": {"kind": "Pool", "instructions": ""}, "next": []}
            ]
        }"#,
    )
    .expect("parse config");

    let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");
    let runner_config = RunnerConfig {
        agent_pool_root: &root,
        config_base_path: Path::new("."),
        wake_script: None,
        initial_tasks: vec![
            Task::new("Unknown", serde_json::json!({})), // Unknown step
            Task::new("Known", serde_json::json!({})),   // Known step
        ],
        agent_pool_binary: Some(&find_agent_pool_binary()),
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

    let _pool = AgentPoolHandle::start(&root);

    let call_count = Arc::new(AtomicUsize::new(0));
    let count_clone = call_count.clone();

    let _agent = GsdTestAgent::start(&root, "agent", Duration::from_millis(10), move |_| {
        count_clone.fetch_add(1, Ordering::SeqCst);
        "[]".to_string()
    });

    // Wait for agent to be ready (has processed initial heartbeat)

    // Config with schema requiring "name" field
    let config: Config = serde_json::from_str(
        r#"{
            "steps": [
                {
                    "name": "Validated",
                    "value_schema": {
                        "type": "object",
                        "required": ["name"],
                        "properties": {"name": {"type": "string"}}
                    },
                    "action": {"kind": "Pool", "instructions": ""},
                    "next": []
                }
            ]
        }"#,
    )
    .expect("parse config");

    let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");
    let runner_config = RunnerConfig {
        agent_pool_root: &root,
        config_base_path: Path::new("."),
        wake_script: None,
        initial_tasks: vec![
            Task::new("Validated", serde_json::json!({})), // Missing required "name"
            Task::new("Validated", serde_json::json!({"name": "ok"})), // Valid
        ],
        agent_pool_binary: Some(&find_agent_pool_binary()),
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

    let _pool = AgentPoolHandle::start(&root);

    let task_count = Arc::new(AtomicUsize::new(0));
    let count_clone = task_count.clone();

    let _agent = GsdTestAgent::start(
        &root,
        "fanout-agent",
        Duration::from_millis(5),
        move |payload| {
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
        },
    );

    // Wait for agent to be ready (has processed initial heartbeat)

    let config: Config = serde_json::from_str(
        r#"{
            "steps": [
                {"name": "Distribute", "action": {"kind": "Pool", "instructions": ""}, "next": ["Worker"]},
                {"name": "Worker", "action": {"kind": "Pool", "instructions": ""}, "next": []}
            ]
        }"#,
    )
    .expect("parse config");

    let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");
    let runner_config = RunnerConfig {
        agent_pool_root: &root,
        config_base_path: Path::new("."),
        wake_script: None,
        initial_tasks: vec![Task::new("Distribute", serde_json::json!({}))],
        agent_pool_binary: Some(&find_agent_pool_binary()),
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
    let config: Config = serde_json::from_str(
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

    let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");
    let runner_config = RunnerConfig {
        agent_pool_root: &root,
        config_base_path: Path::new("."),
        wake_script: None,
        initial_tasks: vec![Task::new("Echo", serde_json::json!({"message": "hello"}))],
        agent_pool_binary: Some(&find_agent_pool_binary()),
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

    let _pool = AgentPoolHandle::start(&root);

    // Agent with minimal delay (zero can cause races)
    let _agent = GsdTestAgent::terminator(&root, "fast-agent", Duration::from_millis(1));

    // Wait for agent to be ready (has processed initial heartbeat)

    let config: Config = serde_json::from_str(
        r#"{
            "steps": [
                {"name": "Fast", "action": {"kind": "Pool", "instructions": ""}, "next": []}
            ]
        }"#,
    )
    .expect("parse config");

    let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");

    // Submit many tasks
    let initial_tasks: Vec<Task> = (0..50)
        .map(|i| Task::new("Fast", serde_json::json!({"id": i})))
        .collect();

    let runner_config = RunnerConfig {
        agent_pool_root: &root,
        config_base_path: Path::new("."),
        wake_script: None,
        initial_tasks,
        agent_pool_binary: Some(&find_agent_pool_binary()),
    };

    gsd_config::run(&config, &schemas, runner_config).expect("run failed");

    cleanup_test_dir(&format!("{TEST_DIR}_rapid"));
}

/// Test that TaskRunner.pending() tracks queue size correctly.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn pending_count_accurate() {
    let root = setup_test_dir(&format!("{TEST_DIR}_pending"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&format!("{TEST_DIR}_pending"));
        return;
    }

    let _pool = AgentPoolHandle::start(&root);
    let _agent = GsdTestAgent::terminator(&root, "agent", Duration::from_millis(50));

    // Wait for agent to be ready (has processed initial heartbeat)

    let config: Config =
        serde_json::from_str(r#"{"steps": [{"name": "Work", "next": []}]}"#).expect("parse config");

    let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");
    let runner_config = RunnerConfig {
        agent_pool_root: &root,
        config_base_path: Path::new("."),
        wake_script: None,
        initial_tasks: vec![
            Task::new("Work", serde_json::json!({})),
            Task::new("Work", serde_json::json!({})),
            Task::new("Work", serde_json::json!({})),
        ],
        agent_pool_binary: Some(&find_agent_pool_binary()),
    };

    let runner = TaskRunner::new(&config, &schemas, runner_config).expect("create runner");

    // Initial pending should be 3 (before any submission)
    assert_eq!(runner.pending(), 3, "Initial pending should be 3");

    cleanup_test_dir(&format!("{TEST_DIR}_pending"));
}
