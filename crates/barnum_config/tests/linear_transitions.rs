//! Linear task queue: Start -> Middle -> End

#![expect(clippy::expect_used)]

mod common;

use barnum_config::{Config, ConfigFile, RunnerConfig, StepInputValue, Task};
use common::{cleanup_test_dir, setup_test_dir, test_state_log_path};
use rstest::rstest;
use std::path::Path;
use std::time::Duration;

const TEST_DIR: &str = "linear_transitions";

fn linear_config() -> Config {
    let config_file: ConfigFile = serde_json::from_str(
        r#"{
            "steps": [
                {
                    "name": "Start",
                    "action": {"kind": "Command", "params": {"script": "echo '[{\"kind\":\"Middle\",\"value\":{}}]'"}},
                    "next": ["Middle"]
                },
                {
                    "name": "Middle",
                    "action": {"kind": "Command", "params": {"script": "echo '[{\"kind\":\"End\",\"value\":{}}]'"}},
                    "next": ["End"]
                },
                {
                    "name": "End",
                    "action": {"kind": "Command", "params": {"script": "echo '[]'"}},
                    "next": []
                }
            ]
        }"#,
    )
    .expect("parse config");
    config_file.resolve(Path::new("."))
}

#[rstest]
#[timeout(Duration::from_secs(20))]
fn three_step_linear_machine() {
    let root = setup_test_dir(TEST_DIR);

    let config = linear_config();
    let initial_tasks = vec![Task::new("Start", StepInputValue(serde_json::json!({})))];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        state_log_path: &state_log,
    };

    barnum_config::run(&config, &runner_config, initial_tasks).expect("run failed");

    cleanup_test_dir(&root);
}
