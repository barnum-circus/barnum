//! Tests for invalid task queue transitions.

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]
#![expect(clippy::redundant_clone)]

mod common;

use common::{AgentPoolHandle, GsdTestAgent, cleanup_test_dir, is_ipc_available, setup_test_dir};
use gsd_config::{CompiledSchemas, Config, RunnerConfig, Task};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

const TEST_DIR: &str = "invalid_transitions";

fn strict_config() -> Config {
    serde_json::from_str(
        r#"{
            "options": {
                "max_retries": 1
            },
            "steps": [
                {
                    "name": "Start",
                    "instructions": "Only allowed to go to Middle.",
                    "next": ["Middle"]
                },
                {
                    "name": "Middle",
                    "instructions": "Only allowed to go to End.",
                    "next": ["End"]
                },
                {
                    "name": "End",
                    "instructions": "Terminal.",
                    "next": []
                }
            ]
        }"#,
    )
    .expect("parse config")
}

#[test]
fn invalid_transition_causes_retry() {
    let root = setup_test_dir(TEST_DIR);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(TEST_DIR);
        return;
    }

    let _pool = AgentPoolHandle::start(&root);

    // Agent tries to skip from Start directly to End (invalid)
    let agent = GsdTestAgent::start(&root, "bad-transition", Duration::from_millis(10), |_| {
        r#"[{"kind": "End", "value": {}}]"#.to_string()
    });

    thread::sleep(Duration::from_millis(200));

    let config = strict_config();
    let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");
    let runner_config = RunnerConfig {
        agent_pool_root: &root,
        wake_script: None,
        initial_tasks: vec![Task::new("Start", serde_json::json!({}))],
    };

    gsd_config::run(&config, &schemas, runner_config).expect("run failed");

    let processed = agent.stop();
    // Original + 1 retry = 2 attempts
    assert_eq!(processed.len(), 2);

    cleanup_test_dir(TEST_DIR);
}

#[test]
fn unknown_step_causes_retry() {
    let root = setup_test_dir(&format!("{TEST_DIR}_unknown"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&format!("{TEST_DIR}_unknown"));
        return;
    }

    let _pool = AgentPoolHandle::start(&root);

    // Agent returns a step that doesn't exist
    let agent = GsdTestAgent::start(&root, "unknown-step", Duration::from_millis(10), |_| {
        r#"[{"kind": "NonExistent", "value": {}}]"#.to_string()
    });

    thread::sleep(Duration::from_millis(200));

    let config = strict_config();
    let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");
    let runner_config = RunnerConfig {
        agent_pool_root: &root,
        wake_script: None,
        initial_tasks: vec![Task::new("Start", serde_json::json!({}))],
    };

    gsd_config::run(&config, &schemas, runner_config).expect("run failed");

    let processed = agent.stop();
    // Original + 1 retry = 2 attempts
    assert_eq!(processed.len(), 2);

    cleanup_test_dir(&format!("{TEST_DIR}_unknown"));
}

#[test]
fn recovery_after_invalid_then_valid() {
    let root = setup_test_dir(&format!("{TEST_DIR}_recovery"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&format!("{TEST_DIR}_recovery"));
        return;
    }

    let _pool = AgentPoolHandle::start(&root);

    // Agent that fails first, then succeeds
    let call_count = Arc::new(AtomicUsize::new(0));
    let call_count_clone = call_count.clone();

    let agent = GsdTestAgent::start(
        &root,
        "recovering-agent",
        Duration::from_millis(10),
        move |payload| {
            let count = call_count_clone.fetch_add(1, Ordering::SeqCst);
            let v: serde_json::Value = serde_json::from_str(payload).unwrap_or_default();
            let kind = v["task"]["kind"].as_str().unwrap_or("");

            match kind {
                "Start" => {
                    if count == 0 {
                        // First attempt: invalid transition
                        r#"[{"kind": "End", "value": {}}]"#.to_string()
                    } else {
                        // Second attempt: valid transition
                        r#"[{"kind": "Middle", "value": {}}]"#.to_string()
                    }
                }
                "Middle" => r#"[{"kind": "End", "value": {}}]"#.to_string(),
                _ => "[]".to_string(),
            }
        },
    );

    thread::sleep(Duration::from_millis(200));

    let config = strict_config();
    let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");
    let runner_config = RunnerConfig {
        agent_pool_root: &root,
        wake_script: None,
        initial_tasks: vec![Task::new("Start", serde_json::json!({}))],
    };

    gsd_config::run(&config, &schemas, runner_config).expect("run failed");

    let processed = agent.stop();
    // Start (fail) + Start (success) + Middle + End = 4
    assert_eq!(processed.len(), 4);

    cleanup_test_dir(&format!("{TEST_DIR}_recovery"));
}
