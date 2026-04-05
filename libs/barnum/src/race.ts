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
// sleep — TypeScript handler that resolves after N milliseconds
// ---------------------------------------------------------------------------

/** The raw Invoke node for the sleep handler. */
const SLEEP_INVOKE: Action = {
  kind: "Invoke",
  handler: {
    kind: "TypeScript",
    module: import.meta.url,
    func: "sleep",
  },
};

/**
 * Delay for a specified duration. Takes the number of milliseconds as
 * pipeline input and returns `void` after the timer fires.
 *
 * `number → void`
 *
 * When the engine cancels the sleep during race teardown, the worker
 * subprocess is killed. The sleep never resolves. Standard cancellation.
 *
 * This is defined inline rather than via `createHandler` to avoid
 * a circular dependency (handler.ts → ast.ts → builtins.ts → handler.ts).
 * The handler definition is attached for the worker to find at runtime.
 */
export function sleep(): TypedAction<number, void> {
  return typedAction<number, void>(SLEEP_INVOKE);
}

// Attach __definition on the sleep function for the worker to find at runtime.
// The handler receives the ms value as input and returns a Promise that
// resolves after that duration.
Object.defineProperty(sleep, "__definition", {
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
      first: { kind: "Chain", first: ms as Action, rest: SLEEP_INVOKE },
      rest: TAG_ERR,
    },
    rest: perform,
  };

  const allAction: Action = { kind: "All", actions: [bodyBranch, sleepBranch] };

  return typedAction(
    buildRestartBranchAction(restartHandlerId, allAction, IDENTITY),
  );
}
