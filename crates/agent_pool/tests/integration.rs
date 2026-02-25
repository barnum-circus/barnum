//! Integration tests for the agent pool daemon.
//!
//! These tests verify the daemon works end-to-end using file-based task submission.

#![expect(clippy::expect_used)]

mod common;

use agent_pool::{AGENTS_DIR, PENDING_DIR, RESPONSE_FILE, TASK_FILE};
use common::{AgentPoolHandle, TestAgent, cleanup_test_dir, is_ipc_available, setup_test_dir};
use std::fs;
use std::path::Path;
use std::thread;
use std::time::Duration;

/// Wait for all agents to be ready (have processed their initial heartbeats).
fn wait_all_ready(agents: &mut [&mut TestAgent]) {
    for agent in agents {
        agent.wait_ready();
    }
}

const TEST_DIR: &str = "integration";

/// Helper to submit a task (wraps content in proper task and Payload format).
///
/// The `data` parameter is the actual payload data (any JSON value).
/// This helper wraps it in the proper task envelope:
/// `{"kind": "Task", "task": {"instructions": "...", "data": <data>}}`
fn submit_task(pending_dir: &Path, task_id: &str, data: &str) -> std::path::PathBuf {
    let submission_dir = pending_dir.join(task_id);
    fs::create_dir_all(&submission_dir).expect("Failed to create submission dir");

    // Parse the data as JSON, or use it as a string
    let data_value: serde_json::Value =
        serde_json::from_str(data).unwrap_or_else(|_| serde_json::Value::String(data.to_string()));

    // Create the task envelope (what the agent receives)
    let task_envelope = serde_json::json!({
        "kind": "Task",
        "task": {
            "instructions": "test task",
            "data": data_value
        }
    });

    // Wrap in Payload envelope - daemon expects {"kind": "Inline", "content": "..."}
    let payload = serde_json::json!({
        "kind": "Inline",
        "content": task_envelope.to_string()
    });
    fs::write(submission_dir.join(TASK_FILE), payload.to_string()).expect("Failed to write task");
    submission_dir
}

/// Helper to wait for a response file with timeout.
fn wait_for_response(submission_dir: &Path, timeout_ms: u64) -> Option<String> {
    let response_file = submission_dir.join(RESPONSE_FILE);
    let attempts = timeout_ms / 10;
    for _ in 0..attempts {
        if response_file.exists() {
            return fs::read_to_string(&response_file).ok();
        }
        thread::sleep(Duration::from_millis(10));
    }
    None
}

/// Extract stdout from a response JSON.
fn extract_stdout(response: &str) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(response).ok()?;
    json.get("stdout")?.as_str().map(String::from)
}

/// Test file-based submission.
#[test]
fn file_based_submit() {
    let root = setup_test_dir(&format!("{TEST_DIR}_file_submit"));

    if !is_ipc_available(&root) {
        cleanup_test_dir(&format!("{TEST_DIR}_file_submit"));
        return;
    }

    let _pool = AgentPoolHandle::start(&root);
    let mut agent = TestAgent::echo(&root, "agent-1", Duration::from_millis(5));

    // Wait for agent to be ready (has processed initial heartbeat)
    agent.wait_ready();

    let pending_dir = root.join(PENDING_DIR);
    let submission_dir = submit_task(&pending_dir, "test-1", r#"{"message": "Hello!"}"#);

    // Wait for response
    let response = wait_for_response(&submission_dir, 2000);
    assert!(response.is_some(), "Response file should exist");
    let response_str = response.expect("response");
    assert!(
        response_str.contains("[processed]"),
        "Response should contain processed marker"
    );

    // Note: processed content is now structured JSON
    let _ = agent.stop();

    cleanup_test_dir(&format!("{TEST_DIR}_file_submit"));
}

/// Test multiple tasks dispatched to a single agent.
#[test]
fn single_agent_multiple_tasks() {
    let root = setup_test_dir(&format!("{TEST_DIR}_single_multi"));

    if !is_ipc_available(&root) {
        cleanup_test_dir(&format!("{TEST_DIR}_single_multi"));
        return;
    }

    let _pool = AgentPoolHandle::start(&root);
    let mut agent = TestAgent::echo(&root, "agent-1", Duration::from_millis(5));

    // Wait for agent to be ready (has processed initial heartbeat)
    agent.wait_ready();

    let pending_dir = root.join(PENDING_DIR);

    // Submit multiple tasks
    for i in 0..3 {
        submit_task(
            &pending_dir,
            &format!("task-{i}"),
            &format!(r#"{{"n": {i}}}"#),
        );
        thread::sleep(Duration::from_millis(20));
    }

    // Wait for all responses
    for i in 0..3 {
        let submission_dir = pending_dir.join(format!("task-{i}"));
        let response = wait_for_response(&submission_dir, 2000);
        assert!(response.is_some(), "Task {i} should complete");
    }

    // Note: processed content is now structured JSON
    let _ = agent.stop();

    cleanup_test_dir(&format!("{TEST_DIR}_single_multi"));
}

/// Test multiple agents handling tasks in parallel.
#[test]
fn multiple_agents_parallel() {
    let root = setup_test_dir(&format!("{TEST_DIR}_multi_parallel"));

    if !is_ipc_available(&root) {
        cleanup_test_dir(&format!("{TEST_DIR}_multi_parallel"));
        return;
    }

    let _pool = AgentPoolHandle::start(&root);

    // Start two agents with slight processing delay
    let mut agent1 = TestAgent::echo(&root, "agent-1", Duration::from_millis(50));
    let mut agent2 = TestAgent::echo(&root, "agent-2", Duration::from_millis(50));

    // Wait for both agents to be ready (have processed initial heartbeats)
    wait_all_ready(&mut [&mut agent1, &mut agent2]);

    let pending_dir = root.join(PENDING_DIR);

    // Submit 4 tasks
    for i in 0..4 {
        submit_task(
            &pending_dir,
            &format!("task-{i}"),
            &format!(r#"{{"n": {i}}}"#),
        );
        thread::sleep(Duration::from_millis(20));
    }

    // Wait for all responses
    for i in 0..4 {
        let submission_dir = pending_dir.join(format!("task-{i}"));
        let response = wait_for_response(&submission_dir, 2000);
        assert!(response.is_some(), "Task {i} should complete");
    }

    let processed1 = agent1.stop();
    let processed2 = agent2.stop();
    let total = processed1.len() + processed2.len();

    assert_eq!(total, 4, "Both agents combined should process all 4 tasks");

    cleanup_test_dir(&format!("{TEST_DIR}_multi_parallel"));
}

/// Test agent deregistration.
#[test]
fn agent_deregistration() {
    let root = setup_test_dir(&format!("{TEST_DIR}_deregister"));

    if !is_ipc_available(&root) {
        cleanup_test_dir(&format!("{TEST_DIR}_deregister"));
        return;
    }

    let _pool = AgentPoolHandle::start(&root);
    let mut agent = TestAgent::echo(&root, "agent-1", Duration::from_millis(5));

    // Wait for agent to be ready (has processed initial heartbeat)
    agent.wait_ready();

    let pending_dir = root.join(PENDING_DIR);
    let submission_dir = submit_task(&pending_dir, "task-before", r#"{"test": "before"}"#);

    let response = wait_for_response(&submission_dir, 2000);
    assert!(response.is_some(), "First task should complete");

    // Stop the agent
    let processed = agent.stop();
    assert_eq!(processed.len(), 1);

    // Remove agent directory
    let agent_dir = root.join(AGENTS_DIR).join("agent-1");
    let _ = fs::remove_dir_all(&agent_dir);

    // Wait for FSWatcher to detect agent removal (this sleep is necessary)
    thread::sleep(Duration::from_millis(100));

    // Start a new agent
    let mut agent2 = TestAgent::echo(&root, "agent-2", Duration::from_millis(5));

    // Wait for new agent to be ready
    agent2.wait_ready();

    let submission_dir2 = submit_task(&pending_dir, "task-after", r#"{"test": "after"}"#);
    let response = wait_for_response(&submission_dir2, 2000);
    assert!(response.is_some(), "New agent should process the task");

    let processed2 = agent2.stop();
    assert_eq!(processed2.len(), 1);

    cleanup_test_dir(&format!("{TEST_DIR}_deregister"));
}

/// Test tasks queued before any agents register.
#[test]
fn tasks_queued_before_agents() {
    let root = setup_test_dir(&format!("{TEST_DIR}_queue_before"));

    if !is_ipc_available(&root) {
        cleanup_test_dir(&format!("{TEST_DIR}_queue_before"));
        return;
    }

    let _pool = AgentPoolHandle::start(&root);
    let pending_dir = root.join(PENDING_DIR);

    // Submit tasks BEFORE any agents register
    for i in 0..3 {
        submit_task(
            &pending_dir,
            &format!("queued-{i}"),
            &format!(r#"{{"n": {i}}}"#),
        );
    }

    // Tasks have settled in pending queue

    // NOW register an agent
    let mut agent = TestAgent::echo(&root, "late-agent", Duration::from_millis(5));

    // Wait for agent to be ready (has processed initial heartbeat)
    agent.wait_ready();

    // Wait for all responses
    for i in 0..3 {
        let submission_dir = pending_dir.join(format!("queued-{i}"));
        let response = wait_for_response(&submission_dir, 2000);
        assert!(response.is_some(), "Task {i} should complete");
    }

    let processed = agent.stop();
    assert_eq!(
        processed.len(),
        3,
        "Agent should have processed all 3 queued tasks"
    );

    cleanup_test_dir(&format!("{TEST_DIR}_queue_before"));
}

/// Test rapid burst of task submissions (stress test for `FSWatcher` deduplication).
#[test]
fn rapid_task_burst() {
    let root = setup_test_dir(&format!("{TEST_DIR}_rapid_burst"));

    if !is_ipc_available(&root) {
        cleanup_test_dir(&format!("{TEST_DIR}_rapid_burst"));
        return;
    }

    let _pool = AgentPoolHandle::start(&root);
    let mut agent = TestAgent::echo(&root, "burst-agent", Duration::from_millis(2));

    // Wait for agent to be ready (has processed initial heartbeat)
    agent.wait_ready();

    let pending_dir = root.join(PENDING_DIR);

    // Submit 10 tasks as fast as possible
    for i in 0..10 {
        submit_task(
            &pending_dir,
            &format!("burst-{i}"),
            &format!(r#"{{"n": {i}}}"#),
        );
    }

    // Wait for all responses
    for i in 0..10 {
        let submission_dir = pending_dir.join(format!("burst-{i}"));
        let response = wait_for_response(&submission_dir, 3000);
        assert!(response.is_some(), "Burst task {i} should complete");
    }

    let processed = agent.stop();
    assert_eq!(
        processed.len(),
        10,
        "Agent should have processed all 10 tasks"
    );

    cleanup_test_dir(&format!("{TEST_DIR}_rapid_burst"));
}

/// Test that tasks with identical content are handled correctly.
#[test]
fn identical_task_content() {
    let root = setup_test_dir(&format!("{TEST_DIR}_identical"));

    if !is_ipc_available(&root) {
        cleanup_test_dir(&format!("{TEST_DIR}_identical"));
        return;
    }

    let _pool = AgentPoolHandle::start(&root);
    let mut agent = TestAgent::echo(&root, "agent-1", Duration::from_millis(5));

    // Wait for agent to be ready (has processed initial heartbeat)
    agent.wait_ready();

    let pending_dir = root.join(PENDING_DIR);

    // Submit 5 tasks with IDENTICAL content
    let identical_content = r#"{"message": "same"}"#;
    for i in 0..5 {
        submit_task(&pending_dir, &format!("identical-{i}"), identical_content);
        thread::sleep(Duration::from_millis(30));
    }

    // Wait for all responses
    for i in 0..5 {
        let submission_dir = pending_dir.join(format!("identical-{i}"));
        let response = wait_for_response(&submission_dir, 2000);
        assert!(response.is_some(), "Task {i} should complete");
    }

    let processed = agent.stop();
    assert_eq!(
        processed.len(),
        5,
        "Agent should have processed all 5 identical tasks"
    );

    cleanup_test_dir(&format!("{TEST_DIR}_identical"));
}

/// Test agent joining while tasks are being processed.
#[test]
fn agent_joins_mid_processing() {
    let root = setup_test_dir(&format!("{TEST_DIR}_mid_join"));

    if !is_ipc_available(&root) {
        cleanup_test_dir(&format!("{TEST_DIR}_mid_join"));
        return;
    }

    let _pool = AgentPoolHandle::start(&root);

    // Start one slow agent
    let mut agent1 = TestAgent::echo(&root, "slow-agent", Duration::from_millis(100));

    // Wait for agent to be ready (has processed initial heartbeat)
    agent1.wait_ready();

    let pending_dir = root.join(PENDING_DIR);

    // Submit 6 tasks
    for i in 0..6 {
        submit_task(
            &pending_dir,
            &format!("task-{i}"),
            &format!(r#"{{"n": {i}}}"#),
        );
    }

    // Wait a bit, then add a second fast agent
    thread::sleep(Duration::from_millis(150));
    let mut agent2 = TestAgent::echo(&root, "fast-agent", Duration::from_millis(5));

    // Wait for second agent to be ready (has processed initial heartbeat)
    agent2.wait_ready();

    // Wait for all responses
    for i in 0..6 {
        let submission_dir = pending_dir.join(format!("task-{i}"));
        let response = wait_for_response(&submission_dir, 3000);
        assert!(response.is_some(), "Task {i} should complete");
    }

    let processed1 = agent1.stop();
    let processed2 = agent2.stop();
    let total = processed1.len() + processed2.len();

    assert_eq!(total, 6, "Both agents combined should process all 6 tasks");
    assert!(!processed2.is_empty(), "Second agent should have helped");

    cleanup_test_dir(&format!("{TEST_DIR}_mid_join"));
}

/// Test that responses are written to the correct submission directories.
#[test]
fn response_isolation() {
    let root = setup_test_dir(&format!("{TEST_DIR}_isolation"));

    if !is_ipc_available(&root) {
        cleanup_test_dir(&format!("{TEST_DIR}_isolation"));
        return;
    }

    let _pool = AgentPoolHandle::start(&root);

    let mut agent = TestAgent::start(&root, "echo-agent", Duration::from_millis(5), |task, _| {
        format!("processed: {}", task.trim())
    });

    // Wait for agent to be ready (has processed initial heartbeat)
    agent.wait_ready();

    let pending_dir = root.join(PENDING_DIR);

    submit_task(&pending_dir, "task-a", r#"{"id": "A"}"#);
    submit_task(&pending_dir, "task-b", r#"{"id": "B"}"#);
    submit_task(&pending_dir, "task-c", r#"{"id": "C"}"#);

    let response_a = wait_for_response(&pending_dir.join("task-a"), 2000);
    let response_b = wait_for_response(&pending_dir.join("task-b"), 2000);
    let response_c = wait_for_response(&pending_dir.join("task-c"), 2000);

    let stdout_a = extract_stdout(&response_a.expect("response A")).expect("stdout A");
    let stdout_b = extract_stdout(&response_b.expect("response B")).expect("stdout B");
    let stdout_c = extract_stdout(&response_c.expect("response C")).expect("stdout C");

    assert!(stdout_a.contains(r#""id":"A""#));
    assert!(stdout_b.contains(r#""id":"B""#));
    assert!(stdout_c.contains(r#""id":"C""#));

    agent.stop();
    cleanup_test_dir(&format!("{TEST_DIR}_isolation"));
}
