//! Tests for branching task queues (one step -> multiple possible next steps).

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]
#![expect(clippy::unwrap_used)]

mod common;

use barnum_config::{CompiledSchemas, Config, ConfigFile, RunnerConfig, StepInputValue, Task};
use common::{
    BarnumTestAgent, TroupeHandle, cleanup_test_dir, create_test_invoker, is_ipc_available,
    setup_test_dir, test_state_log_path,
};
use rstest::rstest;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const TEST_DIR: &str = "branching_transitions";

fn branching_config() -> Config {
    let config_file: ConfigFile = serde_json::from_str(
        r#"{
            "steps": [
                {
                    "name": "Decide",
                    "action": {"kind": "Pool", "instructions": {"inline": "Decide which path to take: PathA or PathB"}},
                    "next": ["PathA", "PathB"]
                },
                {
                    "name": "PathA",
                    "action": {"kind": "Pool", "instructions": {"inline": "You chose path A. Go to Done."}},
                    "next": ["Done"]
                },
                {
                    "name": "PathB",
                    "action": {"kind": "Pool", "instructions": {"inline": "You chose path B. Go to Done."}},
                    "next": ["Done"]
                },
                {
                    "name": "Done",
                    "action": {"kind": "Pool", "instructions": {"inline": "All done."}},
                    "next": []
                }
            ]
        }"#,
    )
    .expect("parse config");
    config_file.resolve(Path::new(".")).expect("resolve config")
}

#[rstest]
#[timeout(Duration::from_secs(20))]
fn branch_to_path_a() {
    let root = setup_test_dir(TEST_DIR);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&root);
        return;
    }

    let pool = TroupeHandle::start(&root);
    let agent = BarnumTestAgent::with_transitions(
        &root,
        Duration::from_millis(10),
        vec![("Decide", "PathA"), ("PathA", "Done"), ("Done", "")],
    );

    // Wait for agent to be ready (has processed initial heartbeat)

    let config = branching_config();
    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");
    let initial_tasks = vec![Task::new("Decide", StepInputValue(serde_json::json!({})))];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        troupe_root: pool.pool_path(),
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &create_test_invoker(),
        state_log_path: &state_log,
    };

    barnum_config::run(&config, &schemas, &runner_config, initial_tasks).expect("run failed");

    let processed = agent.stop();
    let kinds: Vec<String> = processed
        .iter()
        .map(|p| {
            let v: serde_json::Value = serde_json::from_str(p).expect("parse");
            v["task"]["kind"].as_str().unwrap().to_string()
        })
        .collect();

    assert_eq!(kinds, vec!["Decide", "PathA", "Done"]);

    cleanup_test_dir(&root);
}

#[rstest]
#[timeout(Duration::from_secs(20))]
fn branch_to_path_b() {
    let root = setup_test_dir(&format!("{TEST_DIR}_path_b"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&root);
        return;
    }

    let pool = TroupeHandle::start(&root);
    let agent = BarnumTestAgent::with_transitions(
        &root,
        Duration::from_millis(10),
        vec![("Decide", "PathB"), ("PathB", "Done"), ("Done", "")],
    );

    // Wait for agent to be ready (has processed initial heartbeat)

    let config = branching_config();
    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");
    let initial_tasks = vec![Task::new("Decide", StepInputValue(serde_json::json!({})))];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        troupe_root: pool.pool_path(),
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &create_test_invoker(),
        state_log_path: &state_log,
    };

    barnum_config::run(&config, &schemas, &runner_config, initial_tasks).expect("run failed");

    let processed = agent.stop();
    let kinds: Vec<String> = processed
        .iter()
        .map(|p| {
            let v: serde_json::Value = serde_json::from_str(p).expect("parse");
            v["task"]["kind"].as_str().unwrap().to_string()
        })
        .collect();

    assert_eq!(kinds, vec!["Decide", "PathB", "Done"]);

    cleanup_test_dir(&root);
}

#[rstest]
#[timeout(Duration::from_secs(20))]
fn fan_out_multiple_tasks() {
    let root = setup_test_dir(&format!("{TEST_DIR}_fan_out"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&root);
        return;
    }

    let pool = TroupeHandle::start(&root);

    // Agent that fans out: Decide -> [PathA, PathB]
    let call_count = Arc::new(AtomicUsize::new(0));
    let call_count_clone = call_count.clone();

    let agent = BarnumTestAgent::start(&root, Duration::from_millis(10), move |payload| {
        let v: serde_json::Value = serde_json::from_str(payload).unwrap_or_default();
        let kind = v["task"]["kind"].as_str().unwrap_or("");
        call_count_clone.fetch_add(1, Ordering::SeqCst);

        match kind {
            "Decide" => {
                // Fan out to both paths
                r#"[{"kind": "PathA", "value": {}}, {"kind": "PathB", "value": {}}]"#.to_string()
            }
            "PathA" | "PathB" => r#"[{"kind": "Done", "value": {}}]"#.to_string(),
            _ => "[]".to_string(),
        }
    });

    // Wait for agent to be ready (has processed initial heartbeat)

    let config = branching_config();
    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");
    let initial_tasks = vec![Task::new("Decide", StepInputValue(serde_json::json!({})))];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        troupe_root: pool.pool_path(),
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &create_test_invoker(),
        state_log_path: &state_log,
    };

    barnum_config::run(&config, &schemas, &runner_config, initial_tasks).expect("run failed");

    agent.stop();

    // Should process: Decide, PathA, PathB, Done, Done = 5 tasks
    assert_eq!(call_count.load(Ordering::SeqCst), 5);

    cleanup_test_dir(&root);
}
