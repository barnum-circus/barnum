//! Test corresponding to demos/many-agents.sh
//! Multiple agents processing tasks in parallel.

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]
#![expect(clippy::needless_collect)]
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

const TEST_DIR: &str = "many_agents";

#[rstest]
#[case(SubmitMode::DataSocket)]
#[case(SubmitMode::DataFile)]
#[case(SubmitMode::FileSocket)]
#[case(SubmitMode::FileFile)]
fn multiple_agents_parallel_tasks(#[case] mode: SubmitMode) {
    let test_dir = format!("{TEST_DIR}_{mode:?}");
    let root = setup_test_dir(&test_dir);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&test_dir);
        return;
    }

    let _pool = AgentPoolHandle::start(&root);

    // 3 agents with varying response times
    let mut agent1 = TestAgent::echo(&root, "fast-agent", Duration::from_millis(10));
    let mut agent2 = TestAgent::echo(&root, "medium-agent", Duration::from_millis(30));
    let mut agent3 = TestAgent::echo(&root, "slow-agent", Duration::from_millis(50));

    // Wait for all agents to be ready (have processed initial heartbeats)
    wait_all_ready(&mut [&mut agent1, &mut agent2, &mut agent3]);

    // Submit 6 tasks rapidly - they'll be distributed across agents
    let handles: Vec<_> = (1..=6)
        .map(|i| {
            let root = root.clone();
            let task =
                format!(r#"{{"kind":"Task","task":{{"instructions":"echo","data":"Task-{i}"}}}}"#);
            thread::spawn(move || submit_with_mode(&root, &task, mode).expect("Submit failed"))
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

    let _ = agent1.stop();
    let _ = agent2.stop();
    let _ = agent3.stop();

    cleanup_test_dir(&test_dir);
}
