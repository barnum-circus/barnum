//! Tests for invalid task queue transitions.

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]
#![expect(clippy::redundant_clone)]
#![expect(clippy::should_panic_without_expect)]

mod common;

use barnum_config::{CompiledSchemas, Config, ConfigFile, RunnerConfig, StepInputValue, Task};
use common::{
    BarnumTestAgent, TroupeHandle, cleanup_test_dir, create_test_invoker, inject_pool_config,
    is_ipc_available, setup_test_dir, test_state_log_path,
};
use rstest::rstest;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const TEST_DIR: &str = "invalid_transitions";

fn strict_config(pool_root: &Path) -> Config {
    let json = inject_pool_config(
        r#"{
            "options": {
                "max_retries": 1
            },
            "steps": [
                {
                    "name": "Start",
                    "action": {"kind": "Pool", "instructions": {"kind": "Inline", "value": "Only allowed to go to Middle."}},
                    "next": ["Middle"]
                },
                {
                    "name": "Middle",
                    "action": {"kind": "Pool", "instructions": {"kind": "Inline", "value": "Only allowed to go to End."}},
                    "next": ["End"]
                },
                {
                    "name": "End",
                    "action": {"kind": "Pool", "instructions": {"kind": "Inline", "value": "Terminal."}},
                    "next": []
                }
            ]
        }"#,
        pool_root,
    );
    let config_file: ConfigFile = serde_json::from_str(&json).expect("parse config");
    config_file.resolve(Path::new(".")).expect("resolve config")
}

#[rstest]
#[timeout(Duration::from_secs(20))]
#[should_panic]
fn invalid_transition_causes_retry() {
    let root = setup_test_dir(TEST_DIR);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&root);
        return;
    }

    let _pool = TroupeHandle::start(&root);

    // Agent tries to skip from Start directly to End (invalid)
    let agent = BarnumTestAgent::start(&root, Duration::from_millis(10), |_| {
        r#"[{"kind": "End", "value": {}}]"#.to_string()
    });

    // Wait for agent to be ready (has processed initial heartbeat)

    let config = strict_config(&root);
    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");
    let initial_tasks = vec![Task::new("Start", StepInputValue(serde_json::json!({})))];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &create_test_invoker(),
        state_log_path: &state_log,
    };

    // Run should return error because task is dropped after retries exhausted
    let result = barnum_config::run(&config, &schemas, &runner_config, initial_tasks);
    assert!(result.is_err(), "run should fail when tasks are dropped");

    let processed = agent.stop();
    // Original + 1 retry = 2 attempts
    assert_eq!(processed.len(), 2);

    cleanup_test_dir(&root);
}

#[rstest]
#[timeout(Duration::from_secs(20))]
#[should_panic]
fn unknown_step_causes_retry() {
    let root = setup_test_dir(&format!("{TEST_DIR}_unknown"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&root);
        return;
    }

    let _pool = TroupeHandle::start(&root);

    // Agent returns a step that doesn't exist
    let agent = BarnumTestAgent::start(&root, Duration::from_millis(10), |_| {
        r#"[{"kind": "NonExistent", "value": {}}]"#.to_string()
    });

    // Wait for agent to be ready (has processed initial heartbeat)

    let config = strict_config(&root);
    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");
    let initial_tasks = vec![Task::new("Start", StepInputValue(serde_json::json!({})))];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &create_test_invoker(),
        state_log_path: &state_log,
    };

    // Run should return error because task is dropped after retries exhausted
    let result = barnum_config::run(&config, &schemas, &runner_config, initial_tasks);
    assert!(result.is_err(), "run should fail when tasks are dropped");

    let processed = agent.stop();
    // Original + 1 retry = 2 attempts
    assert_eq!(processed.len(), 2);

    cleanup_test_dir(&root);
}

#[rstest]
#[timeout(Duration::from_secs(20))]
fn recovery_after_invalid_then_valid() {
    let root = setup_test_dir(&format!("{TEST_DIR}_recovery"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&root);
        return;
    }

    let _pool = TroupeHandle::start(&root);

    // Agent that fails first, then succeeds
    let call_count = Arc::new(AtomicUsize::new(0));
    let call_count_clone = call_count.clone();

    let agent = BarnumTestAgent::start(&root, Duration::from_millis(10), move |payload| {
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
    });

    // Wait for agent to be ready (has processed initial heartbeat)

    let config = strict_config(&root);
    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");
    let initial_tasks = vec![Task::new("Start", StepInputValue(serde_json::json!({})))];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &create_test_invoker(),
        state_log_path: &state_log,
    };

    barnum_config::run(&config, &schemas, &runner_config, initial_tasks).expect("run failed");

    let processed = agent.stop();
    // Start (fail) + Start (success) + Middle + End = 4
    assert_eq!(processed.len(), 4);

    cleanup_test_dir(&root);
}
