//! Test corresponding to demos/single-basic.sh
//! Single agent, single task.

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]

mod common;

use agent_pool::{AGENTS_DIR, NEXT_TASK_FILE, OUTPUT_FILE, ResponseKind};
use common::{AgentPoolHandle, TestAgent, cleanup_test_dir, is_ipc_available, setup_test_dir};
use std::fs;
use std::thread;
use std::time::Duration;

const TEST_DIR: &str = "single_basic";

#[test]
fn single_agent_single_task() {
    let root = setup_test_dir(TEST_DIR);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(TEST_DIR);
        return;
    }

    let _pool = AgentPoolHandle::start(&root);
    let agent = TestAgent::echo(&root, "agent-1", Duration::from_millis(10));

    // Give agent time to register
    thread::sleep(Duration::from_millis(200));

    let response = agent_pool::submit(&root, "Hello, World!").expect("Submit failed");
    assert_eq!(response.kind, ResponseKind::Processed);
    assert_eq!(
        response.stdout.as_deref().map(str::trim),
        Some("Hello, World! [processed]")
    );

    let processed = agent.stop();
    assert_eq!(processed, vec!["Hello, World!"]);

    cleanup_test_dir(TEST_DIR);
}

#[test]
fn file_protocol_basic() {
    let root = setup_test_dir(&format!("{TEST_DIR}_file_protocol"));

    let agent_dir = root.join(AGENTS_DIR).join("test-agent");
    fs::create_dir_all(&agent_dir).expect("Failed to create agent directory");

    // Write task directly to test the file protocol
    let task_file = agent_dir.join(NEXT_TASK_FILE);
    fs::write(&task_file, "Test task").expect("Failed to write task");

    let agent = TestAgent::echo(&root, "test-agent", Duration::from_millis(10));
    thread::sleep(Duration::from_millis(100));

    let output_file = agent_dir.join(OUTPUT_FILE);
    let output = fs::read_to_string(&output_file).expect("Failed to read output");
    assert_eq!(output, "Test task [processed]");

    let _ = agent.stop();
    cleanup_test_dir(&format!("{TEST_DIR}_file_protocol"));
}
