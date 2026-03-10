//! Test corresponding to demos/greeting.sh
//! Greeting agent with casual and formal styles.

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]
#![expect(clippy::panic)]

mod common;

use common::{
    AgentsSnapshot, DataSource, NotifyMethod, SubmissionsSnapshot, TestAgent, TroupeHandle,
    cleanup_pool, generate_pool, is_ipc_available, mode_abbrev, pool_path, submit_with_mode,
};
use rstest::rstest;
use std::time::Duration;
use troupe::Response;

const TEST_NAME: &str = "greeting";

#[rstest]
#[timeout(std::time::Duration::from_secs(20))]
#[case(DataSource::Inline, NotifyMethod::Socket)]
#[case(DataSource::Inline, NotifyMethod::File)]
#[case(DataSource::FileReference, NotifyMethod::Socket)]
#[case(DataSource::FileReference, NotifyMethod::File)]
fn greeting_casual_and_formal(
    #[case] data_source: DataSource,
    #[case] notify_method: NotifyMethod,
) {
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
    let _pool_handle = TroupeHandle::start(&pool, &pool);
    let agents = AgentsSnapshot::capture(&pool);
    agents.assert_no_agents();

    let agent = TestAgent::greeting(&pool, "friendly-bot", Duration::from_millis(10), &pool);

    let casual = submit_with_mode(
        &pool,
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
        &pool,
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

    // === Sync point 3: Tasks processed, submissions clean ===
    let submissions = SubmissionsSnapshot::capture(&pool);
    submissions.assert_empty();

    // === Sync point 4: Agent stopped ===
    eprintln!("[{pool}] TEST: assertions passed, stopping agent...");
    let _ = agent.stop();
    eprintln!("[{pool}] TEST: agent stopped, cleaning up...");

    cleanup_pool(&pool);
    eprintln!("[{pool}] TEST: cleanup complete, dropping pool...");

    // _pool_handle drops here - calls stop on the daemon
}
