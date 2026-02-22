//! Tests for JSON schema validation.

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]

mod common;

use common::{AgentPoolHandle, GsdTestAgent, cleanup_test_dir, is_ipc_available, setup_test_dir};
use gsd_config::{CompiledSchemas, Config, RunnerConfig, Task};
use std::path::Path;
use std::thread;
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

#[test]
fn valid_schema_passes() {
    let root = setup_test_dir(TEST_DIR);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(TEST_DIR);
        return;
    }

    let _pool = AgentPoolHandle::start(&root);

    // Agent returns valid Output schema
    let agent = GsdTestAgent::start(&root, "schema-agent", Duration::from_millis(10), |_| {
        r#"[{"kind": "Output", "value": {"result": "success"}}]"#.to_string()
    });

    thread::sleep(Duration::from_millis(200));

    let config = config_with_schema();
    let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");
    let runner_config = RunnerConfig {
        agent_pool_root: &root,
        wake_script: None,
        initial_tasks: vec![Task::new("Input", serde_json::json!({"count": 5}))],
    };

    gsd_config::run(&config, &schemas, runner_config).expect("run failed");

    let processed = agent.stop();
    // Input and Output
    assert_eq!(processed.len(), 2);

    cleanup_test_dir(TEST_DIR);
}

#[test]
fn invalid_initial_task_skipped() {
    let root = setup_test_dir(&format!("{TEST_DIR}_invalid_initial"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&format!("{TEST_DIR}_invalid_initial"));
        return;
    }

    let _pool = AgentPoolHandle::start(&root);
    let agent = GsdTestAgent::terminator(&root, "skip-agent", Duration::from_millis(10));

    thread::sleep(Duration::from_millis(200));

    let config = config_with_schema();
    let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");
    let runner_config = RunnerConfig {
        agent_pool_root: &root,
        wake_script: None,
        // Missing required "count" field
        initial_tasks: vec![Task::new("Input", serde_json::json!({}))],
    };

    gsd_config::run(&config, &schemas, runner_config).expect("run failed");

    let processed = agent.stop();
    // Invalid task should be skipped
    assert_eq!(processed.len(), 0);

    cleanup_test_dir(&format!("{TEST_DIR}_invalid_initial"));
}

#[test]
fn invalid_response_causes_retry() {
    let root = setup_test_dir(&format!("{TEST_DIR}_invalid_response"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&format!("{TEST_DIR}_invalid_response"));
        return;
    }

    let _pool = AgentPoolHandle::start(&root);

    // Agent returns invalid Output schema (missing "result")
    let agent = GsdTestAgent::start(&root, "bad-agent", Duration::from_millis(10), |_| {
        r#"[{"kind": "Output", "value": {}}]"#.to_string()
    });

    thread::sleep(Duration::from_millis(200));

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
        wake_script: None,
        initial_tasks: vec![Task::new("Input", serde_json::json!({}))],
    };

    gsd_config::run(&config, &schemas, runner_config).expect("run failed");

    let processed = agent.stop();
    // Initial + 2 retries = 3 attempts
    assert_eq!(processed.len(), 3);

    cleanup_test_dir(&format!("{TEST_DIR}_invalid_response"));
}
