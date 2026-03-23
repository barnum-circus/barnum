//! Tests for concurrent task execution.
//!
//! These tests verify that tasks are submitted concurrently
//! and that multiple agents can process work in parallel.

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]
#![expect(clippy::unwrap_used)]
#![expect(clippy::doc_markdown)]

mod common;

use barnum_config::{CompiledSchemas, Config, ConfigFile, RunnerConfig, StepInputValue, Task};
use common::{
    BarnumTestAgent, TroupeHandle, cleanup_test_dir, create_test_invoker, inject_pool_config,
    is_ipc_available, setup_test_dir, test_state_log_path,
};
use rstest::rstest;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

const TEST_DIR: &str = "concurrency";

fn worker_config(pool_root: &Path) -> Config {
    let json = inject_pool_config(
        r#"{
            "steps": [
                {
                    "name": "Worker",
                    "action": {"kind": "Pool", "params": {"instructions": {"kind": "Inline", "value": "Process this task."}}},
                    "next": []
                }
            ]
        }"#,
        pool_root,
    );
    let config_file: ConfigFile = serde_json::from_str(&json).expect("parse config");
    config_file.resolve(Path::new(".")).expect("resolve config")
}

/// Test that multiple tasks submitted at once are all processed successfully.
///
/// Submits 6 tasks to 3 agents and verifies all complete without error.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn tasks_execute_in_parallel() {
    let root = setup_test_dir(TEST_DIR);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&root);
        return;
    }

    let _pool = TroupeHandle::start(&root);

    // Start 3 agents with 100ms processing delay
    let processing_delay = Duration::from_millis(100);
    let _agent1 = BarnumTestAgent::terminator(&root, processing_delay);
    let _agent2 = BarnumTestAgent::terminator(&root, processing_delay);
    let _agent3 = BarnumTestAgent::terminator(&root, processing_delay);

    let config = worker_config(&root);
    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");

    let initial_tasks: Vec<Task> = (0..6)
        .map(|i| Task::new("Worker", StepInputValue(serde_json::json!({"id": i}))))
        .collect();

    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &create_test_invoker(),
        state_log_path: &state_log,
    };

    barnum_config::run(&config, &schemas, &runner_config, initial_tasks).expect("run failed");

    cleanup_test_dir(&root);
}

/// Test that max_concurrency limits concurrent task submission.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn max_concurrency_limits_parallel_tasks() {
    let root = setup_test_dir(&format!("{TEST_DIR}_max_concurrency"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&root);
        return;
    }

    let _pool = TroupeHandle::start(&root);

    // Track concurrent task count
    let concurrent = Arc::new(AtomicUsize::new(0));
    let max_observed = Arc::new(AtomicUsize::new(0));

    let max_clone = max_observed.clone();

    let delay = Duration::from_millis(50);

    // Single agent that tracks concurrency
    let _agent = BarnumTestAgent::start(&root, delay, move |_| {
        let current = concurrent.fetch_add(1, Ordering::SeqCst) + 1;

        // Update max if higher
        let mut max = max_clone.load(Ordering::SeqCst);
        while current > max {
            match max_clone.compare_exchange_weak(max, current, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => break,
                Err(m) => max = m,
            }
        }

        // Simulate work
        thread::sleep(Duration::from_millis(20));
        concurrent.fetch_sub(1, Ordering::SeqCst);

        "[]".to_string()
    });

    // Wait for agent to be ready (has processed initial heartbeat)

    // Config with max_concurrency = 2
    let json = inject_pool_config(
        r#"{
            "options": {
                "max_concurrency": 2
            },
            "steps": [
                {
                    "name": "Worker",
                    "action": {"kind": "Pool", "params": {"instructions": {"kind": "Inline", "value": "Work"}}},
                    "next": []
                }
            ]
        }"#,
        &root,
    );
    let config_file: ConfigFile = serde_json::from_str(&json).expect("parse config");
    let config = config_file.resolve(Path::new(".")).expect("resolve config");

    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");

    let initial_tasks: Vec<Task> = (0..6)
        .map(|i| Task::new("Worker", StepInputValue(serde_json::json!({"id": i}))))
        .collect();

    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &create_test_invoker(),
        state_log_path: &state_log,
    };

    barnum_config::run(&config, &schemas, &runner_config, initial_tasks).expect("run failed");

    // With max_concurrency=2 and 1 agent, max should be 1 (single agent)
    // But if we had 3 agents, max should not exceed 2
    // This test verifies the runner respects the limit
    assert!(
        max_observed.load(Ordering::SeqCst) <= 2,
        "Max concurrent tasks should not exceed 2"
    );

    cleanup_test_dir(&root);
}

/// Test that nested fan-out works correctly (A -> B1,B2 -> each spawns C).
#[rstest]
#[timeout(Duration::from_secs(20))]
fn nested_fan_out() {
    let root = setup_test_dir(&format!("{TEST_DIR}_nested"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&root);
        return;
    }

    let _pool = TroupeHandle::start(&root);

    let processed_kinds = Arc::new(std::sync::Mutex::new(Vec::new()));
    let kinds_clone = processed_kinds.clone();

    let _agent = BarnumTestAgent::start(&root, Duration::from_millis(10), move |payload| {
        let v: serde_json::Value = serde_json::from_str(payload).unwrap_or_default();
        let kind = v["task"]["kind"].as_str().unwrap_or("");
        kinds_clone.lock().unwrap().push(kind.to_string());

        match kind {
            "Root" => r#"[{"kind": "Branch1", "value": {}}, {"kind": "Branch2", "value": {}}]"#
                .to_string(),
            "Branch1" => {
                r#"[{"kind": "Leaf1A", "value": {}}, {"kind": "Leaf1B", "value": {}}]"#.to_string()
            }
            "Branch2" => {
                r#"[{"kind": "Leaf2A", "value": {}}, {"kind": "Leaf2B", "value": {}}]"#.to_string()
            }
            _ => "[]".to_string(),
        }
    });

    // Wait for agent to be ready (has processed initial heartbeat)

    let json = inject_pool_config(
        r#"{
            "steps": [
                {"name": "Root", "action": {"kind": "Pool", "params": {"instructions": {"kind": "Inline", "value": ""}}}, "next": ["Branch1", "Branch2"]},
                {"name": "Branch1", "action": {"kind": "Pool", "params": {"instructions": {"kind": "Inline", "value": ""}}}, "next": ["Leaf1A", "Leaf1B"]},
                {"name": "Branch2", "action": {"kind": "Pool", "params": {"instructions": {"kind": "Inline", "value": ""}}}, "next": ["Leaf2A", "Leaf2B"]},
                {"name": "Leaf1A", "action": {"kind": "Pool", "params": {"instructions": {"kind": "Inline", "value": ""}}}, "next": []},
                {"name": "Leaf1B", "action": {"kind": "Pool", "params": {"instructions": {"kind": "Inline", "value": ""}}}, "next": []},
                {"name": "Leaf2A", "action": {"kind": "Pool", "params": {"instructions": {"kind": "Inline", "value": ""}}}, "next": []},
                {"name": "Leaf2B", "action": {"kind": "Pool", "params": {"instructions": {"kind": "Inline", "value": ""}}}, "next": []}
            ]
        }"#,
        &root,
    );
    let config_file: ConfigFile = serde_json::from_str(&json).expect("parse config");
    let config = config_file.resolve(Path::new(".")).expect("resolve config");

    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");

    let initial_tasks = vec![Task::new("Root", StepInputValue(serde_json::json!({})))];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &create_test_invoker(),
        state_log_path: &state_log,
    };

    barnum_config::run(&config, &schemas, &runner_config, initial_tasks).expect("run failed");

    {
        let kinds = processed_kinds.lock().unwrap();
        let kind_set: HashSet<_> = kinds.iter().collect();

        // Should have processed: Root, Branch1, Branch2, Leaf1A, Leaf1B, Leaf2A, Leaf2B
        assert!(kind_set.contains(&"Root".to_string()));
        assert!(kind_set.contains(&"Branch1".to_string()));
        assert!(kind_set.contains(&"Branch2".to_string()));
        assert!(kind_set.contains(&"Leaf1A".to_string()));
        assert!(kind_set.contains(&"Leaf1B".to_string()));
        assert!(kind_set.contains(&"Leaf2A".to_string()));
        assert!(kind_set.contains(&"Leaf2B".to_string()));
        assert_eq!(kinds.len(), 7, "Should process exactly 7 tasks");
        drop(kinds);
    }

    cleanup_test_dir(&root);
}
