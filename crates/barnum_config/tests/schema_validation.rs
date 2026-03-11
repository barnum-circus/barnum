//! Tests for JSON schema validation.

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]

mod common;

use barnum_config::{CompiledSchemas, Config, ConfigFile, RunnerConfig, StepInputValue, Task};
use common::{
    BarnumTestAgent, TroupeHandle, cleanup_test_dir, create_test_invoker, is_ipc_available,
    setup_test_dir,
};
use rstest::rstest;
use std::path::Path;
use std::time::Duration;

const TEST_DIR: &str = "schema_validation";

fn config_with_schema() -> Config {
    let config_file: ConfigFile = serde_json::from_str(
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
    .expect("parse config");
    config_file.resolve(Path::new(".")).expect("resolve config")
}

#[rstest]
#[timeout(Duration::from_secs(20))]
fn valid_schema_passes() {
    let root = setup_test_dir(TEST_DIR);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&root);
        return;
    }

    let pool = TroupeHandle::start(&root);

    // Agent returns valid Output schema for Input, empty for Output
    let agent = BarnumTestAgent::start(&root, Duration::from_millis(10), |payload| {
        let v: serde_json::Value = serde_json::from_str(payload).unwrap_or_default();
        let kind = v["task"]["kind"].as_str().unwrap_or("");
        match kind {
            "Input" => r#"[{"kind": "Output", "value": {"result": "success"}}]"#.to_string(),
            _ => "[]".to_string(),
        }
    });

    // Wait for agent to be ready (has processed initial heartbeat)

    let config = config_with_schema();
    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");
    let initial_tasks = vec![Task::new(
        "Input",
        StepInputValue(serde_json::json!({"count": 5})),
    )];
    let runner_config = RunnerConfig {
        troupe_root: pool.pool_path(),
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &create_test_invoker(),
        state_log_path: None,
    };

    barnum_config::run(&config, &schemas, &runner_config, initial_tasks).expect("run failed");

    let processed = agent.stop();
    // Input and Output
    assert_eq!(processed.len(), 2);

    cleanup_test_dir(&root);
}

#[rstest]
#[timeout(Duration::from_secs(20))]
fn invalid_response_causes_retry() {
    let root = setup_test_dir(&format!("{TEST_DIR}_invalid_response"));

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&root);
        return;
    }

    let pool = TroupeHandle::start(&root);

    // Agent returns invalid Output schema (missing "result")
    let agent = BarnumTestAgent::start(&root, Duration::from_millis(50), |_| {
        r#"[{"kind": "Output", "value": {}}]"#.to_string()
    });

    // Wait for agent to be ready (has processed initial heartbeat)

    // Config allows 2 retries
    let config_file: ConfigFile = serde_json::from_str(
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
    let config = config_file.resolve(Path::new(".")).expect("resolve config");

    let schemas = CompiledSchemas::compile(&config).expect("compile schemas");
    let initial_tasks = vec![Task::new("Input", StepInputValue(serde_json::json!({})))];
    let runner_config = RunnerConfig {
        troupe_root: pool.pool_path(),
        working_dir: Path::new("."),
        wake_script: None,
        invoker: &create_test_invoker(),
        state_log_path: None,
    };

    // Run should return error because task is dropped after all retries
    let result = barnum_config::run(&config, &schemas, &runner_config, initial_tasks);
    assert!(result.is_err(), "run should fail when tasks are dropped");

    let processed = agent.stop();
    // Initial + 2 retries = 3 attempts
    assert_eq!(processed.len(), 3);

    cleanup_test_dir(&root);
}
