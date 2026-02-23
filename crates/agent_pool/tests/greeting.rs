//! Test corresponding to demos/greeting.sh
//! Greeting agent with casual and formal styles.

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]
#![expect(clippy::panic)]

mod common;

use agent_pool::{Payload, Response};
use common::{AgentPoolHandle, TestAgent, cleanup_test_dir, is_ipc_available, setup_test_dir};
use std::thread;
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
    let agent = TestAgent::greeting(&root, "friendly-bot", Duration::from_millis(10));

    // Give agent time to register
    thread::sleep(Duration::from_millis(200));

    let casual = agent_pool::submit(&root, &Payload::inline("casual")).expect("Submit failed");
    let Response::Processed { stdout, .. } = casual else {
        panic!("Expected Processed response, got {casual:?}");
    };
    assert_eq!(stdout.trim(), "Hi friendly-bot, how are ya?");

    let formal = agent_pool::submit(&root, &Payload::inline("formal")).expect("Submit failed");
    let Response::Processed { stdout, .. } = formal else {
        panic!("Expected Processed response, got {formal:?}");
    };
    assert_eq!(
        stdout.trim(),
        "Salutations friendly-bot, how are you doing on this most splendiferous and utterly magnificent day?"
    );

    let processed = agent.stop();
    assert_eq!(processed, vec!["casual", "formal"]);

    cleanup_test_dir(TEST_DIR);
}
