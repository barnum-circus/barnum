//! Simplest test: a single step that immediately terminates.

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]

mod common;

use barnum_config::{CompiledSchemas, Config, ConfigFile, RunnerConfig, StepInputValue, Task};
use common::{
    BarnumTestAgent, TroupeHandle, cleanup_test_dir, create_test_invoker, is_ipc_available,
    setup_test_dir, test_state_log_path,
};
use rstest::rstest;
use std::path::Path;
use std::time::Duration;

const TEST_DIR: &str = "simple_termination";

fn simple_config() -> Config {
    let config_file: ConfigFile = serde_json::from_str(
        r#"{
            "steps": [
                {
                    "name": "Start",
                    "action": {"kind": "Pool", "instructions": {"kind": "Inline", "value": "You are at the start. Return an empty array to finish."}},
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
fn single_step_terminates() {
    let root = setup_test_dir(TEST_DIR);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&root);
        return;
    }

    let pool = TroupeHandle::start(&root);
    let agent = BarnumTestAgent::terminator(&root, Duration::from_millis(10));

    // Wait for agent to be ready (has processed initial heartbeat)

    let config = simple_config();
    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");
    let invoker = create_test_invoker();
    let initial_tasks = vec![Task::new("Start", StepInputValue(serde_json::json!({})))];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        troupe_root: pool.pool_path(),
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &invoker,
        state_log_path: &state_log,
    };

    barnum_config::run(&config, &schemas, &runner_config, initial_tasks).expect("run failed");

    let processed = agent.stop();
    assert_eq!(processed.len(), 1);

    // Verify the payload contained the task
    let payload: serde_json::Value = serde_json::from_str(&processed[0]).expect("parse payload");
    assert_eq!(payload["task"]["kind"], "Start");

    cleanup_test_dir(&root);
}

#[rstest]
#[timeout(Duration::from_secs(5))]
fn empty_initial_tasks_does_nothing() {
    let root = setup_test_dir(&format!("{TEST_DIR}_empty"));

    // No IPC needed - we're not even starting the pool
    // With no initial tasks, the runner completes immediately without connecting to the pool
    let config = simple_config();
    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");
    let invoker = create_test_invoker();
    let initial_tasks = vec![];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        troupe_root: &root,
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &invoker,
        state_log_path: &state_log,
    };

    // Should complete immediately without error
    barnum_config::run(&config, &schemas, &runner_config, initial_tasks).expect("run failed");

    cleanup_test_dir(&root);
}
