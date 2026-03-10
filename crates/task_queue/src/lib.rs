//! A task queue that executes shell commands and deserializes their output.
//!
//! # Usage
//!
//! For simple cases, use [`process_queue`] which runs tasks to completion.
//! For more control, use [`TaskRunner`] which implements [`AsyncLendingIterator`]
//! to yield `&mut Ctx` after each task completion.

pub use task_queue_macro::BarnumTask;

use serde::de::DeserializeOwned;
use std::collections::VecDeque;
use std::future::Future;
use std::process::Command;
use tokio::process::Command as TokioCommand;

/// A queue item that can be processed by the task queue.
pub trait QueueItem<Context>: Sized {
    /// State held while the command is running.
    type InProgress;

    /// Deserialized from the command's stdout.
    type Response: DeserializeOwned;

    /// Follow-up tasks to enqueue after processing. Use `NoMoreTasks` for terminal items.
    type NextTasks;

    /// Set up and return a command to run.
    fn start(self, ctx: &mut Context) -> (Self::InProgress, Command);

    /// Process the result and return any follow-up tasks.
    fn process(
        in_progress: Self::InProgress,
        result: Result<Self::Response, serde_json::Error>,
        ctx: &mut Context,
    ) -> Self::NextTasks;
}

/// Marker type for tasks that never produce follow-up tasks.
pub struct NoMoreTasks;

impl<T> From<NoMoreTasks> for Vec<T> {
    fn from(_: NoMoreTasks) -> Self {
        vec![]
    }
}

/// Trait for converting process results into a Vec of tasks.
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
    /// When `None`, concurrency is unbounded.
    pub max_concurrency: Option<usize>,
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

/// An async iterator that yields references tied to `&mut self`.
///
/// Unlike `futures::Stream`, this trait supports lending references
/// where the yielded item's lifetime is tied to the `next()` call.
pub trait AsyncLendingIterator {
    /// The item type yielded by this iterator.
    /// The lifetime `'a` is tied to the `&'a mut self` borrow in `next()`.
    type Item<'a>
    where
        Self: 'a;

    /// Advance the iterator and return the next item.
    ///
    /// Returns `Ok(None)` when iteration is complete.
    fn next(&mut self) -> impl Future<Output = Result<Option<Self::Item<'_>>, ProcessQueueError>>;
}

/// Manages task queue execution as an async lending iterator.
///
/// Yields `&mut Ctx` after each task completion, allowing inspection
/// of the context state between tasks.
///
/// # Example
///
/// ```text
/// let mut runner = TaskRunner::new(tasks, &mut ctx, Some(4));
/// while let Some(ctx) = runner.next().await? {
///     println!("Tasks completed: {}", ctx.completed_count);
/// }
/// ```
pub struct TaskRunner<'ctx, T, InProgress, Ctx> {
    queue: VecDeque<T>,
    in_flight: Vec<InFlightTask<InProgress>>,
    ctx: &'ctx mut Ctx,
    max_concurrency: Option<usize>,
}

impl<'ctx, T, InProgress, Ctx> TaskRunner<'ctx, T, InProgress, Ctx> {
    /// Create a new task runner.
    #[must_use]
    pub fn new(
        initial: impl IntoIterator<Item = T>,
        ctx: &'ctx mut Ctx,
        max_concurrency: Option<usize>,
    ) -> Self {
        Self {
            queue: initial.into_iter().collect(),
            in_flight: Vec::new(),
            ctx,
            max_concurrency,
        }
    }

    /// Returns true if there are no pending or in-flight tasks.
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.queue.is_empty() && self.in_flight.is_empty()
    }

    /// Number of tasks waiting in the queue.
    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.queue.len()
    }

    /// Number of tasks currently running.
    #[must_use]
    pub const fn in_flight_count(&self) -> usize {
        self.in_flight.len()
    }
}

impl<T, InProgress, Ctx> AsyncLendingIterator for TaskRunner<'_, T, InProgress, Ctx>
where
    T: QueueItem<Ctx, InProgress = InProgress>,
    T::NextTasks: Into<Vec<T>>,
{
    type Item<'a>
        = &'a mut Ctx
    where
        Self: 'a;

    async fn next(&mut self) -> Result<Option<Self::Item<'_>>, ProcessQueueError> {
        if self.is_done() {
            return Ok(None);
        }

        // Spawn tasks up to the concurrency limit
        while self
            .max_concurrency
            .is_none_or(|max| self.in_flight.len() < max)
        {
            let Some(item) = self.queue.pop_front() else {
                break;
            };

            let (in_progress, cmd) = item.start(self.ctx);
            let handle = spawn_command(cmd);
            self.in_flight.push(InFlightTask {
                in_progress,
                handle,
            });
        }

        // Wait for one to complete
        let Some(task) = wait_for_any_completion(&mut self.in_flight).await else {
            return Ok(None);
        };

        let stdout = task
            .handle
            .await
            .map_err(|_| ProcessQueueError::TaskPanicked)??;

        let result = serde_json::from_str(&stdout);
        let new_tasks: Vec<T> = T::process(task.in_progress, result, self.ctx).into();

        // Add new tasks to the queue
        self.queue.extend(new_tasks);

        Ok(Some(self.ctx))
    }
}

/// Process a queue of tasks with bounded concurrency.
///
/// This is a convenience wrapper around [`TaskRunner`] that runs all tasks
/// to completion.
///
/// # Errors
///
/// Returns an error if:
/// - A spawned command fails with an I/O error
/// - A spawned task panics
pub async fn process_queue<T, Ctx>(
    initial_queue: Vec<T>,
    ctx: &mut Ctx,
    options: ProcessQueueOptions,
) -> Result<(), ProcessQueueError>
where
    T: QueueItem<Ctx>,
    T::NextTasks: Into<Vec<T>>,
{
    let mut runner = TaskRunner::new(initial_queue, ctx, options.max_concurrency);

    while runner.next().await?.is_some() {}

    Ok(())
}

struct InFlightTask<InProgress> {
    in_progress: InProgress,
    handle: tokio::task::JoinHandle<Result<String, std::io::Error>>,
}

#[expect(clippy::needless_pass_by_value)]
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

async fn wait_for_any_completion<T>(
    in_flight: &mut Vec<InFlightTask<T>>,
) -> Option<InFlightTask<T>> {
    if in_flight.is_empty() {
        return None;
    }

    loop {
        for i in 0..in_flight.len() {
            if in_flight[i].handle.is_finished() {
                return Some(in_flight.remove(i));
            }
        }
        tokio::task::yield_now().await;
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn wait_for_any_completion_returns_none_for_empty() {
        let mut in_flight: Vec<InFlightTask<()>> = vec![];
        let result = wait_for_any_completion(&mut in_flight).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn wait_for_any_completion_removes_and_returns_finished() {
        let handle = tokio::spawn(std::future::ready(Ok::<_, std::io::Error>(
            "done".to_string(),
        )));

        let mut in_flight = vec![InFlightTask {
            in_progress: 42,
            handle,
        }];

        let task = wait_for_any_completion(&mut in_flight).await;
        assert!(task.is_some());
        assert_eq!(task.unwrap().in_progress, 42);
        assert!(in_flight.is_empty());
    }
}
