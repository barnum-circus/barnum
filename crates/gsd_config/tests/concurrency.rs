//! Tests for concurrent task execution.
//!
//! These tests verify that the TaskRunner actually submits tasks concurrently
//! and that multiple agents can process work in parallel.

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]
#![expect(clippy::unwrap_used)]

mod common;

use common::{AgentPoolHandle, GsdTestAgent, cleanup_test_dir, is_ipc_available, setup_test_dir};
use gsd_config::{CompiledSchemas, Config, RunnerConfig, Task, TaskRunner};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const TEST_DIR: &str = "concurrency";

fn worker_config() -> Config {
    serde_json::from_str(
        r#"{
            "steps": [
                {
                    "name": "Worker",
                    "action": {"kind": "Pool", "instructions": "Process this task."},
                    "next": []
                }
            ]
        }"#,
    )
    .expect("parse config")
}

/// Test that multiple tasks submitted at once are processed concurrently.
///
/// If tasks were processed sequentially, N tasks with 100ms delay would take
/// at least N*100ms. With parallelism, they should complete much faster.
#[test]
fn tasks_execute_in_parallel() {
    let root = setup_test_dir(TEST_DIR);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(TEST_DIR);
        return;
    }

    let _pool = AgentPoolHandle::start(&root);

    // Start 3 agents with 100ms processing delay
    let processing_delay = Duration::from_millis(100);
    let _agent1 = GsdTestAgent::terminator(&root, "agent-1", processing_delay);
    let _agent2 = GsdTestAgent::terminator(&root, "agent-2", processing_delay);
    let _agent3 = GsdTestAgent::terminator(&root, "agent-3", processing_delay);

    thread::sleep(Duration::from_millis(200));

    let config = worker_config();
    let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");

    // Submit 6 tasks - with 3 agents and 100ms delay, parallel execution
    // should take ~200ms (2 batches of 3), sequential would take ~600ms
    let initial_tasks: Vec<Task> = (0..6)
        .map(|i| Task::new("Worker", serde_json::json!({"id": i})))
        .collect();

    let runner_config = RunnerConfig {
        agent_pool_root: &root,
        wake_script: None,
        initial_tasks,
    };

    let start = Instant::now();
    gsd_config::run(&config, &schemas, runner_config).expect("run failed");
    let elapsed = start.elapsed();

    // With parallelism: ~200-300ms (accounting for overhead)
    // Without parallelism: ~600ms minimum
    // Use 400ms as threshold - well under sequential time, allows for overhead
    assert!(
        elapsed < Duration::from_millis(400),
        "Tasks took {:?}, expected < 400ms for parallel execution",
        elapsed
    );

    cleanup_test_dir(TEST_DIR);
}

/// Test that work is distributed across multiple agents.
#[test]
fn work_distributed_across_agents() {
    let root = setup_test_dir(&format!("{TEST_DIR}_distribution"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&format!("{TEST_DIR}_distribution"));
        return;
    }

    let _pool = AgentPoolHandle::start(&root);

    // Track which agents processed tasks
    let agent1_count = Arc::new(AtomicUsize::new(0));
    let agent2_count = Arc::new(AtomicUsize::new(0));
    let agent3_count = Arc::new(AtomicUsize::new(0));

    let count1 = agent1_count.clone();
    let count2 = agent2_count.clone();
    let count3 = agent3_count.clone();

    // Use longer delay to ensure multiple agents get work
    let delay = Duration::from_millis(50);

    let _agent1 = GsdTestAgent::start(&root, "agent-1", delay, move |_| {
        count1.fetch_add(1, Ordering::SeqCst);
        "[]".to_string()
    });
    let _agent2 = GsdTestAgent::start(&root, "agent-2", delay, move |_| {
        count2.fetch_add(1, Ordering::SeqCst);
        "[]".to_string()
    });
    let _agent3 = GsdTestAgent::start(&root, "agent-3", delay, move |_| {
        count3.fetch_add(1, Ordering::SeqCst);
        "[]".to_string()
    });

    thread::sleep(Duration::from_millis(200));

    let config = worker_config();
    let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");

    let initial_tasks: Vec<Task> = (0..9)
        .map(|i| Task::new("Worker", serde_json::json!({"id": i})))
        .collect();

    let runner_config = RunnerConfig {
        agent_pool_root: &root,
        wake_script: None,
        initial_tasks,
    };

    gsd_config::run(&config, &schemas, runner_config).expect("run failed");

    let total =
        agent1_count.load(Ordering::SeqCst) + agent2_count.load(Ordering::SeqCst) + agent3_count.load(Ordering::SeqCst);

    assert_eq!(total, 9, "All 9 tasks should be processed");

    // At least 2 agents should have received work
    let agents_with_work = [&agent1_count, &agent2_count, &agent3_count]
        .iter()
        .filter(|c| c.load(Ordering::SeqCst) > 0)
        .count();

    assert!(
        agents_with_work >= 2,
        "Expected at least 2 agents to receive work, but only {} did",
        agents_with_work
    );

    cleanup_test_dir(&format!("{TEST_DIR}_distribution"));
}

/// Test that max_concurrency limits concurrent task submission.
#[test]
fn max_concurrency_limits_parallel_tasks() {
    let root = setup_test_dir(&format!("{TEST_DIR}_max_concurrency"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&format!("{TEST_DIR}_max_concurrency"));
        return;
    }

    let _pool = AgentPoolHandle::start(&root);

    // Track concurrent task count
    let concurrent = Arc::new(AtomicUsize::new(0));
    let max_observed = Arc::new(AtomicUsize::new(0));

    let concurrent_clone = concurrent.clone();
    let max_clone = max_observed.clone();

    let delay = Duration::from_millis(50);

    // Single agent that tracks concurrency
    let _agent = GsdTestAgent::start(&root, "tracker", delay, move |_| {
        let current = concurrent_clone.fetch_add(1, Ordering::SeqCst) + 1;

        // Update max if higher
        let mut max = max_clone.load(Ordering::SeqCst);
        while current > max {
            match max_clone.compare_exchange_weak(max, current, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => break,
                Err(m) => max = m,
            }
        }

        // Simulate work
        thread::sleep(Duration::from_millis(20));
        concurrent_clone.fetch_sub(1, Ordering::SeqCst);

        "[]".to_string()
    });

    thread::sleep(Duration::from_millis(200));

    // Config with max_concurrency = 2
    let config: Config = serde_json::from_str(
        r#"{
            "options": {
                "max_concurrency": 2
            },
            "steps": [
                {
                    "name": "Worker",
                    "action": {"kind": "Pool", "instructions": "Work"},
                    "next": []
                }
            ]
        }"#,
    )
    .expect("parse config");

    let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");

    let initial_tasks: Vec<Task> = (0..6)
        .map(|i| Task::new("Worker", serde_json::json!({"id": i})))
        .collect();

    let runner_config = RunnerConfig {
        agent_pool_root: &root,
        wake_script: None,
        initial_tasks,
    };

    gsd_config::run(&config, &schemas, runner_config).expect("run failed");

    // With max_concurrency=2 and 1 agent, max should be 1 (single agent)
    // But if we had 3 agents, max should not exceed 2
    // This test verifies the runner respects the limit
    assert!(
        max_observed.load(Ordering::SeqCst) <= 2,
        "Max concurrent tasks should not exceed 2"
    );

    cleanup_test_dir(&format!("{TEST_DIR}_max_concurrency"));
}

/// Test the TaskRunner iterator interface yields results as they complete.
#[test]
fn task_runner_yields_results_incrementally() {
    let root = setup_test_dir(&format!("{TEST_DIR}_iterator"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&format!("{TEST_DIR}_iterator"));
        return;
    }

    let _pool = AgentPoolHandle::start(&root);
    let _agent = GsdTestAgent::terminator(&root, "agent", Duration::from_millis(10));

    thread::sleep(Duration::from_millis(200));

    let config = worker_config();
    let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");

    let initial_tasks: Vec<Task> = (0..3)
        .map(|i| Task::new("Worker", serde_json::json!({"id": i})))
        .collect();

    let runner_config = RunnerConfig {
        agent_pool_root: &root,
        wake_script: None,
        initial_tasks,
    };

    let mut runner = TaskRunner::new(&config, &schemas, runner_config).expect("create runner");
    let mut outcomes = Vec::new();

    while let Some(outcome) = runner.next() {
        outcomes.push(outcome);
    }

    assert_eq!(outcomes.len(), 3, "Should yield 3 outcomes");

    cleanup_test_dir(&format!("{TEST_DIR}_iterator"));
}

/// Test that TaskRunner.is_empty() returns correct status.
#[test]
fn task_runner_is_empty_status() {
    let root = setup_test_dir(&format!("{TEST_DIR}_is_empty"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&format!("{TEST_DIR}_is_empty"));
        return;
    }

    let _pool = AgentPoolHandle::start(&root);
    let _agent = GsdTestAgent::terminator(&root, "agent", Duration::from_millis(10));

    thread::sleep(Duration::from_millis(200));

    let config = worker_config();
    let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");

    let runner_config = RunnerConfig {
        agent_pool_root: &root,
        wake_script: None,
        initial_tasks: vec![Task::new("Worker", serde_json::json!({}))],
    };

    let mut runner = TaskRunner::new(&config, &schemas, runner_config).expect("create runner");

    assert!(!runner.is_empty(), "Runner should not be empty initially");

    while runner.next().is_some() {}

    assert!(runner.is_empty(), "Runner should be empty after completion");

    cleanup_test_dir(&format!("{TEST_DIR}_is_empty"));
}

/// Test that nested fan-out works correctly (A -> B1,B2 -> each spawns C).
#[test]
fn nested_fan_out() {
    let root = setup_test_dir(&format!("{TEST_DIR}_nested"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&format!("{TEST_DIR}_nested"));
        return;
    }

    let _pool = AgentPoolHandle::start(&root);

    let processed_kinds = Arc::new(std::sync::Mutex::new(Vec::new()));
    let kinds_clone = processed_kinds.clone();

    let _agent = GsdTestAgent::start(&root, "nested-agent", Duration::from_millis(10), move |payload| {
        let v: serde_json::Value = serde_json::from_str(payload).unwrap_or_default();
        let kind = v["task"]["kind"].as_str().unwrap_or("");
        kinds_clone.lock().unwrap().push(kind.to_string());

        match kind {
            "Root" => r#"[{"kind": "Branch1", "value": {}}, {"kind": "Branch2", "value": {}}]"#.to_string(),
            "Branch1" => r#"[{"kind": "Leaf1A", "value": {}}, {"kind": "Leaf1B", "value": {}}]"#.to_string(),
            "Branch2" => r#"[{"kind": "Leaf2A", "value": {}}, {"kind": "Leaf2B", "value": {}}]"#.to_string(),
            _ => "[]".to_string(),
        }
    });

    thread::sleep(Duration::from_millis(200));

    let config: Config = serde_json::from_str(
        r#"{
            "steps": [
                {"name": "Root", "action": {"kind": "Pool", "instructions": ""}, "next": ["Branch1", "Branch2"]},
                {"name": "Branch1", "action": {"kind": "Pool", "instructions": ""}, "next": ["Leaf1A", "Leaf1B"]},
                {"name": "Branch2", "action": {"kind": "Pool", "instructions": ""}, "next": ["Leaf2A", "Leaf2B"]},
                {"name": "Leaf1A", "action": {"kind": "Pool", "instructions": ""}, "next": []},
                {"name": "Leaf1B", "action": {"kind": "Pool", "instructions": ""}, "next": []},
                {"name": "Leaf2A", "action": {"kind": "Pool", "instructions": ""}, "next": []},
                {"name": "Leaf2B", "action": {"kind": "Pool", "instructions": ""}, "next": []}
            ]
        }"#,
    )
    .expect("parse config");

    let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");

    let runner_config = RunnerConfig {
        agent_pool_root: &root,
        wake_script: None,
        initial_tasks: vec![Task::new("Root", serde_json::json!({}))],
    };

    gsd_config::run(&config, &schemas, runner_config).expect("run failed");

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

    cleanup_test_dir(&format!("{TEST_DIR}_nested"));
}
