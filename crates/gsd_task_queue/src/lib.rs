//! A task queue that executes shell commands and deserializes their output.

pub use gsd_macro::GsdTask;

use serde::de::DeserializeOwned;
use std::collections::VecDeque;
use std::process::Command;
use tokio::process::Command as TokioCommand;

/// A queue item that can be processed by the task queue.
pub trait QueueItem<Context>: Sized {
    /// State held while the command is running.
    type InProgress;

    /// Deserialized from the command's stdout.
    type Response: DeserializeOwned;

    /// Follow-up tasks to enqueue after cleanup. Use `NoMoreTasks` for terminal items.
    type NextTasks;

    /// Set up and return a command to run.
    fn start(self, ctx: &mut Context) -> (Self::InProgress, Command);

    /// Handle the result and return any follow-up tasks.
    fn cleanup(
        in_progress: Self::InProgress,
        result: Result<Self::Response, serde_json::Error>,
        ctx: &mut Context,
    ) -> Self::NextTasks;
}

/// Marker type for tasks that never produce follow-up tasks.
pub struct NoMoreTasks;

impl<T> From<NoMoreTasks> for Vec<T> {
    fn from(_: NoMoreTasks) -> Vec<T> {
        vec![]
    }
}

/// Trait for converting cleanup results into a Vec of tasks.
pub trait IntoTasks<T> {
    /// Convert this value into a vector of tasks.
    fn into_tasks(self) -> Vec<T>;
}

impl<T> IntoTasks<T> for NoMoreTasks {
    fn into_tasks(self) -> Vec<T> {
        vec![]
    }
}

impl<T, U: Into<T>> IntoTasks<T> for Vec<U> {
    fn into_tasks(self) -> Vec<T> {
        self.into_iter().map(Into::into).collect()
    }
}

/// Options for processing the queue.
pub struct ProcessQueueOptions {
    /// Maximum number of tasks to run concurrently.
    pub max_concurrency: usize,
}

/// Errors that can occur while processing the queue.
#[derive(Debug)]
pub enum ProcessQueueError {
    /// An I/O error occurred while spawning or reading from a command.
    Io(std::io::Error),
    /// A spawned task panicked before completing.
    TaskPanicked,
}

impl From<std::io::Error> for ProcessQueueError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl std::fmt::Display for ProcessQueueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "IO error: {e}"),
            Self::TaskPanicked => write!(f, "spawned task panicked"),
        }
    }
}

impl std::error::Error for ProcessQueueError {}

/// Process a queue of tasks with bounded concurrency.
pub async fn process_queue<T, Ctx>(
    initial_queue: Vec<T>,
    ctx: &mut Ctx,
    options: ProcessQueueOptions,
) -> Result<(), ProcessQueueError>
where
    T: QueueItem<Ctx>,
    T::NextTasks: Into<Vec<T>>,
{
    let mut queue: VecDeque<T> = initial_queue.into();
    let mut in_flight: Vec<InFlightTask<T::InProgress>> = Vec::new();

    while !queue.is_empty() || !in_flight.is_empty() {
        while in_flight.len() < options.max_concurrency {
            let Some(item) = queue.pop_front() else {
                break;
            };

            let (in_progress, cmd) = item.start(ctx);
            let handle = spawn_command(cmd);
            in_flight.push(InFlightTask {
                in_progress,
                handle,
            });
        }

        if let Some(idx) = wait_for_any_completion(&in_flight).await {
            let task = in_flight.remove(idx);
            let stdout = task
                .handle
                .await
                .map_err(|_| ProcessQueueError::TaskPanicked)??;

            let result = serde_json::from_str(&stdout);
            let new_tasks: Vec<T> = T::cleanup(task.in_progress, result, ctx).into();

            for new_task in new_tasks {
                queue.push_back(new_task);
            }
        }
    }

    Ok(())
}

struct InFlightTask<InProgress> {
    in_progress: InProgress,
    handle: tokio::task::JoinHandle<Result<String, std::io::Error>>,
}

fn spawn_command(cmd: Command) -> tokio::task::JoinHandle<Result<String, std::io::Error>> {
    let program = cmd.get_program().to_owned();
    let args: Vec<_> = cmd.get_args().map(ToOwned::to_owned).collect();

    tokio::spawn(async move {
        let output = TokioCommand::new(&program)
            .args(&args)
            .stdout(std::process::Stdio::piped())
            .output()
            .await?;
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    })
}

/// Polls in-flight tasks until one completes.
async fn wait_for_any_completion<T>(in_flight: &[InFlightTask<T>]) -> Option<usize> {
    if in_flight.is_empty() {
        return None;
    }

    loop {
        for (i, task) in in_flight.iter().enumerate() {
            if task.handle.is_finished() {
                return Some(i);
            }
        }
        tokio::task::yield_now().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn wait_for_any_completion_returns_none_for_empty() {
        let in_flight: Vec<InFlightTask<()>> = vec![];
        let result = wait_for_any_completion(&in_flight).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn wait_for_any_completion_returns_index_of_finished() {
        let handle = tokio::spawn(std::future::ready(Ok::<_, std::io::Error>(
            "done".to_string(),
        )));

        let in_flight = vec![InFlightTask {
            in_progress: (),
            handle,
        }];

        let result = wait_for_any_completion(&in_flight).await;
        assert_eq!(result, Some(0));
    }
}
