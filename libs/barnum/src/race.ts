import { type Action, type Pipeable, type Result, type TypedAction, typedAction } from "./ast.js";
import { allocateEffectId } from "./effect-id.js";

// ---------------------------------------------------------------------------
// Shared AST fragments
// ---------------------------------------------------------------------------

const EXTRACT_PAYLOAD: Action = {
  kind: "Invoke",
  handler: { kind: "Builtin", builtin: { kind: "ExtractField", value: "payload" } },
};

const TAG_DISCARD: Action = {
  kind: "Invoke",
  handler: { kind: "Builtin", builtin: { kind: "Tag", value: "Discard" } },
};

const TAG_OK: Action = {
  kind: "Invoke",
  handler: { kind: "Builtin", builtin: { kind: "Tag", value: "Ok" } },
};

const TAG_ERR: Action = {
  kind: "Invoke",
  handler: { kind: "Builtin", builtin: { kind: "Tag", value: "Err" } },
};

/** Handler DAG shared by race and withTimeout: extract payload, tag Discard. */
const RACE_HANDLER: Action = {
  kind: "Chain",
  first: EXTRACT_PAYLOAD,
  rest: TAG_DISCARD,
};

// ---------------------------------------------------------------------------
// race — first branch to complete wins, losers cancelled
// ---------------------------------------------------------------------------

/**
 * Run multiple actions concurrently. The first to complete wins; losers
 * are cancelled during Handle frame teardown.
 *
 * All branches must have the same input and output type (since either
 * could win).
 *
 * Compiles to:
 *   Handle(effectId, Chain(ExtractField("payload"), Tag("Discard")),
 *     All(
 *       Chain(action1, Perform(effectId)),
 *       Chain(action2, Perform(effectId)),
 *       ...
 *     ),
 *   )
 */
export function race<TIn, TOut>(
  ...actions: Pipeable<TIn, TOut>[]
): TypedAction<TIn, TOut> {
  const effectId = allocateEffectId();

  const perform: Action = { kind: "Perform", effect_id: effectId };

  const branches = actions.map((action) => ({
    kind: "Chain" as const,
    first: action as Action,
    rest: perform,
  }));

  return typedAction({
    kind: "Handle",
    effect_id: effectId,
    handler: RACE_HANDLER,
    body: { kind: "All", actions: branches },
  });
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
    handle: ({ value }: { value: number }) => {
      return new Promise<void>((resolve) => setTimeout(resolve, value));
    },
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
 * its result differently (Ok vs Err) before Perform. `race()` requires
 * homogeneous output types, but withTimeout needs heterogeneous tagging.
 *
 * Compiles to the same Handle/All/Perform structure as race, with each
 * branch wrapping its result as Ok or Err before Perform.
 */
export function withTimeout<TIn, TOut>(
  ms: Pipeable<TIn, number>,
  body: Pipeable<TIn, TOut>,
): TypedAction<TIn, Result<TOut, void>> {
  const effectId = allocateEffectId();

  const perform: Action = { kind: "Perform", effect_id: effectId };

  // Branch 1: body → Tag("Ok") → Perform
  const bodyBranch: Action = {
    kind: "Chain",
    first: { kind: "Chain", first: body as Action, rest: TAG_OK },
    rest: perform,
  };

  // Branch 2: ms → sleep() → Tag("Err") → Perform
  const sleepBranch: Action = {
    kind: "Chain",
    first: {
      kind: "Chain",
      first: { kind: "Chain", first: ms as Action, rest: SLEEP_INVOKE },
      rest: TAG_ERR,
    },
    rest: perform,
  };

  return typedAction({
    kind: "Handle",
    effect_id: effectId,
    handler: RACE_HANDLER,
    body: { kind: "All", actions: [bodyBranch, sleepBranch] },
  });
}

