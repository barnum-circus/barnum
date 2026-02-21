//! Tests for fan-out pattern: one task spawning multiple follow-up tasks.
//!
//! This test manually implements the Task enum dispatch logic to verify
//! the pattern works correctly before using the derive macro.

use task_queue::{process_queue, IntoTasks, NoMoreTasks, ProcessQueueOptions, QueueItem};
use std::collections::HashSet;
use std::process::Command;

/// Test context tracking task lifecycle events.
#[derive(Default)]
struct Context {
    /// Values collected from completed Print tasks.
    collected: HashSet<String>,
    /// Number of Split tasks started.
    splits_started: usize,
    /// Number of Split tasks cleaned up.
    splits_cleaned_up: usize,
    /// Number of Print tasks created by Split cleanup.
    prints_created: usize,
    /// Number of Print tasks started.
    prints_started: usize,
    /// Number of Print tasks cleaned up.
    prints_cleaned_up: usize,
}

/// The top-level task enum wrapping all task types.
enum Task {
    /// A task that splits CSV into individual values.
    Split(Split),
    /// A task that collects a single value.
    Print(Print),
}

/// Task that splits a CSV string into individual Print tasks.
struct Split {
    /// Comma-separated values to split.
    csv: String,
}

/// Task that collects a single value into the context.
struct Print {
    /// The value to collect.
    value: String,
}

/// In-progress state for Split task.
struct SplitInProgress {
    /// The CSV string being processed.
    csv: String,
}

/// In-progress state for Print task.
struct PrintInProgress {
    /// The value being processed.
    value: String,
}

impl From<Split> for Task {
    fn from(s: Split) -> Self {
        Task::Split(s)
    }
}

impl From<Print> for Task {
    fn from(p: Print) -> Self {
        Task::Print(p)
    }
}

/// In-progress state for the Task enum.
enum TaskInProgress {
    /// In-progress Split task.
    Split(SplitInProgress),
    /// In-progress Print task.
    Print(PrintInProgress),
}

impl QueueItem<Context> for Task {
    type InProgress = TaskInProgress;
    type Response = serde_json::Value;
    type NextTasks = Vec<Task>;

    fn start(self, ctx: &mut Context) -> (Self::InProgress, Command) {
        match self {
            Task::Split(s) => {
                let (ip, cmd) = s.start(ctx);
                (TaskInProgress::Split(ip), cmd)
            }
            Task::Print(p) => {
                let (ip, cmd) = p.start(ctx);
                (TaskInProgress::Print(ip), cmd)
            }
        }
    }

    fn cleanup(
        in_progress: Self::InProgress,
        result: Result<Self::Response, serde_json::Error>,
        ctx: &mut Context,
    ) -> Self::NextTasks {
        match in_progress {
            TaskInProgress::Split(ip) => Split::cleanup(ip, result, ctx).into_tasks(),
            TaskInProgress::Print(ip) => Print::cleanup(ip, result, ctx).into_tasks(),
        }
    }
}

impl QueueItem<Context> for Split {
    type InProgress = SplitInProgress;
    type Response = serde_json::Value;
    type NextTasks = Vec<Print>;

    fn start(self, ctx: &mut Context) -> (Self::InProgress, Command) {
        ctx.splits_started += 1;
        let mut cmd = Command::new("echo");
        cmd.arg("{}");
        (SplitInProgress { csv: self.csv }, cmd)
    }

    fn cleanup(
        in_progress: Self::InProgress,
        _result: Result<Self::Response, serde_json::Error>,
        ctx: &mut Context,
    ) -> Self::NextTasks {
        ctx.splits_cleaned_up += 1;
        in_progress
            .csv
            .split(',')
            .map(|s| {
                ctx.prints_created += 1;
                Print {
                    value: s.to_string(),
                }
            })
            .collect()
    }
}

impl QueueItem<Context> for Print {
    type InProgress = PrintInProgress;
    type Response = serde_json::Value;
    type NextTasks = NoMoreTasks;

    fn start(self, ctx: &mut Context) -> (Self::InProgress, Command) {
        ctx.prints_started += 1;
        let mut cmd = Command::new("echo");
        cmd.arg("{}");
        (PrintInProgress { value: self.value }, cmd)
    }

    fn cleanup(
        in_progress: Self::InProgress,
        _result: Result<Self::Response, serde_json::Error>,
        ctx: &mut Context,
    ) -> Self::NextTasks {
        ctx.prints_cleaned_up += 1;
        ctx.collected.insert(in_progress.value);
        NoMoreTasks
    }
}

#[tokio::test]
async fn split_then_print_collects_all_values() {
    let mut ctx = Context::default();

    let queue = vec![
        Task::Split(Split {
            csv: "a,b,c".to_string(),
        }),
        Task::Split(Split {
            csv: "d,e,f".to_string(),
        }),
    ];

    process_queue(queue, &mut ctx, ProcessQueueOptions { max_concurrency: 4 })
        .await
        .expect("process_queue failed");

    let expected: HashSet<String> = ["a", "b", "c", "d", "e", "f"]
        .iter()
        .map(|s| (*s).to_string())
        .collect();

    assert_eq!(ctx.collected, expected);
    assert_eq!(ctx.splits_started, 2);
    assert_eq!(ctx.splits_cleaned_up, 2);
    assert_eq!(ctx.prints_created, 6);
    assert_eq!(ctx.prints_started, 6);
    assert_eq!(ctx.prints_cleaned_up, 6);
}
