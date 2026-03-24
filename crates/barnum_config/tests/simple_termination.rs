//! Simplest test: a single step that immediately terminates.

#![expect(clippy::expect_used)]

mod common;

use barnum_config::{Config, RunnerConfig, StepInputValue, Task};
use common::{cleanup_test_dir, setup_test_dir, test_state_log_path};
use rstest::rstest;
use std::path::Path;
use std::time::Duration;

const TEST_DIR: &str = "simple_termination";

fn simple_config() -> Config {
    serde_json::from_str(
        r#"{"steps": [{"name": "Start", "action": {"kind": "Bash", "script": "echo '[]'"}, "next": []}]}"#,
    )
    .expect("parse config")
}

#[rstest]
#[timeout(Duration::from_secs(10))]
fn single_step_terminates() {
    let root = setup_test_dir(TEST_DIR);

    let config = simple_config();

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

#[rstest]
#[timeout(Duration::from_secs(5))]
fn empty_initial_tasks_does_nothing() {
    let root = setup_test_dir(&format!("{TEST_DIR}_empty"));

    let config = simple_config();

    let initial_tasks = vec![];
    let state_log = test_state_log_path(&root);
    let runner_config = RunnerConfig {
        working_dir: Path::new("."),
        wake_script: None,
        state_log_path: &state_log,
    };

    // Should complete immediately without error
    barnum_config::run(&config, &runner_config, initial_tasks).expect("run failed");

    cleanup_test_dir(&root);
}
