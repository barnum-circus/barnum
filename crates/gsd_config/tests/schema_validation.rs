//! Tests for JSON schema validation.

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]

mod common;

use common::{
    AgentPoolHandle, GsdTestAgent, cleanup_test_dir, find_agent_pool_binary, is_ipc_available,
    setup_test_dir,
};
use gsd_config::{CompiledSchemas, Config, RunnerConfig, Task};
use rstest::rstest;
use std::path::Path;
use std::time::Duration;

const TEST_DIR: &str = "schema_validation";

fn config_with_schema() -> Config {
    serde_json::from_str(
        r#"{
            "options": {
                "max_retries": 0
            },
            "steps": [
                {
                    "name": "Input",
                    "value_schema": {
                        "type": "object",
                        "properties": {
                            "count": {"type": "integer", "minimum": 1}
                        },
                        "required": ["count"]
                    },
                    "next": ["Output"]
                },
                {
                    "name": "Output",
                    "value_schema": {
                        "type": "object",
                        "properties": {
                            "result": {"type": "string"}
                        },
                        "required": ["result"]
                    },
                    "next": []
                }
            ]
        }"#,
    )
    .expect("parse config")
}

#[rstest]
#[timeout(Duration::from_secs(20))]
fn valid_schema_passes() {
    let root = setup_test_dir(TEST_DIR);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(TEST_DIR);
        return;
    }

    let _pool = AgentPoolHandle::start(&root);

    // Agent returns valid Output schema for Input, empty for Output
    let agent = GsdTestAgent::start(
        &root,
        "schema-agent",
        Duration::from_millis(10),
        |payload| {
            let v: serde_json::Value = serde_json::from_str(payload).unwrap_or_default();
            let kind = v["task"]["kind"].as_str().unwrap_or("");
            match kind {
                "Input" => r#"[{"kind": "Output", "value": {"result": "success"}}]"#.to_string(),
                _ => "[]".to_string(),
            }
        },
    );

    // Wait for agent to be ready (has processed initial heartbeat)

    let config = config_with_schema();
    let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");
    let runner_config = RunnerConfig {
        agent_pool_root: &root,
        config_base_path: Path::new("."),
        wake_script: None,
        initial_tasks: vec![Task::new("Input", serde_json::json!({"count": 5}))],
        agent_pool_binary: Some(&find_agent_pool_binary()),
    };

    gsd_config::run(&config, &schemas, runner_config).expect("run failed");

    let processed = agent.stop();
    // Input and Output
    assert_eq!(processed.len(), 2);

    cleanup_test_dir(TEST_DIR);
}

#[rstest]
#[timeout(Duration::from_secs(20))]
fn invalid_initial_task_skipped() {
    let root = setup_test_dir(&format!("{TEST_DIR}_invalid_initial"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&format!("{TEST_DIR}_invalid_initial"));
        return;
    }

    let _pool = AgentPoolHandle::start(&root);
    let agent = GsdTestAgent::terminator(&root, "skip-agent", Duration::from_millis(10));

    // Wait for agent to be ready (has processed initial heartbeat)

    let config = config_with_schema();
    let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");
    let runner_config = RunnerConfig {
        agent_pool_root: &root,
        config_base_path: Path::new("."),
        wake_script: None,
        // Missing required "count" field
        initial_tasks: vec![Task::new("Input", serde_json::json!({}))],
        agent_pool_binary: Some(&find_agent_pool_binary()),
    };

    gsd_config::run(&config, &schemas, runner_config).expect("run failed");

    let processed = agent.stop();
    // Invalid task should be skipped
    assert_eq!(processed.len(), 0);

    cleanup_test_dir(&format!("{TEST_DIR}_invalid_initial"));
}

#[rstest]
#[timeout(Duration::from_secs(20))]
fn invalid_response_causes_retry() {
    let root = setup_test_dir(&format!("{TEST_DIR}_invalid_response"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&format!("{TEST_DIR}_invalid_response"));
        return;
    }

    let _pool = AgentPoolHandle::start(&root);

    // Agent returns invalid Output schema (missing "result")
    let agent = GsdTestAgent::start(&root, "bad-agent", Duration::from_millis(50), |_| {
        r#"[{"kind": "Output", "value": {}}]"#.to_string()
    });

    // Wait for agent to be ready (has processed initial heartbeat)

    // Config allows 2 retries
    let config: Config = serde_json::from_str(
        r#"{
            "options": {
                "max_retries": 2
            },
            "steps": [
                {
                    "name": "Input",
                    "value_schema": {"type": "object"},
                    "next": ["Output"]
                },
                {
                    "name": "Output",
                    "value_schema": {
                        "type": "object",
                        "properties": {"result": {"type": "string"}},
                        "required": ["result"]
                    },
                    "next": []
                }
            ]
        }"#,
    )
    .expect("parse config");

    let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");
    let runner_config = RunnerConfig {
        agent_pool_root: &root,
        config_base_path: Path::new("."),
        wake_script: None,
        initial_tasks: vec![Task::new("Input", serde_json::json!({}))],
        agent_pool_binary: Some(&find_agent_pool_binary()),
    };

    // Run should return error because task is dropped after all retries
    let result = gsd_config::run(&config, &schemas, runner_config);
    assert!(result.is_err(), "run should fail when tasks are dropped");

    let processed = agent.stop();
    // Initial + 2 retries = 3 attempts
    assert_eq!(processed.len(), 3);

    cleanup_test_dir(&format!("{TEST_DIR}_invalid_response"));
}
