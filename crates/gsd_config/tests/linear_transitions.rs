//! Linear task queue: Start -> Middle -> End

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]
#![expect(clippy::unwrap_used)]

mod common;

use common::{
    AgentPoolHandle, GsdTestAgent, cleanup_test_dir, find_agent_pool_binary, is_ipc_available,
    setup_test_dir,
};
use gsd_config::{CompiledSchemas, Config, RunnerConfig, Task};
use rstest::rstest;
use std::path::Path;
use std::time::Duration;

const TEST_DIR: &str = "linear_transitions";

fn linear_config() -> Config {
    serde_json::from_str(
        r#"{
            "steps": [
                {
                    "name": "Start",
                    "action": {"kind": "Pool", "instructions": "You are at the start. Transition to Middle."},
                    "next": ["Middle"]
                },
                {
                    "name": "Middle",
                    "action": {"kind": "Pool", "instructions": "You are in the middle. Transition to End."},
                    "next": ["End"]
                },
                {
                    "name": "End",
                    "action": {"kind": "Pool", "instructions": "You are at the end. Return empty array."},
                    "next": []
                }
            ]
        }"#,
    )
    .expect("parse config")
}

#[rstest]
#[timeout(Duration::from_secs(20))]
fn three_step_linear_machine() {
    let root = setup_test_dir(TEST_DIR);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(TEST_DIR);
        return;
    }

    let _pool = AgentPoolHandle::start(&root);
    let agent = GsdTestAgent::with_transitions(
        &root,
        "linear-agent",
        Duration::from_millis(10),
        vec![("Start", "Middle"), ("Middle", "End"), ("End", "")],
    );

    // Wait for agent to be ready (has processed initial heartbeat)

    let config = linear_config();
    let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");
    let runner_config = RunnerConfig {
        agent_pool_root: &root,
        config_base_path: Path::new("."),
        wake_script: None,
        initial_tasks: vec![Task::new("Start", serde_json::json!({}))],
        agent_pool_binary: Some(&find_agent_pool_binary()),
    };

    gsd_config::run(&config, &schemas, runner_config).expect("run failed");

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

    cleanup_test_dir(TEST_DIR);
}

#[rstest]
#[timeout(Duration::from_secs(20))]
fn instructions_included_in_payload() {
    let root = setup_test_dir(&format!("{TEST_DIR}_instructions"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&format!("{TEST_DIR}_instructions"));
        return;
    }

    let _pool = AgentPoolHandle::start(&root);
    let agent = GsdTestAgent::terminator(&root, "checker-agent", Duration::from_millis(10));

    // Wait for agent to be ready (has processed initial heartbeat)

    let config = linear_config();
    let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");
    let runner_config = RunnerConfig {
        agent_pool_root: &root,
        config_base_path: Path::new("."),
        wake_script: None,
        initial_tasks: vec![Task::new("Start", serde_json::json!({}))],
        agent_pool_binary: Some(&find_agent_pool_binary()),
    };

    gsd_config::run(&config, &schemas, runner_config).expect("run failed");

    let processed = agent.stop();
    assert_eq!(processed.len(), 1);

    let payload: serde_json::Value = serde_json::from_str(&processed[0]).expect("parse payload");
    let instructions = payload["instructions"].as_str().expect("instructions");

    // Should contain step-specific instructions
    assert!(instructions.contains("You are at the start"));
    // Should contain info about valid responses
    assert!(instructions.contains("Middle"));

    cleanup_test_dir(&format!("{TEST_DIR}_instructions"));
}
