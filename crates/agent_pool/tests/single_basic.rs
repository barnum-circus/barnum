//! Test corresponding to demos/single-basic.sh
//! Single agent, single task.

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]
#![expect(clippy::panic)]

mod common;

use agent_pool::Response;
use common::{
    AgentPoolHandle, DataSource, NotifyMethod, TestAgent, cleanup_pool, generate_pool,
    is_ipc_available, submit_with_mode,
};
use rstest::rstest;
use std::time::Duration;

const TEST_NAME: &str = "single_basic";

#[rstest]
#[timeout(std::time::Duration::from_secs(20))]
#[case(DataSource::Inline, NotifyMethod::Socket)]
#[case(DataSource::Inline, NotifyMethod::File)]
#[case(DataSource::Inline, NotifyMethod::Raw)]
#[case(DataSource::FileReference, NotifyMethod::Socket)]
#[case(DataSource::FileReference, NotifyMethod::File)]
#[case(DataSource::FileReference, NotifyMethod::Raw)]
fn single_agent_single_task(#[case] data_source: DataSource, #[case] notify_method: NotifyMethod) {
    let pool = generate_pool(&format!("{TEST_NAME}_{data_source:?}_{notify_method:?}"));

    if !is_ipc_available() {
        eprintln!("SKIP: IPC not available");
        cleanup_pool(&pool);
        return;
    }

    let _pool_handle = AgentPoolHandle::start(&pool, &pool);
    let mut agent = TestAgent::echo(&pool, "agent-1", Duration::from_millis(10), &pool);

    // Wait for agent to be ready (has processed initial heartbeat)
    agent.wait_ready();

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

    let _ = agent.stop();

    cleanup_pool(&pool);
}
