//! Test corresponding to demos/greeting.sh
//! Greeting agent with casual and formal styles.

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

const TEST_DIR: &str = "greeting";

#[rstest]
#[case(DataSource::Inline, NotifyMethod::Socket)]
#[case(DataSource::Inline, NotifyMethod::File)]
#[case(DataSource::Inline, NotifyMethod::Raw)]
#[case(DataSource::FileReference, NotifyMethod::Socket)]
#[case(DataSource::FileReference, NotifyMethod::File)]
#[case(DataSource::FileReference, NotifyMethod::Raw)]
fn greeting_casual_and_formal(
    #[case] data_source: DataSource,
    #[case] notify_method: NotifyMethod,
) {
    // Use mode in test dir name to avoid conflicts when tests run in parallel
    let test_dir = format!("{TEST_DIR}_{data_source:?}_{notify_method:?}");
    let root = setup_test_dir(&test_dir);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&test_dir);
        return;
    }

    let _pool = AgentPoolHandle::start(&root, &test_dir);
    let mut agent =
        TestAgent::greeting(&root, "friendly-bot", Duration::from_millis(10), &test_dir);

    // Wait for agent to be ready (has processed initial heartbeat)
    agent.wait_ready();

    let casual = submit_with_mode(
        &root,
        r#"{"kind":"Task","task":{"instructions":"greet","data":"casual"}}"#,
        data_source,
        notify_method,
    )
    .expect("Submit failed");
    let Response::Processed { stdout, .. } = casual else {
        panic!("Expected Processed response, got {casual:?}");
    };
    assert_eq!(stdout.trim(), "Hi friendly-bot, how are ya?");

    let formal = submit_with_mode(
        &root,
        r#"{"kind":"Task","task":{"instructions":"greet","data":"formal"}}"#,
        data_source,
        notify_method,
    )
    .expect("Submit failed");
    let Response::Processed { stdout, .. } = formal else {
        panic!("Expected Processed response, got {formal:?}");
    };
    assert_eq!(
        stdout.trim(),
        "Salutations friendly-bot, how are you doing on this most splendiferous and utterly magnificent day?"
    );

    // Note: processed contains the full task JSON
    let _ = agent.stop();

    cleanup_test_dir(&test_dir);
}
