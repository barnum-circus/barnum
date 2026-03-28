//! Barnum workflow runtime: Scheduler + `run_workflow` loop.
//!
//! The [`Scheduler`] dispatches handler invocations as tokio tasks and collects
//! results via an internal channel. [`run_workflow`] drives the
//! [`WorkflowState`] by repeatedly dispatching pending work and feeding
//! completions back until the workflow terminates.

use barnum_ast::HandlerKind;
use barnum_engine::{CompleteError, Dispatch, TaskId, WorkflowState};
use serde_json::Value;
use tokio::sync::mpsc;

// =============================================================================
// Scheduler
// =============================================================================

/// Dispatches handler invocations as tokio tasks and collects results.
///
/// Each [`dispatch`](Scheduler::dispatch) call spawns a lightweight tokio task.
/// Results are collected via [`recv`](Scheduler::recv).
pub struct Scheduler {
    result_tx: mpsc::UnboundedSender<(TaskId, Value)>,
    result_rx: mpsc::UnboundedReceiver<(TaskId, Value)>,
}

impl Scheduler {
    /// Create a new scheduler.
    #[must_use]
    pub fn new() -> Self {
        let (result_tx, result_rx) = mpsc::unbounded_channel();
        Self {
            result_tx,
            result_rx,
        }
    }

    /// Dispatch a handler invocation.
    ///
    /// Spawns a tokio task that executes the handler and sends the result
    /// through the internal channel. Currently all handlers are no-ops that
    /// return an empty JSON object.
    pub fn dispatch(&self, dispatch: &Dispatch, _handler: &HandlerKind) {
        let result_tx = self.result_tx.clone();
        let task_id = dispatch.task_id;
        tokio::spawn(async move {
            let value = Value::Object(serde_json::Map::default());
            let _ = result_tx.send((task_id, value));
        });
    }

    /// Wait for the next handler result.
    ///
    /// Returns `None` if all senders have been dropped (shouldn't happen
    /// during normal operation since `self` holds a sender).
    pub async fn recv(&mut self) -> Option<(TaskId, Value)> {
        self.result_rx.recv().await
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// run_workflow
// =============================================================================

/// Run a workflow to completion.
///
/// Performs the initial advance, then loops: dispatch pending work to the
/// scheduler, receive one result, feed it back to the workflow state. Repeats
/// until the workflow terminates.
///
/// # Errors
///
/// Returns [`CompleteError`] if a completion causes an engine error (e.g.,
/// invalid loop result, advance failure during Chain trampoline).
///
/// # Panics
///
/// Panics if the initial advance fails or the scheduler channel closes
/// unexpectedly.
#[allow(clippy::missing_panics_doc, clippy::expect_used)]
pub async fn run_workflow(
    workflow_state: &mut WorkflowState,
    scheduler: &mut Scheduler,
) -> Result<Value, CompleteError> {
    let root = workflow_state.workflow_root();
    workflow_state
        .advance(root, Value::Null, None)
        .expect("initial advance failed");

    loop {
        let dispatches = workflow_state.take_pending_dispatches();
        for dispatch in &dispatches {
            let handler = workflow_state.handler(dispatch.handler_id);
            scheduler.dispatch(dispatch, handler);
        }

        let (task_id, value) = scheduler
            .recv()
            .await
            .expect("scheduler channel closed unexpectedly");

        if let Some(terminal_value) = workflow_state.complete(task_id, value)? {
            return Ok(terminal_value);
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use barnum_ast::flat::flatten;
    use barnum_ast::{Action, Config, TypeScriptHandler};
    use intern::string_key::Intern as _;
    use std::collections::HashMap;

    fn ts_handler(module: &str, func: &str) -> Action {
        Action::Invoke(barnum_ast::InvokeAction {
            handler: HandlerKind::TypeScript(TypeScriptHandler {
                module: module.intern().into(),
                func: func.intern().into(),
                step_config_schema: None,
                value_schema: None,
            }),
        })
    }

    fn config(workflow: Action) -> Config {
        Config {
            workflow,
            steps: HashMap::default(),
        }
    }

    #[tokio::test]
    async fn chain_of_two_invokes() {
        let flat_config = flatten(config(Action::Chain(barnum_ast::ChainAction {
            first: Box::new(ts_handler("./a.ts", "a")),
            rest: Box::new(ts_handler("./b.ts", "b")),
        })))
        .unwrap();
        let mut workflow_state = WorkflowState::new(flat_config);
        let mut scheduler = Scheduler::new();

        let result = run_workflow(&mut workflow_state, &mut scheduler)
            .await
            .unwrap();

        // Both handlers return {}, so the final result is {}
        assert_eq!(result, serde_json::json!({}));
    }

    #[tokio::test]
    async fn parallel_two_invokes() {
        let flat_config = flatten(config(Action::Parallel(barnum_ast::ParallelAction {
            actions: vec![ts_handler("./a.ts", "a"), ts_handler("./b.ts", "b")],
        })))
        .unwrap();
        let mut workflow_state = WorkflowState::new(flat_config);
        let mut scheduler = Scheduler::new();

        let result = run_workflow(&mut workflow_state, &mut scheduler)
            .await
            .unwrap();

        // Parallel collects results into an array
        assert_eq!(result, serde_json::json!([{}, {}]));
    }

    #[tokio::test]
    async fn single_invoke() {
        let flat_config = flatten(config(ts_handler("./a.ts", "a"))).unwrap();
        let mut workflow_state = WorkflowState::new(flat_config);
        let mut scheduler = Scheduler::new();

        let result = run_workflow(&mut workflow_state, &mut scheduler)
            .await
            .unwrap();

        assert_eq!(result, serde_json::json!({}));
    }
}
