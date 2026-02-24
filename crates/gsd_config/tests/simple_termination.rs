//! Simplest test: a single step that immediately terminates.

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]

mod common;

use common::{AgentPoolHandle, GsdTestAgent, cleanup_test_dir, is_ipc_available, setup_test_dir};
use gsd_config::{CompiledSchemas, Config, RunnerConfig, Task};
use std::path::Path;
use std::time::Duration;

const TEST_DIR: &str = "simple_termination";

fn simple_config() -> Config {
    serde_json::from_str(
        r#"{
            "steps": [
                {
                    "name": "Start",
                    "action": {"kind": "Pool", "instructions": "You are at the start. Return an empty array to finish."},
                    "next": []
                }
            ]
        }"#,
    )
    .expect("parse config")
}

#[test]
fn single_step_terminates() {
    let root = setup_test_dir(TEST_DIR);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(TEST_DIR);
        return;
    }

    let _pool = AgentPoolHandle::start(&root);
    let mut agent = GsdTestAgent::terminator(&root, "test-agent", Duration::from_millis(10));

    // Wait for agent to be ready (has processed initial heartbeat)
    agent.wait_ready();

    let config = simple_config();
    let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");
    let runner_config = RunnerConfig {
        agent_pool_root: &root,
        wake_script: None,
        initial_tasks: vec![Task::new("Start", serde_json::json!({}))],
    };

    gsd_config::run(&config, &schemas, runner_config).expect("run failed");

    let processed = agent.stop();
    assert_eq!(processed.len(), 1);

    // Verify the payload contained the task
    let payload: serde_json::Value = serde_json::from_str(&processed[0]).expect("parse payload");
    assert_eq!(payload["task"]["kind"], "Start");

    cleanup_test_dir(TEST_DIR);
}

#[test]
fn empty_initial_tasks_does_nothing() {
    let root = setup_test_dir(&format!("{TEST_DIR}_empty"));

    // No IPC needed - we're not even starting the pool
    let config = simple_config();
    let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");
    let runner_config = RunnerConfig {
        agent_pool_root: &root,
        wake_script: None,
        initial_tasks: vec![],
    };

    // Should complete immediately without error
    gsd_config::run(&config, &schemas, runner_config).expect("run failed");

    cleanup_test_dir(&format!("{TEST_DIR}_empty"));
}
