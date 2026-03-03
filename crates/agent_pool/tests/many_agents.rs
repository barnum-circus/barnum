//! Test corresponding to demos/many-agents.sh
//! Multiple agents processing tasks in parallel.

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]
#![expect(clippy::needless_collect)]
#![expect(clippy::panic)]

mod common;

use agent_pool::Response;
use common::{
    AgentPoolHandle, AgentsSnapshot, DataSource, NotifyMethod, SubmissionsSnapshot, TestAgent,
    cleanup_pool, generate_pool, is_ipc_available, mode_abbrev, pool_path, submit_with_mode,
};
use rstest::rstest;
use std::thread;
use std::time::Duration;

const TEST_NAME: &str = "many_agents";

#[rstest]
#[timeout(std::time::Duration::from_secs(20))]
#[case(DataSource::Inline, NotifyMethod::Socket)]
#[case(DataSource::Inline, NotifyMethod::File)]
#[case(DataSource::FileReference, NotifyMethod::Socket)]
#[case(DataSource::FileReference, NotifyMethod::File)]
fn multiple_agents_parallel_tasks(
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
    let _pool_handle = AgentPoolHandle::start(&pool, &pool);
    let agents = AgentsSnapshot::capture(&pool);
    agents.assert_no_agents();

    // 3 agents with varying response times
    let agent1 = TestAgent::echo(&pool, "fast-agent", Duration::from_millis(10), &pool);
    let agent2 = TestAgent::echo(&pool, "medium-agent", Duration::from_millis(30), &pool);
    let agent3 = TestAgent::echo(&pool, "slow-agent", Duration::from_millis(50), &pool);

    // Submit 6 tasks rapidly - they'll be distributed across agents
    let handles: Vec<_> = (1..=6)
        .map(|i| {
            let pool = pool.clone();
            let task =
                format!(r#"{{"kind":"Task","task":{{"instructions":"echo","data":"Task-{i}"}}}}"#);
            thread::spawn(move || {
                submit_with_mode(&pool, &task, data_source, notify_method).expect("Submit failed")
            })
        })
        .collect();

    let results: Vec<_> = handles
        .into_iter()
        .map(|h| h.join().expect("Thread panicked"))
        .collect();

    for result in &results {
        let Response::Processed { stdout, .. } = result else {
            panic!("Expected Processed response, got {result:?}");
        };
        assert!(stdout.contains("[processed]"));
    }

    // === Sync point 3: All tasks processed, submissions clean ===
    let submissions = SubmissionsSnapshot::capture(&pool);
    submissions.assert_empty();

    // === Sync point 4: All agents stopped ===
    let _ = agent1.stop();
    let _ = agent2.stop();
    let _ = agent3.stop();

    cleanup_pool(&pool);
}
