#![allow(missing_docs)]

use gsd_task_queue::{process_queue, NoMoreTasks, ProcessQueueOptions, QueueItem};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct ConcurrencyTask {
    current_count: Arc<AtomicUsize>,
    max_observed: Arc<AtomicUsize>,
}

struct ConcurrencyInProgress {
    current_count: Arc<AtomicUsize>,
}

impl QueueItem<()> for ConcurrencyTask {
    type InProgress = ConcurrencyInProgress;
    type Response = serde_json::Value;
    type NextTasks = NoMoreTasks;

    fn start(self, _ctx: &mut ()) -> (Self::InProgress, Command) {
        let prev = self.current_count.fetch_add(1, Ordering::SeqCst);
        self.max_observed.fetch_max(prev + 1, Ordering::SeqCst);

        let mut cmd = Command::new("sh");
        cmd.args(["-c", r#"sleep 0.05 && echo '{}'"#]);

        (
            ConcurrencyInProgress {
                current_count: self.current_count,
            },
            cmd,
        )
    }

    fn cleanup(
        in_progress: Self::InProgress,
        _result: Result<Self::Response, serde_json::Error>,
        _ctx: &mut (),
    ) -> Self::NextTasks {
        in_progress.current_count.fetch_sub(1, Ordering::SeqCst);
        NoMoreTasks
    }
}

// If this test becomes flaky in CI, increase the sleep duration or lower the
// "at least 2" assertion. Some timing dependency is unavoidable when testing
// real concurrent command execution without mocking.
#[tokio::test]
async fn respects_max_concurrency() {
    let current_count = Arc::new(AtomicUsize::new(0));
    let max_observed = Arc::new(AtomicUsize::new(0));

    let queue: Vec<ConcurrencyTask> = (0..10)
        .map(|_| ConcurrencyTask {
            current_count: Arc::clone(&current_count),
            max_observed: Arc::clone(&max_observed),
        })
        .collect();

    process_queue(queue, &mut (), ProcessQueueOptions { max_concurrency: 3 })
        .await
        .expect("process_queue failed");

    let max = max_observed.load(Ordering::SeqCst);
    assert!(max <= 3, "max concurrent was {max}, expected at most 3");
    assert!(
        max >= 2,
        "max concurrent was {max}, expected at least 2 (test may be flaky if system is slow)"
    );
}
