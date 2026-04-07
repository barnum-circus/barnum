import {
  type Action,
  type Pipeable,
  type Result,
  type TypedAction,
  typedAction,
  buildRestartBranchAction,
  TAG_BREAK,
  IDENTITY,
} from "./ast.js";
import {
  allocateRestartHandlerId,
  type RestartHandlerId,
} from "./effect-id.js";

// ---------------------------------------------------------------------------
// Shared AST fragments
// ---------------------------------------------------------------------------

const TAG_OK: Action = {
  kind: "Invoke",
  handler: { kind: "Builtin", builtin: { kind: "Tag", value: "Ok" } },
};

const TAG_ERR: Action = {
  kind: "Invoke",
  handler: { kind: "Builtin", builtin: { kind: "Tag", value: "Err" } },
};

/**
 * `Chain(Tag("Break"), RestartPerform(id))` — shared by race branches.
 * The winning branch tags its result as Break, then performs. The handler
 * restarts the body; Branch takes the Break arm (identity), `RestartHandle` exits.
 */
function breakPerform(restartHandlerId: RestartHandlerId): Action {
  return {
    kind: "Chain",
    first: TAG_BREAK,
    rest: { kind: "RestartPerform", restart_handler_id: restartHandlerId },
  };
}

// ---------------------------------------------------------------------------
// race — first branch to complete wins, losers cancelled
// ---------------------------------------------------------------------------

/**
 * Run multiple actions concurrently. The first to complete wins; losers
 * are cancelled during `RestartHandle` frame teardown.
 *
 * All branches must have the same input and output type (since either
 * could win).
 *
 * Compiled form (restart+Branch, same substrate as loop/earlyReturn):
 *   `Chain(Tag("Continue"),`
 *     `RestartHandle(id, ExtractIndex(0),`
 *       `Branch({`
 *         `Continue: All(Chain(a, breakPerform), Chain(b, breakPerform), ...),`
 *         `Break: identity,`
 *       `})))`
 *
 * First branch to complete tags Break → `RestartPerform` → handler restarts →
 * Branch takes Break arm → identity → `RestartHandle` exits with winner's value.
 */
export function race<TIn, TOut>(
  ...actions: Pipeable<TIn, TOut>[]
): TypedAction<TIn, TOut> {
  const restartHandlerId = allocateRestartHandlerId();
  const perform = breakPerform(restartHandlerId);

  const branches = actions.map((action) => ({
    kind: "Chain" as const,
    first: action as Action,
    rest: perform,
  }));

  const allAction: Action = { kind: "All", actions: branches };

  return typedAction(
    buildRestartBranchAction(restartHandlerId, allAction, IDENTITY),
  );
}

// ---------------------------------------------------------------------------
// sleep — Rust builtin that delays for a fixed duration (passthrough)
// ---------------------------------------------------------------------------

/**
 * Sleep for a fixed duration, ignoring input and returning void.
 *
 * `ms` is baked into the AST at construction time. Executed by the Rust
 * scheduler via `tokio::time::sleep` — no subprocess spawned.
 *
 * To preserve data across a sleep, use `bindInput`.
 */
export function sleep(ms: number): TypedAction<any, never> {
  return typedAction<any, never>({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Sleep", value: ms } },
  });
}

// ---------------------------------------------------------------------------
// dynamicSleep — TypeScript handler for withTimeout (takes ms as input)
// ---------------------------------------------------------------------------

/** The raw Invoke node for the dynamic sleep handler. */
const DYNAMIC_SLEEP_INVOKE: Action = {
  kind: "Invoke",
  handler: {
    kind: "TypeScript",
    module: import.meta.url,
    func: "dynamicSleep",
  },
};

/**
 * @internal TypeScript handler that takes ms as pipeline input and returns
 * void after the timer fires. Used by `withTimeout` where the duration
 * comes from a runtime pipeline, not a build-time constant.
 */
// eslint-disable-next-line @typescript-eslint/no-empty-function
export function dynamicSleep(): void {}
Object.defineProperty(dynamicSleep, "__definition", {
  value: {
    handle: ({ value }: { value: number }) =>
      new Promise<void>((resolve) => setTimeout(resolve, value)),
  },
  enumerable: false,
});

// ---------------------------------------------------------------------------
// withTimeout — race body against sleep, return Result
// ---------------------------------------------------------------------------

/**
 * Race the body against a sleep timer. Returns `Result<TOut, void>`:
 * - Ok(value) if the body completed first
 * - Err(void) if the timeout fired first
 *
 * The `ms` parameter is an AST node that evaluates to the timeout duration
 * in milliseconds. Use `constant(5000)` for a fixed timeout, or any action
 * that computes a duration from the pipeline input.
 *
 * Built as raw AST rather than through `race()` because each branch wraps
 * its result differently (Ok vs Err) before the Break+Perform. `race()`
 * requires homogeneous output types, but withTimeout needs heterogeneous
 * tagging.
 *
 * Same restart+Branch substrate as race: each branch tags Break after
 * wrapping its result as Ok or Err.
 */
export function withTimeout<TIn, TOut>(
  ms: Pipeable<TIn, number>,
  body: Pipeable<TIn, TOut>,
): TypedAction<TIn, Result<TOut, void>> {
  const restartHandlerId = allocateRestartHandlerId();
  const perform = breakPerform(restartHandlerId);

  // Branch 1: body → Tag("Ok") → Tag("Break") → RestartPerform
  const bodyBranch: Action = {
    kind: "Chain",
    first: { kind: "Chain", first: body as Action, rest: TAG_OK },
    rest: perform,
  };

  // Branch 2: ms → sleep() → Tag("Err") → Tag("Break") → RestartPerform
  const sleepBranch: Action = {
    kind: "Chain",
    first: {
      kind: "Chain",
      first: { kind: "Chain", first: ms as Action, rest: DYNAMIC_SLEEP_INVOKE },
      rest: TAG_ERR,
    },
    rest: perform,
  };

  const allAction: Action = { kind: "All", actions: [bodyBranch, sleepBranch] };

  return typedAction(
    buildRestartBranchAction(restartHandlerId, allAction, IDENTITY),
  );
}
