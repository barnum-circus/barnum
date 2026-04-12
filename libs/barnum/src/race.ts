import {
  type Action,
  type Pipeable,
  type Result,
  type TypedAction,
  typedAction,
  buildRestartBranchAction,
} from "./ast.js";
import { chain } from "./chain.js";
import { identity, tag } from "./builtins.js";
import {
  allocateRestartHandlerId,
  type RestartHandlerId,
} from "./effect-id.js";

/**
 * `Chain(Tag("Break"), RestartPerform(id))` — shared by race branches.
 * The winning branch tags its result as Break, then performs. The handler
 * restarts the body; Branch takes the Break arm (identity), `RestartHandle` exits.
 */
function breakPerform(restartHandlerId: RestartHandlerId): Action {
  return chain(
    tag("Break") as any,
    {
      kind: "RestartPerform",
      restart_handler_id: restartHandlerId,
    } as any,
  ) as Action;
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
 *     `RestartHandle(id, GetIndex(0),`
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

  const branches = actions.map(
    (action) => chain(action as any, perform as any) as Action,
  );

  const allAction: Action = { kind: "All", actions: branches };

  return typedAction(
    buildRestartBranchAction(restartHandlerId, allAction, identity() as Action),
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
    handler: { kind: "Builtin", builtin: { kind: "Sleep", ms } },
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

  // Branch 1: body → Tag("Ok") → Break → RestartPerform
  const bodyBranch = chain(
    chain(body as any, tag("Ok")),
    perform as any,
  ) as Action;

  // Branch 2: ms → sleep() → Tag("Err") → Break → RestartPerform
  const sleepBranch = chain(
    chain(chain(ms as any, DYNAMIC_SLEEP_INVOKE as any), tag("Err")),
    perform as any,
  ) as Action;

  const allAction: Action = { kind: "All", actions: [bodyBranch, sleepBranch] };

  return typedAction(
    buildRestartBranchAction(restartHandlerId, allAction, identity() as Action),
  );
}
