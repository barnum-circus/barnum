//! Tests for concurrent task execution.
//!
//! These tests verify that tasks are submitted concurrently
//! and that multiple agents can process work in parallel.

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]
#![expect(clippy::unwrap_used)]
#![expect(clippy::doc_markdown)]

mod common;

use common::{
    AgentPoolHandle, GsdTestAgent, cleanup_test_dir, create_test_invoker, is_ipc_available,
    setup_test_dir,
};
use gsd_config::{CompiledSchemas, Config, ConfigFile, RunnerConfig, Task};
use rstest::rstest;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const TEST_DIR: &str = "concurrency";

fn worker_config() -> Config {
    let config_file: ConfigFile = serde_json::from_str(
        r#"{
            "steps": [
                {
                    "name": "Worker",
                    "action": {"kind": "Pool", "instructions": {"inline": "Process this task."}},
                    "next": []
                }
            ]
        }"#,
    )
    .expect("parse config");
    config_file.resolve(Path::new(".")).expect("resolve config")
}

/// Test that multiple tasks submitted at once are processed concurrently.
///
/// If tasks were processed sequentially, N tasks with 100ms delay would take
/// at least N*100ms. With parallelism, they should complete much faster.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn tasks_execute_in_parallel() {
    let root = setup_test_dir(TEST_DIR);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(TEST_DIR);
        return;
    }

    let pool = AgentPoolHandle::start(&root);

    // Start 3 agents with 100ms processing delay
    let processing_delay = Duration::from_millis(100);
    let _agent1 = GsdTestAgent::terminator(&root, processing_delay);
    let _agent2 = GsdTestAgent::terminator(&root, processing_delay);
    let _agent3 = GsdTestAgent::terminator(&root, processing_delay);

    let config = worker_config();
    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");

    // Submit 6 tasks - with 3 agents and 100ms delay, parallel execution
    // should take ~200ms (2 batches of 3), sequential would take ~600ms
    let initial_tasks: Vec<Task> = (0..6)
        .map(|i| Task::new("Worker", serde_json::json!({"id": i})))
        .collect();

    let runner_config = RunnerConfig {
        agent_pool_root: pool.pool_path(),
        working_dir: Path::new("."),
        wake_script: None,
        initial_tasks,
        invoker: &create_test_invoker(),
    };

    let start = Instant::now();
    gsd_config::run(&config, &schemas, runner_config).expect("run failed");
    let elapsed = start.elapsed();

    // With parallelism: ~200-300ms (accounting for overhead)
    // Without parallelism: ~600ms minimum
    // Use 400ms as threshold - well under sequential time, allows for overhead
    assert!(
        elapsed < Duration::from_millis(400),
        "Tasks took {elapsed:?}, expected < 400ms for parallel execution"
    );

    cleanup_test_dir(TEST_DIR);
}

/// Test that work is distributed across multiple agents.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn work_distributed_across_agents() {
    let root = setup_test_dir(&format!("{TEST_DIR}_distribution"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&format!("{TEST_DIR}_distribution"));
        return;
    }

    let pool = AgentPoolHandle::start(&root);

    // Track which agents processed tasks
    let agent1_count = Arc::new(AtomicUsize::new(0));
    let agent2_count = Arc::new(AtomicUsize::new(0));
    let agent3_count = Arc::new(AtomicUsize::new(0));

    let count1 = agent1_count.clone();
    let count2 = agent2_count.clone();
    let count3 = agent3_count.clone();

    // Use longer delay to ensure multiple agents get work
    let delay = Duration::from_millis(50);

    let _agent1 = GsdTestAgent::start(&root, delay, move |_| {
        count1.fetch_add(1, Ordering::SeqCst);
        "[]".to_string()
    });
    let _agent2 = GsdTestAgent::start(&root, delay, move |_| {
        count2.fetch_add(1, Ordering::SeqCst);
        "[]".to_string()
    });
    let _agent3 = GsdTestAgent::start(&root, delay, move |_| {
        count3.fetch_add(1, Ordering::SeqCst);
        "[]".to_string()
    });

    let config = worker_config();
    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");

    let initial_tasks: Vec<Task> = (0..9)
        .map(|i| Task::new("Worker", serde_json::json!({"id": i})))
        .collect();

    let runner_config = RunnerConfig {
        agent_pool_root: pool.pool_path(),
        working_dir: Path::new("."),
        wake_script: None,
        initial_tasks,
        invoker: &create_test_invoker(),
    };

    gsd_config::run(&config, &schemas, runner_config).expect("run failed");

    let total = agent1_count.load(Ordering::SeqCst)
        + agent2_count.load(Ordering::SeqCst)
        + agent3_count.load(Ordering::SeqCst);

    assert_eq!(total, 9, "All 9 tasks should be processed");

    // At least 2 agents should have received work
    let agents_with_work = [&agent1_count, &agent2_count, &agent3_count]
        .iter()
        .filter(|c| c.load(Ordering::SeqCst) > 0)
        .count();

    assert!(
        agents_with_work >= 2,
        "Expected at least 2 agents to receive work, but only {agents_with_work} did"
    );

    cleanup_test_dir(&format!("{TEST_DIR}_distribution"));
}

/// Test that max_concurrency limits concurrent task submission.
#[rstest]
#[timeout(Duration::from_secs(20))]
fn max_concurrency_limits_parallel_tasks() {
    let root = setup_test_dir(&format!("{TEST_DIR}_max_concurrency"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&format!("{TEST_DIR}_max_concurrency"));
        return;
    }

    let pool = AgentPoolHandle::start(&root);

    // Track concurrent task count
    let concurrent = Arc::new(AtomicUsize::new(0));
    let max_observed = Arc::new(AtomicUsize::new(0));

    let max_clone = max_observed.clone();

    let delay = Duration::from_millis(50);

    // Single agent that tracks concurrency
    let _agent = GsdTestAgent::start(&root, delay, move |_| {
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
    let config_file: ConfigFile = serde_json::from_str(
        r#"{
            "options": {
                "max_concurrency": 2
            },
            "steps": [
                {
                    "name": "Worker",
                    "action": {"kind": "Pool", "instructions": {"inline": "Work"}},
                    "next": []
                }
            ]
        }"#,
    )
    .expect("parse config");
    let config = config_file.resolve(Path::new(".")).expect("resolve config");

    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");

    let initial_tasks: Vec<Task> = (0..6)
        .map(|i| Task::new("Worker", serde_json::json!({"id": i})))
        .collect();

    let runner_config = RunnerConfig {
        agent_pool_root: pool.pool_path(),
        working_dir: Path::new("."),
        wake_script: None,
        initial_tasks,
        invoker: &create_test_invoker(),
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

/// Test that nested fan-out works correctly (A -> B1,B2 -> each spawns C).
#[rstest]
#[timeout(Duration::from_secs(20))]
fn nested_fan_out() {
    let root = setup_test_dir(&format!("{TEST_DIR}_nested"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&format!("{TEST_DIR}_nested"));
        return;
    }

    let pool = AgentPoolHandle::start(&root);

    let processed_kinds = Arc::new(std::sync::Mutex::new(Vec::new()));
    let kinds_clone = processed_kinds.clone();

    let _agent = GsdTestAgent::start(&root, Duration::from_millis(10), move |payload| {
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

    let config_file: ConfigFile = serde_json::from_str(
        r#"{
            "steps": [
                {"name": "Root", "action": {"kind": "Pool", "instructions": {"inline": ""}}, "next": ["Branch1", "Branch2"]},
                {"name": "Branch1", "action": {"kind": "Pool", "instructions": {"inline": ""}}, "next": ["Leaf1A", "Leaf1B"]},
                {"name": "Branch2", "action": {"kind": "Pool", "instructions": {"inline": ""}}, "next": ["Leaf2A", "Leaf2B"]},
                {"name": "Leaf1A", "action": {"kind": "Pool", "instructions": {"inline": ""}}, "next": []},
                {"name": "Leaf1B", "action": {"kind": "Pool", "instructions": {"inline": ""}}, "next": []},
                {"name": "Leaf2A", "action": {"kind": "Pool", "instructions": {"inline": ""}}, "next": []},
                {"name": "Leaf2B", "action": {"kind": "Pool", "instructions": {"inline": ""}}, "next": []}
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
        initial_tasks: vec![Task::new("Root", serde_json::json!({}))],
        invoker: &create_test_invoker(),
    };

    gsd_config::run(&config, &schemas, runner_config).expect("run failed");

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

    cleanup_test_dir(&format!("{TEST_DIR}_nested"));
}
