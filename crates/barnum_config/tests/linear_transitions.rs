//! Linear task queue: Start -> Middle -> End

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]
#![expect(clippy::unwrap_used)]

mod common;

use barnum_config::{CompiledSchemas, Config, ConfigFile, RunnerConfig, StepInputValue, Task};
use common::{
    BarnumTestAgent, TroupeHandle, cleanup_test_dir, create_test_invoker, inject_pool_config,
    is_ipc_available, setup_test_dir, test_state_log_path,
};
use rstest::rstest;
use std::path::Path;
use std::time::Duration;

const TEST_DIR: &str = "linear_transitions";

fn linear_config(pool_root: &Path) -> Config {
    let json = inject_pool_config(
        r#"{
            "steps": [
                {
                    "name": "Start",
                    "action": {"kind": "Pool", "instructions": {"kind": "Inline", "value": "You are at the start. Transition to Middle."}},
                    "next": ["Middle"]
                },
                {
                    "name": "Middle",
                    "action": {"kind": "Pool", "instructions": {"kind": "Inline", "value": "You are in the middle. Transition to End."}},
                    "next": ["End"]
                },
                {
                    "name": "End",
                    "action": {"kind": "Pool", "instructions": {"kind": "Inline", "value": "You are at the end. Return empty array."}},
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
fn three_step_linear_machine() {
    let root = setup_test_dir(TEST_DIR);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&root);
        return;
    }

    let _pool = TroupeHandle::start(&root);
    let agent = BarnumTestAgent::with_transitions(
        &root,
        Duration::from_millis(10),
        vec![("Start", "Middle"), ("Middle", "End"), ("End", "")],
    );

    // Wait for agent to be ready (has processed initial heartbeat)

    let config = linear_config(&root);
    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");
    let invoker = create_test_invoker();
    let initial_tasks = vec![Task::new("Start", StepInputValue(serde_json::json!({})))];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &invoker,
        state_log_path: &state_log,
    };

    barnum_config::run(&config, &schemas, &runner_config, initial_tasks).expect("run failed");

    let processed = agent.stop();
    assert_eq!(processed.len(), 3);

    // Verify order of execution
    let kinds: Vec<String> = processed
        .iter()
        .map(|p| {
            let v: serde_json::Value = serde_json::from_str(p).expect("parse");
            v["task"]["kind"].as_str().unwrap().to_string()
        })
        .collect();
    assert_eq!(kinds, vec!["Start", "Middle", "End"]);

    cleanup_test_dir(&root);
}

#[rstest]
#[timeout(Duration::from_secs(20))]
fn instructions_included_in_payload() {
    let root = setup_test_dir(&format!("{TEST_DIR}_instructions"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&root);
        return;
    }

    let _pool = TroupeHandle::start(&root);
    let agent = BarnumTestAgent::terminator(&root, Duration::from_millis(10));

    // Wait for agent to be ready (has processed initial heartbeat)

    let config = linear_config(&root);
    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");
    let invoker = create_test_invoker();
    let initial_tasks = vec![Task::new("Start", StepInputValue(serde_json::json!({})))];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &invoker,
        state_log_path: &state_log,
    };

    barnum_config::run(&config, &schemas, &runner_config, initial_tasks).expect("run failed");

    let processed = agent.stop();
    assert_eq!(processed.len(), 1);

    let payload: serde_json::Value = serde_json::from_str(&processed[0]).expect("parse payload");
    let instructions = payload["instructions"].as_str().expect("instructions");

    // Should contain step-specific instructions
    assert!(instructions.contains("You are at the start"));
    // Should contain info about valid responses
    assert!(instructions.contains("Middle"));

    cleanup_test_dir(&root);
}
