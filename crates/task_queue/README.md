# Task Queue

A type-safe Rust library for defining task queues as state machines.

## Concepts

- **Queue Items** - Items that implement the `QueueItem` trait
- **Task Enum** - Wraps queue items, dispatches to underlying impl (can be derived with `#[derive(BarnumTask)]`)
- **Task Results** - Deserialized from script stdout

Tasks returned from `process` are added back to the queue. Only script execution is parallel; everything else is synchronous.

## Example Usage

```rust
use task_queue::{BarnumTask, QueueItem, NoMoreTasks, ProcessQueueOptions, process_queue};
use serde::Deserialize;
use std::process::Command;

#[derive(BarnumTask)]
enum Task {
    AnalyzeFile(AnalyzeFile),
}

struct AnalyzeFile {
    path: String,
}

struct AnalyzeFileInProgress {
    path: String,
}

struct Context {
    results: Vec<String>,
}

impl QueueItem<Context> for AnalyzeFile {
    type InProgress = AnalyzeFileInProgress;
    type Response = serde_json::Value;
    type NextTasks = NoMoreTasks;

    fn start(self, _ctx: &mut Context) -> (Self::InProgress, Command) {
        let mut cmd = Command::new("./analyze.sh");
        cmd.arg(&self.path);
        (AnalyzeFileInProgress { path: self.path }, cmd)
    }

    fn process(
        in_progress: Self::InProgress,
        result: Result<Self::Response, serde_json::Error>,
        ctx: &mut Context,
    ) -> Self::NextTasks {
        match result {
            Ok(response) => {
                ctx.results.push(format!("{}: {:?}", in_progress.path, response));
            }
            Err(e) => {
                ctx.results.push(format!("{}: error - {}", in_progress.path, e));
            }
        }
        NoMoreTasks
    }
}

#[tokio::main]
async fn main() {
    let mut ctx = Context { results: vec![] };

    let queue = vec![
        Task::AnalyzeFile(AnalyzeFile { path: "src/main.rs".into() }),
        Task::AnalyzeFile(AnalyzeFile { path: "src/lib.rs".into() }),
    ];

    process_queue(queue, &mut ctx, ProcessQueueOptions { max_concurrency: Some(4) })
        .await
        .expect("process_queue failed");

    for result in &ctx.results {
        println!("{}", result);
    }
}
```
