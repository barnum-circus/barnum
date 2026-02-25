//! Test corresponding to demos/greeting.sh
//! Greeting agent with casual and formal styles.

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]
#![expect(clippy::panic)]

mod common;

use agent_pool::Response;
use common::{
    AgentPoolHandle, TestAgent, cleanup_test_dir, is_ipc_available, setup_test_dir, submit_via_cli,
};
use std::time::Duration;

const TEST_DIR: &str = "greeting";

#[test]
fn greeting_casual_and_formal() {
    let root = setup_test_dir(TEST_DIR);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(TEST_DIR);
        return;
    }

    let _pool = AgentPoolHandle::start(&root);
    let mut agent = TestAgent::greeting(&root, "friendly-bot", Duration::from_millis(10));

    // Wait for agent to be ready (has processed initial heartbeat)
    agent.wait_ready();

    let casual = submit_via_cli(
        &root,
        r#"{"kind":"Task","task":{"instructions":"greet","data":"casual"}}"#,
        "socket",
    )
    .expect("Submit failed");
    let Response::Processed { stdout, .. } = casual else {
        panic!("Expected Processed response, got {casual:?}");
    };
    assert_eq!(stdout.trim(), "Hi friendly-bot, how are ya?");

    let formal = submit_via_cli(
        &root,
        r#"{"kind":"Task","task":{"instructions":"greet","data":"formal"}}"#,
        "socket",
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

    cleanup_test_dir(TEST_DIR);
}
