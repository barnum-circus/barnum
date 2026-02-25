//! Test corresponding to demos/single-basic.sh
//! Single agent, single task.

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]
#![expect(clippy::panic)]

mod common;

use agent_pool::Response;
use common::{
    AgentPoolHandle, DataSource, NotifyMethod, TestAgent, cleanup_test_dir, is_ipc_available,
    setup_test_dir, submit_with_mode,
};
use rstest::rstest;
use std::time::Duration;

const TEST_DIR: &str = "single_basic";

#[rstest]
#[case(DataSource::Inline, NotifyMethod::Socket)]
#[case(DataSource::Inline, NotifyMethod::File)]
#[case(DataSource::Inline, NotifyMethod::Raw)]
#[case(DataSource::FileReference, NotifyMethod::Socket)]
#[case(DataSource::FileReference, NotifyMethod::File)]
#[case(DataSource::FileReference, NotifyMethod::Raw)]
fn single_agent_single_task(#[case] data_source: DataSource, #[case] notify_method: NotifyMethod) {
    let test_dir = format!("{TEST_DIR}_{data_source:?}_{notify_method:?}");
    let root = setup_test_dir(&test_dir);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&test_dir);
        return;
    }

    let _pool = AgentPoolHandle::start(&root, &test_dir);
    let mut agent = TestAgent::echo(&root, "agent-1", Duration::from_millis(10), &test_dir);

    // Wait for agent to be ready (has processed initial heartbeat)
    agent.wait_ready();

    let response = submit_with_mode(
        &root,
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

    cleanup_test_dir(&test_dir);
}
