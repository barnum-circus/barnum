//! Integration tests for the agent pool daemon.
//!
//! These tests verify the daemon works end-to-end using CLI-based task submission.

#![expect(clippy::expect_used)]
#![expect(clippy::panic)]

mod common;

use agent_pool::Response;
use common::{
    AgentPoolHandle, SubmitMode, TestAgent, cleanup_test_dir, is_ipc_available, setup_test_dir,
    submit_with_mode,
};
use rstest::rstest;
use std::thread;
use std::time::Duration;

/// Wait for all agents to be ready (have processed their initial heartbeats).
fn wait_all_ready(agents: &mut [&mut TestAgent]) {
    for agent in agents {
        agent.wait_ready();
    }
}

const TEST_DIR: &str = "integration";

/// Test basic submission flow.
#[rstest]
#[case(SubmitMode::DataSocket)]
#[case(SubmitMode::DataFile)]
#[case(SubmitMode::FileSocket)]
#[case(SubmitMode::FileFile)]
fn basic_submit(#[case] mode: SubmitMode) {
    let test_dir = format!("{TEST_DIR}_basic_{mode:?}");
    let root = setup_test_dir(&test_dir);

    if !is_ipc_available(&root) {
        cleanup_test_dir(&test_dir);
        return;
    }

    let _pool = AgentPoolHandle::start(&root);
    let mut agent = TestAgent::echo(&root, "agent-1", Duration::from_millis(5));
    agent.wait_ready();

    let response = submit_with_mode(
        &root,
        r#"{"kind":"Task","task":{"instructions":"echo","data":{"message":"Hello!"}}}"#,
        mode,
    )
    .expect("Submit failed");

    let Response::Processed { stdout, .. } = response else {
        panic!("Expected Processed response");
    };
    assert!(stdout.contains("[processed]"));

    let _ = agent.stop();
    cleanup_test_dir(&test_dir);
}

/// Test multiple tasks dispatched to a single agent.
#[rstest]
#[case(SubmitMode::DataSocket)]
#[case(SubmitMode::DataFile)]
#[case(SubmitMode::FileSocket)]
#[case(SubmitMode::FileFile)]
fn single_agent_multiple_tasks(#[case] mode: SubmitMode) {
    let test_dir = format!("{TEST_DIR}_single_multi_{mode:?}");
    let root = setup_test_dir(&test_dir);

    if !is_ipc_available(&root) {
        cleanup_test_dir(&test_dir);
        return;
    }

    let _pool = AgentPoolHandle::start(&root);
    let mut agent = TestAgent::echo(&root, "agent-1", Duration::from_millis(5));
    agent.wait_ready();

    // Submit 3 tasks sequentially
    for i in 0..3 {
        let response = submit_with_mode(
            &root,
            &format!(r#"{{"kind":"Task","task":{{"instructions":"echo","data":{{"n":{i}}}}}}}"#),
            mode,
        )
        .expect("Submit failed");

        let Response::Processed { stdout, .. } = response else {
            panic!("Expected Processed response for task {i}");
        };
        assert!(stdout.contains("[processed]"));
    }

    let processed = agent.stop();
    assert_eq!(processed.len(), 3, "Agent should process all 3 tasks");

    cleanup_test_dir(&test_dir);
}

/// Test multiple agents handling tasks in parallel.
#[rstest]
#[case(SubmitMode::DataSocket)]
#[case(SubmitMode::DataFile)]
#[case(SubmitMode::FileSocket)]
#[case(SubmitMode::FileFile)]
fn multiple_agents_parallel(#[case] mode: SubmitMode) {
    let test_dir = format!("{TEST_DIR}_multi_parallel_{mode:?}");
    let root = setup_test_dir(&test_dir);

    if !is_ipc_available(&root) {
        cleanup_test_dir(&test_dir);
        return;
    }

    let _pool = AgentPoolHandle::start(&root);

    let mut agent1 = TestAgent::echo(&root, "agent-1", Duration::from_millis(50));
    let mut agent2 = TestAgent::echo(&root, "agent-2", Duration::from_millis(50));
    wait_all_ready(&mut [&mut agent1, &mut agent2]);

    // Submit 4 tasks in parallel
    let handles: Vec<_> = (0..4)
        .map(|i| {
            let root = root.clone();
            let task =
                format!(r#"{{"kind":"Task","task":{{"instructions":"echo","data":{{"n":{i}}}}}}}"#);
            thread::spawn(move || submit_with_mode(&root, &task, mode).expect("Submit failed"))
        })
        .collect();

    for handle in handles {
        let response = handle.join().expect("Thread panicked");
        let Response::Processed { stdout, .. } = response else {
            panic!("Expected Processed response");
        };
        assert!(stdout.contains("[processed]"));
    }

    let processed1 = agent1.stop();
    let processed2 = agent2.stop();
    let total = processed1.len() + processed2.len();

    assert_eq!(total, 4, "Both agents combined should process all 4 tasks");

    cleanup_test_dir(&test_dir);
}

/// Test agent deregistration.
#[rstest]
#[case(SubmitMode::DataSocket)]
#[case(SubmitMode::DataFile)]
#[case(SubmitMode::FileSocket)]
#[case(SubmitMode::FileFile)]
fn agent_deregistration(#[case] mode: SubmitMode) {
    let test_dir = format!("{TEST_DIR}_deregister_{mode:?}");
    let root = setup_test_dir(&test_dir);

    if !is_ipc_available(&root) {
        cleanup_test_dir(&test_dir);
        return;
    }

    let _pool = AgentPoolHandle::start(&root);
    let mut agent = TestAgent::echo(&root, "agent-1", Duration::from_millis(5));
    agent.wait_ready();

    let response = submit_with_mode(
        &root,
        r#"{"kind":"Task","task":{"instructions":"echo","data":{"test":"before"}}}"#,
        mode,
    )
    .expect("Submit failed");
    assert!(matches!(response, Response::Processed { .. }));

    // Stop the agent
    let processed = agent.stop();
    assert_eq!(processed.len(), 1);

    // Wait for daemon to notice agent is gone
    thread::sleep(Duration::from_millis(100));

    // Start a new agent
    let mut agent2 = TestAgent::echo(&root, "agent-2", Duration::from_millis(5));
    agent2.wait_ready();

    let response2 = submit_with_mode(
        &root,
        r#"{"kind":"Task","task":{"instructions":"echo","data":{"test":"after"}}}"#,
        mode,
    )
    .expect("Submit failed");
    assert!(matches!(response2, Response::Processed { .. }));

    let processed2 = agent2.stop();
    assert_eq!(processed2.len(), 1);

    cleanup_test_dir(&test_dir);
}

/// Test tasks submitted before any agents register (queued).
#[rstest]
#[case(SubmitMode::DataSocket)]
#[case(SubmitMode::DataFile)]
#[case(SubmitMode::FileSocket)]
#[case(SubmitMode::FileFile)]
fn tasks_queued_before_agents(#[case] mode: SubmitMode) {
    let test_dir = format!("{TEST_DIR}_queue_before_{mode:?}");
    let root = setup_test_dir(&test_dir);

    if !is_ipc_available(&root) {
        cleanup_test_dir(&test_dir);
        return;
    }

    let _pool = AgentPoolHandle::start(&root);

    // Submit tasks BEFORE any agents register (they'll block until an agent picks them up)
    let handles: Vec<_> = (0..3)
        .map(|i| {
            let root = root.clone();
            let task =
                format!(r#"{{"kind":"Task","task":{{"instructions":"echo","data":{{"n":{i}}}}}}}"#);
            thread::spawn(move || submit_with_mode(&root, &task, mode).expect("Submit failed"))
        })
        .collect();

    // Small delay, then register an agent
    thread::sleep(Duration::from_millis(50));
    let mut agent = TestAgent::echo(&root, "late-agent", Duration::from_millis(5));
    agent.wait_ready();

    // Wait for all tasks to complete
    for handle in handles {
        let response = handle.join().expect("Thread panicked");
        assert!(matches!(response, Response::Processed { .. }));
    }

    let processed = agent.stop();
    assert_eq!(
        processed.len(),
        3,
        "Agent should process all 3 queued tasks"
    );

    cleanup_test_dir(&test_dir);
}

/// Test rapid burst of task submissions.
#[rstest]
#[case(SubmitMode::DataSocket)]
#[case(SubmitMode::DataFile)]
#[case(SubmitMode::FileSocket)]
#[case(SubmitMode::FileFile)]
fn rapid_task_burst(#[case] mode: SubmitMode) {
    let test_dir = format!("{TEST_DIR}_rapid_burst_{mode:?}");
    let root = setup_test_dir(&test_dir);

    if !is_ipc_available(&root) {
        cleanup_test_dir(&test_dir);
        return;
    }

    let _pool = AgentPoolHandle::start(&root);
    let mut agent = TestAgent::echo(&root, "burst-agent", Duration::from_millis(2));
    agent.wait_ready();

    // Submit 10 tasks as fast as possible in parallel
    let handles: Vec<_> = (0..10)
        .map(|i| {
            let root = root.clone();
            let task =
                format!(r#"{{"kind":"Task","task":{{"instructions":"echo","data":{{"n":{i}}}}}}}"#);
            thread::spawn(move || submit_with_mode(&root, &task, mode).expect("Submit failed"))
        })
        .collect();

    for handle in handles {
        let response = handle.join().expect("Thread panicked");
        assert!(matches!(response, Response::Processed { .. }));
    }

    let processed = agent.stop();
    assert_eq!(processed.len(), 10, "Agent should process all 10 tasks");

    cleanup_test_dir(&test_dir);
}

/// Test that tasks with identical content are handled correctly.
#[rstest]
#[case(SubmitMode::DataSocket)]
#[case(SubmitMode::DataFile)]
#[case(SubmitMode::FileSocket)]
#[case(SubmitMode::FileFile)]
fn identical_task_content(#[case] mode: SubmitMode) {
    let test_dir = format!("{TEST_DIR}_identical_{mode:?}");
    let root = setup_test_dir(&test_dir);

    if !is_ipc_available(&root) {
        cleanup_test_dir(&test_dir);
        return;
    }

    let _pool = AgentPoolHandle::start(&root);
    let mut agent = TestAgent::echo(&root, "agent-1", Duration::from_millis(5));
    agent.wait_ready();

    // Submit 5 tasks with IDENTICAL content
    let task = r#"{"kind":"Task","task":{"instructions":"echo","data":{"message":"same"}}}"#;
    for _ in 0..5 {
        let response = submit_with_mode(&root, task, mode).expect("Submit failed");
        assert!(matches!(response, Response::Processed { .. }));
    }

    let processed = agent.stop();
    assert_eq!(
        processed.len(),
        5,
        "Agent should process all 5 identical tasks"
    );

    cleanup_test_dir(&test_dir);
}

/// Test agent joining while tasks are being processed.
#[rstest]
#[case(SubmitMode::DataSocket)]
#[case(SubmitMode::DataFile)]
#[case(SubmitMode::FileSocket)]
#[case(SubmitMode::FileFile)]
fn agent_joins_mid_processing(#[case] mode: SubmitMode) {
    let test_dir = format!("{TEST_DIR}_mid_join_{mode:?}");
    let root = setup_test_dir(&test_dir);

    if !is_ipc_available(&root) {
        cleanup_test_dir(&test_dir);
        return;
    }

    let _pool = AgentPoolHandle::start(&root);

    // Start one slow agent
    let mut agent1 = TestAgent::echo(&root, "slow-agent", Duration::from_millis(100));
    agent1.wait_ready();

    // Submit 6 tasks in parallel
    let handles: Vec<_> = (0..6)
        .map(|i| {
            let root = root.clone();
            let task =
                format!(r#"{{"kind":"Task","task":{{"instructions":"echo","data":{{"n":{i}}}}}}}"#);
            thread::spawn(move || submit_with_mode(&root, &task, mode).expect("Submit failed"))
        })
        .collect();

    // Wait a bit, then add a second fast agent
    thread::sleep(Duration::from_millis(150));
    let mut agent2 = TestAgent::echo(&root, "fast-agent", Duration::from_millis(5));
    agent2.wait_ready();

    // Wait for all tasks to complete
    for handle in handles {
        let response = handle.join().expect("Thread panicked");
        assert!(matches!(response, Response::Processed { .. }));
    }

    let processed1 = agent1.stop();
    let processed2 = agent2.stop();
    let total = processed1.len() + processed2.len();

    assert_eq!(total, 6, "Both agents combined should process all 6 tasks");
    assert!(!processed2.is_empty(), "Second agent should have helped");

    cleanup_test_dir(&test_dir);
}

/// Test that responses are written to the correct submitters.
#[rstest]
#[case(SubmitMode::DataSocket)]
#[case(SubmitMode::DataFile)]
#[case(SubmitMode::FileSocket)]
#[case(SubmitMode::FileFile)]
fn response_isolation(#[case] mode: SubmitMode) {
    let test_dir = format!("{TEST_DIR}_isolation_{mode:?}");
    let root = setup_test_dir(&test_dir);

    if !is_ipc_available(&root) {
        cleanup_test_dir(&test_dir);
        return;
    }

    let _pool = AgentPoolHandle::start(&root);

    let mut agent = TestAgent::start(&root, "echo-agent", Duration::from_millis(5), |task, _| {
        format!("processed: {}", task.trim())
    });
    agent.wait_ready();

    // Submit tasks with distinct IDs in parallel
    let handles: Vec<_> = ["A", "B", "C"]
        .iter()
        .map(|id| {
            let root = root.clone();
            let task = format!(
                r#"{{"kind":"Task","task":{{"instructions":"echo","data":{{"id":"{id}"}}}}}}"#
            );
            let expected_id = id.to_string();
            thread::spawn(move || {
                let response = submit_with_mode(&root, &task, mode).expect("Submit failed");
                (expected_id, response)
            })
        })
        .collect();

    for handle in handles {
        let (expected_id, response) = handle.join().expect("Thread panicked");
        let Response::Processed { stdout, .. } = response else {
            panic!("Expected Processed response");
        };
        assert!(
            stdout.contains(&format!(r#""id":"{expected_id}""#)),
            "Response should contain the correct ID. Expected {expected_id}, got: {stdout}"
        );
    }

    agent.stop();
    cleanup_test_dir(&test_dir);
}
