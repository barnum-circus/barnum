# Coding Standards

## Code should be easy to review and reason about

- **Make impossible states unrepresentable.** Use enums with data, not structs with conditional fields. Go overboard with this - always prefer making impossible states unrepresentable over verbosity. There is no "reasonable" limit. Corollary: **single source of truth** - don't pass around data that can be derived (e.g., don't pass step_name if it can be looked up from task_id).

- **Functions stay at the same abstraction level.** The function the reviewer reads should not mix high and low-level details.

- **Pure core, impure shell.** Business logic in pure functions; I/O in thin wrappers.

- **Large data structures are a smell.** Prefer small structs; group related fields into sub-structs.

- **Extract testable inner functions.** Loop bodies become methods.

- **Inner functions accept narrow types.** Don't take `Option<T>` - accept `T` and unwrap outside.

- **Aggressively extract orthogonal functionality into crates.** Small focused crates are not a smell. Coding-style crates (e.g., newtype wrappers) are especially good extraction targets.

- **Files under ~400 lines.** Split by concern when files grow.

- **Minimal pub visibility.** Start private. Periodically audit that anything pub from a crate is actually used within the project. The project is one giant crate with no external consumers.

- **Incorporate matched information statically.** After matching on something, don't match on it again. Structure code so the matched variant's data is carried through, making re-matching unnecessary. If you find yourself checking `if let Some(x) = ...` after already matching that it's `Some`, restructure to pass `x` directly.

## Project-level decisions

- **No timeouts, no polling.** Use channels.

- **Test across crates using public APIs.** When testing functionality that spans crates, use the CLI or other public interfaces, not internal APIs.

- **Validate once, panic on invariant violations.** Validate external input (user input, files, network) at the boundary. After validation, internal code can panic if invariants are violated - this indicates a bug, not bad input.

## Low-level conventions

- **Newtypes for semantic clarity.** Wrap primitives to prevent mixing up values. Cloning newtypes (especially string-based) is fine - they will eventually be interned.

- **Use `#[expect(...)]`, not `#[allow(...)]`.** Lint suppressions error when no longer needed.

- **Pass Copy types by value, not reference.**

- **Variable names default to snake_case of their type.** Prefer long descriptive names over short ones.

- **Serde tagging:** `#[serde(tag = "kind")]` for internally tagged enums.

- **Use `NonZeroU*`** when zero is invalid.

- **Use `thiserror`** for error enums.

- **Use `with_xyz` for scoped resources.** For setup/teardown patterns, accept `impl FnOnce()` and handle cleanup automatically.
