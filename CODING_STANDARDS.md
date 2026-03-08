# Coding Standards

## Code should be easy to review and reason about

- **Make impossible states unrepresentable.** Use enums with data, not structs with conditional fields. Go overboard with this - always prefer making impossible states unrepresentable over verbosity. There is no "reasonable" limit. Corollaries:
  - **Single source of truth** - don't pass around data that can be derived (e.g., don't pass step_name if it can be looked up from task_id).
  - **Values exist only where meaningful** - don't carry data alongside error paths where it's semantically invalid. If a value only makes sense when an operation succeeded, put it in the success variant, not in a shared struct.
  - **Unnecessary cloning signals missing single source of truth** - if you're cloning data to pass it somewhere, ask whether the recipient could look it up instead. The performance cost of cloning is minor; the real issue is that redundant data can diverge and makes code harder to reason about.
  - **No magic strings** - use enums instead of special sentinel values like `"__finally__"` or `"__internal__"`. Magic strings make impossible states representable (any string could be the magic value) and bypass the type system.

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

- **Flaky tests are a five-alarm fire.** A flaky test indicates bad modeling - either the test is testing non-deterministic behavior, or the system under test has race conditions. Either way, flaky tests erode trust and must be fixed immediately. Never increase timeouts to fix flakiness - that treats symptoms, not causes. If a test is flaky, either fix the underlying issue or delete the test.

- **Validate once, panic on invariant violations.** Validate external input (user input, files, network) at the boundary. After validation, internal code can panic if invariants are violated - this indicates a bug, not bad input.

- **Never silently skip or accommodate errors.** If something goes wrong, fail loudly. Don't write code that "gracefully handles" invalid states by skipping over them - this masks bugs and makes debugging harder. If you find yourself writing code to skip invalid data, ask whether the data should have been validated upstream.

## Low-level conventions

- **Newtypes for semantic clarity.** Wrap primitives and opaque types (strings, numbers, `serde_json::Value`) to prevent mixing up values. Cloning newtypes (especially string-based) is fine - they will eventually be interned. Corollaries:
  - **Wrap at production site** - create the newtype when you produce the value and know its semantic type, not when you consume it.
  - **Never convert between newtypes** - if you need to transform `TypeA` to `TypeB`, unwrap to the primitive, transform, then wrap. Direct newtype-to-newtype conversion hides what's actually happening.

- **Use `#[expect(...)]`, not `#[allow(...)]`.** Lint suppressions error when no longer needed.

- **Use `assert!`, not `debug_assert!`.** If an invariant is worth checking, check it always. Debug-only assertions mask bugs in release builds.

- **Unique error codes for all errors.** Every error message includes a unique code like `[E042]`. This makes errors searchable and unambiguous. Reserve ranges by crate if helpful.

- **Use `.expect()`, not `.unwrap()`.** Always include a message with a unique panic code like `[P001]`. Makes panics searchable and provides context. Example: `.expect("[P001] task_id must exist in map")`

- **Pass Copy types by value, not reference.**

- **Variable names default to snake_case of their type.** Prefer long descriptive names over short ones.

- **Serde tagging:** `#[serde(tag = "kind")]` for internally tagged enums.

- **Use `NonZeroU*`** when zero is invalid.

- **Use `thiserror`** for error enums.

- **Use `with_xyz` for scoped resources.** For setup/teardown patterns, accept `impl FnOnce()` and handle cleanup automatically.

- **TODOs in markdown, not code.** Track TODOs in `refactors/pending/todos.md`, not as `// TODO` comments in code. Code comments become stale; the markdown file is the single source of truth for work items.

- **Comment graceful early returns.** When code returns early or handles a case gracefully (rather than panicking), add a comment explaining why this is expected, not an error. Example: `let Some(x) = foo else { return }; // expected: root tasks have no parent`
