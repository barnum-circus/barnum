//! Test corresponding to demos/many-agents.sh
//! Multiple agents processing tasks in parallel.

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]
#![expect(clippy::needless_collect)]

mod common;

use agent_pool::{AGENTS_DIR, INPUT_EXT, ResponseKind};
use common::{AgentPoolHandle, TestAgent, cleanup_test_dir, is_ipc_available, setup_test_dir};
use std::fs;
use std::thread;
use std::time::Duration;

const TEST_DIR: &str = "many_agents";

#[test]
fn multiple_agents_parallel_tasks() {
    let root = setup_test_dir(TEST_DIR);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(TEST_DIR);
        return;
    }

    let _pool = AgentPoolHandle::start(&root);

    // 3 agents with varying response times
    let agent1 = TestAgent::echo(&root, "fast-agent", Duration::from_millis(10));
    let agent2 = TestAgent::echo(&root, "medium-agent", Duration::from_millis(30));
    let agent3 = TestAgent::echo(&root, "slow-agent", Duration::from_millis(50));

    thread::sleep(Duration::from_millis(200));

    // Submit 6 tasks rapidly - they'll be distributed across agents
    let handles: Vec<_> = (1..=6)
        .map(|i| {
            let root = root.clone();
            let task = format!("Task-{i}");
            thread::spawn(move || agent_pool::submit(&root, &task).expect("Submit failed"))
        })
        .collect();

    let results: Vec<_> = handles
        .into_iter()
        .map(|h| h.join().expect("Thread panicked"))
        .collect();

    for result in &results {
        assert_eq!(result.kind, ResponseKind::Processed);
        assert!(
            result
                .stdout
                .as_ref()
                .is_some_and(|s| s.contains("[processed]"))
        );
    }

    let processed1 = agent1.stop();
    let processed2 = agent2.stop();
    let processed3 = agent3.stop();

    let total = processed1.len() + processed2.len() + processed3.len();
    assert_eq!(total, 6);

    cleanup_test_dir(TEST_DIR);
}

#[test]
fn multiple_agents_direct_dispatch() {
    let root = setup_test_dir(&format!("{TEST_DIR}_direct"));

    let agent1 = TestAgent::echo(&root, "agent-a", Duration::from_millis(10));
    let agent2 = TestAgent::echo(&root, "agent-b", Duration::from_millis(10));
    let agent3 = TestAgent::echo(&root, "agent-c", Duration::from_millis(10));

    thread::sleep(Duration::from_millis(50));

    // Write tasks directly to each agent via file protocol (id.input)
    fs::write(
        root.join(AGENTS_DIR)
            .join("agent-a")
            .join(format!("1.{INPUT_EXT}")),
        "Task A",
    )
    .expect("Failed to write task A");
    fs::write(
        root.join(AGENTS_DIR)
            .join("agent-b")
            .join(format!("1.{INPUT_EXT}")),
        "Task B",
    )
    .expect("Failed to write task B");
    fs::write(
        root.join(AGENTS_DIR)
            .join("agent-c")
            .join(format!("1.{INPUT_EXT}")),
        "Task C",
    )
    .expect("Failed to write task C");

    thread::sleep(Duration::from_millis(100));

    assert_eq!(agent1.stop(), vec!["Task A"]);
    assert_eq!(agent2.stop(), vec!["Task B"]);
    assert_eq!(agent3.stop(), vec!["Task C"]);

    cleanup_test_dir(&format!("{TEST_DIR}_direct"));
}
