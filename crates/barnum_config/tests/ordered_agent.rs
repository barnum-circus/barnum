//! Tests for `OrderedAgentController` - deterministic task completion ordering.

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]

mod common;

use barnum_config::{CompiledSchemas, Config, ConfigFile, RunnerConfig, StepInputValue, Task};
use common::{
    BarnumTestAgent, TroupeHandle, cleanup_test_dir, create_test_invoker, inject_pool_config,
    is_ipc_available, setup_test_dir, test_state_log_path,
};
use rstest::rstest;
use std::io;
use std::path::Path;
use std::thread;
use std::time::Duration;

const TEST_DIR: &str = "ordered_agent";

fn simple_config(pool_root: &Path) -> Config {
    let json = inject_pool_config(
        r#"{
            "steps": [
                {
                    "name": "Start",
                    "action": {"kind": "Pool", "instructions": {"kind": "Inline", "value": "Process this task."}},
                    "next": []
                }
            ]
        }"#,
        pool_root,
    );
    let config_file: ConfigFile = serde_json::from_str(&json).expect("parse config");
    config_file.resolve(Path::new(".")).expect("resolve config")
}

fn fan_out_config(pool_root: &Path) -> Config {
    let json = inject_pool_config(
        r#"{
            "steps": [
                {
                    "name": "Distribute",
                    "action": {"kind": "Pool", "instructions": {"kind": "Inline", "value": "Spawn workers."}},
                    "next": ["Worker"]
                },
                {
                    "name": "Worker",
                    "action": {"kind": "Pool", "instructions": {"kind": "Inline", "value": "Do work."}},
                    "next": []
                }
            ]
        }"#,
        pool_root,
    );
    let config_file: ConfigFile = serde_json::from_str(&json).expect("parse config");
    config_file.resolve(Path::new(".")).expect("resolve config")
}

/// Helper to run Barnum in a background thread with ordered agent.
fn run_barnum_background(
    config: Config,
    initial_tasks: Vec<Task>,
    _pool: &TroupeHandle,
    root: &Path,
) -> thread::JoinHandle<io::Result<()>> {
    let invoker = create_test_invoker();
    let state_log = test_state_log_path(root);

    thread::spawn(move || {
        let schemas = CompiledSchemas::compile(&config).expect("compile schemas");
        let runner_config = RunnerConfig {
            working_dir: Path::new("."),
            wake_script: None,
            invoker: &invoker,
            state_log_path: &state_log,
        };
        barnum_config::run(&config, &schemas, &runner_config, initial_tasks)
    })
}

#[rstest]
#[timeout(Duration::from_secs(20))]
fn ordered_agent_single_task() {
    let root = setup_test_dir(&format!("{TEST_DIR}_single"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&root);
        return;
    }

    let pool = TroupeHandle::start(&root);
    let (agent, ctrl) = BarnumTestAgent::ordered(&root);

    let config = simple_config(&root);
    let initial_tasks = vec![Task::new("Start", StepInputValue(serde_json::json!({})))];
    let handle = run_barnum_background(config, initial_tasks, &pool, &root);

    // Wait for task to arrive
    ctrl.wait_for_tasks(1);

    // Verify it's there
    let tasks = ctrl.waiting_tasks();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].0, "Start");

    // Complete it
    ctrl.complete_at(0, "[]");

    // Barnum should finish
    handle.join().expect("thread panicked").expect("run failed");

    let processed = agent.stop();
    assert_eq!(processed.len(), 1);

    cleanup_test_dir(&root);
}

#[rstest]
#[timeout(Duration::from_secs(20))]
fn ordered_agent_wait_for_multiple() {
    let root = setup_test_dir(&format!("{TEST_DIR}_multiple"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&root);
        return;
    }

    let pool = TroupeHandle::start(&root);
    let (agent, ctrl) = BarnumTestAgent::ordered(&root);

    let config = fan_out_config(&root);
    let initial_tasks = vec![Task::new(
        "Distribute",
        StepInputValue(serde_json::json!({})),
    )];
    let handle = run_barnum_background(config, initial_tasks, &pool, &root);

    // Wait for Distribute task
    ctrl.wait_for_tasks(1);
    assert_eq!(ctrl.waiting_tasks()[0].0, "Distribute");

    // Complete Distribute, spawning 3 workers
    ctrl.complete_at(
        0,
        r#"[
        {"kind": "Worker", "value": {"id": 1}},
        {"kind": "Worker", "value": {"id": 2}},
        {"kind": "Worker", "value": {"id": 3}}
    ]"#,
    );

    // Wait for all 3 workers to arrive
    ctrl.wait_for_tasks(3);

    let tasks = ctrl.waiting_tasks();
    assert_eq!(tasks.len(), 3);
    assert!(tasks.iter().all(|(kind, _)| kind == "Worker"));

    // Complete all workers
    ctrl.complete_at(0, "[]");
    ctrl.complete_at(0, "[]");
    ctrl.complete_at(0, "[]");

    // Barnum should finish
    handle.join().expect("thread panicked").expect("run failed");

    let processed = agent.stop();
    // 1 Distribute + 3 Workers = 4 tasks
    assert_eq!(processed.len(), 4);

    cleanup_test_dir(&root);
}

#[rstest]
#[timeout(Duration::from_secs(20))]
fn ordered_agent_complete_out_of_order() {
    let root = setup_test_dir(&format!("{TEST_DIR}_out_of_order"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&root);
        return;
    }

    let pool = TroupeHandle::start(&root);
    let (agent, ctrl) = BarnumTestAgent::ordered(&root);

    let config = fan_out_config(&root);
    let initial_tasks = vec![Task::new(
        "Distribute",
        StepInputValue(serde_json::json!({})),
    )];
    let handle = run_barnum_background(config, initial_tasks, &pool, &root);

    // Wait for Distribute and complete it
    ctrl.wait_for_tasks(1);
    ctrl.complete_at(
        0,
        r#"[
        {"kind": "Worker", "value": {"id": 1}},
        {"kind": "Worker", "value": {"id": 2}},
        {"kind": "Worker", "value": {"id": 3}}
    ]"#,
    );

    // Wait for all workers
    ctrl.wait_for_tasks(3);

    // Complete in reverse order (index 2 first, then 1, then 0)
    // Note: after removing index 2, the list shrinks, so next index 2 doesn't exist
    // We need to remove from the end: 2, then 1, then 0
    ctrl.complete_at(2, "[]"); // Worker 3
    ctrl.complete_at(1, "[]"); // Worker 2
    ctrl.complete_at(0, "[]"); // Worker 1

    handle.join().expect("thread panicked").expect("run failed");

    let processed = agent.stop();
    assert_eq!(processed.len(), 4);

    cleanup_test_dir(&root);
}

#[rstest]
#[timeout(Duration::from_secs(20))]
fn ordered_agent_waiting_tasks_query() {
    let root = setup_test_dir(&format!("{TEST_DIR}_query"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&root);
        return;
    }

    let pool = TroupeHandle::start(&root);
    let (agent, ctrl) = BarnumTestAgent::ordered(&root);

    let config = fan_out_config(&root);
    let initial_tasks = vec![Task::new(
        "Distribute",
        StepInputValue(serde_json::json!({})),
    )];
    let handle = run_barnum_background(config, initial_tasks, &pool, &root);

    // Initially no tasks
    assert_eq!(ctrl.waiting_tasks().len(), 0);

    // Wait for Distribute
    ctrl.wait_for_tasks(1);

    // Now one task
    let tasks = ctrl.waiting_tasks();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].0, "Distribute");

    // Verify payload contains expected structure
    let payload: serde_json::Value = serde_json::from_str(&tasks[0].1).expect("parse payload");
    assert_eq!(payload["task"]["kind"], "Distribute");

    // Complete and spawn workers
    ctrl.complete_at(
        0,
        r#"[
        {"kind": "Worker", "value": {"id": "A"}},
        {"kind": "Worker", "value": {"id": "B"}}
    ]"#,
    );

    // Wait for workers
    ctrl.wait_for_tasks(2);

    // Query shows both workers with their payloads
    let tasks = ctrl.waiting_tasks();
    assert_eq!(tasks.len(), 2);
    assert!(tasks.iter().all(|(kind, _)| kind == "Worker"));

    // Payloads contain the values we spawned
    let ids: Vec<_> = tasks
        .iter()
        .map(|(_, payload)| {
            let v: serde_json::Value = serde_json::from_str(payload).expect("parse");
            v["task"]["value"]["id"]
                .as_str()
                .expect("id should be string")
                .to_string()
        })
        .collect();
    assert!(ids.contains(&"A".to_string()));
    assert!(ids.contains(&"B".to_string()));

    // Complete both
    ctrl.complete_at(0, "[]");
    ctrl.complete_at(0, "[]");

    handle.join().expect("thread panicked").expect("run failed");
    agent.stop();

    cleanup_test_dir(&root);
}
