//! Test corresponding to demos/single-basic.sh
//! Single agent, single task.

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]
#![expect(clippy::panic)]

mod common;

use agent_pool::{AGENTS_DIR, Payload, RESPONSE_FILE, Response, TASK_FILE, submit_file};
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

    let response = agent_pool::submit(&root, &Payload::inline("Hello, World!")).expect("Submit failed");
    let Response::Processed { stdout, .. } = response else {
        panic!("Expected Processed response, got {response:?}");
    };
    assert_eq!(stdout.trim(), "Hello, World! [processed]");

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
    let task_file = agent_dir.join(TASK_FILE);
    fs::write(&task_file, "Test task").expect("Failed to write task");

    let agent = TestAgent::echo(&root, "test-agent", Duration::from_millis(10));
    thread::sleep(Duration::from_millis(100));

    let response_file = agent_dir.join(RESPONSE_FILE);
    let output = fs::read_to_string(&response_file).expect("Failed to read output");
    assert_eq!(output, "Test task [processed]");

    let _ = agent.stop();
    cleanup_test_dir(&format!("{TEST_DIR}_file_protocol"));
}

/// Test file-based submission (for sandboxed environments).
/// This tests the full round-trip through the daemon using file IPC.
#[test]
fn file_based_submit() {
    let root = setup_test_dir(&format!("{TEST_DIR}_file_submit"));

    // Start daemon - file-based submit works even when socket IPC is blocked
    // because it only uses file I/O
    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available (daemon needs it internally)");
        cleanup_test_dir(&format!("{TEST_DIR}_file_submit"));
        return;
    }

    let _pool = AgentPoolHandle::start(&root);
    let agent = TestAgent::echo(&root, "agent-1", Duration::from_millis(10));

    // Give agent time to register
    thread::sleep(Duration::from_millis(200));

    // Submit using file-based protocol
    let response = submit_file(&root, &Payload::inline("Hello via file!")).expect("File submit failed");
    let Response::Processed { stdout, .. } = response else {
        panic!("Expected Processed response, got {response:?}");
    };
    assert_eq!(stdout.trim(), "Hello via file! [processed]");

    let processed = agent.stop();
    assert_eq!(processed, vec!["Hello via file!"]);

    cleanup_test_dir(&format!("{TEST_DIR}_file_submit"));
}
