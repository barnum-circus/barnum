#![allow(missing_docs)]

use gsd_task_queue::{process_queue, NoMoreTasks, ProcessQueueOptions, QueueItem};
use serde::Deserialize;
use std::process::Command;

#[derive(Deserialize)]
struct SimpleResponse {
    value: i32,
}

struct SimpleTask {
    json_to_echo: String,
}

struct SimpleContext {
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

    fn cleanup(
        _in_progress: Self::InProgress,
        result: Result<Self::Response, serde_json::Error>,
        ctx: &mut SimpleContext,
    ) -> Self::NextTasks {
        ctx.result = Some(result.map_err(|e| e.to_string()));
        NoMoreTasks
    }
}

#[tokio::test]
async fn valid_json_gives_ok_result() {
    let mut ctx = SimpleContext { result: None };

    let queue = vec![SimpleTask {
        json_to_echo: r#"{"value": 42}"#.to_string(),
    }];

    process_queue(queue, &mut ctx, ProcessQueueOptions { max_concurrency: 1 })
        .await
        .expect("process_queue failed");

    let result = ctx.result.expect("result should be set");
    let response = result.expect("should be Ok");
    assert_eq!(response.value, 42);
}

#[tokio::test]
async fn invalid_json_gives_err_result() {
    let mut ctx = SimpleContext { result: None };

    let queue = vec![SimpleTask {
        json_to_echo: "not json at all".to_string(),
    }];

    process_queue(queue, &mut ctx, ProcessQueueOptions { max_concurrency: 1 })
        .await
        .expect("process_queue failed");

    let result = ctx.result.expect("result should be set");
    assert!(result.is_err());
}
