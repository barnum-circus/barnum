//! Test corresponding to demos/single-basic.sh
//! Single agent, single task.

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]
#![expect(clippy::panic)]

mod common;

use agent_pool::{Payload, Response, submit_file};
use common::{AgentPoolHandle, TestAgent, cleanup_test_dir, is_ipc_available, setup_test_dir};
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
    let mut agent = TestAgent::echo(&root, "agent-1", Duration::from_millis(10));

    // Wait for agent to be ready (has processed initial heartbeat)
    agent.wait_ready();

    let response = agent_pool::submit(
        &root,
        &Payload::inline(r#"{"kind":"Task","task":{"instructions":"echo","data":"Hello, World!"}}"#),
    )
    .expect("Submit failed");
    let Response::Processed { stdout, .. } = response else {
        panic!("Expected Processed response, got {response:?}");
    };
    assert!(stdout.contains(r#""data":"Hello, World!""#));
    assert!(stdout.contains("[processed]"));

    // Note: processed contains the full task JSON
    let _ = agent.stop();

    cleanup_test_dir(TEST_DIR);
}

// Note: file_protocol_basic test removed - it was testing internal implementation
// details that are no longer relevant with CLI-based agents. The proper way to
// test task processing is through the daemon using submit().

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
    let mut agent = TestAgent::echo(&root, "agent-1", Duration::from_millis(10));

    // Wait for agent to be ready (has processed initial heartbeat)
    agent.wait_ready();

    // Submit using file-based protocol
    let response = submit_file(
        &root,
        &Payload::inline(r#"{"kind":"Task","task":{"instructions":"echo","data":"Hello via file!"}}"#),
    )
    .expect("File submit failed");
    let Response::Processed { stdout, .. } = response else {
        panic!("Expected Processed response, got {response:?}");
    };
    assert!(stdout.contains(r#""data":"Hello via file!""#));
    assert!(stdout.contains("[processed]"));

    // Note: processed contains the full task JSON
    let _ = agent.stop();

    cleanup_test_dir(&format!("{TEST_DIR}_file_submit"));
}
