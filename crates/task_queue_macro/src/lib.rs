//! Derive macros for Barnum task queue.
//!
//! This crate provides the `BarnumTask` derive macro for automatically implementing
//! the `QueueItem` trait on enums where each variant wraps a type that already
//! implements `QueueItem`.

mod barnum_task_macro;

use proc_macro::TokenStream;

/// Derive the `QueueItem` trait for an enum of task types.
///
/// Each variant must wrap a type that implements `QueueItem<Context>` for
/// the same context type. The macro generates the dispatch logic to delegate
/// to the inner type's implementation.
#[proc_macro_derive(BarnumTask, attributes(barnum_task))]
pub fn barnum_task_derive(input: TokenStream) -> TokenStream {
    barnum_task_macro::barnum_task_macro(input)
}
