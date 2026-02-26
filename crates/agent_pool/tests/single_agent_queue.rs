//! Test corresponding to demos/single-agent-queue.sh
//! Single agent, multiple tasks (queuing behavior).

#![expect(clippy::print_stderr)]
#![expect(clippy::expect_used)]
#![expect(clippy::needless_collect)]
#![expect(clippy::panic)]

mod common;

use agent_pool::Response;
use common::{
    AgentPoolHandle, DataSource, NotifyMethod, TestAgent, cleanup_pool, generate_pool,
    is_ipc_available, submit_with_mode,
};
use rstest::rstest;
use std::thread;
use std::time::Duration;

const TEST_NAME: &str = "single_agent_queue";

#[rstest]
#[timeout(std::time::Duration::from_secs(20))]
#[case(DataSource::Inline, NotifyMethod::Socket)]
#[case(DataSource::Inline, NotifyMethod::File)]
#[case(DataSource::Inline, NotifyMethod::Raw)]
#[case(DataSource::FileReference, NotifyMethod::Socket)]
#[case(DataSource::FileReference, NotifyMethod::File)]
#[case(DataSource::FileReference, NotifyMethod::Raw)]
fn single_agent_queues_multiple_tasks(
    #[case] data_source: DataSource,
    #[case] notify_method: NotifyMethod,
) {
    let pool = generate_pool(&format!("{TEST_NAME}_{data_source:?}_{notify_method:?}"));

    if !is_ipc_available() {
        eprintln!("SKIP: IPC not available");
        cleanup_pool(&pool);
        return;
    }

    let _pool_handle = AgentPoolHandle::start(&pool, &pool);
    let mut agent = TestAgent::echo(&pool, "only-agent", Duration::from_millis(50), &pool);

    // Wait for agent to be ready (has processed initial heartbeat)
    agent.wait_ready();

    // Submit 4 tasks rapidly (they should queue since there's only one agent)
    let handles: Vec<_> = ["Task-A", "Task-B", "Task-C", "Task-D"]
        .iter()
        .map(|task| {
            let pool = pool.clone();
            let task_json =
                format!(r#"{{"kind":"Task","task":{{"instructions":"echo","data":"{task}"}}}}"#);
            thread::spawn(move || {
                submit_with_mode(&pool, &task_json, data_source, notify_method)
                    .expect("Submit failed")
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

    let _ = agent.stop();

    cleanup_pool(&pool);
}
