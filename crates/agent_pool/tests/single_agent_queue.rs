//! Test corresponding to demos/single-agent-queue.sh
//! Single agent, multiple tasks (queuing behavior).

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]
#![expect(clippy::needless_collect)]
#![expect(clippy::panic)]

mod common;

use agent_pool::{Payload, Response};
use common::{AgentPoolHandle, TestAgent, cleanup_test_dir, is_ipc_available, setup_test_dir};
use std::thread;
use std::time::Duration;

const TEST_DIR: &str = "single_agent_queue";

#[test]
fn single_agent_queues_multiple_tasks() {
    let root = setup_test_dir(TEST_DIR);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(TEST_DIR);
        return;
    }

    let _pool = AgentPoolHandle::start(&root);
    let mut agent = TestAgent::echo(&root, "only-agent", Duration::from_millis(50));

    // Wait for agent to be ready (has processed initial heartbeat)
    agent.wait_ready();

    // Submit 4 tasks rapidly (they should queue since there's only one agent)
    let handles: Vec<_> = ["Task-A", "Task-B", "Task-C", "Task-D"]
        .iter()
        .map(|task| {
            let root = root.clone();
            let task_json = format!(
                r#"{{"kind":"Task","task":{{"instructions":"echo","data":"{task}"}}}}"#
            );
            thread::spawn(move || {
                agent_pool::submit(&root, &Payload::inline(task_json)).expect("Submit failed")
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

    // Just verify we processed tasks
    let _ = agent.stop();

    cleanup_test_dir(TEST_DIR);
}

// Note: sequential_tasks_same_agent test removed - it was testing internal
// implementation details (direct file writes) that are no longer relevant with
// CLI-based agents. The proper way to test sequential task processing is through
// the daemon using submit().
