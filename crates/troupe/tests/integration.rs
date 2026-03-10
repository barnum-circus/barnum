//! Integration tests for the troupe daemon.
//!
//! These tests verify the daemon works end-to-end using CLI-based task submission.

#![expect(clippy::expect_used)]
#![expect(clippy::panic)]

mod common;

use common::{
    AgentsSnapshot, DataSource, NotifyMethod, SubmissionsSnapshot, TestAgent, TroupeHandle,
    cleanup_pool, generate_pool, is_ipc_available, mode_abbrev, pool_path, submit_with_mode,
};
use rstest::rstest;
use std::thread;
use std::time::Duration;
use troupe::Response;

/// Test basic submission flow.
#[rstest]
#[timeout(std::time::Duration::from_secs(20))]
#[case(DataSource::Inline, NotifyMethod::Socket)]
#[case(DataSource::Inline, NotifyMethod::File)]
#[case(DataSource::FileReference, NotifyMethod::Socket)]
#[case(DataSource::FileReference, NotifyMethod::File)]
fn basic_submit(#[case] data_source: DataSource, #[case] notify_method: NotifyMethod) {
    let pool = generate_pool(&format!(
        "basic_{}",
        mode_abbrev(data_source, notify_method)
    ));

    if !is_ipc_available(&pool_path(&pool)) {
        cleanup_pool(&pool);
        return;
    }

    // === Sync point 1: Pool started, no agents ===
    let _pool_handle = TroupeHandle::start(&pool, &pool);
    AgentsSnapshot::capture(&pool).assert_no_agents();

    let agent = TestAgent::echo(&pool, "agent-1", Duration::from_millis(5), &pool);

    let response = submit_with_mode(
        &pool,
        r#"{"kind":"Task","task":{"instructions":"echo","data":{"message":"Hello!"}}}"#,
        data_source,
        notify_method,
    )
    .expect("Submit failed");

    let Response::Processed { stdout, .. } = response else {
        panic!("Expected Processed response");
    };
    assert!(stdout.contains("[processed]"));

    // === Sync point 3: Task processed, submissions clean ===
    SubmissionsSnapshot::capture(&pool).assert_empty();

    // === Sync point 4: Agent stopped ===
    let _ = agent.stop();

    cleanup_pool(&pool);
}

/// Test multiple tasks dispatched to a single agent.
#[rstest]
#[timeout(std::time::Duration::from_secs(20))]
#[case(DataSource::Inline, NotifyMethod::Socket)]
#[case(DataSource::Inline, NotifyMethod::File)]
#[case(DataSource::FileReference, NotifyMethod::Socket)]
#[case(DataSource::FileReference, NotifyMethod::File)]
fn single_agent_multiple_tasks(
    #[case] data_source: DataSource,
    #[case] notify_method: NotifyMethod,
) {
    let pool = generate_pool(&format!(
        "single_multi_{}",
        mode_abbrev(data_source, notify_method)
    ));

    if !is_ipc_available(&pool_path(&pool)) {
        cleanup_pool(&pool);
        return;
    }

    // === Sync point 1: Pool started, no agents ===
    let _pool_handle = TroupeHandle::start(&pool, &pool);
    AgentsSnapshot::capture(&pool).assert_no_agents();

    let agent = TestAgent::echo(&pool, "agent-1", Duration::from_millis(5), &pool);

    // Submit 3 tasks sequentially
    for i in 0..3 {
        let response = submit_with_mode(
            &pool,
            &format!(r#"{{"kind":"Task","task":{{"instructions":"echo","data":{{"n":{i}}}}}}}"#),
            data_source,
            notify_method,
        )
        .expect("Submit failed");

        let Response::Processed { stdout, .. } = response else {
            panic!("Expected Processed response for task {i}");
        };
        assert!(stdout.contains("[processed]"));
    }

    // === Sync point 3: All tasks processed, submissions clean ===
    SubmissionsSnapshot::capture(&pool).assert_empty();

    // === Sync point 4: Agent stopped ===
    let processed = agent.stop();
    assert_eq!(processed.len(), 3, "Agent should process all 3 tasks");

    cleanup_pool(&pool);
}

/// Test multiple agents handling tasks in parallel.
#[rstest]
#[timeout(std::time::Duration::from_secs(20))]
#[case(DataSource::Inline, NotifyMethod::Socket)]
#[case(DataSource::Inline, NotifyMethod::File)]
#[case(DataSource::FileReference, NotifyMethod::Socket)]
#[case(DataSource::FileReference, NotifyMethod::File)]
fn multiple_agents_parallel(#[case] data_source: DataSource, #[case] notify_method: NotifyMethod) {
    let pool = generate_pool(&format!(
        "multi_para_{}",
        mode_abbrev(data_source, notify_method)
    ));

    if !is_ipc_available(&pool_path(&pool)) {
        cleanup_pool(&pool);
        return;
    }

    // === Sync point 1: Pool started, no agents ===
    let _pool_handle = TroupeHandle::start(&pool, &pool);
    AgentsSnapshot::capture(&pool).assert_no_agents();

    let agent1 = TestAgent::echo(&pool, "agent-1", Duration::from_millis(50), &pool);
    let agent2 = TestAgent::echo(&pool, "agent-2", Duration::from_millis(50), &pool);

    // Submit 4 tasks in parallel
    let handles: Vec<_> = (0..4)
        .map(|i| {
            let pool = pool.clone();
            let task =
                format!(r#"{{"kind":"Task","task":{{"instructions":"echo","data":{{"n":{i}}}}}}}"#);
            thread::spawn(move || {
                submit_with_mode(&pool, &task, data_source, notify_method).expect("Submit failed")
            })
        })
        .collect();

    for handle in handles {
        let response = handle.join().expect("Thread panicked");
        let Response::Processed { stdout, .. } = response else {
            panic!("Expected Processed response");
        };
        assert!(stdout.contains("[processed]"));
    }

    // === Sync point 3: All tasks processed, submissions clean ===
    SubmissionsSnapshot::capture(&pool).assert_empty();

    // === Sync point 4: Both agents stopped ===
    let processed1 = agent1.stop();
    let processed2 = agent2.stop();
    let total = processed1.len() + processed2.len();

    assert_eq!(total, 4, "Both agents combined should process all 4 tasks");

    cleanup_pool(&pool);
}

/// Test tasks submitted before any agents register (queued).
#[rstest]
#[timeout(std::time::Duration::from_secs(20))]
#[case(DataSource::Inline, NotifyMethod::Socket)]
#[case(DataSource::Inline, NotifyMethod::File)]
#[case(DataSource::FileReference, NotifyMethod::Socket)]
#[case(DataSource::FileReference, NotifyMethod::File)]
fn tasks_queued_before_agents(
    #[case] data_source: DataSource,
    #[case] notify_method: NotifyMethod,
) {
    let pool = generate_pool(&format!(
        "queued_{}",
        mode_abbrev(data_source, notify_method)
    ));

    if !is_ipc_available(&pool_path(&pool)) {
        cleanup_pool(&pool);
        return;
    }

    // === Sync point 1: Pool started, no agents ===
    let _pool_handle = TroupeHandle::start(&pool, &pool);
    AgentsSnapshot::capture(&pool).assert_no_agents();

    // Submit tasks BEFORE any agents register (they'll block until an agent picks them up)
    let handles: Vec<_> = (0..3)
        .map(|i| {
            let pool = pool.clone();
            let task =
                format!(r#"{{"kind":"Task","task":{{"instructions":"echo","data":{{"n":{i}}}}}}}"#);
            thread::spawn(move || {
                submit_with_mode(&pool, &task, data_source, notify_method).expect("Submit failed")
            })
        })
        .collect();

    // Small delay to ensure tasks are queued, then register an agent
    thread::sleep(Duration::from_millis(50));

    // Agent joins late
    let agent = TestAgent::echo(&pool, "late-agent", Duration::from_millis(5), &pool);

    // Wait for all tasks to complete
    for handle in handles {
        let response = handle.join().expect("Thread panicked");
        assert!(matches!(response, Response::Processed { .. }));
    }

    // === Sync point 3: All tasks processed, submissions clean ===
    SubmissionsSnapshot::capture(&pool).assert_empty();

    // === Sync point 4: Agent stopped ===
    let processed = agent.stop();
    assert_eq!(
        processed.len(),
        3,
        "Agent should process all 3 queued tasks"
    );

    cleanup_pool(&pool);
}

/// Test rapid burst of task submissions.
#[rstest]
#[timeout(std::time::Duration::from_secs(20))]
#[case(DataSource::Inline, NotifyMethod::Socket)]
#[case(DataSource::Inline, NotifyMethod::File)]
#[case(DataSource::FileReference, NotifyMethod::Socket)]
#[case(DataSource::FileReference, NotifyMethod::File)]
fn rapid_task_burst(#[case] data_source: DataSource, #[case] notify_method: NotifyMethod) {
    let pool = generate_pool(&format!(
        "burst_{}",
        mode_abbrev(data_source, notify_method)
    ));

    if !is_ipc_available(&pool_path(&pool)) {
        cleanup_pool(&pool);
        return;
    }

    // === Sync point 1: Pool started, no agents ===
    let _pool_handle = TroupeHandle::start(&pool, &pool);
    AgentsSnapshot::capture(&pool).assert_no_agents();

    let agent = TestAgent::echo(&pool, "burst-agent", Duration::from_millis(2), &pool);

    // Submit 10 tasks as fast as possible in parallel
    let handles: Vec<_> = (0..10)
        .map(|i| {
            let pool = pool.clone();
            let task =
                format!(r#"{{"kind":"Task","task":{{"instructions":"echo","data":{{"n":{i}}}}}}}"#);
            thread::spawn(move || {
                submit_with_mode(&pool, &task, data_source, notify_method).expect("Submit failed")
            })
        })
        .collect();

    for handle in handles {
        let response = handle.join().expect("Thread panicked");
        assert!(matches!(response, Response::Processed { .. }));
    }

    // === Sync point 3: All tasks processed, submissions clean ===
    SubmissionsSnapshot::capture(&pool).assert_empty();

    // === Sync point 4: Agent stopped ===
    let processed = agent.stop();
    assert_eq!(processed.len(), 10, "Agent should process all 10 tasks");

    cleanup_pool(&pool);
}

/// Test that tasks with identical content are handled correctly.
#[rstest]
#[timeout(std::time::Duration::from_secs(20))]
#[case(DataSource::Inline, NotifyMethod::Socket)]
#[case(DataSource::Inline, NotifyMethod::File)]
#[case(DataSource::FileReference, NotifyMethod::Socket)]
#[case(DataSource::FileReference, NotifyMethod::File)]
fn identical_task_content(#[case] data_source: DataSource, #[case] notify_method: NotifyMethod) {
    let pool = generate_pool(&format!(
        "ident_{}",
        mode_abbrev(data_source, notify_method)
    ));

    if !is_ipc_available(&pool_path(&pool)) {
        cleanup_pool(&pool);
        return;
    }

    // === Sync point 1: Pool started, no agents ===
    let _pool_handle = TroupeHandle::start(&pool, &pool);
    AgentsSnapshot::capture(&pool).assert_no_agents();

    let agent = TestAgent::echo(&pool, "agent-1", Duration::from_millis(5), &pool);

    // Submit 5 tasks with IDENTICAL content
    let task = r#"{"kind":"Task","task":{"instructions":"echo","data":{"message":"same"}}}"#;
    for _ in 0..5 {
        let response =
            submit_with_mode(&pool, task, data_source, notify_method).expect("Submit failed");
        assert!(matches!(response, Response::Processed { .. }));
    }

    // === Sync point 3: All tasks processed, submissions clean ===
    SubmissionsSnapshot::capture(&pool).assert_empty();

    // === Sync point 4: Agent stopped ===
    let processed = agent.stop();
    assert_eq!(
        processed.len(),
        5,
        "Agent should process all 5 identical tasks"
    );

    cleanup_pool(&pool);
}

/// Test agent joining while tasks are being processed.
#[rstest]
#[timeout(std::time::Duration::from_secs(20))]
#[case(DataSource::Inline, NotifyMethod::Socket)]
#[case(DataSource::Inline, NotifyMethod::File)]
#[case(DataSource::FileReference, NotifyMethod::Socket)]
#[case(DataSource::FileReference, NotifyMethod::File)]
fn agent_joins_mid_processing(
    #[case] data_source: DataSource,
    #[case] notify_method: NotifyMethod,
) {
    let pool = generate_pool(&format!(
        "join_mid_{}",
        mode_abbrev(data_source, notify_method)
    ));

    if !is_ipc_available(&pool_path(&pool)) {
        cleanup_pool(&pool);
        return;
    }

    // === Sync point 1: Pool started, no agents ===
    let _pool_handle = TroupeHandle::start(&pool, &pool);
    AgentsSnapshot::capture(&pool).assert_no_agents();

    // Start one slow agent
    let agent1 = TestAgent::echo(&pool, "slow-agent", Duration::from_millis(100), &pool);

    // Submit 6 tasks in parallel
    let handles: Vec<_> = (0..6)
        .map(|i| {
            let pool = pool.clone();
            let task =
                format!(r#"{{"kind":"Task","task":{{"instructions":"echo","data":{{"n":{i}}}}}}}"#);
            thread::spawn(move || {
                submit_with_mode(&pool, &task, data_source, notify_method).expect("Submit failed")
            })
        })
        .collect();

    // Wait a bit to let first agent start processing, then add a second fast agent
    thread::sleep(Duration::from_millis(150));

    // Second agent joins mid-processing
    let agent2 = TestAgent::echo(&pool, "fast-agent", Duration::from_millis(5), &pool);

    // Wait for all tasks to complete
    for handle in handles {
        let response = handle.join().expect("Thread panicked");
        assert!(matches!(response, Response::Processed { .. }));
    }

    // === Sync point 4: All tasks processed, submissions clean ===
    SubmissionsSnapshot::capture(&pool).assert_empty();

    // === Sync point 5: Both agents stopped ===
    let processed1 = agent1.stop();
    let processed2 = agent2.stop();
    let total = processed1.len() + processed2.len();

    assert_eq!(total, 6, "Both agents combined should process all 6 tasks");
    assert!(!processed2.is_empty(), "Second agent should have helped");

    cleanup_pool(&pool);
}

/// Test that responses are written to the correct submitters.
#[rstest]
#[timeout(std::time::Duration::from_secs(20))]
#[case(DataSource::Inline, NotifyMethod::Socket)]
#[case(DataSource::Inline, NotifyMethod::File)]
#[case(DataSource::FileReference, NotifyMethod::Socket)]
#[case(DataSource::FileReference, NotifyMethod::File)]
fn response_isolation(#[case] data_source: DataSource, #[case] notify_method: NotifyMethod) {
    let pool = generate_pool(&format!(
        "resp_iso_{}",
        mode_abbrev(data_source, notify_method)
    ));

    if !is_ipc_available(&pool_path(&pool)) {
        cleanup_pool(&pool);
        return;
    }

    // === Sync point 1: Pool started, no agents ===
    let _pool_handle = TroupeHandle::start(&pool, &pool);
    AgentsSnapshot::capture(&pool).assert_no_agents();

    let agent = TestAgent::start(
        &pool,
        "echo-agent",
        Duration::from_millis(5),
        |task, _| format!("processed: {}", task.trim()),
        &pool,
    );

    // Submit tasks with distinct IDs in parallel
    let handles: Vec<_> = ["A", "B", "C"]
        .iter()
        .map(|id| {
            let pool = pool.clone();
            let task = format!(
                r#"{{"kind":"Task","task":{{"instructions":"echo","data":{{"id":"{id}"}}}}}}"#
            );
            let expected_id = id.to_string();
            thread::spawn(move || {
                let response = submit_with_mode(&pool, &task, data_source, notify_method)
                    .expect("Submit failed");
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

    // === Sync point 3: All tasks processed, submissions clean ===
    SubmissionsSnapshot::capture(&pool).assert_empty();

    // === Sync point 4: Agent stopped ===
    agent.stop();

    cleanup_pool(&pool);
}
