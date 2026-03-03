//! Tests for basic task queue functionality with JSON deserialization.

#![expect(clippy::expect_used)]

use rstest::rstest;
use serde::Deserialize;
use std::process::Command;
use std::time::Duration;
use task_queue::{NoMoreTasks, ProcessQueueOptions, QueueItem, process_queue};

/// Response deserialized from the echo command's JSON output.
#[derive(Deserialize)]
struct SimpleResponse {
    /// The integer value from the JSON.
    value: i32,
}

/// A task that echoes JSON and expects it to be deserialized.
struct SimpleTask {
    /// The JSON string to echo.
    json_to_echo: String,
}

/// Context that captures the task result.
struct SimpleContext {
    /// The result of processing, if any.
    result: Option<Result<SimpleResponse, String>>,
}

impl QueueItem<SimpleContext> for SimpleTask {
    type InProgress = ();
    type Response = SimpleResponse;
    type NextTasks = NoMoreTasks;

    fn start(self, _ctx: &mut SimpleContext) -> (Self::InProgress, Command) {
        let mut cmd = Command::new("echo");
        cmd.arg(&self.json_to_echo);
        ((), cmd)
    }

    fn process(
        _in_progress: Self::InProgress,
        result: Result<Self::Response, serde_json::Error>,
        ctx: &mut SimpleContext,
    ) -> Self::NextTasks {
        ctx.result = Some(result.map_err(|e| e.to_string()));
        NoMoreTasks
    }
}

#[rstest]
#[tokio::test]
#[timeout(Duration::from_secs(10))]
async fn valid_json_gives_ok_result() {
    let mut ctx = SimpleContext { result: None };

    let queue = vec![SimpleTask {
        json_to_echo: r#"{"value": 42}"#.to_string(),
    }];

    process_queue(
        queue,
        &mut ctx,
        ProcessQueueOptions {
            max_concurrency: Some(1),
        },
    )
    .await
    .expect("process_queue failed");

    let result = ctx.result.expect("result should be set");
    let response = result.expect("should be Ok");
    assert_eq!(response.value, 42);
}

#[rstest]
#[tokio::test]
#[timeout(Duration::from_secs(10))]
async fn invalid_json_gives_err_result() {
    let mut ctx = SimpleContext { result: None };

    let queue = vec![SimpleTask {
        json_to_echo: "not json at all".to_string(),
    }];

    process_queue(
        queue,
        &mut ctx,
        ProcessQueueOptions {
            max_concurrency: Some(1),
        },
    )
    .await
    .expect("process_queue failed");

    let result = ctx.result.expect("result should be set");
    assert!(result.is_err());
}
