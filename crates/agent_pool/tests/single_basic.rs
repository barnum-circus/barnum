//! Test corresponding to demos/single-basic.sh
//! Single agent, single task.

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]
#![expect(clippy::panic)]

mod common;

use agent_pool::Response;
use common::{
    AgentPoolHandle, AgentsSnapshot, DataSource, NotifyMethod, SubmissionsSnapshot, TestAgent,
    cleanup_pool, generate_pool, is_ipc_available, mode_abbrev, pool_path, submit_with_mode,
};
use rstest::rstest;
use std::time::Duration;

const TEST_NAME: &str = "single_basic";

#[rstest]
#[timeout(std::time::Duration::from_secs(20))]
#[case(DataSource::Inline, NotifyMethod::Socket)]
#[case(DataSource::Inline, NotifyMethod::File)]
#[case(DataSource::FileReference, NotifyMethod::Socket)]
#[case(DataSource::FileReference, NotifyMethod::File)]
fn single_agent_single_task(#[case] data_source: DataSource, #[case] notify_method: NotifyMethod) {
    let pool = generate_pool(&format!(
        "{TEST_NAME}_{}",
        mode_abbrev(data_source, notify_method)
    ));

    if !is_ipc_available(&pool_path(&pool)) {
        eprintln!("SKIP: IPC not available");
        cleanup_pool(&pool);
        return;
    }

    // === Sync point 1: Pool started, no agents yet ===
    let _pool_handle = AgentPoolHandle::start(&pool, &pool);
    let agents = AgentsSnapshot::capture(&pool);
    agents.assert_no_agents();

    let mut agent = TestAgent::echo(&pool, "agent-1", Duration::from_millis(10), &pool);

    // === Sync point 2: Agent ready (processed initial heartbeat) ===
    agent.wait_ready();
    let agents = AgentsSnapshot::capture(&pool);
    agents.assert_agent_exists("agent-1");
    // Agent dir should have task.json (heartbeat response pending or next heartbeat)
    // Note: exact files depend on timing, so we just check the dir exists

    // === Sync point 3: Task submitted and processed ===
    let response = submit_with_mode(
        &pool,
        r#"{"kind":"Task","task":{"instructions":"echo","data":"Hello, World!"}}"#,
        data_source,
        notify_method,
    )
    .expect("Submit failed");
    let Response::Processed { stdout, .. } = response else {
        panic!("Expected Processed response, got {response:?}");
    };
    assert!(stdout.contains(r#""data":"Hello, World!""#));
    assert!(stdout.contains("[processed]"));

    // After task processed, submissions dir should be clean (response delivered)
    let submissions = SubmissionsSnapshot::capture(&pool);
    submissions.assert_empty();

    // === Sync point 4: Agent stopped ===
    let _ = agent.stop();
    let agents = AgentsSnapshot::capture(&pool);
    agents.assert_agent_not_exists("agent-1");

    cleanup_pool(&pool);
}
