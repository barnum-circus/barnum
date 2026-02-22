//! Test corresponding to demos/single-agent-queue.sh
//! Single agent, multiple tasks (queuing behavior).

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]
#![expect(clippy::needless_collect)]

mod common;

use agent_pool::{AGENTS_DIR, NEXT_TASK_FILE, OUTPUT_FILE, ResponseKind};
use common::{AgentPoolHandle, TestAgent, cleanup_test_dir, is_ipc_available, setup_test_dir};
use std::fs;
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
    let agent = TestAgent::echo(&root, "only-agent", Duration::from_millis(50));

    // Give agent time to register
    thread::sleep(Duration::from_millis(200));

    // Submit 4 tasks rapidly (they should queue since there's only one agent)
    let handles: Vec<_> = ["Task-A", "Task-B", "Task-C", "Task-D"]
        .iter()
        .map(|task| {
            let root = root.clone();
            let task = (*task).to_string();
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

    let processed = agent.stop();
    assert_eq!(processed.len(), 4);

    cleanup_test_dir(TEST_DIR);
}

#[test]
fn sequential_tasks_same_agent() {
    let root = setup_test_dir(&format!("{TEST_DIR}_sequential"));

    let agent = TestAgent::echo(&root, "seq-agent", Duration::from_millis(10));

    let agent_dir = root.join(AGENTS_DIR).join("seq-agent");
    let task_file = agent_dir.join(NEXT_TASK_FILE);
    let output_file = agent_dir.join(OUTPUT_FILE);

    // Process three tasks sequentially via file protocol
    for i in 1..=3 {
        let task = format!("Task-{i}");
        fs::write(&task_file, &task).expect("Failed to write task");

        thread::sleep(Duration::from_millis(100));

        let output = fs::read_to_string(&output_file).expect("Failed to read output");
        assert_eq!(output, format!("{task} [processed]"));

        let _ = fs::remove_file(&output_file);
    }

    let processed = agent.stop();
    assert_eq!(processed, vec!["Task-1", "Task-2", "Task-3"]);

    cleanup_test_dir(&format!("{TEST_DIR}_sequential"));
}
