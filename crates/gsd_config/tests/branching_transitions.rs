//! Tests for branching task queues (one step -> multiple possible next steps).

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]
#![expect(clippy::unwrap_used)]

mod common;

use common::{AgentPoolHandle, GsdTestAgent, cleanup_test_dir, is_ipc_available, setup_test_dir};
use gsd_config::{CompiledSchemas, Config, RunnerConfig, Task};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

const TEST_DIR: &str = "branching_transitions";

fn branching_config() -> Config {
    serde_json::from_str(
        r#"{
            "steps": [
                {
                    "name": "Decide",
                    "action": {"kind": "Pool", "instructions": "Decide which path to take: PathA or PathB"},
                    "next": ["PathA", "PathB"]
                },
                {
                    "name": "PathA",
                    "action": {"kind": "Pool", "instructions": "You chose path A. Go to Done."},
                    "next": ["Done"]
                },
                {
                    "name": "PathB",
                    "action": {"kind": "Pool", "instructions": "You chose path B. Go to Done."},
                    "next": ["Done"]
                },
                {
                    "name": "Done",
                    "action": {"kind": "Pool", "instructions": "All done."},
                    "next": []
                }
            ]
        }"#,
    )
    .expect("parse config")
}

#[test]
fn branch_to_path_a() {
    let root = setup_test_dir(TEST_DIR);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(TEST_DIR);
        return;
    }

    let _pool = AgentPoolHandle::start(&root);
    let agent = GsdTestAgent::with_transitions(
        &root,
        "path-a-agent",
        Duration::from_millis(10),
        vec![("Decide", "PathA"), ("PathA", "Done"), ("Done", "")],
    );

    thread::sleep(Duration::from_millis(200));

    let config = branching_config();
    let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");
    let runner_config = RunnerConfig {
        agent_pool_root: &root,
        wake_script: None,
        initial_tasks: vec![Task::new("Decide", serde_json::json!({}))],
    };

    gsd_config::run(&config, &schemas, runner_config).expect("run failed");

    let processed = agent.stop();
    let kinds: Vec<String> = processed
        .iter()
        .map(|p| {
            let v: serde_json::Value = serde_json::from_str(p).expect("parse");
            v["task"]["kind"].as_str().unwrap().to_string()
        })
        .collect();

    assert_eq!(kinds, vec!["Decide", "PathA", "Done"]);

    cleanup_test_dir(TEST_DIR);
}

#[test]
fn branch_to_path_b() {
    let root = setup_test_dir(&format!("{TEST_DIR}_path_b"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&format!("{TEST_DIR}_path_b"));
        return;
    }

    let _pool = AgentPoolHandle::start(&root);
    let agent = GsdTestAgent::with_transitions(
        &root,
        "path-b-agent",
        Duration::from_millis(10),
        vec![("Decide", "PathB"), ("PathB", "Done"), ("Done", "")],
    );

    thread::sleep(Duration::from_millis(200));

    let config = branching_config();
    let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");
    let runner_config = RunnerConfig {
        agent_pool_root: &root,
        wake_script: None,
        initial_tasks: vec![Task::new("Decide", serde_json::json!({}))],
    };

    gsd_config::run(&config, &schemas, runner_config).expect("run failed");

    let processed = agent.stop();
    let kinds: Vec<String> = processed
        .iter()
        .map(|p| {
            let v: serde_json::Value = serde_json::from_str(p).expect("parse");
            v["task"]["kind"].as_str().unwrap().to_string()
        })
        .collect();

    assert_eq!(kinds, vec!["Decide", "PathB", "Done"]);

    cleanup_test_dir(&format!("{TEST_DIR}_path_b"));
}

#[test]
fn fan_out_multiple_tasks() {
    let root = setup_test_dir(&format!("{TEST_DIR}_fan_out"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&format!("{TEST_DIR}_fan_out"));
        return;
    }

    let _pool = AgentPoolHandle::start(&root);

    // Agent that fans out: Decide -> [PathA, PathB]
    let call_count = Arc::new(AtomicUsize::new(0));
    let call_count_clone = call_count.clone();

    let agent = GsdTestAgent::start(
        &root,
        "fan-out-agent",
        Duration::from_millis(10),
        move |payload| {
            let v: serde_json::Value = serde_json::from_str(payload).unwrap_or_default();
            let kind = v["task"]["kind"].as_str().unwrap_or("");
            call_count_clone.fetch_add(1, Ordering::SeqCst);

            match kind {
                "Decide" => {
                    // Fan out to both paths
                    r#"[{"kind": "PathA", "value": {}}, {"kind": "PathB", "value": {}}]"#
                        .to_string()
                }
                "PathA" | "PathB" => r#"[{"kind": "Done", "value": {}}]"#.to_string(),
                _ => "[]".to_string(),
            }
        },
    );

    thread::sleep(Duration::from_millis(200));

    let config = branching_config();
    let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");
    let runner_config = RunnerConfig {
        agent_pool_root: &root,
        wake_script: None,
        initial_tasks: vec![Task::new("Decide", serde_json::json!({}))],
    };

    gsd_config::run(&config, &schemas, runner_config).expect("run failed");

    agent.stop();

    // Should process: Decide, PathA, PathB, Done, Done = 5 tasks
    assert_eq!(call_count.load(Ordering::SeqCst), 5);

    cleanup_test_dir(&format!("{TEST_DIR}_fan_out"));
}
